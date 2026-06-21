use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

use crate::config::Config;
use crate::github::IssueRef;
use crate::handoff::Handoff;
use crate::inbox;
use crate::paths::IssueFinderPaths;

use super::memory::record_issue_review_signal;
use super::model::{
    AgentArtifact, ApprovalRequest, ApprovalStatus, ApprovalType, IssueTask, IssueTaskStatus,
    MemoryEvent, NewApprovalRequest, NewArtifact, NewIssueTask,
};
use super::store::DispatchStore;
use super::task_package::IssueTaskPackage;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PackageImportResult {
    pub status: String,
    pub issue_task: IssueTask,
    pub handoff_artifact: AgentArtifact,
    pub profile_snapshot_artifact: AgentArtifact,
    pub approval_request: ApprovalRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_artifact: Option<AgentArtifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<IssueTaskPackage>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IssueReviewDetail {
    pub issue_task: IssueTask,
    pub approval_request: ApprovalRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handoff_artifact: Option<AgentArtifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_snapshot_artifact: Option<AgentArtifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_artifact: Option<AgentArtifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<IssueTaskPackage>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IssueReviewResolution {
    pub issue_task: IssueTask,
    pub approval_request: ApprovalRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_artifact: Option<AgentArtifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<IssueTaskPackage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_event: Option<MemoryEvent>,
}

pub fn import_handoff_from_inbox(
    store: &DispatchStore,
    inbox_id: &str,
) -> Result<PackageImportResult> {
    let item = inbox::find_item(&store.paths(), inbox_id)?;
    import_handoff_item(store, &item)
}

pub fn ensure_issue_task_for_issue(store: &DispatchStore, issue: &str) -> Result<IssueTask> {
    let issue_ref = IssueRef::parse(issue)?;
    let issue_key = issue_key(&issue_ref);
    match store.find_issue_task_by_key(&issue_key)? {
        Some(issue_task) => Ok(issue_task),
        None => Ok(import_ready_handoff_for_issue(store, &issue_ref, &issue_key)?.issue_task),
    }
}

pub fn ensure_packaged_issue_task_for_issue(
    store: &DispatchStore,
    issue: &str,
) -> Result<IssueTask> {
    let issue_ref = IssueRef::parse(issue)?;
    let issue_key = issue_key(&issue_ref);
    match store.find_issue_task_by_key(&issue_key)? {
        Some(issue_task) if issue_task.current_package_artifact_id.is_some() => Ok(issue_task),
        Some(issue_task) => {
            if let Some(review) = pending_issue_review_for_task(store, &issue_task.id)? {
                anyhow::bail!(pending_issue_review_message(&issue_key, &review.id));
            }
            let imported = import_ready_handoff_for_issue(store, &issue_ref, &issue_key)?;
            match imported.package_artifact {
                Some(_) => Ok(imported.issue_task),
                None => anyhow::bail!(issue_review_block_message(
                    &issue_key,
                    &imported.approval_request
                )),
            }
        }
        None => {
            let imported = import_ready_handoff_for_issue(store, &issue_ref, &issue_key)?;
            match imported.package_artifact {
                Some(_) => Ok(imported.issue_task),
                None => anyhow::bail!(issue_review_block_message(
                    &issue_key,
                    &imported.approval_request
                )),
            }
        }
    }
}

pub fn list_issue_reviews(store: &DispatchStore) -> Result<Vec<IssueReviewDetail>> {
    store
        .list_approval_requests_by_type(ApprovalType::IssueReview)?
        .into_iter()
        .map(|approval| issue_review_detail(store, approval))
        .collect()
}

pub fn show_issue_review(
    store: &DispatchStore,
    approval_request_id: &str,
) -> Result<IssueReviewDetail> {
    let approval = store.get_approval_request(approval_request_id)?;
    if approval.approval_type != ApprovalType::IssueReview {
        anyhow::bail!(
            "approval request {} is {}, not issue_review",
            approval.id,
            approval.approval_type
        );
    }
    issue_review_detail(store, approval)
}

pub fn approve_issue_review(
    store: &DispatchStore,
    approval_request_id: &str,
) -> Result<IssueReviewResolution> {
    let pending = pending_issue_review(store, approval_request_id)?;
    let issue_task = issue_task_from_review(store, &pending)?;
    let handoff_artifact = artifact_from_review(store, &pending, "handoffArtifactId")?;
    let handoff =
        serde_json::from_slice::<Handoff>(&store.read_artifact_bytes(&handoff_artifact.id)?)
            .context("review handoff artifact is not valid handoff JSON")?;
    let profile_snapshot_artifact =
        artifact_from_review(store, &pending, "profileSnapshotArtifactId")?;
    let profile_snapshot =
        serde_json::from_slice::<Value>(&store.read_artifact_bytes(&profile_snapshot_artifact.id)?)
            .context("profile snapshot artifact is not valid JSON")?;

    let approval_request = store.resolve_approval_request(&pending.id, ApprovalStatus::Approved)?;
    let package = IssueTaskPackage::from_reviewed_handoff(
        &handoff,
        &handoff_artifact.id,
        json!({
            "artifactId": profile_snapshot_artifact.id,
            "snapshot": profile_snapshot
        }),
        &approval_request,
    );
    let package_artifact = store.write_task_package_artifact(&issue_task.id, &package)?;
    let issue_task =
        store.update_issue_task_status(&issue_task.id, IssueTaskStatus::UserApproved)?;
    let memory_event = record_issue_review_signal(store, &issue_task, &approval_request, None)?;

    Ok(IssueReviewResolution {
        issue_task,
        approval_request,
        package_artifact: Some(package_artifact),
        package: Some(package),
        memory_event,
    })
}

pub fn reject_issue_review(
    store: &DispatchStore,
    approval_request_id: &str,
    reason: Option<String>,
) -> Result<IssueReviewResolution> {
    let pending = pending_issue_review(store, approval_request_id)?;
    let issue_task = issue_task_from_review(store, &pending)?;
    let approval_request = store.resolve_approval_request(&pending.id, ApprovalStatus::Rejected)?;
    let memory_event = record_issue_review_signal(store, &issue_task, &approval_request, reason)?;

    Ok(IssueReviewResolution {
        issue_task,
        approval_request,
        package_artifact: None,
        package: None,
        memory_event,
    })
}

fn import_ready_handoff_for_issue(
    store: &DispatchStore,
    issue_ref: &IssueRef,
    issue_key: &str,
) -> Result<PackageImportResult> {
    let repo_full_name = issue_ref.repo_full_name();
    let mut candidates = inbox::load_index(&store.paths())?
        .items
        .into_iter()
        .filter(|item| {
            item.repo_full_name == repo_full_name
                && item.issue_number == issue_ref.number
                && item.status == inbox::InboxStatus::Ready
                && !item.handoff_json_path.trim().is_empty()
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.score.cmp(&left.score))
    });
    let item = candidates.into_iter().next().with_context(|| {
        format!("issue task {issue_key} has no task package artifact and no ready inbox handoff was found")
    })?;
    import_handoff_item(store, &item)
}

fn import_handoff_item(
    store: &DispatchStore,
    item: &inbox::InboxItem,
) -> Result<PackageImportResult> {
    let raw = std::fs::read(&item.handoff_json_path)
        .with_context(|| format!("unable to read {}", item.handoff_json_path))?;
    let handoff = serde_json::from_slice::<Handoff>(&raw)
        .with_context(|| format!("unable to parse {}", item.handoff_json_path))?;
    if handoff.issue.repo_full_name != item.repo_full_name
        || handoff.issue.number != item.issue_number
    {
        anyhow::bail!(
            "handoff {} describes {}#{}, but inbox item {} points to {}#{}",
            item.handoff_json_path,
            handoff.issue.repo_full_name,
            handoff.issue.number,
            item.id,
            item.repo_full_name,
            item.issue_number
        );
    }

    let issue_ref = IssueRef::parse(&format!(
        "{}#{}",
        handoff.issue.repo_full_name, handoff.issue.number
    ))?;
    let issue_key = issue_key(&issue_ref);
    let existing = store.find_issue_task_by_key(&issue_key)?;
    let issue_task = store.upsert_issue_task(NewIssueTask {
        repo_full_name: handoff.issue.repo_full_name.clone(),
        issue_number: handoff.issue.number,
        title: handoff.issue.title.clone(),
        url: handoff.issue.url.clone(),
        status: review_candidate_status(existing.as_ref(), &handoff),
        priority: Some(item.score.into()),
        category: Some(handoff.value_assessment.recommendation_category.to_string()),
    })?;
    if let Some(existing) = existing_import_result(store, &issue_task, &item.id)? {
        return Ok(existing);
    }

    let handoff_artifact = store.write_artifact(
        NewArtifact {
            issue_task_id: Some(issue_task.id.clone()),
            run_id: None,
            kind: "handoff_json".to_string(),
            content_type: "application/json".to_string(),
            metadata_json: json!({
                "inboxId": item.id,
                "sourcePath": item.handoff_json_path
            }),
        },
        raw,
    )?;
    let profile_snapshot = user_profile_snapshot(&store.paths());
    let profile_snapshot_artifact =
        store.write_profile_snapshot_artifact(&issue_task.id, &profile_snapshot)?;
    let approval_request = create_issue_review_approval(
        store,
        &issue_task,
        &handoff_artifact,
        &profile_snapshot_artifact,
        item,
        &handoff,
    )?;

    Ok(PackageImportResult {
        status: package_import_status(&approval_request, None),
        issue_task: store.get_issue_task(&issue_task.id)?,
        handoff_artifact,
        profile_snapshot_artifact,
        approval_request,
        package_artifact: None,
        package: None,
    })
}

fn existing_import_result(
    store: &DispatchStore,
    issue_task: &IssueTask,
    inbox_id: &str,
) -> Result<Option<PackageImportResult>> {
    let artifacts = store.list_artifacts_for_issue_task(&issue_task.id)?;
    let Some(handoff_artifact) = artifacts
        .iter()
        .rev()
        .find(|artifact| {
            artifact.kind == "handoff_json"
                && artifact
                    .metadata_json
                    .get("inboxId")
                    .and_then(Value::as_str)
                    == Some(inbox_id)
        })
        .cloned()
    else {
        return Ok(None);
    };
    let profile_snapshot_artifact = issue_task
        .profile_snapshot_artifact_id
        .as_deref()
        .map(|artifact_id| store.get_artifact(artifact_id))
        .transpose()?
        .with_context(|| {
            format!(
                "issue task {} has a prior handoff import but no profile snapshot artifact",
                issue_task.issue_key
            )
        })?;
    let approval_request =
        issue_review_for_inbox(store, &issue_task.id, inbox_id)?.with_context(|| {
            format!(
                "issue task {} has a prior handoff import but no issue review approval",
                issue_task.issue_key
            )
        })?;
    let (package_artifact, package) = read_current_package(store, issue_task)?;

    Ok(Some(PackageImportResult {
        status: package_import_status(&approval_request, package_artifact.as_ref()),
        issue_task: store.get_issue_task(&issue_task.id)?,
        handoff_artifact,
        profile_snapshot_artifact,
        approval_request,
        package_artifact,
        package,
    }))
}

fn create_issue_review_approval(
    store: &DispatchStore,
    issue_task: &IssueTask,
    handoff_artifact: &AgentArtifact,
    profile_snapshot_artifact: &AgentArtifact,
    item: &inbox::InboxItem,
    handoff: &Handoff,
) -> Result<ApprovalRequest> {
    store.create_approval_request(NewApprovalRequest {
        run_id: None,
        approval_type: ApprovalType::IssueReview,
        status: ApprovalStatus::Pending,
        prompt: format!(
            "Approve {} as an IssueTaskPackage v3 candidate?",
            issue_task.issue_key
        ),
        details_json: json!({
            "issueTaskId": issue_task.id,
            "issueKey": issue_task.issue_key,
            "inboxId": item.id,
            "handoffArtifactId": handoff_artifact.id,
            "profileSnapshotArtifactId": profile_snapshot_artifact.id,
            "packageVersion": 3,
            "priority": item.score,
            "category": handoff.value_assessment.recommendation_category,
            "llmConfirmation": handoff.llm_confirmation,
            "reviewKind": "issue_task_package_v3"
        }),
    })
}

fn issue_review_detail(
    store: &DispatchStore,
    approval_request: ApprovalRequest,
) -> Result<IssueReviewDetail> {
    let issue_task = issue_task_from_review(store, &approval_request)?;
    let handoff_artifact =
        optional_artifact_from_review(store, &approval_request, "handoffArtifactId")?;
    let profile_snapshot_artifact =
        optional_artifact_from_review(store, &approval_request, "profileSnapshotArtifactId")?;
    let (package_artifact, package) = read_current_package(store, &issue_task)?;
    Ok(IssueReviewDetail {
        issue_task,
        approval_request,
        handoff_artifact,
        profile_snapshot_artifact,
        package_artifact,
        package,
    })
}

fn read_current_package(
    store: &DispatchStore,
    issue_task: &IssueTask,
) -> Result<(Option<AgentArtifact>, Option<IssueTaskPackage>)> {
    let Some(package_artifact_id) = issue_task.current_package_artifact_id.as_deref() else {
        return Ok((None, None));
    };
    let package_artifact = store.get_artifact(package_artifact_id)?;
    let package = serde_json::from_slice::<IssueTaskPackage>(
        &store.read_artifact_bytes(&package_artifact.id)?,
    )
    .context("existing IssueTaskPackage artifact is not valid v3 JSON")?;
    Ok((Some(package_artifact), Some(package)))
}

fn pending_issue_review(
    store: &DispatchStore,
    approval_request_id: &str,
) -> Result<ApprovalRequest> {
    let approval = store.get_approval_request(approval_request_id)?;
    if approval.approval_type != ApprovalType::IssueReview {
        anyhow::bail!(
            "approval request {} is {}, not issue_review",
            approval.id,
            approval.approval_type
        );
    }
    if approval.status != ApprovalStatus::Pending {
        anyhow::bail!(
            "approval request {} is {}, not pending",
            approval.id,
            approval.status
        );
    }
    Ok(approval)
}

fn pending_issue_review_for_task(
    store: &DispatchStore,
    issue_task_id: &str,
) -> Result<Option<ApprovalRequest>> {
    Ok(store
        .list_approval_requests_by_type(ApprovalType::IssueReview)?
        .into_iter()
        .rev()
        .find(|approval| {
            approval.status == ApprovalStatus::Pending
                && approval
                    .details_json
                    .get("issueTaskId")
                    .and_then(Value::as_str)
                    == Some(issue_task_id)
        }))
}

fn issue_review_for_inbox(
    store: &DispatchStore,
    issue_task_id: &str,
    inbox_id: &str,
) -> Result<Option<ApprovalRequest>> {
    Ok(store
        .list_approval_requests_by_type(ApprovalType::IssueReview)?
        .into_iter()
        .rev()
        .find(|approval| {
            approval
                .details_json
                .get("issueTaskId")
                .and_then(Value::as_str)
                == Some(issue_task_id)
                && approval.details_json.get("inboxId").and_then(Value::as_str) == Some(inbox_id)
        }))
}

fn issue_task_from_review(store: &DispatchStore, approval: &ApprovalRequest) -> Result<IssueTask> {
    let issue_task_id = approval
        .details_json
        .get("issueTaskId")
        .and_then(Value::as_str)
        .context("issue review approval missing issueTaskId")?;
    store.get_issue_task(issue_task_id)
}

fn artifact_from_review(
    store: &DispatchStore,
    approval: &ApprovalRequest,
    field: &str,
) -> Result<AgentArtifact> {
    let artifact_id = approval
        .details_json
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("issue review approval missing {field}"))?;
    store.get_artifact(artifact_id)
}

fn optional_artifact_from_review(
    store: &DispatchStore,
    approval: &ApprovalRequest,
    field: &str,
) -> Result<Option<AgentArtifact>> {
    approval
        .details_json
        .get(field)
        .and_then(Value::as_str)
        .map(|artifact_id| store.get_artifact(artifact_id))
        .transpose()
}

fn review_candidate_status(existing: Option<&IssueTask>, handoff: &Handoff) -> IssueTaskStatus {
    if let Some(existing) = existing {
        if matches!(
            existing.status,
            IssueTaskStatus::UserApproved
                | IssueTaskStatus::Dispatched
                | IssueTaskStatus::InProgress
                | IssueTaskStatus::FixReady
                | IssueTaskStatus::GithubPosted
                | IssueTaskStatus::Done
        ) {
            return existing.status;
        }
    }
    if handoff.llm_confirmation.status == "success" {
        IssueTaskStatus::LlmConfirmed
    } else {
        IssueTaskStatus::Discovered
    }
}

fn package_import_status(
    approval_request: &ApprovalRequest,
    package_artifact: Option<&AgentArtifact>,
) -> String {
    if package_artifact.is_some() {
        "packaged".to_string()
    } else if approval_request.status == ApprovalStatus::Pending {
        "pending_issue_review".to_string()
    } else {
        approval_request.status.to_string()
    }
}

fn pending_issue_review_message(issue_key: &str, approval_request_id: &str) -> String {
    format!("issue task {issue_key} is pending issue review approval {approval_request_id}")
}

fn issue_review_block_message(issue_key: &str, approval_request: &ApprovalRequest) -> String {
    match approval_request.status {
        ApprovalStatus::Pending => pending_issue_review_message(issue_key, &approval_request.id),
        ApprovalStatus::Rejected => {
            format!(
                "issue task {issue_key} issue review {} was rejected",
                approval_request.id
            )
        }
        ApprovalStatus::Canceled => {
            format!(
                "issue task {issue_key} issue review {} was canceled",
                approval_request.id
            )
        }
        ApprovalStatus::Approved => pending_issue_review_message(issue_key, &approval_request.id),
    }
}

fn issue_key(issue_ref: &IssueRef) -> String {
    format!("{}#{}", issue_ref.repo_full_name(), issue_ref.number)
}

fn user_profile_snapshot(paths: &IssueFinderPaths) -> serde_json::Value {
    match Config::load_or_default(paths) {
        Ok(config) => json!({
            "source": "config_profile",
            "profile": {
                "techStack": config.profile.tech_stack,
                "keywords": config.profile.keywords
            }
        }),
        Err(error) => json!({
            "source": "config_profile",
            "profile": null,
            "loadError": error.to_string()
        }),
    }
}
