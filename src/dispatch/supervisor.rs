use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::json;

use super::codex_runtime::{
    CodexRuntimeManager, CodexRuntimeStore, RuntimeDiscovery, SendTurnRequest, WorkerMcpConfig,
};
use super::model::{
    DispatchOutcomeFailureClass, DispatchOutcomeKind, DispatchRun, DispatchRunStatus,
    DispatchValidationOutcome, IssueTaskStatus,
};
use super::runtime::{DispatchOutcomeRecordRequest, DispatchRuntime};
use super::store::DispatchStore;
use super::task_contract::TaskPackage;
use crate::paths::IssueFinderPaths;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DispatchExecutionResult {
    pub run: DispatchRun,
    pub supervisor_pid: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisedRunResult {
    pub run: DispatchRun,
    pub discovery: RuntimeDiscovery,
    pub thread_id: String,
    pub final_turn_id: Option<String>,
}

pub fn launch(paths: &IssueFinderPaths, run_id: &str) -> Result<DispatchExecutionResult> {
    let store = DispatchStore::open(paths.clone())?;
    let run = store.get_dispatch_run(run_id)?;
    if run.status != DispatchRunStatus::Approved {
        anyhow::bail!("run {run_id} is not approved");
    }
    let child = Command::new(std::env::current_exe()?)
        .args(["supervise", "--run-id", run_id])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("unable to launch persistent run supervisor")?;
    let supervisor_pid = child.id();
    let run = store.set_dispatch_run_supervisor_pid(run_id, supervisor_pid)?;
    Ok(DispatchExecutionResult {
        run,
        supervisor_pid,
    })
}

pub struct RunSupervisor {
    paths: IssueFinderPaths,
}

impl RunSupervisor {
    pub fn new(paths: IssueFinderPaths) -> Self {
        Self { paths }
    }

    pub async fn run(&self, run_id: &str) -> Result<SupervisedRunResult> {
        match self.run_inner(run_id).await {
            Ok(result) => Ok(result),
            Err(error) => {
                self.record_runtime_error(run_id, &error)?;
                Err(error)
            }
        }
    }

    async fn run_inner(&self, run_id: &str) -> Result<SupervisedRunResult> {
        let store = DispatchStore::open(self.paths.clone())?;
        let approved = store.claim_dispatch_run_for_execution(run_id)?;
        let issue = store.get_issue_task(&approved.issue_task_id)?;
        let package_id = issue
            .current_package_artifact_id
            .clone()
            .context("approved task has no TaskPackage")?;
        let package: TaskPackage = serde_json::from_slice(&store.read_artifact_bytes(&package_id)?)
            .context("approved TaskPackage is invalid")?;
        package.validate_for_execution()?;
        verify_package_runtime(&store, &issue.id, &package)?;

        let worker = WorkerMcpConfig {
            run_id: approved.id.clone(),
            issue_task_id: issue.id.clone(),
            package_id: package_id.clone(),
            snapshot_id: package.context_snapshot.snapshot_id.clone(),
            workspace: package.workspace.path.clone(),
        };
        let runtime_store = CodexRuntimeStore::open(&self.paths)?;
        let mut manager = CodexRuntimeManager::connect_worker(runtime_store, &worker).await?;
        let discovery = manager.discover_runtime("issue_finder").await?;
        let thread_id = if let Some(thread_id) = approved.selected_thread_id.as_deref() {
            manager.resume(thread_id).await?;
            thread_id.to_string()
        } else {
            manager
                .start_thread(
                    &format!("issue-finder: {}", issue.issue_key),
                    &package.workspace.path,
                )
                .await?
        };
        manager
            .set_goal(&thread_id, &package.goal.objective)
            .await?;
        store.set_dispatch_run_thread(run_id, &thread_id)?;
        store.update_issue_task_status(&issue.id, IssueTaskStatus::InProgress)?;
        store.update_dispatch_run_status(run_id, DispatchRunStatus::Running, None)?;

        let started_at = std::time::Instant::now();
        let mut attempt = 1_u32;
        let mut turn_id =
            start_attempt(&manager, &approved, &package, &thread_id, attempt, None).await?;
        store.set_dispatch_run_runtime_progress(run_id, &turn_id, attempt)?;
        loop {
            if started_at.elapsed() >= Duration::from_secs(package.runtime_policy.max_time_seconds)
            {
                record_exhausted(&self.paths, run_id, "run time budget exhausted")?;
                break;
            }
            manager.deliver_ready_responses().await?;
            manager.deliver_control_outbox(&thread_id).await?;
            let has_pending = manager
                .pending_requests()?
                .iter()
                .any(|request| request.thread_id.as_deref() == Some(thread_id.as_str()));
            let active_request_id = manager
                .pending_requests()?
                .into_iter()
                .find(|request| request.thread_id.as_deref() == Some(thread_id.as_str()))
                .map(|request| request.id);
            store.set_dispatch_run_active_request(run_id, active_request_id.as_deref())?;
            let current = store.get_dispatch_run(run_id)?;
            if has_pending && current.status != DispatchRunStatus::NeedsUser {
                store.update_dispatch_run_status(run_id, DispatchRunStatus::NeedsUser, None)?;
            } else if !has_pending && current.status == DispatchRunStatus::NeedsUser {
                store.update_dispatch_run_status(run_id, DispatchRunStatus::Running, None)?;
            }
            let current = store.get_dispatch_run(run_id)?;
            if terminal(current.status) {
                break;
            }
            match tokio::time::timeout(Duration::from_secs(1), manager.pump_once()).await {
                Ok(Ok(true)) | Err(_) => {}
                Ok(Ok(false)) => {
                    manager.reconnect(std::slice::from_ref(&thread_id)).await?;
                    continue;
                }
                Ok(Err(error)) => return Err(error),
            }
            let runtime_view = CodexRuntimeStore::open(&self.paths)?;
            let Some(turn) = runtime_view.latest_turn(&thread_id)? else {
                continue;
            };
            if turn.id != turn_id || !turn_terminal(&turn.status) {
                continue;
            }
            if turn_interrupted(&turn.status) {
                DispatchRuntime::open(self.paths.clone())?.commit_terminal_outcome(
                    DispatchOutcomeRecordRequest {
                        run_id: run_id.to_string(),
                        idempotency_key: Some(format!("supervisor-interrupted:{run_id}")),
                        outcome_kind: DispatchOutcomeKind::Canceled,
                        failure_class: None,
                        failure_detail: Some("Codex confirmed turn interruption".to_string()),
                        task_class: None,
                        validation_outcome: None,
                        result_artifact_id: None,
                        metadata_json: json!({"source":"run_supervisor","turnId":turn.id}),
                    },
                )?;
                break;
            }
            let current = store.get_dispatch_run(run_id)?;
            if terminal(current.status) {
                break;
            }
            if current.status == DispatchRunStatus::NeedsUser {
                continue;
            }
            if attempt >= package.runtime_policy.max_attempts {
                record_exhausted(
                    &self.paths,
                    run_id,
                    "Codex completed without an accepted candidate result",
                )?;
                break;
            }
            let feedback = latest_feedback(&store, run_id)?.unwrap_or_else(|| {
                "The previous turn completed without an accepted submit_result call. Continue and submit structured evidence through issue_finder_submit_result.".to_string()
            });
            attempt += 1;
            turn_id = start_attempt(
                &manager,
                &approved,
                &package,
                &thread_id,
                attempt,
                Some(feedback),
            )
            .await?;
            store.set_dispatch_run_runtime_progress(run_id, &turn_id, attempt)?;
            store.update_dispatch_run_status(run_id, DispatchRunStatus::Running, None)?;
        }
        let run = store.get_dispatch_run(run_id)?;
        manager.shutdown().await;
        Ok(SupervisedRunResult {
            run,
            discovery,
            thread_id,
            final_turn_id: Some(turn_id),
        })
    }

    fn record_runtime_error(&self, run_id: &str, error: &anyhow::Error) -> Result<()> {
        let store = DispatchStore::open(self.paths.clone())?;
        let run = store.get_dispatch_run(run_id)?;
        if terminal(run.status) {
            return Ok(());
        }
        let failures = store.list_dispatch_failures_for_run(run_id)?.len() as u32 + 1;
        store.record_dispatch_failure(super::failure::execution_failure(
            run_id,
            "supervisor",
            error,
        ))?;
        if failures >= run.max_attempts {
            record_exhausted(&self.paths, run_id, &error.to_string())?;
        } else {
            store.update_dispatch_run_status(
                run_id,
                DispatchRunStatus::Approved,
                Some(error.to_string()),
            )?;
        }
        Ok(())
    }
}

async fn start_attempt(
    manager: &CodexRuntimeManager,
    run: &DispatchRun,
    package: &TaskPackage,
    thread_id: &str,
    attempt: u32,
    feedback: Option<String>,
) -> Result<String> {
    let prompt = match feedback {
        Some(feedback) => format!(
            "Evaluator feedback for attempt {attempt}:\n{feedback}\nContinue in the same workspace and submit the next candidate through issue_finder_submit_result."
        ),
        None => format!(
            "Execute the approved TaskPackage in {}. Read immutable context through issue_finder_read_context. Call issue_finder_submit_result when ready; turn completion alone is not success.",
            package.workspace.path
        ),
    };
    Ok(manager
        .send(SendTurnRequest {
            thread_id: thread_id.to_string(),
            prompt,
            cwd: package.workspace.path.clone(),
            client_user_message_id: format!("issue-finder:{}:attempt:{attempt}", run.id),
            output_schema: Some(crate::tool_specs::candidate_result_schema()),
        })
        .await?
        .turn_id)
}

fn verify_package_runtime(
    store: &DispatchStore,
    issue_task_id: &str,
    package: &TaskPackage,
) -> Result<()> {
    let workspace = std::path::Path::new(&package.workspace.path);
    if !workspace.is_absolute() || !workspace.is_dir() {
        anyhow::bail!("TaskPackage workspace is not an existing absolute directory");
    }
    let snapshot = store.get_artifact(&package.context_snapshot.artifact_id)?;
    if snapshot.issue_task_id.as_deref() != Some(issue_task_id)
        || snapshot.kind != "context_snapshot"
    {
        anyhow::bail!("TaskPackage snapshot is not owned by the active task");
    }
    Ok(())
}

fn latest_feedback(store: &DispatchStore, run_id: &str) -> Result<Option<String>> {
    let artifact = store
        .list_artifacts_for_run(run_id)?
        .into_iter()
        .rev()
        .find(|artifact| artifact.kind == "evaluation_report");
    let Some(artifact) = artifact else {
        return Ok(None);
    };
    let report: super::evaluator::EvaluationReport =
        serde_json::from_slice(&store.read_artifact_bytes(&artifact.id)?)?;
    Ok((!report.feedback.is_empty()).then(|| report.feedback.join("\n")))
}

fn record_exhausted(paths: &IssueFinderPaths, run_id: &str, reason: &str) -> Result<()> {
    DispatchRuntime::open(paths.clone())?.commit_terminal_outcome(
        DispatchOutcomeRecordRequest {
            run_id: run_id.to_string(),
            idempotency_key: Some(format!("supervisor-exhausted:{run_id}")),
            outcome_kind: DispatchOutcomeKind::Failed,
            failure_class: Some(DispatchOutcomeFailureClass::AgentRuntimeError),
            failure_detail: Some(reason.to_string()),
            task_class: None,
            validation_outcome: Some(DispatchValidationOutcome::Failed),
            result_artifact_id: None,
            metadata_json: json!({"source":"run_supervisor"}),
        },
    )?;
    Ok(())
}

fn terminal(status: DispatchRunStatus) -> bool {
    matches!(
        status,
        DispatchRunStatus::Succeeded
            | DispatchRunStatus::Partial
            | DispatchRunStatus::Failed
            | DispatchRunStatus::Canceled
    )
}

fn turn_terminal(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "completed" | "failed" | "interrupted" | "cancelled" | "canceled"
    )
}

fn turn_interrupted(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "interrupted" | "cancelled" | "canceled"
    )
}
