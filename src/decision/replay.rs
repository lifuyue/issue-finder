//! Complete, offline ranking inputs captured before display truncation.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::ProfileConfig;
use crate::paths::{atomic_write, IssueFinderPaths};
use crate::recommendation::events::IssueKey;
use crate::recommendation::feed_ranker::{apply_recommendation_assessments, sort_by_feed};
use crate::recommendation::state::{FeedbackPolicy, RecommendationIssueState};
use crate::value_scoring::{assess_issue, RankedValueIssue};

use super::questions::{SemanticAnswers, HISTORICAL_QUESTION_SET_VERSION, QUESTION_SET_VERSION};
use super::{request_for_version, JudgmentStatus};

pub const SCOUT_REPLAY_VERSION: u32 = 2;

fn historical_question_version() -> String {
    HISTORICAL_QUESTION_SET_VERSION.into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScoutReplay {
    pub schema_version: u32,
    /// Missing on schema 1 captures, which retain their original eight-question semantics.
    #[serde(default = "historical_question_version")]
    pub question_set_version: String,
    pub captured_at: String,
    pub profile: ProfileConfig,
    pub feedback_policy: FeedbackPolicy,
    /// Includes hidden and budget-skipped candidates, before presentation limits.
    pub ranked: Vec<RankedValueIssue>,
    /// A sorted list because JSON object keys cannot encode IssueKey structs.
    pub states: Vec<RecommendationIssueState>,
}

impl ScoutReplay {
    pub fn capture(
        profile: &ProfileConfig,
        ranked: &[RankedValueIssue],
        states: &HashMap<IssueKey, RecommendationIssueState>,
        feedback_policy: FeedbackPolicy,
    ) -> Result<Self> {
        let keys: BTreeSet<_> = ranked
            .iter()
            .map(|item| IssueKey::from_issue(&item.issue))
            .collect();
        let mut states: Vec<_> = states
            .iter()
            .filter(|(key, _)| keys.contains(key))
            .map(|(_, state)| state.clone())
            .collect();
        states.sort_by(|left, right| left.issue_key.cmp(&right.issue_key));
        let replay = Self {
            schema_version: SCOUT_REPLAY_VERSION,
            question_set_version: QUESTION_SET_VERSION.into(),
            captured_at: Utc::now().to_rfc3339(),
            profile: profile.clone(),
            feedback_policy,
            ranked: ranked.to_vec(),
            states,
        };
        replay.validate()?;
        Ok(replay)
    }

    pub fn save(&self, paths: &IssueFinderPaths) -> Result<PathBuf> {
        self.validate()?;
        let encoded = serde_json::to_vec_pretty(self)?;
        let key = format!("{:x}", Sha256::digest(&encoded));
        let path = paths.decision_replay_path(&key);
        atomic_write(&path, encoded)
            .with_context(|| format!("save scout replay {}", path.display()))?;
        Ok(path)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let encoded =
            std::fs::read(path).with_context(|| format!("read scout replay {}", path.display()))?;
        let replay: Self = serde_json::from_slice(&encoded).context("parse scout replay")?;
        replay.validate()?;
        Ok(replay)
    }

    pub fn replay(&self) -> Result<Vec<RankedValueIssue>> {
        self.validate()?;
        // Historical captures are recorded outcomes, not inputs to the current policy.
        // Validate their original contract, then preserve scores, visibility and order.
        if self.is_historical() {
            return Ok(self.ranked.clone());
        }
        let mut ranked = self.ranked.clone();
        for item in &mut ranked {
            let snapshot = item
                .enriched_issue
                .decision
                .as_mut()
                .context("missing semantic judgment")?;
            if let Some(response) = &snapshot.response {
                let evidence = snapshot
                    .evidence
                    .as_ref()
                    .context("answered judgment is missing its material")?;
                let request =
                    request_for_version(&self.question_set_version, evidence, &self.profile)?;
                snapshot.answers = Some(SemanticAnswers::from_response(&request, response)?);
            }
            // The original evaluated_at remains the clock for age and feedback.
            item.value_assessment = assess_issue(&item.enriched_issue, &self.profile);
        }
        let states = self
            .states
            .iter()
            .map(|state| (state.issue_key.clone(), state.clone()))
            .collect();
        apply_recommendation_assessments(&mut ranked, &states);
        sort_by_feed(&mut ranked);
        Ok(ranked)
    }

    pub fn is_historical(&self) -> bool {
        self.question_set_version == HISTORICAL_QUESTION_SET_VERSION
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1 || self.schema_version == SCOUT_REPLAY_VERSION,
            "unsupported scout replay version"
        );
        ensure!(
            (self.schema_version == 1 && self.is_historical())
                || (self.schema_version == SCOUT_REPLAY_VERSION
                    && self.question_set_version == QUESTION_SET_VERSION),
            "scout replay schema and semantic question version disagree"
        );
        ensure!(
            self.feedback_policy == FeedbackPolicy::CodexExposure,
            "semantic replay requires CodexExposure feedback policy"
        );
        DateTime::parse_from_rfc3339(&self.captured_at)
            .context("invalid scout replay capture timestamp")?;
        let mut keys = BTreeSet::new();
        for item in &self.ranked {
            let key = IssueKey::from_issue(&item.issue);
            ensure!(
                keys.insert(key),
                "scout replay contains duplicate candidates"
            );
            ensure!(
                item.recommendation.memory_adjustment == 0,
                "semantic replay cannot depend on legacy memory ranking hints"
            );
            let snapshot = item
                .enriched_issue
                .decision
                .as_ref()
                .context("scout replay requires a semantic judgment for every candidate")?;
            ensure!(
                snapshot
                    .question_set_version
                    .as_deref()
                    .unwrap_or(HISTORICAL_QUESTION_SET_VERSION)
                    == self.question_set_version,
                "scout replay contains a judgment from another semantic question version"
            );
            ensure!(
                !matches!(
                    snapshot.status,
                    JudgmentStatus::Pending | JudgmentStatus::NotEvaluated
                ),
                "scout replay candidate has not completed its screening stage"
            );
            DateTime::parse_from_rfc3339(&snapshot.evaluated_at)
                .context("invalid frozen judgment timestamp")?;
            if let Some(response) = &snapshot.response {
                let evidence = snapshot
                    .evidence
                    .as_ref()
                    .context("answered judgment is missing its material")?;
                let request =
                    request_for_version(&self.question_set_version, evidence, &self.profile)?;
                let answers = SemanticAnswers::from_response(&request, response)?;
                ensure!(
                    snapshot.input_id.as_ref() == Some(&request.input_id),
                    "scout replay judgment input identity changed"
                );
                ensure!(
                    snapshot.answers.as_ref() == Some(&answers),
                    "scout replay typed answers disagree with its validated response"
                );
            } else {
                ensure!(
                    snapshot.answers.is_none(),
                    "scout replay cannot use answers without a validated provider response"
                );
                ensure!(
                    matches!(
                        snapshot.status,
                        JudgmentStatus::Failed
                            | JudgmentStatus::SkippedBudget
                            | JudgmentStatus::SkippedFacts
                    ),
                    "answered scout replay is missing its provider response"
                );
            }
        }
        let mut state_keys = BTreeSet::new();
        for state in &self.states {
            ensure!(
                keys.contains(&state.issue_key) && state_keys.insert(state.issue_key.clone()),
                "scout replay contains unrelated or duplicate feedback states"
            );
            ensure!(
                !state.done
                    && !state.dismissed
                    && state.prepared_count == 0
                    && state.restored_at.is_none()
                    && state.last_prepared_at.is_none(),
                "scout replay feedback was not filtered with CodexExposure policy"
            );
        }
        Ok(())
    }
}
