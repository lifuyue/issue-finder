use serde::{Deserialize, Serialize};
use std::fmt;

use crate::config::ProfileConfig;
use crate::github::GitHubIssue;
use crate::github_enrichment::EnrichedIssue;
use crate::value_signals::{
    build_risk_tags, build_value_signals, risk_penalty, SignalAxis, ValueSignal,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RankedValueIssue {
    pub issue: GitHubIssue,
    pub score: i32,
    pub value_assessment: ValueAssessment,
    pub enriched_issue: EnrichedIssue,
    pub explanation: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ValueAssessment {
    pub final_rank_score: i32,
    pub attention_score: i32,
    pub execution_score: i32,
    pub profile_fit_score: i32,
    pub risk_penalty: i32,
    pub recommendation_category: RecommendationCategory,
    pub attention_band: ScoreBand,
    pub execution_band: ScoreBand,
    pub signals: Vec<ValueSignal>,
    pub risk_tags: Vec<RiskTag>,
    pub missing_evidence: Vec<String>,
    pub explanation: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationCategory {
    AgentReadyHighValue,
    HighAttention,
    HighAttentionLowDepth,
    NicheButActionable,
    NeedsTriage,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ScoreBand {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RiskTag {
    NoCodeRequired,
    MicroContribution,
    ContentFill,
    TemplateLike,
    EventNoise,
    ThinTask,
    HighTriageLoad,
    MissingMaintainerSignal,
    WeakValidationPath,
}

impl fmt::Display for RecommendationCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AgentReadyHighValue => "agent_ready_high_value",
            Self::HighAttention => "high_attention",
            Self::HighAttentionLowDepth => "high_attention_low_depth",
            Self::NicheButActionable => "niche_but_actionable",
            Self::NeedsTriage => "needs_triage",
        })
    }
}

impl fmt::Display for ScoreBand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        })
    }
}

impl fmt::Display for RiskTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NoCodeRequired => "no_code_required",
            Self::MicroContribution => "micro_contribution",
            Self::ContentFill => "content_fill",
            Self::TemplateLike => "template_like",
            Self::EventNoise => "event_noise",
            Self::ThinTask => "thin_task",
            Self::HighTriageLoad => "high_triage_load",
            Self::MissingMaintainerSignal => "missing_maintainer_signal",
            Self::WeakValidationPath => "weak_validation_path",
        })
    }
}

pub fn assess_issue(enriched: &EnrichedIssue, profile: &ProfileConfig) -> ValueAssessment {
    let signals = build_value_signals(enriched, profile);
    let risk_tags = build_risk_tags(enriched);
    aggregate_signals(signals, risk_tags, enriched)
}

pub fn aggregate_signals(
    signals: Vec<ValueSignal>,
    risk_tags: Vec<RiskTag>,
    enriched: &EnrichedIssue,
) -> ValueAssessment {
    let attention_score = axis_score(&signals, SignalAxis::Attention);
    let execution_score = axis_score(&signals, SignalAxis::Execution);
    let profile_fit_score = axis_score(&signals, SignalAxis::ProfileFit);
    let risk_penalty = risk_penalty(&risk_tags);
    let final_rank_score = final_rank_score(
        attention_score,
        execution_score,
        profile_fit_score,
        risk_penalty,
    );
    let attention_band = score_band(attention_score);
    let execution_band = score_band(execution_score);
    let recommendation_category =
        recommendation_category(attention_band, execution_band, risk_penalty, &risk_tags);
    let missing_evidence = missing_evidence(enriched);
    let explanation = top_explanations(&signals);

    ValueAssessment {
        final_rank_score,
        attention_score,
        execution_score,
        profile_fit_score,
        risk_penalty,
        recommendation_category,
        attention_band,
        execution_band,
        signals,
        risk_tags,
        missing_evidence,
        explanation,
    }
}

pub fn final_rank_score(
    attention_score: i32,
    execution_score: i32,
    profile_fit_score: i32,
    risk_penalty: i32,
) -> i32 {
    ((attention_score as f64 * 0.55)
        + (execution_score as f64 * 0.30)
        + (profile_fit_score as f64 * 0.10)
        - (risk_penalty as f64 * 0.15))
        .round()
        .clamp(0.0, 100.0) as i32
}

pub fn score_band(score: i32) -> ScoreBand {
    if score >= 70 {
        ScoreBand::High
    } else if score >= 30 {
        ScoreBand::Medium
    } else {
        ScoreBand::Low
    }
}

pub fn recommendation_category(
    attention_band: ScoreBand,
    execution_band: ScoreBand,
    risk_penalty: i32,
    risk_tags: &[RiskTag],
) -> RecommendationCategory {
    if attention_band == ScoreBand::High && has_low_depth_tag(risk_tags) {
        return RecommendationCategory::HighAttentionLowDepth;
    }
    if attention_band == ScoreBand::High && risk_penalty >= 45 {
        return RecommendationCategory::NeedsTriage;
    }
    if attention_band == ScoreBand::High && execution_band == ScoreBand::High && risk_penalty < 30 {
        return RecommendationCategory::AgentReadyHighValue;
    }
    if attention_band == ScoreBand::High {
        return RecommendationCategory::HighAttention;
    }
    if execution_band == ScoreBand::High {
        return RecommendationCategory::NicheButActionable;
    }
    RecommendationCategory::NeedsTriage
}

pub fn is_daily_prepare_candidate(assessment: &ValueAssessment) -> bool {
    !(assessment.recommendation_category == RecommendationCategory::NeedsTriage
        && assessment.attention_score < 60)
}

fn axis_score(signals: &[ValueSignal], axis: SignalAxis) -> i32 {
    signals
        .iter()
        .filter(|signal| signal.axis == axis)
        .map(|signal| signal.score_delta)
        .sum::<i32>()
        .clamp(0, 100)
}

fn has_low_depth_tag(risk_tags: &[RiskTag]) -> bool {
    risk_tags.iter().any(|tag| {
        matches!(
            tag,
            RiskTag::NoCodeRequired
                | RiskTag::MicroContribution
                | RiskTag::ContentFill
                | RiskTag::ThinTask
        )
    })
}

fn missing_evidence(enriched: &EnrichedIssue) -> Vec<String> {
    let mut missing = Vec::new();
    if enriched.growth.recent_stargazer_sample.is_empty() {
        missing.push("Recent stargazer sample was unavailable".to_string());
    }
    if enriched.growth.newest_fork_sample.is_empty() {
        missing.push("Newest fork sample was unavailable".to_string());
    }
    if enriched.comments.is_empty() && enriched.issue.comments_count > 0 {
        missing
            .push("Issue comments count exists but comment excerpts were unavailable".to_string());
    }
    missing.extend(enriched.warnings.iter().cloned());
    missing.sort();
    missing.dedup();
    missing
}

fn top_explanations(signals: &[ValueSignal]) -> Vec<String> {
    let mut ordered = signals.to_vec();
    ordered.sort_by_key(|signal| std::cmp::Reverse(signal.score_delta));
    ordered
        .into_iter()
        .filter(|signal| signal.score_delta > 0)
        .take(5)
        .map(|signal| signal.summary)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        aggregate_signals, final_rank_score, recommendation_category, RecommendationCategory,
        RiskTag, ScoreBand,
    };
    use crate::github::GitHubIssue;
    use crate::github_enrichment::EnrichedIssue;
    use crate::value_signals::{SignalAxis, ValueSignal, ValueSignalKind};

    fn signal(kind: ValueSignalKind, axis: SignalAxis, delta: i32) -> ValueSignal {
        ValueSignal {
            kind,
            axis,
            score_delta: delta,
            summary: "summary".to_string(),
            evidence_refs: vec!["issue:body".to_string()],
        }
    }

    fn enriched() -> EnrichedIssue {
        let issue = GitHubIssue {
            id: 1,
            number: 1,
            title: "Issue".to_string(),
            body: "Body".to_string(),
            labels: vec![],
            url: "https://github.com/owner/repo/issues/1".to_string(),
            repo_full_name: "owner/repo".to_string(),
            repo_name: "repo".to_string(),
            repo_description: String::new(),
            repo_stars: 0,
            created_at: "2026-06-01T00:00:00Z".to_string(),
            updated_at: "2026-06-01T00:00:00Z".to_string(),
        };
        EnrichedIssue::from_issue(&issue)
    }

    #[test]
    fn applies_final_rank_formula() {
        assert_eq!(final_rank_score(100, 100, 100, 100), 80);
    }

    #[test]
    fn classifies_agent_ready_high_value() {
        assert_eq!(
            recommendation_category(ScoreBand::High, ScoreBand::High, 10, &[]),
            RecommendationCategory::AgentReadyHighValue
        );
    }

    #[test]
    fn low_depth_tag_overrides_high_attention() {
        assert_eq!(
            recommendation_category(
                ScoreBand::High,
                ScoreBand::Low,
                40,
                &[RiskTag::NoCodeRequired]
            ),
            RecommendationCategory::HighAttentionLowDepth
        );
    }

    #[test]
    fn aggregates_axis_scores_and_risk_penalty() {
        let assessment = aggregate_signals(
            vec![
                signal(
                    ValueSignalKind::EstablishedImpact,
                    SignalAxis::Attention,
                    35,
                ),
                signal(ValueSignalKind::GrowthMomentum, SignalAxis::Attention, 35),
                signal(ValueSignalKind::IssueClarity, SignalAxis::Execution, 25),
                signal(
                    ValueSignalKind::ReproductionSteps,
                    SignalAxis::Execution,
                    25,
                ),
                signal(ValueSignalKind::IssueFit, SignalAxis::ProfileFit, 50),
            ],
            vec![],
            &enriched(),
        );
        assert_eq!(assessment.attention_score, 70);
        assert_eq!(assessment.execution_score, 50);
        assert_eq!(
            assessment.recommendation_category,
            RecommendationCategory::HighAttention
        );
    }
}
