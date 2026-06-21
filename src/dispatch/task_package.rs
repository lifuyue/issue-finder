use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent_policy::AgentPolicyManifest;
use crate::context_pack::ContextPack;
use crate::evidence_pack::EvidencePack;
use crate::handoff::{Handoff, HandoffContext, HandoffMemoryContext, HandoffWorkspace};
use crate::llm_review::LlmConfirmation;
use crate::probe::{ProbeFacts, ProbePack};
use crate::readiness::ExecutionReadiness;
use crate::recommendation::RecommendationAssessment;
use crate::repo_scan::{CandidateFile, ValidationCommand};
use crate::value_scoring::ValueAssessment;

use super::model::{ApprovalRequest, ApprovalStatus};

const PACKAGE_KIND: &str = "issue_finder_task_package";
const PACKAGE_VERSION: u8 = 3;
const FIX_RESULT_ARTIFACT: &str = "fix_result.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IssueTaskPackage {
    pub kind: String,
    pub version: u8,
    pub issue: IssueTaskPackageIssue,
    pub source: PackageSource,
    pub evidence: PackageEvidence,
    pub llm_confirmation: LlmConfirmation,
    pub human_review: PackageHumanReview,
    pub user_profile_snapshot: Value,
    pub workspace_policy: WorkspacePolicyContract,
    pub context_pack: PackageContextPack,
    pub validation_hints: ValidationHints,
    pub memory_context: HandoffMemoryContext,
    pub expected_outputs: Vec<String>,
    pub callback_policy: CallbackPolicy,
    pub reproduction_contract: ReproductionContract,
    pub success_criteria: SuccessCriteriaContract,
    pub change_budget: ChangeBudgetContract,
    pub environment_contract: EnvironmentContract,
    pub interaction_policy: InteractionPolicyContract,
    pub session_context: SessionContextContract,
    pub outcome_contract: OutcomeContract,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueTaskPackageIssue {
    pub repo_full_name: String,
    pub number: u64,
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackageSource {
    pub source_handoff_id: String,
    pub handoff_artifact_id: String,
    pub packaged_by_approval_request_id: String,
    pub package_version: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PackageEvidence {
    pub handoff_artifact_id: String,
    pub value_assessment: ValueAssessment,
    pub recommendation: RecommendationAssessment,
    pub evidence_pack: EvidencePack,
    pub source_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PackageHumanReview {
    pub approval_request_id: String,
    pub status: ApprovalStatus,
    pub resolved_at: Option<String>,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePolicyContract {
    pub workspace: HandoffWorkspace,
    pub agent_policy: AgentPolicyManifest,
    pub issue_finder_boundary: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PackageContextPack {
    pub context: HandoffContext,
    pub context_pack: ContextPack,
    pub probe_pack: ProbePack,
    pub initial_read_order: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ValidationHints {
    pub readiness: ExecutionReadiness,
    pub validation_commands: Vec<ValidationCommand>,
    pub validation_obligations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CallbackPolicy {
    pub expected_artifacts: Vec<String>,
    pub optional_artifacts: Vec<String>,
    pub source_handoff_id: String,
    pub import_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReproductionContract {
    pub version: u8,
    pub strategy: String,
    pub obligations: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub issue_body_available: bool,
    pub suggested_start: Vec<String>,
    pub validation_commands: Vec<ValidationCommand>,
    pub blocker_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SuccessCriteriaContract {
    pub version: u8,
    pub criteria: Vec<String>,
    pub validation_expectations: Vec<String>,
    pub done_definition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChangeBudgetContract {
    pub version: u8,
    pub scope: String,
    pub preferred_files: Vec<CandidateFile>,
    pub max_files_hint: usize,
    pub allowed_changes: Vec<String>,
    pub escalation_triggers: Vec<String>,
    pub forbidden_without_approval: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentContract {
    pub version: u8,
    pub workspace: EnvironmentWorkspace,
    pub safe_probe_status: String,
    pub probe_facts: ProbeFacts,
    pub readiness: ExecutionReadiness,
    pub warnings: Vec<String>,
    pub issue_finder_boundary: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentWorkspace {
    pub path: String,
    pub default_branch: String,
    pub branch: String,
    pub dirty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InteractionPolicyContract {
    pub version: u8,
    pub maintainer_policy: String,
    pub github_policy: String,
    pub network_policy: String,
    pub approval_required_for: Vec<String>,
    pub forbidden_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionContextContract {
    pub version: u8,
    pub source_handoff_id: String,
    pub handoff_artifact_id: String,
    pub packaged_by_approval_request_id: String,
    pub initial_read_order: Vec<String>,
    pub resumability: ResumabilityContract,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResumabilityContract {
    pub resume_strategy: String,
    pub checkpoint_artifact: String,
    pub required_result_artifact: String,
    pub session_metadata_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OutcomeContract {
    pub version: u8,
    pub required_artifact: String,
    pub required_fields: Vec<String>,
    pub status_values: Vec<String>,
    pub reproduction_fields: Vec<String>,
    pub success_criteria_fields: Vec<String>,
    pub validation_requirements: Vec<String>,
    pub optional_artifacts: Vec<String>,
    pub memory_inputs: OutcomeMemoryInputs,
    pub failure_contract: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OutcomeMemoryInputs {
    pub agent_id: bool,
    pub task_type: String,
    pub validation_paths: bool,
    pub failure_reason: bool,
    pub artifact_refs: bool,
    pub reproduction_attempted: bool,
    pub success_criteria_status: bool,
}

impl IssueTaskPackage {
    pub fn new(issue: IssueTaskPackageIssue) -> Self {
        Self {
            kind: PACKAGE_KIND.to_string(),
            version: PACKAGE_VERSION,
            issue,
            source: PackageSource {
                source_handoff_id: String::new(),
                handoff_artifact_id: String::new(),
                packaged_by_approval_request_id: String::new(),
                package_version: PACKAGE_VERSION,
            },
            evidence: PackageEvidence {
                handoff_artifact_id: String::new(),
                value_assessment: ValueAssessment::default(),
                recommendation: RecommendationAssessment::default(),
                evidence_pack: EvidencePack::empty(),
                source_refs: Vec::new(),
            },
            llm_confirmation: LlmConfirmation::disabled(),
            human_review: PackageHumanReview {
                approval_request_id: String::new(),
                status: ApprovalStatus::Approved,
                resolved_at: None,
                details: Value::Null,
            },
            user_profile_snapshot: Value::Null,
            workspace_policy: WorkspacePolicyContract {
                workspace: HandoffWorkspace {
                    path: String::new(),
                    default_branch: String::new(),
                    branch: String::new(),
                    dirty: false,
                },
                agent_policy: AgentPolicyManifest::default(),
                issue_finder_boundary: issue_finder_boundary(),
            },
            context_pack: PackageContextPack {
                context: HandoffContext {
                    candidate_files: Vec::new(),
                    validation_commands: Vec::new(),
                    warnings: Vec::new(),
                },
                context_pack: crate::context_pack::default_context_pack(),
                probe_pack: ProbePack::default(),
                initial_read_order: initial_read_order(),
            },
            validation_hints: ValidationHints {
                readiness: ExecutionReadiness::default(),
                validation_commands: Vec::new(),
                validation_obligations: validation_obligations(),
            },
            memory_context: HandoffMemoryContext::default(),
            expected_outputs: vec![FIX_RESULT_ARTIFACT.to_string()],
            callback_policy: CallbackPolicy {
                expected_artifacts: vec![FIX_RESULT_ARTIFACT.to_string()],
                optional_artifacts: optional_artifacts(),
                source_handoff_id: String::new(),
                import_mode: "local_artifact_only".to_string(),
            },
            reproduction_contract: default_reproduction_contract(
                false,
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            success_criteria: default_success_criteria(),
            change_budget: default_change_budget(Vec::new()),
            environment_contract: EnvironmentContract {
                version: 1,
                workspace: EnvironmentWorkspace {
                    path: String::new(),
                    default_branch: String::new(),
                    branch: String::new(),
                    dirty: false,
                },
                safe_probe_status: "not_run".to_string(),
                probe_facts: ProbeFacts::default(),
                readiness: ExecutionReadiness::default(),
                warnings: Vec::new(),
                issue_finder_boundary: issue_finder_boundary(),
            },
            interaction_policy: default_interaction_policy(),
            session_context: default_session_context("", "", ""),
            outcome_contract: default_outcome_contract(),
        }
    }

    pub fn from_reviewed_handoff(
        handoff: &Handoff,
        handoff_artifact_id: &str,
        user_profile_snapshot: Value,
        review_approval: &ApprovalRequest,
    ) -> Self {
        let expected_outputs = normalized_expected_outputs(&handoff.instructions.expected_output);
        let source_refs = handoff.evidence_pack.source_refs.clone();
        let validation_commands = handoff.context.validation_commands.clone();
        let candidate_files = handoff.context.candidate_files.clone();
        let initial_read_order = initial_read_order();
        let workspace = EnvironmentWorkspace {
            path: handoff.workspace.path.clone(),
            default_branch: handoff.workspace.default_branch.clone(),
            branch: handoff.workspace.branch.clone(),
            dirty: handoff.workspace.dirty,
        };
        let mut warnings = handoff.context.warnings.clone();
        warnings.extend(handoff.probe_pack.warnings.clone());
        warnings.extend(handoff.readiness.warnings.clone());
        warnings.sort();
        warnings.dedup();

        Self {
            kind: PACKAGE_KIND.to_string(),
            version: PACKAGE_VERSION,
            issue: IssueTaskPackageIssue {
                repo_full_name: handoff.issue.repo_full_name.clone(),
                number: handoff.issue.number,
                title: handoff.issue.title.clone(),
                url: handoff.issue.url.clone(),
            },
            source: PackageSource {
                source_handoff_id: handoff.id.clone(),
                handoff_artifact_id: handoff_artifact_id.to_string(),
                packaged_by_approval_request_id: review_approval.id.clone(),
                package_version: PACKAGE_VERSION,
            },
            evidence: PackageEvidence {
                handoff_artifact_id: handoff_artifact_id.to_string(),
                value_assessment: handoff.value_assessment.clone(),
                recommendation: handoff.recommendation.clone(),
                evidence_pack: handoff.evidence_pack.clone(),
                source_refs: source_refs.clone(),
            },
            llm_confirmation: handoff.llm_confirmation.clone(),
            human_review: PackageHumanReview {
                approval_request_id: review_approval.id.clone(),
                status: review_approval.status,
                resolved_at: review_approval.resolved_at.clone(),
                details: review_approval.details_json.clone(),
            },
            user_profile_snapshot,
            workspace_policy: WorkspacePolicyContract {
                workspace: handoff.workspace.clone(),
                agent_policy: handoff.agent_policy.clone(),
                issue_finder_boundary: issue_finder_boundary(),
            },
            context_pack: PackageContextPack {
                context: handoff.context.clone(),
                context_pack: handoff.context_pack.clone(),
                probe_pack: handoff.probe_pack.clone(),
                initial_read_order: initial_read_order.clone(),
            },
            validation_hints: ValidationHints {
                readiness: handoff.readiness.clone(),
                validation_commands: validation_commands.clone(),
                validation_obligations: validation_obligations(),
            },
            memory_context: handoff.memory_context.clone(),
            expected_outputs,
            callback_policy: CallbackPolicy {
                expected_artifacts: vec![FIX_RESULT_ARTIFACT.to_string()],
                optional_artifacts: optional_artifacts(),
                source_handoff_id: handoff.id.clone(),
                import_mode: "local_artifact_only".to_string(),
            },
            reproduction_contract: default_reproduction_contract(
                !handoff.issue.body.trim().is_empty(),
                handoff.instructions.suggested_start.clone(),
                validation_commands.clone(),
                source_refs,
            ),
            success_criteria: default_success_criteria(),
            change_budget: default_change_budget(candidate_files),
            environment_contract: EnvironmentContract {
                version: 1,
                workspace,
                safe_probe_status: handoff.probe_pack.status.clone(),
                probe_facts: handoff.probe_pack.facts.clone(),
                readiness: handoff.readiness.clone(),
                warnings,
                issue_finder_boundary: issue_finder_boundary(),
            },
            interaction_policy: default_interaction_policy(),
            session_context: default_session_context(
                &handoff.id,
                handoff_artifact_id,
                &review_approval.id,
            ),
            outcome_contract: default_outcome_contract(),
        }
    }
}

fn normalized_expected_outputs(values: &[String]) -> Vec<String> {
    let mut outputs = values
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if !outputs.iter().any(|value| value == FIX_RESULT_ARTIFACT) {
        outputs.insert(0, FIX_RESULT_ARTIFACT.to_string());
    }
    outputs
}

fn default_reproduction_contract(
    issue_body_available: bool,
    suggested_start: Vec<String>,
    validation_commands: Vec<ValidationCommand>,
    evidence_refs: Vec<String>,
) -> ReproductionContract {
    ReproductionContract {
        version: 1,
        strategy: "Use the issue body, context pack, candidate files, and validation hints to attempt reproduction when practical; do not infer missing reproduction steps as facts.".to_string(),
        obligations: vec![
            "Read the issue body or issue context before editing.".to_string(),
            "State whether reproduction was attempted, skipped, or blocked.".to_string(),
            "Record commands, inputs, observations, and blocker reasons in fix_result.json.".to_string(),
            "If reproduction is impractical, explain why and validate the smallest relevant behavior instead.".to_string(),
        ],
        evidence_refs,
        issue_body_available,
        suggested_start,
        validation_commands,
        blocker_policy: "A missing setup, missing credentials, network requirement, dependency install, or unsafe command is a blocker to record, not a reason to fabricate reproduction evidence.".to_string(),
    }
}

fn default_success_criteria() -> SuccessCriteriaContract {
    SuccessCriteriaContract {
        version: 1,
        criteria: vec![
            "The implemented change directly addresses the GitHub issue described by this package.".to_string(),
            "The patch is scoped to the relevant repository files and avoids unrelated refactors.".to_string(),
            "Validation was run when safe and approved, or the result explains why validation could not be run.".to_string(),
            "Residual risks and any incomplete reproduction evidence are explicitly reported.".to_string(),
            "The suggested GitHub reply is ready for review but is not posted automatically.".to_string(),
        ],
        validation_expectations: validation_obligations(),
        done_definition: "A local fix is ready for human review when fix_result.json satisfies the outcome contract and any patch remains inside the target workspace.".to_string(),
    }
}

fn default_change_budget(preferred_files: Vec<CandidateFile>) -> ChangeBudgetContract {
    ChangeBudgetContract {
        version: 1,
        scope: "Prefer the smallest targeted source and test change that resolves the issue."
            .to_string(),
        max_files_hint: preferred_files.len().clamp(1, 5),
        preferred_files,
        allowed_changes: vec![
            "Read and edit target workspace source files relevant to the issue.".to_string(),
            "Add or update focused tests when they are the narrowest validation path.".to_string(),
            "Update local documentation only when it is necessary to keep the fix accurate."
                .to_string(),
        ],
        escalation_triggers: vec![
            "Changing broad architecture or public APIs.".to_string(),
            "Touching files outside candidate areas without a clear reason.".to_string(),
            "Needing dependency installation, generated artifacts, credentials, or network access."
                .to_string(),
            "Finding the issue already fixed, claimed, or blocked by maintainer direction."
                .to_string(),
        ],
        forbidden_without_approval: vec![
            "Committing, pushing, or creating a pull request.".to_string(),
            "Installing dependencies or running networked setup.".to_string(),
            "Resetting, cleaning, or deleting target workspace state.".to_string(),
            "Editing Issue Finder inbox, context, or artifact files as target source.".to_string(),
        ],
    }
}

fn default_interaction_policy() -> InteractionPolicyContract {
    InteractionPolicyContract {
        version: 1,
        maintainer_policy: "Do not contact maintainers or claim the issue automatically; draft any question or status update for user approval.".to_string(),
        github_policy: "Issue Finder may draft local GitHub comments, but posting requires an explicit later approval.".to_string(),
        network_policy: "Network access, dependency installation, external services, and repository-defined commands require user approval or native agent approval.".to_string(),
        approval_required_for: vec![
            "Posting GitHub comments.".to_string(),
            "Opening pull requests.".to_string(),
            "Running validation that executes repository code.".to_string(),
            "Installing dependencies or using network services.".to_string(),
            "Changing native session state beyond the approved dispatch.".to_string(),
        ],
        forbidden_actions: vec![
            "Commit.".to_string(),
            "Push.".to_string(),
            "Create pull requests.".to_string(),
            "Modify Issue Finder-generated artifacts as target source.".to_string(),
            "Overwrite unrelated local changes.".to_string(),
        ],
    }
}

fn default_session_context(
    source_handoff_id: &str,
    handoff_artifact_id: &str,
    approval_request_id: &str,
) -> SessionContextContract {
    SessionContextContract {
        version: 1,
        source_handoff_id: source_handoff_id.to_string(),
        handoff_artifact_id: handoff_artifact_id.to_string(),
        packaged_by_approval_request_id: approval_request_id.to_string(),
        initial_read_order: initial_read_order(),
        resumability: ResumabilityContract {
            resume_strategy: "On resume, reload this package, inspect existing workspace changes, then continue from the latest local result or session transcript artifact.".to_string(),
            checkpoint_artifact: FIX_RESULT_ARTIFACT.to_string(),
            required_result_artifact: FIX_RESULT_ARTIFACT.to_string(),
            session_metadata_keys: vec![
                "runId".to_string(),
                "issueTaskId".to_string(),
                "issueKey".to_string(),
                "packageArtifactId".to_string(),
                "packagePath".to_string(),
            ],
        },
    }
}

fn default_outcome_contract() -> OutcomeContract {
    OutcomeContract {
        version: 2,
        required_artifact: FIX_RESULT_ARTIFACT.to_string(),
        required_fields: vec![
            "status".to_string(),
            "summary".to_string(),
            "changedFiles".to_string(),
            "reproduction".to_string(),
            "successCriteria".to_string(),
            "validation".to_string(),
            "residualRisks".to_string(),
            "failureReason".to_string(),
            "suggestedGitHubReply".to_string(),
            "sessionContext".to_string(),
        ],
        status_values: vec![
            "success".to_string(),
            "partial".to_string(),
            "failed".to_string(),
            "needs_user".to_string(),
        ],
        reproduction_fields: vec![
            "attempted".to_string(),
            "status".to_string(),
            "commands".to_string(),
            "observations".to_string(),
            "blockers".to_string(),
        ],
        success_criteria_fields: vec![
            "criteriaMet".to_string(),
            "criteriaNotMet".to_string(),
            "notes".to_string(),
        ],
        validation_requirements: validation_obligations(),
        optional_artifacts: optional_artifacts(),
        memory_inputs: OutcomeMemoryInputs {
            agent_id: true,
            task_type: "fix_github_issue".to_string(),
            validation_paths: true,
            failure_reason: true,
            artifact_refs: true,
            reproduction_attempted: true,
            success_criteria_status: true,
        },
        failure_contract: vec![
            "Use needs_user when blocked by approval, credentials, unsafe commands, or missing maintainer context.".to_string(),
            "Use failed only when the task cannot progress and no useful partial fix exists.".to_string(),
            "Always include failureReason when status is failed, partial, or needs_user.".to_string(),
        ],
    }
}

fn initial_read_order() -> Vec<String> {
    vec![
        "issue_task_package_v3.json".to_string(),
        "handoff.json".to_string(),
        "context/entry.md".to_string(),
        "context/safety.md".to_string(),
        "context/probe.md".to_string(),
        "context/issue.md".to_string(),
        "context/repo.md".to_string(),
        "context/validation.md".to_string(),
    ]
}

fn validation_obligations() -> Vec<String> {
    vec![
        "Prefer focused validation tied to the changed files.".to_string(),
        "Record every validation command, exit status, and relevant output summary.".to_string(),
        "If a command requires approval, network, dependencies, or credentials, ask or record it as blocked.".to_string(),
    ]
}

fn optional_artifacts() -> Vec<String> {
    vec![
        "patch".to_string(),
        "pr_link".to_string(),
        "session_link".to_string(),
        "validation_log".to_string(),
    ]
}

fn issue_finder_boundary() -> Vec<String> {
    vec![
        "Issue Finder prepares local workspace and handoff artifacts only.".to_string(),
        "Issue Finder must not modify target repository source itself.".to_string(),
        "Issue Finder must not install dependencies, commit, push, create pull requests, or post GitHub comments.".to_string(),
    ]
}
