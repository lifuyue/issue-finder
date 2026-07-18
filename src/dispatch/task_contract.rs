use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context_snapshot::StoredContextSnapshot;
use crate::handoff::Handoff;

use super::model::{AgentArtifact, ApprovalRequest, ApprovalStatus};
use super::store::DispatchStore;

const PACKAGE_KIND: &str = "issue_finder_task_package";
const PACKAGE_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskPackage {
    pub kind: String,
    pub version: u8,
    pub identity: TaskIdentity,
    pub goal: TaskGoal,
    pub constraints: TaskConstraints,
    pub success_criteria: Vec<String>,
    pub context_snapshot: TaskContextSnapshot,
    pub workspace: TaskWorkspace,
    pub interaction_policy: TaskInteractionPolicy,
    pub runtime_policy: TaskRuntimePolicy,
    pub result_contract: TaskResultContract,
    pub provenance: TaskProvenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskIdentity {
    pub repo_full_name: String,
    pub issue_number: u64,
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskGoal {
    pub objective: String,
    pub suggested_start: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskConstraints {
    pub instructions: Vec<String>,
    pub forbidden_actions: Vec<String>,
    pub preferred_files: Vec<String>,
    pub max_files_hint: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskContextSnapshot {
    pub snapshot_id: String,
    pub artifact_id: String,
    pub entry_artifact_id: String,
    pub initial_artifact_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskWorkspace {
    pub path: String,
    pub default_branch: String,
    pub branch: String,
    pub dirty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskInteractionPolicy {
    pub approval_required_for: Vec<String>,
    pub external_publication: String,
    pub dependency_installation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskRuntimePolicy {
    pub max_attempts: u32,
    pub max_time_seconds: u64,
    pub token_budget: Option<u64>,
    pub sandbox: String,
    pub network_access: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskResultContract {
    pub tool: String,
    pub statuses: Vec<String>,
    pub required_fields: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskProvenance {
    pub handoff_id: String,
    pub handoff_artifact_id: String,
    pub context_snapshot_artifact_id: String,
    pub profile_snapshot_artifact_id: String,
    pub review_approval_id: String,
    pub review_resolved_at: Option<String>,
    pub prepare_category: String,
    pub value_evidence_refs: Vec<String>,
    pub review_details: Value,
}

impl TaskPackage {
    pub fn new(identity: TaskIdentity) -> Self {
        Self {
            kind: PACKAGE_KIND.to_string(),
            version: PACKAGE_VERSION,
            identity,
            goal: TaskGoal {
                objective: "Resolve the approved GitHub issue and report verifiable evidence."
                    .to_string(),
                suggested_start: Vec::new(),
            },
            constraints: TaskConstraints {
                instructions: Vec::new(),
                forbidden_actions: default_forbidden_actions(),
                preferred_files: Vec::new(),
                max_files_hint: 8,
            },
            success_criteria: default_success_criteria(),
            context_snapshot: TaskContextSnapshot {
                snapshot_id: String::new(),
                artifact_id: String::new(),
                entry_artifact_id: String::new(),
                initial_artifact_ids: Vec::new(),
            },
            workspace: TaskWorkspace {
                path: String::new(),
                default_branch: String::new(),
                branch: String::new(),
                dirty: false,
            },
            interaction_policy: default_interaction_policy(),
            runtime_policy: default_runtime_policy(),
            result_contract: default_result_contract(),
            provenance: TaskProvenance {
                handoff_id: String::new(),
                handoff_artifact_id: String::new(),
                context_snapshot_artifact_id: String::new(),
                profile_snapshot_artifact_id: String::new(),
                review_approval_id: String::new(),
                review_resolved_at: None,
                prepare_category: String::new(),
                value_evidence_refs: Vec::new(),
                review_details: Value::Null,
            },
        }
    }

    pub fn from_reviewed_handoff(
        store: &DispatchStore,
        handoff: &Handoff,
        handoff_artifact: &AgentArtifact,
        context_snapshot_artifact: &AgentArtifact,
        profile_snapshot_artifact: &AgentArtifact,
        review: &ApprovalRequest,
    ) -> Result<Self> {
        if review.status != ApprovalStatus::Approved {
            anyhow::bail!("task package requires an approved issue review");
        }
        let stored: StoredContextSnapshot =
            serde_json::from_slice(&store.read_artifact_bytes(&context_snapshot_artifact.id)?)
                .context("context snapshot artifact is invalid")?;
        if stored.snapshot.handoff_id != handoff.id {
            anyhow::bail!("context snapshot does not belong to reviewed handoff");
        }
        let entry_artifact_id = artifact_for_path(&stored, "codex.md")?;
        let initial_artifact_ids = ["context/entry.md", "context/safety.md", "context/probe.md"]
            .into_iter()
            .map(|path| artifact_for_path(&stored, path))
            .collect::<Result<Vec<_>>>()?;
        let success_criteria = if handoff.instructions.expected_output.is_empty() {
            default_success_criteria()
        } else {
            handoff.instructions.expected_output.clone()
        };
        let preferred_files = handoff
            .context
            .candidate_files
            .iter()
            .map(|file| file.path.clone())
            .collect();
        let mut package = Self {
            kind: PACKAGE_KIND.to_string(),
            version: PACKAGE_VERSION,
            identity: TaskIdentity {
                repo_full_name: handoff.issue.repo_full_name.clone(),
                issue_number: handoff.issue.number,
                title: handoff.issue.title.clone(),
                url: handoff.issue.url.clone(),
            },
            goal: TaskGoal {
                objective: handoff.instructions.goal.clone(),
                suggested_start: handoff.instructions.suggested_start.clone(),
            },
            constraints: TaskConstraints {
                instructions: handoff.instructions.constraints.clone(),
                forbidden_actions: default_forbidden_actions(),
                preferred_files,
                max_files_hint: 8,
            },
            success_criteria,
            context_snapshot: TaskContextSnapshot {
                snapshot_id: stored.snapshot.id,
                artifact_id: context_snapshot_artifact.id.clone(),
                entry_artifact_id,
                initial_artifact_ids,
            },
            workspace: TaskWorkspace {
                path: handoff.workspace.path.clone(),
                default_branch: handoff.workspace.default_branch.clone(),
                branch: handoff.workspace.branch.clone(),
                dirty: handoff.workspace.dirty,
            },
            interaction_policy: default_interaction_policy(),
            runtime_policy: default_runtime_policy(),
            result_contract: default_result_contract(),
            provenance: TaskProvenance {
                handoff_id: handoff.id.clone(),
                handoff_artifact_id: handoff_artifact.id.clone(),
                context_snapshot_artifact_id: context_snapshot_artifact.id.clone(),
                profile_snapshot_artifact_id: profile_snapshot_artifact.id.clone(),
                review_approval_id: review.id.clone(),
                review_resolved_at: review.resolved_at.clone(),
                prepare_category: handoff.value_assessment.recommendation_category.to_string(),
                value_evidence_refs: handoff.evidence_pack.source_refs.clone(),
                review_details: review.details_json.clone(),
            },
        };
        package.constraints.instructions.sort();
        package.constraints.instructions.dedup();
        package.validate_for_execution()?;
        Ok(package)
    }

    pub fn validate_for_execution(&self) -> Result<()> {
        if self.kind != PACKAGE_KIND || self.version != PACKAGE_VERSION {
            anyhow::bail!("unsupported task package kind or version");
        }
        if self.identity.repo_full_name.trim().is_empty()
            || self.identity.issue_number == 0
            || self.identity.url.trim().is_empty()
        {
            anyhow::bail!("task identity is incomplete");
        }
        if self.goal.objective.trim().is_empty() || self.success_criteria.is_empty() {
            anyhow::bail!("task goal and success criteria are required");
        }
        if self.context_snapshot.snapshot_id.trim().is_empty()
            || self.context_snapshot.artifact_id.trim().is_empty()
            || self.context_snapshot.entry_artifact_id.trim().is_empty()
        {
            anyhow::bail!("task context snapshot is incomplete");
        }
        if !Path::new(&self.workspace.path).is_absolute() {
            anyhow::bail!("task workspace path must be absolute");
        }
        if self.runtime_policy.max_attempts == 0 || self.runtime_policy.max_time_seconds == 0 {
            anyhow::bail!("task runtime budgets must be positive");
        }
        if self.result_contract.tool != "issue-finder.submit_result"
            || self.result_contract.statuses != ["success", "partial", "failed", "needs_user"]
        {
            anyhow::bail!("task result contract is invalid");
        }
        Ok(())
    }
}

fn artifact_for_path(snapshot: &StoredContextSnapshot, path: &str) -> Result<String> {
    snapshot
        .files
        .iter()
        .find(|file| file.relative_path == path)
        .map(|file| file.artifact_id.clone())
        .with_context(|| format!("context snapshot is missing {path}"))
}

fn default_success_criteria() -> Vec<String> {
    vec![
        "The issue's observable behavior is resolved or its non-reproducibility is demonstrated."
            .to_string(),
        "Relevant focused validation succeeds, or blockers are reported with evidence.".to_string(),
        "The submitted result matches the observed workspace diff and command evidence."
            .to_string(),
    ]
}

fn default_forbidden_actions() -> Vec<String> {
    vec![
        "commit_or_push".to_string(),
        "create_pull_request".to_string(),
        "post_github_comment".to_string(),
        "overwrite_unrelated_changes".to_string(),
    ]
}

fn default_interaction_policy() -> TaskInteractionPolicy {
    TaskInteractionPolicy {
        approval_required_for: vec![
            "dependency_installation".to_string(),
            "network_access".to_string(),
            "filesystem_outside_workspace".to_string(),
            "external_publication".to_string(),
        ],
        external_publication: "forbidden_without_human_approval".to_string(),
        dependency_installation: "request_approval".to_string(),
    }
}

fn default_runtime_policy() -> TaskRuntimePolicy {
    TaskRuntimePolicy {
        max_attempts: 3,
        max_time_seconds: 3600,
        token_budget: None,
        sandbox: "workspaceWrite".to_string(),
        network_access: false,
    }
}

fn default_result_contract() -> TaskResultContract {
    TaskResultContract {
        tool: "issue-finder.submit_result".to_string(),
        statuses: ["success", "partial", "failed", "needs_user"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        required_fields: [
            "status",
            "summary",
            "changedFiles",
            "reproduction",
            "successCriteria",
            "validation",
            "residualRisks",
            "failureReason",
            "suggestedGitHubReply",
            "sessionContext",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
    }
}
