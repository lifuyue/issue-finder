use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

use crate::config::Config;
use crate::paths::IssueFinderPaths;

use super::a2a_gateway::{self, A2aApprovalResult, A2aExportResult, A2aResultImport};
use super::capability_probe::{probe_agent, AgentProbeReport};
use super::evaluator::{
    evaluate_candidate, CandidateResult, EvaluatedCandidate, EvaluationDisposition,
};
use super::events::dispatch_run_event;
use super::github_projection::{
    self, GitHubApprovalResult, GitHubCommentPolicyResult, GitHubCommentWriter, GitHubPostResult,
    ReqwestGitHubCommentWriter,
};
use super::memory::record_dispatch_approval_signal;
use super::model::{
    AgentArtifact, AgentCapability, AgentCapabilityName, AgentProfile, ApprovalRequest,
    ApprovalStatus, ApprovalType, CapabilityStatus, DispatchEvent, DispatchEventKind,
    DispatchEventSeverity, DispatchEventSource, DispatchFailure, DispatchOutcomeFailureClass,
    DispatchOutcomeKind, DispatchRun, DispatchRunOutcome, DispatchRunStatus, DispatchTaskClass,
    DispatchValidationOutcome, GitHubInteraction, IssueTaskStatus, NewAgentCapability,
    NewAgentProfile, NewApprovalRequest, NewDispatchRun, NewDispatchRunOutcome, PolicyAction,
};
use super::packaging::{self, IssueReviewDetail, IssueReviewResolution, PackageImportResult};
use super::policy::{classify_action, ensure_capability_preconditions};
use super::store::DispatchStore;
use super::supervisor::{self, DispatchExecutionResult};
use super::timeline::{
    approval_latency, dispatch_timeline, dispatch_trace, ApprovalLatency, DispatchTimeline,
    DispatchTrace,
};

pub struct DispatchRuntime {
    store: DispatchStore,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilitiesView {
    pub agent: AgentProfile,
    pub capabilities: Vec<AgentCapability>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DispatchStatusSnapshot {
    pub run: DispatchRun,
    pub issue_task: super::model::IssueTask,
    pub agent: AgentProfile,
    pub selected_thread_id: Option<String>,
    pub approval_requests: Vec<ApprovalRequest>,
    pub approval_latencies: Vec<ApprovalLatency>,
    pub artifacts: Vec<AgentArtifact>,
    pub failures: Vec<DispatchFailure>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DispatchProposal {
    pub status: String,
    pub run: DispatchRun,
    pub approval_request: ApprovalRequest,
    pub issue_task: super::model::IssueTask,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DispatchApprovalResolution {
    pub run: DispatchRun,
    pub approval_request: ApprovalRequest,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DispatchOutcomeRecordResult {
    pub run: DispatchRun,
    pub issue_task: super::model::IssueTask,
    pub outcome: DispatchRunOutcome,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SubmitResultOutcome {
    pub evaluation: EvaluatedCandidate,
    pub terminal: Option<DispatchOutcomeRecordResult>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DispatchOutcomeRecordRequest {
    pub run_id: String,
    pub idempotency_key: Option<String>,
    pub outcome_kind: DispatchOutcomeKind,
    pub failure_class: Option<DispatchOutcomeFailureClass>,
    pub failure_detail: Option<String>,
    pub task_class: Option<DispatchTaskClass>,
    pub validation_outcome: Option<DispatchValidationOutcome>,
    pub result_artifact_id: Option<String>,
    pub metadata_json: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchProposalRequest {
    pub issue: String,
    pub agent_id: String,
    pub requested_by: String,
    pub selected_thread_id: Option<String>,
    pub new_session: bool,
}

impl DispatchRuntime {
    pub fn open(paths: IssueFinderPaths) -> Result<Self> {
        let store = DispatchStore::open(paths)?;
        ensure_builtin_agents(&store)?;
        Ok(Self { store })
    }

    pub fn store(&self) -> &DispatchStore {
        &self.store
    }

    pub fn list_agents(&self) -> Result<Vec<AgentProfile>> {
        self.store.list_agent_profiles()
    }

    pub fn agent_capabilities(&self, agent_id: &str) -> Result<AgentCapabilitiesView> {
        Ok(AgentCapabilitiesView {
            agent: self.store.get_agent_profile(agent_id)?,
            capabilities: self.store.list_agent_capabilities(agent_id)?,
        })
    }

    pub fn dispatch_status(&self, run_id: &str) -> Result<DispatchStatusSnapshot> {
        let run = self.store.get_dispatch_run(run_id)?;
        let issue_task = self.store.get_issue_task(&run.issue_task_id)?;
        let agent = self.store.get_agent_profile(&run.agent_id)?;
        let approval_requests = self.store.list_approval_requests_for_run(run_id)?;
        let approval_latencies = approval_requests.iter().map(approval_latency).collect();
        let artifacts = self.store.list_artifacts_for_run(run_id)?;
        let failures = self.store.list_dispatch_failures_for_run(run_id)?;
        let selected_thread_id = run.selected_thread_id.clone();

        Ok(DispatchStatusSnapshot {
            run,
            issue_task,
            agent,
            selected_thread_id,
            approval_requests,
            approval_latencies,
            artifacts,
            failures,
        })
    }

    pub fn dispatch_events(&self, run_id: &str) -> Result<Vec<DispatchEvent>> {
        self.store.list_dispatch_events_for_run(run_id)
    }

    pub fn dispatch_artifacts(&self, run_id: &str) -> Result<Vec<AgentArtifact>> {
        self.store.list_artifacts_for_run(run_id)
    }

    pub fn dispatch_timeline(&self, run_id: &str) -> Result<DispatchTimeline> {
        dispatch_timeline(&self.store, run_id)
    }

    pub fn dispatch_trace(&self, run_id: &str) -> Result<DispatchTrace> {
        dispatch_trace(&self.store, run_id)
    }

    pub fn probe_agent(&self, agent_id: &str, refresh: bool) -> Result<AgentProbeReport> {
        probe_agent(&self.store, agent_id, refresh)
    }

    pub fn import_handoff_from_inbox(&self, inbox_id: &str) -> Result<PackageImportResult> {
        packaging::import_handoff_from_inbox(&self.store, inbox_id)
    }

    pub fn list_issue_reviews(&self) -> Result<Vec<IssueReviewDetail>> {
        packaging::list_issue_reviews(&self.store)
    }

    pub fn show_issue_review(&self, approval_request_id: &str) -> Result<IssueReviewDetail> {
        packaging::show_issue_review(&self.store, approval_request_id)
    }

    pub fn approve_issue_review(&self, approval_request_id: &str) -> Result<IssueReviewResolution> {
        packaging::approve_issue_review(&self.store, approval_request_id)
    }

    pub fn reject_issue_review(
        &self,
        approval_request_id: &str,
        reason: Option<String>,
    ) -> Result<IssueReviewResolution> {
        packaging::reject_issue_review(&self.store, approval_request_id, reason)
    }

    pub fn export_a2a_task(&self, issue: &str) -> Result<A2aExportResult> {
        packaging::ensure_packaged_issue_task_for_issue(&self.store, issue)?;
        a2a_gateway::export_task(&self.store, issue)
    }

    pub fn approve_a2a_send(&self, approval_request_id: &str) -> Result<A2aApprovalResult> {
        a2a_gateway::approve_send(&self.store, approval_request_id)
    }

    pub fn reject_a2a_send(&self, approval_request_id: &str) -> Result<A2aApprovalResult> {
        a2a_gateway::reject_send(&self.store, approval_request_id)
    }

    pub fn import_a2a_result(&self, run_id: &str, path: &Path) -> Result<A2aResultImport> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("unable to read A2A candidate result {}", path.display()))?;
        let candidate: CandidateResult =
            serde_json::from_slice(&bytes).context("A2A candidate result is invalid")?;
        if candidate.run_id != run_id {
            anyhow::bail!("A2A candidate result runId does not match import target");
        }
        let evaluated = self.submit_result(candidate)?;
        let run = self.store.get_dispatch_run(run_id)?;
        self.store.append_dispatch_event(dispatch_run_event(
            &run,
            DispatchEventKind::A2aResultImported,
            DispatchEventSource::A2a,
            DispatchEventSeverity::Info,
            json!({
                "candidateResultArtifactId": evaluated.evaluation.result_artifact.id,
                "evaluationArtifactId": evaluated.evaluation.evaluation_artifact.id,
                "sourcePath": path
            }),
        ))?;
        Ok(A2aResultImport {
            run: self.store.get_dispatch_run(run_id)?,
            artifact: evaluated.evaluation.result_artifact,
            outcome: evaluated.terminal.map(|terminal| terminal.outcome),
        })
    }

    pub fn propose_dispatch(&self, request: DispatchProposalRequest) -> Result<DispatchProposal> {
        if request.new_session && request.selected_thread_id.is_some() {
            anyhow::bail!("new_session cannot be combined with selected_thread_id");
        }

        let agent = self.store.get_agent_profile(&request.agent_id)?;
        let issue_task =
            packaging::ensure_packaged_issue_task_for_issue(&self.store, &request.issue)?;
        let issue_key = issue_task.issue_key.clone();
        let selected_thread_id = request.selected_thread_id;
        let dispatch_capability = if selected_thread_id.is_some() {
            PolicyAction::ResumeDispatch
        } else {
            PolicyAction::StartDispatch
        };
        let policy = classify_action(dispatch_capability);
        ensure_capability_preconditions(&self.store, &agent.id, &policy)?;

        let run = self.store.create_dispatch_run(NewDispatchRun {
            issue_task_id: issue_task.id.clone(),
            agent_id: agent.id,
            status: DispatchRunStatus::Proposed,
            requested_by: request.requested_by,
            approval_state: ApprovalStatus::Pending,
            selected_thread_id,
        })?;
        let approval_request = self.store.create_approval_request(NewApprovalRequest {
            run_id: Some(run.id.clone()),
            approval_type: ApprovalType::Dispatch,
            status: ApprovalStatus::Pending,
            prompt: dispatch_approval_prompt(&issue_key, &run.agent_id, run.selected_thread_id.as_deref()),
            details_json: json!({
                "issueKey": issue_key,
                "agentId": run.agent_id,
                "executionMode": if run.selected_thread_id.is_some() { "resume_thread" } else { "start_thread" },
                "newSession": run.selected_thread_id.is_none(),
                "requestedNewSession": request.new_session,
                "selectedThreadId": run.selected_thread_id,
                "policy": policy
            }),
        })?;

        Ok(DispatchProposal {
            status: "pending_approval".to_string(),
            run,
            approval_request,
            issue_task,
        })
    }

    pub fn resolve_dispatch_approval(
        &self,
        run_id: &str,
        status: ApprovalStatus,
    ) -> Result<DispatchApprovalResolution> {
        if status == ApprovalStatus::Pending {
            anyhow::bail!("dispatch approval cannot be resolved to pending");
        }

        let run = self.store.get_dispatch_run(run_id)?;
        let approval_request = self
            .store
            .list_approval_requests_for_run(run_id)?
            .into_iter()
            .rev()
            .find(|approval| {
                approval.approval_type == ApprovalType::Dispatch
                    && approval.status == ApprovalStatus::Pending
            })
            .with_context(|| format!("dispatch run {run_id} has no pending dispatch approval"))?;
        let approval_request = self
            .store
            .resolve_approval_request(&approval_request.id, status)?;
        let run = self
            .store
            .update_dispatch_run_approval_state(&run.id, status)?;
        let run = match status {
            ApprovalStatus::Approved => {
                let run = self.store.update_dispatch_run_status(
                    &run.id,
                    DispatchRunStatus::Approved,
                    None,
                )?;
                self.store
                    .update_issue_task_status(&run.issue_task_id, IssueTaskStatus::Dispatched)?;
                run
            }
            ApprovalStatus::Rejected | ApprovalStatus::Canceled => self
                .store
                .update_dispatch_run_status(&run.id, DispatchRunStatus::Canceled, None)?,
            ApprovalStatus::Pending => unreachable!(),
        };
        self.store.append_dispatch_event(dispatch_run_event(
            &run,
            DispatchEventKind::DispatchApprovalResolved,
            DispatchEventSource::Runtime,
            DispatchEventSeverity::Info,
            json!({
                "approvalRequestId": approval_request.id,
                "approvalStatus": status.as_str(),
                "runStatus": run.status.as_str()
            }),
        ))?;
        record_dispatch_approval_signal(&self.store, &run, &approval_request)?;

        Ok(DispatchApprovalResolution {
            run,
            approval_request,
        })
    }

    pub(crate) fn commit_terminal_outcome(
        &self,
        request: DispatchOutcomeRecordRequest,
    ) -> Result<DispatchOutcomeRecordResult> {
        let run = self.store.get_dispatch_run(&request.run_id)?;
        let issue_task = self.store.get_issue_task(&run.issue_task_id)?;
        super::outcome_validator::validate_outcome(&self.store, &run, &issue_task, &request)?;
        let idempotency_key = request
            .idempotency_key
            .clone()
            .unwrap_or_else(|| format!("dispatch_outcome:{}:{}", run.id, request.outcome_kind));
        let outcome = self
            .store
            .record_dispatch_run_outcome(NewDispatchRunOutcome {
                run_id: run.id.clone(),
                idempotency_key,
                outcome_kind: request.outcome_kind,
                failure_class: request.failure_class,
                failure_detail: request.failure_detail.clone(),
                task_class: request.task_class,
                validation_outcome: request.validation_outcome,
                result_artifact_id: request.result_artifact_id.clone(),
                metadata_json: request.metadata_json,
            })?;
        let run = self.store.get_dispatch_run(&run.id)?;
        let outcome_event_exists = self
            .store
            .list_dispatch_events_for_run(&run.id)?
            .into_iter()
            .any(|event| {
                event.event_kind == DispatchEventKind::DispatchOutcomeRecorded
                    && event.payload_json.get("outcomeId").and_then(Value::as_str)
                        == Some(outcome.id.as_str())
            });
        if !outcome_event_exists {
            self.store.append_dispatch_event(dispatch_run_event(
                &run,
                DispatchEventKind::DispatchOutcomeRecorded,
                DispatchEventSource::Runtime,
                DispatchEventSeverity::Info,
                json!({
                    "outcomeId": outcome.id,
                    "outcomeKind": outcome.outcome_kind,
                    "failureClass": outcome.failure_class,
                    "taskClass": outcome.task_class,
                    "validationOutcome": outcome.validation_outcome,
                }),
            ))?;
        }

        let issue_task = self.store.get_issue_task(&run.issue_task_id)?;
        Ok(DispatchOutcomeRecordResult {
            run,
            issue_task,
            outcome,
        })
    }

    pub fn submit_result(&self, result: CandidateResult) -> Result<SubmitResultOutcome> {
        let run = self.store.get_dispatch_run(&result.run_id)?;
        let evaluation = evaluate_candidate(&self.store, &run, &result)?;
        let terminal = match evaluation.report.disposition {
            EvaluationDisposition::AcceptedSuccess => {
                Some(self.commit_terminal_outcome(DispatchOutcomeRecordRequest {
                    run_id: run.id.clone(),
                    idempotency_key: Some(format!(
                        "evaluation:{}",
                        evaluation.evaluation_artifact.id
                    )),
                    outcome_kind: DispatchOutcomeKind::Success,
                    failure_class: None,
                    failure_detail: None,
                    task_class: None,
                    validation_outcome: Some(DispatchValidationOutcome::Passed),
                    result_artifact_id: Some(evaluation.result_artifact.id.clone()),
                    metadata_json: json!({
                        "source": "deterministic_evaluator",
                        "evaluationArtifactId": evaluation.evaluation_artifact.id,
                        "attempt": evaluation.report.attempt,
                    }),
                })?)
            }
            EvaluationDisposition::AcceptedPartial => {
                Some(self.commit_terminal_outcome(DispatchOutcomeRecordRequest {
                    run_id: run.id.clone(),
                    idempotency_key: Some(format!(
                        "evaluation:{}",
                        evaluation.evaluation_artifact.id
                    )),
                    outcome_kind: DispatchOutcomeKind::Partial,
                    failure_class: None,
                    failure_detail: None,
                    task_class: None,
                    validation_outcome: Some(DispatchValidationOutcome::Passed),
                    result_artifact_id: Some(evaluation.result_artifact.id.clone()),
                    metadata_json: json!({
                        "source": "deterministic_evaluator",
                        "evaluationArtifactId": evaluation.evaluation_artifact.id,
                        "attempt": evaluation.report.attempt,
                    }),
                })?)
            }
            EvaluationDisposition::Failed => Some(
                self.commit_terminal_outcome(DispatchOutcomeRecordRequest {
                    run_id: run.id.clone(),
                    idempotency_key: Some(format!(
                        "evaluation:{}",
                        evaluation.evaluation_artifact.id
                    )),
                    outcome_kind: DispatchOutcomeKind::Failed,
                    failure_class: Some(DispatchOutcomeFailureClass::ValidationFailed),
                    failure_detail: result
                        .failure_reason
                        .clone()
                        .or_else(|| Some(evaluation.report.feedback.join(" "))),
                    task_class: None,
                    validation_outcome: Some(DispatchValidationOutcome::Failed),
                    result_artifact_id: Some(evaluation.result_artifact.id.clone()),
                    metadata_json: json!({
                        "source": "deterministic_evaluator",
                        "evaluationArtifactId": evaluation.evaluation_artifact.id,
                        "attempt": evaluation.report.attempt,
                    }),
                })?,
            ),
            EvaluationDisposition::Retry | EvaluationDisposition::NeedsUser => None,
        };
        if let Some(terminal) = terminal.as_ref() {
            super::projectors::project_terminal_outcome(&self.store, &terminal.outcome)?;
        }
        Ok(SubmitResultOutcome {
            evaluation,
            terminal,
        })
    }

    pub fn execute_dispatch(&self, run_id: &str) -> Result<DispatchExecutionResult> {
        supervisor::launch(&self.store.paths(), run_id)
    }

    pub fn sync_dispatch(&self, run_id: &str) -> Result<DispatchStatusSnapshot> {
        let run = self.store.get_dispatch_run(run_id)?;
        let _ = run;
        self.dispatch_status(run_id)
    }

    pub fn pending_requests(
        &self,
        run_id: &str,
    ) -> Result<Vec<super::codex_runtime::PendingRequest>> {
        let run = self.store.get_dispatch_run(run_id)?;
        let thread_id = run.selected_thread_id.as_deref();
        Ok(
            super::codex_runtime::CodexRuntimeStore::open(&self.store.paths())?
                .pending_server_requests()?
                .into_iter()
                .filter(|request| request.thread_id.as_deref() == thread_id)
                .collect(),
        )
    }

    pub fn respond_pending_request(&self, request_id: &str, response: Value) -> Result<()> {
        super::codex_runtime::CodexRuntimeStore::open(&self.store.paths())?
            .queue_pending_response(request_id, &response)
    }

    pub fn steer_dispatch(&self, run_id: &str, message: &str) -> Result<()> {
        let run = self.store.get_dispatch_run(run_id)?;
        let thread_id = run.selected_thread_id.context("run has no Codex thread")?;
        let runtime = super::codex_runtime::CodexRuntimeStore::open(&self.store.paths())?;
        let turn = runtime
            .latest_turn(&thread_id)?
            .context("run has no active turn")?;
        runtime.enqueue_control(
            &format!(
                "control:steer:{}:{}",
                run.id,
                chrono::Utc::now().timestamp_micros()
            ),
            &thread_id,
            "turn/steer",
            &json!({
                "threadId": thread_id,
                "expectedTurnId": turn.id,
                "clientUserMessageId": format!("issue-finder:steer:{}", run.id),
                "input":[{"type":"text","text":message}]
            }),
        )?;
        Ok(())
    }

    pub fn interrupt_dispatch(&self, run_id: &str) -> Result<()> {
        let run = self.store.get_dispatch_run(run_id)?;
        let thread_id = run.selected_thread_id.context("run has no Codex thread")?;
        let runtime = super::codex_runtime::CodexRuntimeStore::open(&self.store.paths())?;
        let turn = runtime
            .latest_turn(&thread_id)?
            .context("run has no active turn")?;
        runtime.enqueue_control(
            &format!("control:interrupt:{}", run.id),
            &thread_id,
            "turn/interrupt",
            &json!({"threadId":thread_id,"turnId":turn.id}),
        )?;
        Ok(())
    }

    pub fn draft_github_tracking_comment(
        &self,
        issue: &str,
        body_override: Option<String>,
    ) -> Result<GitHubCommentPolicyResult> {
        packaging::ensure_packaged_issue_task_for_issue(&self.store, issue)?;
        github_projection::draft_tracking_comment(&self.store, issue, body_override)
    }

    pub fn draft_github_final_comment(
        &self,
        run_id: &str,
        body_override: Option<String>,
    ) -> Result<GitHubCommentPolicyResult> {
        github_projection::draft_final_comment(&self.store, run_id, body_override)
    }

    pub fn approve_github_interaction(&self, interaction_id: &str) -> Result<GitHubApprovalResult> {
        github_projection::approve_github_interaction(&self.store, interaction_id)
    }

    pub fn reject_github_interaction(&self, interaction_id: &str) -> Result<GitHubApprovalResult> {
        github_projection::reject_github_interaction(&self.store, interaction_id)
    }

    pub fn post_github_interaction(
        &self,
        config: &Config,
        interaction_id: &str,
    ) -> Result<GitHubPostResult> {
        let mut writer = ReqwestGitHubCommentWriter::from_config(config)?;
        self.post_github_interaction_with_writer(&mut writer, interaction_id)
    }

    pub fn post_github_interaction_with_writer<W>(
        &self,
        writer: &mut W,
        interaction_id: &str,
    ) -> Result<GitHubPostResult>
    where
        W: GitHubCommentWriter,
    {
        github_projection::post_github_interaction(&self.store, writer, interaction_id)
    }

    pub fn retry_github_interaction(
        &self,
        config: &Config,
        interaction_id: &str,
    ) -> Result<GitHubPostResult> {
        let mut writer = ReqwestGitHubCommentWriter::from_config(config)?;
        self.retry_github_interaction_with_writer(&mut writer, interaction_id)
    }

    pub fn retry_github_interaction_with_writer<W>(
        &self,
        writer: &mut W,
        interaction_id: &str,
    ) -> Result<GitHubPostResult>
    where
        W: GitHubCommentWriter,
    {
        github_projection::retry_github_interaction(&self.store, writer, interaction_id)
    }

    pub fn list_github_interactions(&self, issue: &str) -> Result<Vec<GitHubInteraction>> {
        github_projection::list_github_interactions(&self.store, issue)
    }
}

fn ensure_builtin_agents(store: &DispatchStore) -> Result<()> {
    store.ensure_agent_profile(NewAgentProfile {
        id: Some("codex".to_string()),
        kind: "codex".to_string(),
        display_name: "Codex".to_string(),
        adapter: "codex_app_server".to_string(),
        config_json: json!({
            "source": "builtin",
            "adapterBoundary": "experimental_codex_app_server"
        }),
        enabled: true,
    })?;

    // Built-in capability declarations are seeded as one stable set. Runtime-specific
    // handshake facts belong to explicit adapter probes; opening an unrelated command
    // must not rewrite capability audit state from the caller's PATH or CODEX_HOME.
    if !store.list_agent_capabilities("codex")?.is_empty() {
        return Ok(());
    }

    for (capability, status, details) in codex_capabilities() {
        store.upsert_agent_capability(NewAgentCapability {
            agent_id: "codex".to_string(),
            capability,
            status,
            details_json: details,
        })?;
    }

    Ok(())
}

fn dispatch_approval_prompt(
    issue_key: &str,
    agent_id: &str,
    selected_thread_id: Option<&str>,
) -> String {
    match selected_thread_id {
        Some(thread_id) => {
            format!("Dispatch {issue_key} to {agent_id} by resuming native thread {thread_id}?")
        }
        None => format!("Dispatch {issue_key} to {agent_id} by starting a new native thread?"),
    }
}

fn codex_capabilities() -> Vec<(AgentCapabilityName, CapabilityStatus, serde_json::Value)> {
    let methods = [
        (AgentCapabilityName::StartSession, "thread/start"),
        (AgentCapabilityName::ResumeSession, "thread/resume"),
        (AgentCapabilityName::RenameSession, "thread/name/set"),
        (AgentCapabilityName::SetGoal, "thread/goal/set"),
        (AgentCapabilityName::ReadTranscript, "thread/read"),
        (AgentCapabilityName::StreamEvents, "turn/start"),
        (AgentCapabilityName::InterruptRun, "turn/interrupt"),
        (AgentCapabilityName::ReviewMode, "review/start"),
    ];
    let mut capabilities = methods
        .into_iter()
        .map(|(capability, method)| {
            (
                capability,
                CapabilityStatus::Supported,
                json!({
                    "protocol": "codex_app_server_json_rpc",
                    "method": method,
                    "runtimeOwner": "dispatch/codex_runtime",
                    "liveDiscoveryRequired": true
                }),
            )
        })
        .collect::<Vec<_>>();
    capabilities.push((
        AgentCapabilityName::OpenPr,
        CapabilityStatus::Unsupported,
        json!({
            "reason": "Issue Finder does not publish pull requests"
        }),
    ));
    capabilities
}
