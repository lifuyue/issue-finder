//! Provider-independent screening: material snapshots, finite questions and replayable results.
pub mod codex;
pub mod contract;
pub mod evidence;
pub mod policy;
pub mod questions;
pub mod replay;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::ProfileConfig;
use crate::github_enrichment::EnrichedIssue;
use crate::paths::{atomic_write, IssueFinderPaths};
use contract::{DecisionRequest, DecisionResponse, Provider, ResponseStatus};
use evidence::EvidenceSnapshot;
use questions::SemanticAnswers;

const SNAPSHOT_TTL_MINUTES: i64 = 360;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionReport {
    pub concurrency: usize,
    pub peak_in_flight: usize,
    pub tasks: Vec<TaskTiming>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskTiming {
    pub candidate: String,
    pub queue_wait_ms: u64,
    pub execution_ms: u64,
    pub model_duration_ms: Option<u64>,
    pub cache_hit: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JudgmentStatus {
    Pending,
    Completed,
    Partial,
    Failed,
    SkippedBudget,
    SkippedFacts,
    NotEvaluated,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JudgmentSnapshot {
    #[serde(default)]
    pub question_set_version: Option<String>,
    pub status: JudgmentStatus,
    pub input_id: Option<String>,
    pub provider_fingerprint: Option<String>,
    pub evaluated_at: String,
    pub answers: Option<SemanticAnswers>,
    pub response: Option<DecisionResponse>,
    pub evidence: Option<EvidenceSnapshot>,
    pub error: Option<String>,
    pub snapshot_path: Option<String>,
    pub cache_hit: bool,
}

impl JudgmentSnapshot {
    pub fn pending(reason: &str) -> Self {
        Self {
            question_set_version: Some(questions::QUESTION_SET_VERSION.to_string()),
            status: JudgmentStatus::Pending,
            input_id: None,
            provider_fingerprint: None,
            evaluated_at: Utc::now().to_rfc3339(),
            answers: None,
            response: None,
            evidence: None,
            error: Some(reason.to_string()),
            snapshot_path: None,
            cache_hit: false,
        }
    }

    pub fn summary(&self) -> Value {
        let material = self.evidence.as_ref().map(|e| json!({
            "issueUrl": e.issue_url, "issueUpdatedAt": e.issue_updated_at,
            "materialHash": e.material_hash(), "bodyTruncated": e.body.truncated,
            "commentStatus": e.comments.status, "commentsTotal": e.comments.total_count,
            "commentsIncluded": e.comments.comments.len(), "commentsOmitted": e.comments.omitted_count,
            "commentsTruncated": e.comments.bodies_truncated,
            "chronologyComplete": e.comments.chronological_order_complete,
            "selection": e.comments.selection,
            "commentSources": e.comments.comments.iter().map(|c| json!({"id":c.id,"url":c.url,"updatedAt":c.updated_at})).collect::<Vec<_>>(),
            "githubStatus":e.github_status, "warnings":e.warnings,
        }));
        json!({
            "questionSetVersion": self.question_set_version.as_deref().unwrap_or("scout-semantics-v1"),
            "historical": self.question_set_version.as_deref() != Some(questions::QUESTION_SET_VERSION),
            "status": self.status, "answers": self.answers, "inputId": self.input_id,
            "providerFingerprint": self.provider_fingerprint,
            "provider": self.response.as_ref().map(|r| &r.metadata),
            "probabilities": self.response.as_ref().map(|r| r.answers.iter().map(|a| json!({"questionId":a.question_id,"status":a.status,"probabilities":a.probabilities})).collect::<Vec<_>>()),
            "evaluatedAt": self.evaluated_at, "materials": material,
            "error": self.error, "snapshotPath": self.snapshot_path, "cacheHit": self.cache_hit,
            "meaning":"Screening interpretation of the recorded material, not repair authorization or verified GitHub facts. assess must read current evidence."
        })
    }

    pub fn incomplete(&self) -> bool {
        self.status != JudgmentStatus::Completed
    }
}

pub fn request_for(evidence: &EvidenceSnapshot, profile: &ProfileConfig) -> DecisionRequest {
    request_for_version(questions::QUESTION_SET_VERSION, evidence, profile)
        .expect("current question set is supported")
}

pub fn request_for_version(
    version: &str,
    evidence: &EvidenceSnapshot,
    profile: &ProfileConfig,
) -> Result<DecisionRequest> {
    let questions = questions::questions_for_version(version, evidence, profile)?;
    let input = serde_json::to_vec(&(
        contract::CONTRACT_VERSION,
        version,
        evidence.material_hash(),
        &questions,
    ))
    .expect("serializable questions");
    Ok(DecisionRequest {
        candidate_id: format!("{}:{}", evidence.repo_full_name, evidence.issue_id),
        input_id: format!("{:x}", Sha256::digest(input)),
        questions,
    })
}

/// The same evidence/question/provider identity is the sole cache and replay boundary.
pub async fn judge(
    paths: &IssueFinderPaths,
    provider: &dyn Provider,
    evidence: EvidenceSnapshot,
    profile: &ProfileConfig,
    refresh: bool,
) -> Result<JudgmentSnapshot> {
    let request = request_for(&evidence, profile);
    request.validate()?;
    let fingerprint = provider.fingerprint();
    let key = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(&request.input_id, &fingerprint))?)
    );
    let path = paths.system1_snapshot_path(&key);
    if !refresh && path.exists() {
        let raw = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        if let Ok(mut snapshot) = serde_json::from_slice::<JudgmentSnapshot>(&raw) {
            let fresh = DateTime::parse_from_rfc3339(&snapshot.evaluated_at).is_ok_and(|at| {
                let age = Utc::now() - at.with_timezone(&Utc);
                age >= Duration::zero() && age < Duration::minutes(SNAPSHOT_TTL_MINUTES)
            });
            if fresh
                && snapshot.status == JudgmentStatus::Completed
                && snapshot.question_set_version.as_deref() == Some(questions::QUESTION_SET_VERSION)
                && snapshot.input_id.as_ref() == Some(&request.input_id)
                && snapshot.provider_fingerprint.as_ref() == Some(&fingerprint)
                && snapshot
                    .evidence
                    .as_ref()
                    .is_some_and(|old| old.material_hash() == evidence.material_hash())
            {
                if let Some(response) = &snapshot.response {
                    if response.validate(&request).is_ok() {
                        // Derive business answers again, rather than trusting a separately stored interpretation.
                        snapshot.answers =
                            Some(SemanticAnswers::from_response(&request, response)?);
                        snapshot.cache_hit = true;
                        return Ok(snapshot);
                    }
                }
            }
        }
    }
    let mut snapshot = JudgmentSnapshot::pending("System 1 request has not completed");
    snapshot.input_id = Some(request.input_id.clone());
    snapshot.provider_fingerprint = Some(fingerprint);
    snapshot.evidence = Some(evidence);
    snapshot.snapshot_path = Some(path.display().to_string());
    match provider.decide(&request).await.and_then(|response| {
        response.validate(&request)?;
        Ok(response)
    }) {
        Ok(response) => {
            match SemanticAnswers::from_response(&request, &response) {
                Ok(answers) => {
                    snapshot.status = match response.status {
                        ResponseStatus::Complete => JudgmentStatus::Completed,
                        ResponseStatus::Partial => JudgmentStatus::Partial,
                        ResponseStatus::Failed => JudgmentStatus::Failed,
                    };
                    snapshot.answers = Some(answers);
                    snapshot.error = (snapshot.status != JudgmentStatus::Completed).then(|| {
                        "One or more semantic questions could not be answered".to_string()
                    });
                }
                Err(error) => {
                    snapshot.status = JudgmentStatus::Failed;
                    snapshot.error = Some(error.to_string());
                }
            }
            snapshot.response = Some(response);
        }
        Err(error) => {
            snapshot.status = JudgmentStatus::Failed;
            snapshot.error = Some(error.to_string());
        }
    }
    atomic_write(&path, serde_json::to_vec_pretty(&snapshot)?)?;
    Ok(snapshot)
}

/// Legacy comment counts are interpretations, not GitHub facts. They must not leak downstream.
pub fn factual_competition_only(enriched: &mut EnrichedIssue) {
    let facts = &mut enriched.competition;
    facts.attempt_comments = 0;
    facts.claim_comments = 0;
    facts.working_comments = 0;
    facts.fix_submitted_comments = 0;
    facts.competition_points = (facts.open_pr_refs * 3) as i32;
    facts.competition_band = match facts.open_pr_refs {
        0 => crate::competition::CompetitionBand::Clear,
        1 => crate::competition::CompetitionBand::Contested,
        _ => crate::competition::CompetitionBand::Saturated,
    };
}

/// Frozen clock for reproducible screening/feedback replay; legacy paths keep wall time.
pub fn ranking_time(enriched: &EnrichedIssue) -> DateTime<Utc> {
    enriched
        .system1
        .as_ref()
        .and_then(|snapshot| DateTime::parse_from_rfc3339(&snapshot.evaluated_at).ok())
        .map(|at| at.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}

pub async fn probe(binary: Option<String>, timeout_seconds: u64) -> Result<Value> {
    let provider =
        codex::CodexProvider::new(binary, std::time::Duration::from_secs(timeout_seconds))?;
    let request = DecisionRequest {
        candidate_id: "startup-probe".into(),
        input_id: "system1-startup-v1".into(),
        questions: vec![contract::Question {
            id: "health".into(),
            prompt: "Select ready.".into(),
            criteria: vec!["This is a schema and authentication check; answer ready.".into()],
            context: json!({"purpose":"System 1 runtime verification"}),
            kind: contract::QuestionKind::Choice {
                options: vec!["ready".into(), "unavailable".into()],
            },
        }],
    };
    let outcome = provider.decide(&request).await;
    provider.close().await;
    let response = outcome?;
    response.validate(&request)?;
    anyhow::ensure!(
        response.answers[0].answer == Some(contract::Answer::Choice("ready".into())),
        "System 1 probe did not answer ready"
    );
    Ok(json!({"success":true,"providerFingerprint":provider.fingerprint(),"response":response}))
}
