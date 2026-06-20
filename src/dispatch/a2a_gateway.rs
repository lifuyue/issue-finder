use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

use crate::github::IssueRef;

use super::model::{
    A2aArtifactRef, A2aCallbackPolicy, A2aTask, A2aTaskExport, AgentArtifact, ApprovalRequest,
    ApprovalStatus, ApprovalType, DispatchRun, DispatchRunStatus, IssueTask, IssueTaskStatus,
    NewAgentEvent, NewApprovalRequest, NewArtifact,
};
use super::store::DispatchStore;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct A2aExportResult {
    pub status: String,
    pub issue_task: IssueTask,
    pub package_artifact: AgentArtifact,
    pub export_artifact: AgentArtifact,
    pub task: A2aTaskExport,
    pub approval_request: ApprovalRequest,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct A2aApprovalResult {
    pub approval_request: ApprovalRequest,
    pub issue_task: IssueTask,
    pub export_artifact: AgentArtifact,
    pub task: A2aTaskExport,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct A2aResultImport {
    pub run: DispatchRun,
    pub artifact: AgentArtifact,
}

pub fn export_task(store: &DispatchStore, issue: &str) -> Result<A2aExportResult> {
    let issue_task = imported_issue_task(store, issue)?;
    let package_artifact_id = issue_task
        .current_package_artifact_id
        .as_deref()
        .with_context(|| {
            format!(
                "issue task {} has no task package artifact",
                issue_task.issue_key
            )
        })?;
    let package_artifact = store.get_artifact(package_artifact_id)?;
    let task = A2aTaskExport {
        kind: "issue_finder_a2a_task_export".to_string(),
        version: 1,
        task: A2aTask {
            id: format!("a2a-{}", issue_task.id),
            task_type: "fix_github_issue".to_string(),
            issue_key: issue_task.issue_key.clone(),
            title: issue_task.title.clone(),
        },
        input_artifacts: vec![A2aArtifactRef {
            role: "issue_task_package".to_string(),
            name: "issue_task_package.json".to_string(),
            artifact_id: package_artifact.id.clone(),
            path: package_artifact.path.clone(),
            content_type: package_artifact.content_type.clone(),
        }],
        expected_artifacts: vec![
            "fix_result.json".to_string(),
            "patch".to_string(),
            "pr_link".to_string(),
            "session_link".to_string(),
        ],
        callback: A2aCallbackPolicy {
            expected_result_artifact: "fix_result.json".to_string(),
            import_mode: "local_artifact_only".to_string(),
        },
    };
    let export_artifact = store.write_artifact(
        NewArtifact {
            issue_task_id: Some(issue_task.id.clone()),
            run_id: None,
            kind: "a2a_task_export".to_string(),
            content_type: "application/json".to_string(),
            metadata_json: json!({
                "issueKey": issue_task.issue_key,
                "packageArtifactId": package_artifact.id
            }),
        },
        serde_json::to_vec_pretty(&task)?,
    )?;
    let approval_request = store.create_approval_request(NewApprovalRequest {
        run_id: None,
        approval_type: ApprovalType::A2aSend,
        status: ApprovalStatus::Pending,
        prompt: format!(
            "Authorize outbound A2A task artifact for {}?",
            issue_task.issue_key
        ),
        details_json: json!({
            "issueTaskId": issue_task.id,
            "issueKey": issue_task.issue_key,
            "packageArtifactId": package_artifact.id,
            "a2aTaskArtifactId": export_artifact.id,
            "importMode": task.callback.import_mode
        }),
    })?;

    Ok(A2aExportResult {
        status: "pending_approval".to_string(),
        issue_task,
        package_artifact,
        export_artifact,
        task,
        approval_request,
    })
}

pub fn approve_send(store: &DispatchStore, approval_request_id: &str) -> Result<A2aApprovalResult> {
    resolve_send(store, approval_request_id, ApprovalStatus::Approved)
}

pub fn reject_send(store: &DispatchStore, approval_request_id: &str) -> Result<A2aApprovalResult> {
    resolve_send(store, approval_request_id, ApprovalStatus::Rejected)
}

pub fn import_result(
    store: &DispatchStore,
    run_id: &str,
    path: &Path,
    kind: &str,
    content_type: &str,
    status: Option<DispatchRunStatus>,
) -> Result<A2aResultImport> {
    let run = store.get_dispatch_run(run_id)?;
    let contents =
        std::fs::read(path).with_context(|| format!("unable to read {}", path.display()))?;
    let artifact = store.write_artifact(
        NewArtifact {
            issue_task_id: Some(run.issue_task_id.clone()),
            run_id: Some(run.id.clone()),
            kind: kind.to_string(),
            content_type: content_type.to_string(),
            metadata_json: json!({
                "source": "a2a_local_import",
                "sourcePath": path.to_string_lossy()
            }),
        },
        contents,
    )?;
    store.append_agent_event(NewAgentEvent {
        run_id: Some(run.id.clone()),
        session_link_id: run.selected_session_link_id.clone(),
        event_type: "a2a_result_imported".to_string(),
        native_event_id: None,
        payload_json: json!({
            "artifactId": artifact.id,
            "kind": kind,
            "sourcePath": path.to_string_lossy()
        }),
    })?;

    if kind == "fix_result" {
        store.set_dispatch_run_result_artifact(&run.id, &artifact.id)?;
    }
    let run = match status {
        Some(status) => store.update_dispatch_run_status(&run.id, status, None)?,
        None => store.get_dispatch_run(&run.id)?,
    };
    if kind == "fix_result" && run.status == DispatchRunStatus::Completed {
        store.update_issue_task_status(&run.issue_task_id, IssueTaskStatus::FixReady)?;
    }

    Ok(A2aResultImport { run, artifact })
}

fn resolve_send(
    store: &DispatchStore,
    approval_request_id: &str,
    status: ApprovalStatus,
) -> Result<A2aApprovalResult> {
    if status == ApprovalStatus::Pending {
        anyhow::bail!("A2A send approval cannot be resolved to pending");
    }
    let pending = pending_send_approval(store, approval_request_id)?;
    let approval_request = store.resolve_approval_request(&pending.id, status)?;
    let issue_task = store.get_issue_task(
        pending
            .details_json
            .get("issueTaskId")
            .and_then(Value::as_str)
            .context("A2A send approval missing issueTaskId")?,
    )?;
    let export_artifact = store.get_artifact(
        pending
            .details_json
            .get("a2aTaskArtifactId")
            .and_then(Value::as_str)
            .context("A2A send approval missing a2aTaskArtifactId")?,
    )?;
    let task =
        serde_json::from_slice::<A2aTaskExport>(&store.read_artifact_bytes(&export_artifact.id)?)
            .context("A2A task artifact is not a valid A2A export")?;

    Ok(A2aApprovalResult {
        approval_request,
        issue_task,
        export_artifact,
        task,
    })
}

fn pending_send_approval(
    store: &DispatchStore,
    approval_request_id: &str,
) -> Result<ApprovalRequest> {
    let approval = store.get_approval_request(approval_request_id)?;
    if approval.approval_type != ApprovalType::A2aSend {
        anyhow::bail!(
            "approval request {} is {}, not a2a_send",
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

fn imported_issue_task(store: &DispatchStore, issue: &str) -> Result<IssueTask> {
    let issue_ref = IssueRef::parse(issue)?;
    let issue_key = format!("{}#{}", issue_ref.repo_full_name(), issue_ref.number);
    store
        .find_issue_task_by_key(&issue_key)?
        .with_context(|| format!("issue task {issue_key} has not been imported"))
}
