use serde::{Deserialize, Serialize};
use std::fmt;

use crate::github::GitHubIssue;
use crate::github_enrichment::EnrichedIssue;
use crate::recommendation::RecommendationAssessment;
use crate::value_signals::ValueSignal;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RankedValueIssue {
    pub issue: GitHubIssue,
    pub score: i32,
    pub value_assessment: ValueAssessment,
    pub enriched_issue: EnrichedIssue,
    pub explanation: Vec<String>,
    #[serde(default)]
    pub recommendation: RecommendationAssessment,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ValueAssessment {
    pub final_rank_score: i32,
    #[serde(default)]
    pub category: RecommendationCategory,
    pub recommendation_category: RecommendationCategory,
    #[serde(default)]
    pub gates: ValueGates,
    #[serde(default)]
    pub scores: ValueScores,
    pub risk_tags: Vec<RiskTag>,
    #[serde(default)]
    pub evidence: Vec<ValueEvidence>,
    pub missing_evidence: Vec<String>,
    pub explanation: Vec<String>,

    #[serde(default)]
    pub attention_score: i32,
    #[serde(default)]
    pub execution_score: i32,
    #[serde(default)]
    pub profile_fit_score: i32,
    #[serde(default)]
    pub risk_penalty: i32,
    #[serde(default)]
    pub attention_band: ScoreBand,
    #[serde(default)]
    pub execution_band: ScoreBand,
    #[serde(default)]
    pub signals: Vec<ValueSignal>,
}

impl Default for ValueAssessment {
    fn default() -> Self {
        Self {
            final_rank_score: 0,
            category: RecommendationCategory::NeedsTriage,
            recommendation_category: RecommendationCategory::NeedsTriage,
            gates: ValueGates::default(),
            scores: ValueScores::default(),
            risk_tags: Vec::new(),
            evidence: Vec::new(),
            missing_evidence: Vec::new(),
            explanation: Vec::new(),
            attention_score: 0,
            execution_score: 0,
            profile_fit_score: 0,
            risk_penalty: 0,
            attention_band: ScoreBand::Low,
            execution_band: ScoreBand::Low,
            signals: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationCategory {
    #[serde(alias = "agent_ready_high_value")]
    HighValueReady,
    #[serde(alias = "high_attention")]
    HighValueNeedsScoping,
    NicheButActionable,
    ContestedOrLowTrust,
    #[serde(alias = "high_attention_low_depth")]
    FilteredLowDepth,
    #[default]
    NeedsTriage,
}

impl RecommendationCategory {
    pub fn sort_rank(self) -> u8 {
        match self {
            Self::HighValueReady => 0,
            Self::HighValueNeedsScoping => 1,
            Self::NicheButActionable => 2,
            Self::ContestedOrLowTrust => 3,
            Self::NeedsTriage => 4,
            Self::FilteredLowDepth => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScoreBand {
    #[default]
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Pass,
    SoftFail,
    #[default]
    HardFail,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GateBand {
    Strong,
    Acceptable,
    #[default]
    Weak,
    Suspicious,
    Contested,
    Saturated,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GateVerdict {
    pub status: GateStatus,
    pub band: GateBand,
    pub reasons: Vec<String>,
    pub evidence_refs: Vec<String>,
}

impl GateVerdict {
    pub fn new(
        status: GateStatus,
        band: GateBand,
        reasons: Vec<String>,
        evidence_refs: Vec<String>,
    ) -> Self {
        Self {
            status,
            band,
            reasons,
            evidence_refs,
        }
    }

    pub fn pass(reason: impl Into<String>, evidence_refs: Vec<String>) -> Self {
        Self::new(
            GateStatus::Pass,
            GateBand::Strong,
            vec![reason.into()],
            evidence_refs,
        )
    }
}

impl Default for GateVerdict {
    fn default() -> Self {
        Self {
            status: GateStatus::HardFail,
            band: GateBand::Weak,
            reasons: vec!["Gate was not evaluated".to_string()],
            evidence_refs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ValueGates {
    pub low_depth: GateVerdict,
    pub repo_influence: GateVerdict,
    pub competition: GateVerdict,
    pub profile_fit: GateVerdict,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ValueScores {
    pub repo_influence_score: i32,
    pub profile_fit_score: i32,
    pub execution_quality_score: i32,
    pub maintainer_signal_score: i32,
    pub freshness_score: i32,
    pub risk_score: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValueEvidence {
    pub summary: String,
    pub evidence_refs: Vec<String>,
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
    LowTrustRepo,
    LowImpactRepo,
    ForkStarAnomaly,
    MarketplaceNoise,
    CompetitionContested,
    CompetitionSaturated,
    CompetitionEvidenceMissing,
    ProfileMismatch,
    ScopeRisk,
}

impl fmt::Display for RecommendationCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::HighValueReady => "high_value_ready",
            Self::HighValueNeedsScoping => "high_value_needs_scoping",
            Self::NicheButActionable => "niche_but_actionable",
            Self::ContestedOrLowTrust => "contested_or_low_trust",
            Self::FilteredLowDepth => "filtered_low_depth",
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

impl fmt::Display for GateStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Pass => "pass",
            Self::SoftFail => "soft_fail",
            Self::HardFail => "hard_fail",
        })
    }
}

impl fmt::Display for GateBand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Strong => "strong",
            Self::Acceptable => "acceptable",
            Self::Weak => "weak",
            Self::Suspicious => "suspicious",
            Self::Contested => "contested",
            Self::Saturated => "saturated",
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
            Self::LowTrustRepo => "low_trust_repo",
            Self::LowImpactRepo => "low_impact_repo",
            Self::ForkStarAnomaly => "fork_star_anomaly",
            Self::MarketplaceNoise => "marketplace_noise",
            Self::CompetitionContested => "competition_contested",
            Self::CompetitionSaturated => "competition_saturated",
            Self::CompetitionEvidenceMissing => "competition_evidence_missing",
            Self::ProfileMismatch => "profile_mismatch",
            Self::ScopeRisk => "scope_risk",
        })
    }
}
