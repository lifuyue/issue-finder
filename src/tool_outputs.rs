use serde::Serialize;
use serde_json::Value;

use crate::discovery::DiscoveryDiagnostics;
use crate::github::GitHubIssue;
use crate::prepare_gate::{
    allowed_prepare_categories, default_prepare_allowed, prepare_gate_reasons, PrepareGateDecision,
};
use crate::recommendation::RecommendationAssessment;
use crate::report::{FailedReportItem, PreparedReportItem};
use crate::value_scoring::{GateVerdict, RankedValueIssue, ValueAssessment};

const OUTPUT_KIND: &str = "issue_finder_tool_output";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IssueOutput {
    pub repo_full_name: String,
    pub number: u64,
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CandidateOutput {
    pub issue: IssueOutput,
    pub category: String,
    pub rank_score: i32,
    pub recommendation: RecommendationOutput,
    pub scores: ScoresOutput,
    pub gates: GatesOutput,
    pub risk_tags: Vec<String>,
    pub missing_evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssessmentOutput {
    pub category: String,
    pub rank_score: i32,
    pub recommendation: RecommendationOutput,
    pub gates: GatesOutput,
    pub scores: ScoresOutput,
    pub risk_tags: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub competition: CompetitionOutput,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompetitionOutput {
    pub open_pr_refs: usize,
    pub closed_pr_refs: usize,
    pub attempt_comments: usize,
    pub claim_comments: usize,
    pub working_comments: usize,
    pub fix_submitted_comments: usize,
    pub competition_points: i32,
    pub competition_band: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScoresOutput {
    #[serde(rename = "repoInfluence")]
    pub repo_influence: i32,
    pub profile_fit: i32,
    pub execution_quality: i32,
    pub maintainer_signal: i32,
    pub freshness: i32,
    pub risk: i32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecommendationOutput {
    pub base_category: String,
    pub base_rank_score: i32,
    pub freshness_boost: i32,
    pub feedback_penalty: i32,
    pub quality_penalty: i32,
    pub reactivation_boost: i32,
    pub memory_adjustment: i32,
    pub final_feed_score: i32,
    pub visibility: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GatesOutput {
    pub low_depth: GateOutput,
    pub repo_influence: GateOutput,
    pub competition: GateOutput,
    pub profile_fit: GateOutput,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GateOutput {
    pub status: String,
    pub band: String,
    pub reasons: Vec<String>,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrepareGateOutput {
    pub default_allowed: bool,
    pub allowed_categories: Vec<String>,
    pub requires_bypass: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_category: Option<String>,
    pub reasons: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bypass_available: Option<bool>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GateBypassOutput {
    pub allowed: bool,
    pub reason: String,
    pub original_blocked_category: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HandoffOutput {
    pub id: String,
    pub dir: String,
    pub handoff_json_path: String,
    pub handoff_markdown_path: String,
    pub codex_markdown_path: String,
    pub agent_policy_path: String,
    pub probe_json_path: String,
    pub prepare_events_path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessOutput {
    pub score: i32,
    pub band: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FailureOutput {
    pub repo_full_name: String,
    pub issue_number: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StatusConfigOutput {
    pub path: String,
    pub exists: bool,
    pub load_ok: bool,
    pub load_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StatusGitHubOutput {
    pub token_source: String,
    pub auth: StatusGitHubAuthOutput,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StatusGitHubAuthOutput {
    pub checked: bool,
    pub ok: bool,
    pub login: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StatusStructuredOutput {
    pub kind: String,
    pub tool: String,
    pub status: String,
    pub success: bool,
    pub config: StatusConfigOutput,
    pub github: StatusGitHubOutput,
    pub next_fix_command: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScoutStructuredOutput {
    pub kind: String,
    pub tool: String,
    pub status: String,
    pub success: bool,
    pub candidates: Vec<CandidateOutput>,
    pub filtered_count: usize,
    #[serde(flatten)]
    pub diagnostics: DiscoveryDiagnostics,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssessStructuredOutput {
    pub kind: String,
    pub tool: String,
    pub status: String,
    pub success: bool,
    pub issue: IssueOutput,
    pub assessment: AssessmentOutput,
    pub prepare_gate: PrepareGateOutput,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrepareBlockedStructuredOutput {
    pub kind: String,
    pub tool: String,
    pub status: String,
    pub success: bool,
    pub issue: IssueOutput,
    pub assessment: AssessmentOutput,
    pub prepare_gate: PrepareGateOutput,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreparePreparedStructuredOutput {
    pub kind: String,
    pub tool: String,
    pub status: String,
    pub success: bool,
    pub issue: IssueOutput,
    pub assessment: AssessmentOutput,
    pub prepare_gate: PrepareGateOutput,
    pub handoff: HandoffOutput,
    pub readiness: ReadinessOutput,
    pub gate_bypass: Option<GateBypassOutput>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrepareFailedStructuredOutput {
    pub kind: String,
    pub tool: String,
    pub status: String,
    pub success: bool,
    pub issue: IssueOutput,
    pub assessment: AssessmentOutput,
    pub prepare_gate: PrepareGateOutput,
    pub failure: FailureOutput,
    pub gate_bypass: Option<GateBypassOutput>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReadContextStructuredOutput {
    pub kind: String,
    pub tool: String,
    pub status: String,
    pub success: bool,
    pub handoff_id: String,
    pub section: String,
    pub path: String,
    pub truncated: bool,
    pub content: String,
}

pub fn issue_output(issue: &GitHubIssue) -> IssueOutput {
    IssueOutput {
        repo_full_name: issue.repo_full_name.clone(),
        number: issue.number,
        title: issue.title.clone(),
        url: issue.url.clone(),
    }
}

pub fn candidate_output(candidate: &RankedValueIssue) -> CandidateOutput {
    CandidateOutput {
        issue: issue_output(&candidate.issue),
        category: candidate
            .value_assessment
            .recommendation_category
            .to_string(),
        rank_score: candidate.value_assessment.final_rank_score,
        recommendation: recommendation_output(&candidate.recommendation),
        scores: scores_output(&candidate.value_assessment),
        gates: gates_output(&candidate.value_assessment),
        risk_tags: risk_tags_output(&candidate.value_assessment),
        missing_evidence: candidate.value_assessment.missing_evidence.clone(),
    }
}

pub fn assessment_output(candidate: &RankedValueIssue) -> AssessmentOutput {
    let competition = &candidate.enriched_issue.competition;
    AssessmentOutput {
        category: candidate
            .value_assessment
            .recommendation_category
            .to_string(),
        rank_score: candidate.value_assessment.final_rank_score,
        recommendation: recommendation_output(&candidate.recommendation),
        gates: gates_output(&candidate.value_assessment),
        scores: scores_output(&candidate.value_assessment),
        risk_tags: risk_tags_output(&candidate.value_assessment),
        missing_evidence: candidate.value_assessment.missing_evidence.clone(),
        competition: CompetitionOutput {
            open_pr_refs: competition.open_pr_refs,
            closed_pr_refs: competition.closed_pr_refs,
            attempt_comments: competition.attempt_comments,
            claim_comments: competition.claim_comments,
            working_comments: competition.working_comments,
            fix_submitted_comments: competition.fix_submitted_comments,
            competition_points: competition.competition_points,
            competition_band: competition.competition_band.to_string(),
            warnings: competition.warnings.clone(),
        },
    }
}

pub fn prepare_gate_output(assessment: &ValueAssessment) -> PrepareGateOutput {
    let category = assessment.recommendation_category;
    let allowed_categories = allowed_prepare_categories()
        .into_iter()
        .map(|category| category.to_string())
        .collect::<Vec<_>>();

    if default_prepare_allowed(category) {
        return PrepareGateOutput {
            default_allowed: true,
            allowed_categories,
            requires_bypass: false,
            blocked_category: None,
            reasons: Vec::new(),
            bypass_available: None,
        };
    }

    PrepareGateOutput {
        default_allowed: false,
        allowed_categories,
        requires_bypass: true,
        blocked_category: Some(category.to_string()),
        reasons: prepare_gate_reasons(assessment),
        bypass_available: Some(true),
    }
}

pub fn gate_bypass_output(decision: &PrepareGateDecision) -> Option<GateBypassOutput> {
    match decision {
        PrepareGateDecision::Bypassed { category, reason } => Some(GateBypassOutput {
            allowed: true,
            reason: reason.clone(),
            original_blocked_category: category.to_string(),
        }),
        PrepareGateDecision::Allowed | PrepareGateDecision::Blocked { .. } => None,
    }
}

pub fn scout_structured_output(
    tool: &str,
    candidates: Vec<CandidateOutput>,
    filtered_count: usize,
    diagnostics: DiscoveryDiagnostics,
) -> Value {
    to_value(ScoutStructuredOutput {
        kind: OUTPUT_KIND.to_string(),
        tool: tool.to_string(),
        status: "ok".to_string(),
        success: true,
        candidates,
        filtered_count,
        diagnostics,
    })
}

pub fn assess_structured_output(
    tool: &str,
    issue: IssueOutput,
    assessment: AssessmentOutput,
    prepare_gate: PrepareGateOutput,
) -> Value {
    to_value(AssessStructuredOutput {
        kind: OUTPUT_KIND.to_string(),
        tool: tool.to_string(),
        status: "ok".to_string(),
        success: true,
        issue,
        assessment,
        prepare_gate,
    })
}

pub fn prepare_blocked_structured_output(
    tool: &str,
    issue: IssueOutput,
    assessment: AssessmentOutput,
    prepare_gate: PrepareGateOutput,
) -> Value {
    to_value(PrepareBlockedStructuredOutput {
        kind: OUTPUT_KIND.to_string(),
        tool: tool.to_string(),
        status: "blocked_by_gate".to_string(),
        success: true,
        issue,
        assessment,
        prepare_gate,
    })
}

pub fn prepare_prepared_structured_output(
    tool: &str,
    issue: IssueOutput,
    assessment: AssessmentOutput,
    prepare_gate: PrepareGateOutput,
    handoff: HandoffOutput,
    readiness: ReadinessOutput,
    gate_bypass: Option<GateBypassOutput>,
) -> Value {
    to_value(PreparePreparedStructuredOutput {
        kind: OUTPUT_KIND.to_string(),
        tool: tool.to_string(),
        status: "prepared".to_string(),
        success: true,
        issue,
        assessment,
        prepare_gate,
        handoff,
        readiness,
        gate_bypass,
    })
}

pub fn prepare_failed_structured_output(
    tool: &str,
    issue: IssueOutput,
    assessment: AssessmentOutput,
    prepare_gate: PrepareGateOutput,
    failure: FailureOutput,
    gate_bypass: Option<GateBypassOutput>,
) -> Value {
    to_value(PrepareFailedStructuredOutput {
        kind: OUTPUT_KIND.to_string(),
        tool: tool.to_string(),
        status: "prepare_failed".to_string(),
        success: false,
        issue,
        assessment,
        prepare_gate,
        failure,
        gate_bypass,
    })
}

pub fn status_structured_output(
    tool: &str,
    status: String,
    config: StatusConfigOutput,
    github: StatusGitHubOutput,
    next_fix_command: Option<String>,
) -> Value {
    to_value(StatusStructuredOutput {
        kind: OUTPUT_KIND.to_string(),
        tool: tool.to_string(),
        status,
        success: true,
        config,
        github,
        next_fix_command,
    })
}

pub fn handoff_output(item: &PreparedReportItem, dir: String) -> HandoffOutput {
    HandoffOutput {
        id: item.id.clone(),
        dir,
        handoff_json_path: item.handoff_json_path.clone(),
        handoff_markdown_path: item.handoff_md_path.clone(),
        codex_markdown_path: item.codex_md_path.clone(),
        agent_policy_path: item.agent_policy_path.clone(),
        probe_json_path: item.probe_json_path.clone(),
        prepare_events_path: item.prepare_events_path.clone(),
    }
}

pub fn readiness_output(item: &PreparedReportItem) -> ReadinessOutput {
    ReadinessOutput {
        score: item.readiness_score,
        band: item.readiness_band.clone(),
    }
}

pub fn failure_output(item: &FailedReportItem) -> FailureOutput {
    FailureOutput {
        repo_full_name: item.repo_full_name.clone(),
        issue_number: item.issue_number,
        reason: item.reason.clone(),
    }
}

pub fn read_context_structured_output(
    tool: &str,
    handoff_id: String,
    section: String,
    path: String,
    truncated: bool,
    content: String,
) -> ReadContextStructuredOutput {
    ReadContextStructuredOutput {
        kind: OUTPUT_KIND.to_string(),
        tool: tool.to_string(),
        status: "ok".to_string(),
        success: true,
        handoff_id,
        section,
        path,
        truncated,
        content,
    }
}

pub fn to_value<T: Serialize>(value: T) -> Value {
    serde_json::to_value(value).expect("Issue Finder tool output DTO serialization should not fail")
}

fn scores_output(assessment: &ValueAssessment) -> ScoresOutput {
    ScoresOutput {
        repo_influence: assessment.scores.repo_influence_score,
        profile_fit: assessment.scores.profile_fit_score,
        execution_quality: assessment.scores.execution_quality_score,
        maintainer_signal: assessment.scores.maintainer_signal_score,
        freshness: assessment.scores.freshness_score,
        risk: assessment.scores.risk_score,
    }
}

fn recommendation_output(assessment: &RecommendationAssessment) -> RecommendationOutput {
    RecommendationOutput {
        base_category: assessment.base_category.to_string(),
        base_rank_score: assessment.base_rank_score,
        freshness_boost: assessment.freshness_boost,
        feedback_penalty: assessment.feedback_penalty,
        quality_penalty: assessment.quality_penalty,
        reactivation_boost: assessment.reactivation_boost,
        memory_adjustment: assessment.memory_adjustment,
        final_feed_score: assessment.final_feed_score,
        visibility: assessment.visibility.to_string(),
        reasons: assessment.reasons.clone(),
    }
}

fn gates_output(assessment: &ValueAssessment) -> GatesOutput {
    GatesOutput {
        low_depth: gate_output(&assessment.gates.low_depth),
        repo_influence: gate_output(&assessment.gates.repo_influence),
        competition: gate_output(&assessment.gates.competition),
        profile_fit: gate_output(&assessment.gates.profile_fit),
    }
}

fn gate_output(gate: &GateVerdict) -> GateOutput {
    GateOutput {
        status: gate.status.to_string(),
        band: gate.band.to_string(),
        reasons: gate.reasons.clone(),
        evidence_refs: gate.evidence_refs.clone(),
    }
}

fn risk_tags_output(assessment: &ValueAssessment) -> Vec<String> {
    assessment
        .risk_tags
        .iter()
        .map(ToString::to_string)
        .collect()
}
