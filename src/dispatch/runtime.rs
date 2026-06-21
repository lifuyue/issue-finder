use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::json;

use crate::config::Config;
use crate::github::IssueRef;
use crate::paths::IssueFinderPaths;

use super::a2a_gateway::{self, A2aApprovalResult, A2aExportResult, A2aResultImport};
use super::adapters::codex_app_server::{
    codex_capability_mappings, default_codex_app_server_startup_metadata,
};
use super::capability_probe::{probe_agent, AgentProbeReport};
use super::events::dispatch_run_event;
use super::execution::{execute_approved_codex_app_server_dispatch, DispatchExecutionResult};
use super::github_projection::{
    self, GitHubApprovalResult, GitHubCommentPolicyResult, GitHubCommentWriter, GitHubPostResult,
    ReqwestGitHubCommentWriter,
};
use super::memory::record_dispatch_approval_signal;
use super::model::{
    AgentArtifact, AgentCapability, AgentCapabilityName, AgentProfile, AgentSessionLink,
    AgentSessionStatus, ApprovalRequest, ApprovalStatus, ApprovalType, CapabilityStatus,
    DispatchEvent, DispatchEventKind, DispatchEventSeverity, DispatchEventSource, DispatchFailure,
    DispatchOutcomeFailureClass, DispatchOutcomeKind, DispatchRun, DispatchRunOutcome,
    DispatchRunStatus, DispatchTaskClass, DispatchValidationOutcome, GitHubInteraction,
    IssueTaskStatus, NewAgentCapability, NewAgentProfile, NewApprovalRequest, NewDispatchRun,
    NewDispatchRunOutcome, PolicyAction, SessionTranscriptItem,
};
use super::packaging::{self, IssueReviewDetail, IssueReviewResolution, PackageImportResult};
use super::policy::{classify_action, ensure_capability_preconditions};
use super::session_approvals::{
    approve_session_mutation_with_adapter, pending_session_mutation, reject_session_mutation,
    request_session_archive, request_session_fork, request_session_rename, PendingSessionMutation,
    SessionMutationApprovalResolution, SessionMutationProposal,
};
use super::session_ops::{
    read_codex_session_transcript, sync_codex_sessions, SessionTranscriptResult,
    SessionsSyncRequest, SessionsSyncResult,
};
use super::store::DispatchStore;
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
pub struct SessionSearchResult {
    pub issue_key: String,
    pub issue_task_found: bool,
    pub sessions: Vec<AgentSessionLink>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DispatchStatusSnapshot {
    pub run: DispatchRun,
    pub issue_task: super::model::IssueTask,
    pub agent: AgentProfile,
    pub selected_session: Option<AgentSessionLink>,
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

#[derive(Debug, Clone, PartialEq)]
pub struct DispatchOutcomeRecordRequest {
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
    pub selected_session_link_id: Option<String>,
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

    pub fn list_sessions(&self, agent_id: Option<&str>) -> Result<Vec<AgentSessionLink>> {
        self.store.list_session_links(agent_id)
    }

    pub fn search_sessions(
        &self,
        issue: &str,
        agent_id: Option<&str>,
    ) -> Result<SessionSearchResult> {
        let issue_ref = IssueRef::parse(issue)?;
        let issue_key = format!("{}#{}", issue_ref.repo_full_name(), issue_ref.number);
        let Some(issue_task) = self.store.find_issue_task_by_key(&issue_key)? else {
            return Ok(SessionSearchResult {
                issue_key,
                issue_task_found: false,
                sessions: Vec::new(),
            });
        };

        let mut sessions = self
            .store
            .list_session_links_for_issue_task(&issue_task.id)?;
        if let Some(agent_id) = agent_id {
            sessions.retain(|session| session.agent_id == agent_id);
        }

        Ok(SessionSearchResult {
            issue_key,
            issue_task_found: true,
            sessions,
        })
    }

    pub fn sync_sessions(&self, request: SessionsSyncRequest) -> Result<SessionsSyncResult> {
        let agent = self.store.get_agent_profile(&request.agent_id)?;
        if agent.adapter != "codex_app_server" {
            anyhow::bail!(
                "agent {} uses adapter {}, not codex_app_server",
                agent.id,
                agent.adapter
            );
        }
        let capability = if request
            .search
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            AgentCapabilityName::SearchSessions
        } else {
            AgentCapabilityName::ListSessions
        };
        self.ensure_agent_capability(&agent.id, capability)?;
        sync_codex_sessions(&self.store, request)
    }

    pub fn read_session_transcript(
        &self,
        session_link_id: &str,
    ) -> Result<SessionTranscriptResult> {
        self.ensure_session_policy(session_link_id, PolicyAction::ReadSessionTranscript)?;
        read_codex_session_transcript(&self.store, session_link_id)
    }

    pub fn session_replay(&self, session_link_id: &str) -> Result<Vec<SessionTranscriptItem>> {
        self.store.list_session_transcript_items(session_link_id)
    }

    pub fn rename_session(
        &self,
        session_link_id: &str,
        display_name: &str,
    ) -> Result<SessionMutationProposal> {
        if display_name.trim().is_empty() {
            anyhow::bail!("session display name cannot be empty");
        }
        self.ensure_session_policy(session_link_id, PolicyAction::RenameSession)?;
        request_session_rename(&self.store, session_link_id, display_name)
    }

    pub fn archive_session(&self, session_link_id: &str) -> Result<SessionMutationProposal> {
        self.ensure_session_policy(session_link_id, PolicyAction::ArchiveSession)?;
        request_session_archive(&self.store, session_link_id)
    }

    pub fn fork_session(&self, session_link_id: &str) -> Result<SessionMutationProposal> {
        self.ensure_session_policy(session_link_id, PolicyAction::ForkSession)?;
        request_session_fork(&self.store, session_link_id)
    }

    pub fn approve_session_mutation(
        &self,
        approval_request_id: &str,
    ) -> Result<SessionMutationApprovalResolution> {
        let mutation = pending_session_mutation(&self.store, approval_request_id)?;
        match &mutation {
            PendingSessionMutation::Rename {
                session_link_id, ..
            } => self.ensure_session_policy(session_link_id, PolicyAction::RenameSession)?,
            PendingSessionMutation::Fork { session_link_id } => {
                self.ensure_session_policy(session_link_id, PolicyAction::ForkSession)?
            }
            PendingSessionMutation::Archive { session_link_id } => {
                self.ensure_session_policy(session_link_id, PolicyAction::ArchiveSession)?
            }
        }
        let session = self.store.get_session_link(mutation.session_link_id())?;
        let agent = self.store.get_agent_profile(&session.agent_id)?;
        if agent.adapter != "codex_app_server" {
            anyhow::bail!(
                "session link {} uses adapter {}, not codex_app_server",
                session.id,
                agent.adapter
            );
        }
        let transport = super::adapters::codex_app_server::CodexAppServerStdioTransport::connect()?;
        let mut adapter = super::adapters::codex_app_server::CodexAppServerAdapter::new(transport);
        approve_session_mutation_with_adapter(&self.store, &mut adapter, approval_request_id)
    }

    pub fn reject_session_mutation(
        &self,
        approval_request_id: &str,
    ) -> Result<SessionMutationApprovalResolution> {
        reject_session_mutation(&self.store, approval_request_id)
    }

    pub fn dispatch_status(&self, run_id: &str) -> Result<DispatchStatusSnapshot> {
        let run = self.store.get_dispatch_run(run_id)?;
        let issue_task = self.store.get_issue_task(&run.issue_task_id)?;
        let agent = self.store.get_agent_profile(&run.agent_id)?;
        let selected_session = run
            .selected_session_link_id
            .as_deref()
            .map(|session_id| self.store.get_session_link(session_id))
            .transpose()?;
        let approval_requests = self.store.list_approval_requests_for_run(run_id)?;
        let approval_latencies = approval_requests.iter().map(approval_latency).collect();
        let artifacts = self.store.list_artifacts_for_run(run_id)?;
        let failures = self.store.list_dispatch_failures_for_run(run_id)?;

        Ok(DispatchStatusSnapshot {
            run,
            issue_task,
            agent,
            selected_session,
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

    pub fn import_a2a_result(
        &self,
        run_id: &str,
        path: &Path,
        kind: &str,
        content_type: &str,
        status: Option<DispatchRunStatus>,
        outcome: Option<DispatchOutcomeRecordRequest>,
    ) -> Result<A2aResultImport> {
        let mut result =
            a2a_gateway::import_result(&self.store, run_id, path, kind, content_type, status)?;
        let outcome_request = outcome.or_else(|| {
            status.and_then(|status| {
                terminal_outcome_for_status(status).map(|outcome_kind| {
                    DispatchOutcomeRecordRequest {
                        run_id: run_id.to_string(),
                        idempotency_key: Some(format!("a2a_result_import:{}", result.artifact.id)),
                        outcome_kind,
                        failure_class: None,
                        failure_detail: None,
                        task_class: None,
                        validation_outcome: None,
                        result_artifact_id: Some(result.artifact.id.clone()),
                        metadata_json: json!({
                            "source": "a2a_import_result",
                            "artifactKind": kind,
                            "coarseTerminalOutcome": true
                        }),
                    }
                })
            })
        });
        if let Some(mut request) = outcome_request {
            request.run_id = run_id.to_string();
            if request.result_artifact_id.is_none() {
                request.result_artifact_id = Some(result.artifact.id.clone());
            }
            if request.idempotency_key.is_none() {
                request.idempotency_key = Some(format!("a2a_result_import:{}", result.artifact.id));
            }
            let recorded = self.record_dispatch_outcome(request)?;
            result.run = recorded.run;
            result.outcome = Some(recorded.outcome);
        }
        Ok(result)
    }

    pub fn propose_dispatch(&self, request: DispatchProposalRequest) -> Result<DispatchProposal> {
        if request.new_session && request.selected_session_link_id.is_some() {
            anyhow::bail!("new_session cannot be combined with selected_session_link_id");
        }

        let agent = self.store.get_agent_profile(&request.agent_id)?;
        let issue_task =
            packaging::ensure_packaged_issue_task_for_issue(&self.store, &request.issue)?;
        let issue_key = issue_task.issue_key.clone();
        let selected_session_link_id = match request.selected_session_link_id {
            Some(selector) => Some(self.resolve_dispatch_session_selector(&agent.id, &selector)?),
            None => None,
        };
        let selected_session = selected_session_link_id
            .as_deref()
            .map(|session_link_id| self.store.get_session_link(session_link_id))
            .transpose()?;
        let dispatch_capability = if selected_session_link_id.is_some() {
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
            selected_session_link_id,
        })?;
        let approval_request = self.store.create_approval_request(NewApprovalRequest {
            run_id: Some(run.id.clone()),
            approval_type: ApprovalType::Dispatch,
            status: ApprovalStatus::Pending,
            prompt: dispatch_approval_prompt(&issue_key, &run.agent_id, selected_session.as_ref()),
            details_json: json!({
                "issueKey": issue_key,
                "agentId": run.agent_id,
                "executionMode": if selected_session.is_some() { "resume_session" } else { "start_session" },
                "newSession": selected_session.is_none(),
                "requestedNewSession": request.new_session,
                "selectedSessionLinkId": run.selected_session_link_id,
                "selectedNativeSessionId": selected_session.as_ref().map(|session| session.native_session_id.as_str()),
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

    pub fn record_dispatch_outcome(
        &self,
        request: DispatchOutcomeRecordRequest,
    ) -> Result<DispatchOutcomeRecordResult> {
        let run = self.store.get_dispatch_run(&request.run_id)?;
        let already_recorded = self
            .store
            .find_dispatch_run_outcome_by_run(&run.id)?
            .is_some();
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
        if already_recorded {
            let issue_task = self.store.get_issue_task(&run.issue_task_id)?;
            return Ok(DispatchOutcomeRecordResult {
                run,
                issue_task,
                outcome,
            });
        }

        if let Some(artifact_id) = outcome.result_artifact_id.as_deref() {
            if run.result_artifact_id.as_deref() != Some(artifact_id) {
                self.store
                    .set_dispatch_run_result_artifact(&run.id, artifact_id)?;
            }
        }
        let failure_reason = outcome.failure_detail.clone().or_else(|| {
            outcome
                .failure_class
                .map(|class| class.as_str().to_string())
        });
        let mut run = self.store.update_dispatch_run_status(
            &run.id,
            outcome.outcome_kind.terminal_status(),
            failure_reason,
        )?;
        if outcome.outcome_kind == DispatchOutcomeKind::FixReady {
            self.store
                .update_issue_task_status(&run.issue_task_id, IssueTaskStatus::FixReady)?;
        }
        run = self.store.get_dispatch_run(&run.id)?;
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

        let issue_task = self.store.get_issue_task(&run.issue_task_id)?;
        Ok(DispatchOutcomeRecordResult {
            run,
            issue_task,
            outcome,
        })
    }

    pub fn execute_dispatch(&self, run_id: &str) -> Result<DispatchExecutionResult> {
        execute_approved_codex_app_server_dispatch(&self.store, run_id)
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

    fn resolve_dispatch_session_selector(&self, agent_id: &str, selector: &str) -> Result<String> {
        let session = match self.store.get_session_link(selector) {
            Ok(session) => session,
            Err(link_error) => self
                .store
                .find_session_link_by_native_id_opt(agent_id, selector)?
                .with_context(|| {
                    format!(
                        "session selector {selector} is neither a local session link id nor a native session id for agent {agent_id}: {link_error}"
                    )
                })?,
        };
        if session.agent_id != agent_id {
            anyhow::bail!(
                "session link {} belongs to agent {}, not {}",
                session.id,
                session.agent_id,
                agent_id
            );
        }
        if session.status == AgentSessionStatus::Archived {
            anyhow::bail!("session link {} is archived", session.id);
        }
        Ok(session.id)
    }

    fn ensure_session_policy(&self, session_link_id: &str, action: PolicyAction) -> Result<()> {
        let session = self.store.get_session_link(session_link_id)?;
        let agent = self.store.get_agent_profile(&session.agent_id)?;
        if agent.adapter != "codex_app_server" {
            anyhow::bail!(
                "session link {session_link_id} uses adapter {}, not codex_app_server",
                agent.adapter
            );
        }
        let decision = classify_action(action);
        ensure_capability_preconditions(&self.store, &agent.id, &decision)?;
        Ok(())
    }

    fn ensure_agent_capability(
        &self,
        agent_id: &str,
        capability: AgentCapabilityName,
    ) -> Result<()> {
        let capability = self.store.get_agent_capability(agent_id, capability)?;
        if capability.status == CapabilityStatus::Unsupported {
            anyhow::bail!(
                "agent {agent_id} does not support capability {}",
                capability.capability.as_str()
            );
        }
        Ok(())
    }
}

fn terminal_outcome_for_status(status: DispatchRunStatus) -> Option<DispatchOutcomeKind> {
    match status {
        DispatchRunStatus::Completed => Some(DispatchOutcomeKind::FixReady),
        DispatchRunStatus::Failed => Some(DispatchOutcomeKind::Failed),
        DispatchRunStatus::Canceled => Some(DispatchOutcomeKind::Canceled),
        _ => None,
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
    selected_session: Option<&AgentSessionLink>,
) -> String {
    match selected_session {
        Some(session) => format!(
            "Dispatch {issue_key} to {agent_id} by resuming native session {} ({})?",
            session.native_session_id, session.id
        ),
        None => format!("Dispatch {issue_key} to {agent_id} by starting a new native session?"),
    }
}

fn codex_capabilities() -> Vec<(AgentCapabilityName, CapabilityStatus, serde_json::Value)> {
    let startup = default_codex_app_server_startup_metadata();
    let binary = startup
        .get("binary")
        .cloned()
        .unwrap_or_else(|| json!({ "name": "codex", "available": false }));
    let mut capabilities = codex_capability_mappings()
        .into_iter()
        .map(|mapping| {
            let wired_to_runtime = matches!(
                mapping.capability,
                AgentCapabilityName::StartSession
                    | AgentCapabilityName::ResumeSession
                    | AgentCapabilityName::ForkSession
                    | AgentCapabilityName::RenameSession
                    | AgentCapabilityName::ListSessions
                    | AgentCapabilityName::SearchSessions
                    | AgentCapabilityName::ReadTranscript
                    | AgentCapabilityName::SetGoal
                    | AgentCapabilityName::SetMetadata
                    | AgentCapabilityName::ArchiveSession
            );
            if wired_to_runtime {
                (
                    mapping.capability,
                    CapabilityStatus::Experimental,
                    json!({
                        "protocol": "codex_app_server_json_rpc",
                        "method": mapping.method,
                        "status": "wired_to_dispatch_runtime",
                        "binary": binary.clone(),
                        "startup": startup.clone()
                    }),
                )
            } else {
                (
                    mapping.capability,
                    CapabilityStatus::Unsupported,
                    json!({
                        "protocol": "codex_app_server_json_rpc",
                        "method": mapping.method,
                        "reason": "Codex exposes this native method, but Issue Finder does not wire this capability in the first dispatch runtime implementation",
                        "binary": binary.clone(),
                        "startup": startup.clone()
                    }),
                )
            }
        })
        .collect::<Vec<_>>();
    capabilities.push((
        AgentCapabilityName::OpenPr,
        CapabilityStatus::Unsupported,
        json!({
            "reason": "Issue Finder must not create pull requests in the first implementation",
            "startup": startup
        }),
    ));
    capabilities
}
