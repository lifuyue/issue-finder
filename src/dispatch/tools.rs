use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::Config;
use crate::paths::IssueFinderPaths;
use crate::tool_specs::{
    TOOL_A2A_APPROVE_SEND, TOOL_A2A_EXPORT_TASK, TOOL_A2A_IMPORT_RESULT, TOOL_A2A_REJECT_SEND,
    TOOL_AGENTS_LIST, TOOL_AGENT_CAPABILITIES, TOOL_AGENT_PROBE, TOOL_DISPATCH,
    TOOL_DISPATCH_APPROVE, TOOL_DISPATCH_ARTIFACTS, TOOL_DISPATCH_EVENTS, TOOL_DISPATCH_EXECUTE,
    TOOL_DISPATCH_IMPORT_HANDOFF, TOOL_DISPATCH_PROPOSE, TOOL_DISPATCH_RECORD_OUTCOME,
    TOOL_DISPATCH_REJECT, TOOL_DISPATCH_REVIEW_APPROVE, TOOL_DISPATCH_REVIEW_LIST,
    TOOL_DISPATCH_REVIEW_REJECT, TOOL_DISPATCH_REVIEW_SHOW, TOOL_DISPATCH_STATUS,
    TOOL_DISPATCH_TIMELINE, TOOL_DISPATCH_TRACE, TOOL_GITHUB_APPROVE_COMMENT,
    TOOL_GITHUB_DRAFT_FINAL_COMMENT, TOOL_GITHUB_DRAFT_TRACKING_COMMENT, TOOL_GITHUB_INTERACTIONS,
    TOOL_GITHUB_POST_COMMENT, TOOL_GITHUB_REJECT_COMMENT, TOOL_GITHUB_RETRY_COMMENT,
    TOOL_SESSIONS_APPROVE_MUTATION, TOOL_SESSIONS_ARCHIVE, TOOL_SESSIONS_FORK, TOOL_SESSIONS_LIST,
    TOOL_SESSIONS_READ, TOOL_SESSIONS_REJECT_MUTATION, TOOL_SESSIONS_RENAME, TOOL_SESSIONS_REPLAY,
    TOOL_SESSIONS_SEARCH, TOOL_SESSIONS_SYNC,
};

use super::github_projection::GitHubCommentPolicyResult;
use super::model::{
    ApprovalStatus, DispatchOutcomeFailureClass, DispatchOutcomeKind, DispatchRunStatus,
    DispatchTaskClass, DispatchValidationOutcome,
};
use super::runtime::{DispatchOutcomeRecordRequest, DispatchProposalRequest, DispatchRuntime};
use super::session_ops::SessionsSyncRequest;

#[derive(Debug, Clone, PartialEq)]
pub struct DispatchToolOutput {
    pub status: String,
    pub content_text: String,
    pub structured_fields: Value,
}

#[derive(Debug)]
pub enum DispatchToolError {
    InvalidArguments(String),
    BusinessBlock(DispatchToolOutput),
    System(anyhow::Error),
}

impl DispatchToolOutput {
    pub fn structured_content(&self, tool_name: &str) -> Value {
        let mut structured = serde_json::Map::new();
        structured.insert(
            "kind".to_string(),
            Value::String("issue_finder_tool_output".to_string()),
        );
        structured.insert("tool".to_string(), Value::String(tool_name.to_string()));
        structured.insert("status".to_string(), Value::String(self.status.clone()));
        structured.insert("success".to_string(), Value::Bool(true));
        if let Value::Object(fields) = self.structured_fields.clone() {
            structured.extend(fields);
        }
        Value::Object(structured)
    }
}

pub fn is_dispatch_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        TOOL_AGENTS_LIST
            | TOOL_AGENT_CAPABILITIES
            | TOOL_AGENT_PROBE
            | TOOL_SESSIONS_LIST
            | TOOL_SESSIONS_SYNC
            | TOOL_SESSIONS_SEARCH
            | TOOL_SESSIONS_READ
            | TOOL_SESSIONS_REPLAY
            | TOOL_SESSIONS_RENAME
            | TOOL_SESSIONS_FORK
            | TOOL_SESSIONS_ARCHIVE
            | TOOL_SESSIONS_APPROVE_MUTATION
            | TOOL_SESSIONS_REJECT_MUTATION
            | TOOL_DISPATCH_STATUS
            | TOOL_DISPATCH_EVENTS
            | TOOL_DISPATCH_TIMELINE
            | TOOL_DISPATCH_TRACE
            | TOOL_DISPATCH_ARTIFACTS
            | TOOL_DISPATCH_IMPORT_HANDOFF
            | TOOL_DISPATCH_REVIEW_LIST
            | TOOL_DISPATCH_REVIEW_SHOW
            | TOOL_DISPATCH_REVIEW_APPROVE
            | TOOL_DISPATCH_REVIEW_REJECT
            | TOOL_DISPATCH
            | TOOL_DISPATCH_PROPOSE
            | TOOL_DISPATCH_APPROVE
            | TOOL_DISPATCH_REJECT
            | TOOL_DISPATCH_EXECUTE
            | TOOL_DISPATCH_RECORD_OUTCOME
            | TOOL_A2A_EXPORT_TASK
            | TOOL_A2A_APPROVE_SEND
            | TOOL_A2A_REJECT_SEND
            | TOOL_A2A_IMPORT_RESULT
            | TOOL_GITHUB_DRAFT_TRACKING_COMMENT
            | TOOL_GITHUB_DRAFT_FINAL_COMMENT
            | TOOL_GITHUB_APPROVE_COMMENT
            | TOOL_GITHUB_REJECT_COMMENT
            | TOOL_GITHUB_POST_COMMENT
            | TOOL_GITHUB_RETRY_COMMENT
            | TOOL_GITHUB_INTERACTIONS
    )
}

pub fn execute_dispatch_tool(
    paths: IssueFinderPaths,
    config: &Config,
    tool_name: &str,
    arguments: &Value,
) -> std::result::Result<DispatchToolOutput, DispatchToolError> {
    let runtime = DispatchRuntime::open(paths).map_err(DispatchToolError::System)?;
    let result = match tool_name {
        TOOL_AGENTS_LIST => {
            let _: EmptyToolArgs = parse_arguments(arguments)?;
            let agents = runtime.list_agents().map_err(DispatchToolError::System)?;
            Ok(output(
                "ok",
                format!("Found {} agents.", agents.len()),
                json!({ "agents": agents }),
            ))
        }
        TOOL_AGENT_CAPABILITIES => {
            let args: AgentCapabilitiesToolArgs = parse_arguments(arguments)?;
            let capabilities = runtime
                .agent_capabilities(&args.agent)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "ok",
                format!(
                    "Found {} capabilities for {}.",
                    capabilities.capabilities.len(),
                    capabilities.agent.id
                ),
                json!({ "agentCapabilities": capabilities }),
            ))
        }
        TOOL_AGENT_PROBE => {
            let args: AgentProbeToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .probe_agent(&args.agent, args.refresh.unwrap_or(false))
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "ok",
                format!(
                    "Recorded {} probe results for {}.",
                    result.probes.len(),
                    result.agent_id
                ),
                json!({ "agentProbe": result }),
            ))
        }
        TOOL_SESSIONS_LIST => {
            let args: SessionsListToolArgs = parse_arguments(arguments)?;
            let sessions = runtime
                .list_sessions(normalized_optional(args.agent).as_deref())
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "ok",
                format!("Found {} local session links.", sessions.len()),
                json!({ "sessions": sessions }),
            ))
        }
        TOOL_SESSIONS_SYNC => {
            let args: SessionsSyncToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .sync_sessions(SessionsSyncRequest {
                    agent_id: args.agent.unwrap_or_else(|| "codex".to_string()),
                    search: normalized_optional(args.search),
                    limit: Some(args.limit.unwrap_or(20)),
                })
                .map_err(map_runtime_error)?;
            Ok(output(
                "ok",
                format!(
                    "Synced {} native sessions for {}.",
                    result.synced.len(),
                    result.agent_id
                ),
                json!({ "sessionsSync": result }),
            ))
        }
        TOOL_SESSIONS_SEARCH => {
            let args: SessionsSearchToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .search_sessions(&args.issue, normalized_optional(args.agent).as_deref())
                .map_err(map_issue_ref_error)?;
            Ok(output(
                "ok",
                format!(
                    "Found {} local session links for {}.",
                    result.sessions.len(),
                    result.issue_key
                ),
                json!({ "sessionSearch": result }),
            ))
        }
        TOOL_SESSIONS_READ => {
            let args: SessionLinkReadToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .read_session_transcript(&args.session_link_id)
                .map_err(map_runtime_error)?;
            Ok(output(
                "ok",
                format!(
                    "Read session {} transcript into artifact {}.",
                    result.session.id, result.transcript_artifact.id
                ),
                json!({ "sessionTranscript": result }),
            ))
        }
        TOOL_SESSIONS_REPLAY => {
            let args: SessionLinkReadToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .session_replay(&args.session_link_id)
                .map_err(map_runtime_error)?;
            Ok(output(
                "ok",
                format!("Found {} replay items.", result.len()),
                json!({ "sessionReplay": result }),
            ))
        }
        TOOL_SESSIONS_RENAME => {
            let args: SessionsRenameToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .rename_session(&args.session_link_id, &args.name)
                .map_err(map_runtime_error)?;
            Ok(output(
                "pending_approval",
                format!(
                    "Session mutation {} is pending approval.",
                    result.approval_request.id
                ),
                json!({ "sessionMutationProposal": result }),
            ))
        }
        TOOL_SESSIONS_ARCHIVE => {
            let args: SessionLinkReadToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .archive_session(&args.session_link_id)
                .map_err(map_runtime_error)?;
            Ok(output(
                "pending_approval",
                format!(
                    "Session mutation {} is pending approval.",
                    result.approval_request.id
                ),
                json!({ "sessionMutationProposal": result }),
            ))
        }
        TOOL_SESSIONS_FORK => {
            let args: SessionLinkReadToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .fork_session(&args.session_link_id)
                .map_err(map_runtime_error)?;
            Ok(output(
                "pending_approval",
                format!(
                    "Session mutation {} is pending approval.",
                    result.approval_request.id
                ),
                json!({ "sessionMutationProposal": result }),
            ))
        }
        TOOL_SESSIONS_APPROVE_MUTATION => {
            let args: SessionMutationApprovalToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .approve_session_mutation(&args.approval_request_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "approved",
                format!("Session mutation {} approved.", result.approval_request.id),
                json!({ "sessionMutationApproval": result }),
            ))
        }
        TOOL_SESSIONS_REJECT_MUTATION => {
            let args: SessionMutationApprovalToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .reject_session_mutation(&args.approval_request_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "rejected",
                format!("Session mutation {} rejected.", result.approval_request.id),
                json!({ "sessionMutationApproval": result }),
            ))
        }
        TOOL_DISPATCH_STATUS => {
            let args: DispatchRunReadToolArgs = parse_arguments(arguments)?;
            let status = runtime
                .dispatch_status(&args.run_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "ok",
                format!("Dispatch run {} is {}.", status.run.id, status.run.status),
                json!({ "dispatchStatus": status }),
            ))
        }
        TOOL_DISPATCH_EVENTS => {
            let args: DispatchRunReadToolArgs = parse_arguments(arguments)?;
            let events = runtime
                .dispatch_events(&args.run_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "ok",
                format!("Found {} dispatch events.", events.len()),
                json!({ "events": events }),
            ))
        }
        TOOL_DISPATCH_TIMELINE => {
            let args: DispatchRunReadToolArgs = parse_arguments(arguments)?;
            let timeline = runtime
                .dispatch_timeline(&args.run_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "ok",
                format!("Found {} timeline items.", timeline.items.len()),
                json!({ "dispatchTimeline": timeline }),
            ))
        }
        TOOL_DISPATCH_TRACE => {
            let args: DispatchRunReadToolArgs = parse_arguments(arguments)?;
            let trace = runtime
                .dispatch_trace(&args.run_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "ok",
                format!("Read dispatch trace for {}.", trace.run.id),
                json!({ "dispatchTrace": trace }),
            ))
        }
        TOOL_DISPATCH_ARTIFACTS => {
            let args: DispatchRunReadToolArgs = parse_arguments(arguments)?;
            let artifacts = runtime
                .dispatch_artifacts(&args.run_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "ok",
                format!("Found {} dispatch artifacts.", artifacts.len()),
                json!({ "artifacts": artifacts }),
            ))
        }
        TOOL_DISPATCH_IMPORT_HANDOFF => {
            let args: DispatchImportHandoffToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .import_handoff_from_inbox(&args.inbox_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                result.status.clone(),
                format!(
                    "Imported {}#{} as issue review {}.",
                    result.issue_task.repo_full_name,
                    result.issue_task.issue_number,
                    result.approval_request.id
                ),
                json!({ "packageImport": result }),
            ))
        }
        TOOL_DISPATCH_REVIEW_LIST => {
            let _: EmptyToolArgs = parse_arguments(arguments)?;
            let reviews = runtime
                .list_issue_reviews()
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "ok",
                format!("Found {} issue review requests.", reviews.len()),
                json!({ "issueReviews": reviews }),
            ))
        }
        TOOL_DISPATCH_REVIEW_SHOW => {
            let args: IssueReviewApprovalToolArgs = parse_arguments(arguments)?;
            let review = runtime
                .show_issue_review(&args.approval_request_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "ok",
                format!("Read issue review {}.", review.approval_request.id),
                json!({ "issueReview": review }),
            ))
        }
        TOOL_DISPATCH_REVIEW_APPROVE => {
            let args: IssueReviewApprovalToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .approve_issue_review(&args.approval_request_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "approved",
                format!(
                    "Approved issue review {} and created package.",
                    result.approval_request.id
                ),
                json!({ "issueReviewApproval": result }),
            ))
        }
        TOOL_DISPATCH_REVIEW_REJECT => {
            let args: IssueReviewRejectToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .reject_issue_review(&args.approval_request_id, normalized_optional(args.reason))
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "rejected",
                format!("Rejected issue review {}.", result.approval_request.id),
                json!({ "issueReviewApproval": result }),
            ))
        }
        TOOL_DISPATCH | TOOL_DISPATCH_PROPOSE => {
            let args: DispatchProposeToolArgs = parse_arguments(arguments)?;
            if args.new_session.unwrap_or(false) && args.session.is_some() {
                return Err(DispatchToolError::InvalidArguments(
                    "newSession cannot be combined with session".to_string(),
                ));
            }
            let proposal = runtime
                .propose_dispatch(DispatchProposalRequest {
                    issue: args.issue,
                    agent_id: args.agent.unwrap_or_else(|| "codex".to_string()),
                    requested_by: "tool".to_string(),
                    selected_session_link_id: normalized_optional(args.session),
                    new_session: args.new_session.unwrap_or(false),
                })
                .map_err(map_issue_ref_error)?;
            Ok(output(
                "pending_approval",
                format!("Dispatch proposal {} is pending approval.", proposal.run.id),
                json!({ "dispatchProposal": proposal }),
            ))
        }
        TOOL_DISPATCH_APPROVE => {
            let args: DispatchRunReadToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .resolve_dispatch_approval(&args.run_id, ApprovalStatus::Approved)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "approved",
                format!("Dispatch run {} is approved.", result.run.id),
                json!({ "dispatchApproval": result }),
            ))
        }
        TOOL_DISPATCH_REJECT => {
            let args: DispatchRunReadToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .resolve_dispatch_approval(&args.run_id, ApprovalStatus::Rejected)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "rejected",
                format!("Dispatch run {} is rejected.", result.run.id),
                json!({ "dispatchApproval": result }),
            ))
        }
        TOOL_DISPATCH_EXECUTE => {
            let args: DispatchRunReadToolArgs = parse_arguments(arguments)?;
            match runtime.execute_dispatch(&args.run_id) {
                Ok(result) => Ok(output(
                    "running",
                    format!(
                        "Dispatch run {} started native turn {}.",
                        result.run.id, result.turn.native_turn_id
                    ),
                    json!({ "dispatchExecution": result }),
                )),
                Err(error) if error.to_string().contains("is not approved") => Ok(output(
                    "pending_approval",
                    error.to_string(),
                    json!({ "runId": args.run_id, "approvalRequired": true }),
                )),
                Err(error) => {
                    let message = error.to_string();
                    match business_block_output(&message) {
                        Some(output) => Ok(output),
                        None => Err(DispatchToolError::System(error)),
                    }
                }
            }
        }
        TOOL_DISPATCH_RECORD_OUTCOME => {
            let args: DispatchRecordOutcomeToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .record_dispatch_outcome(DispatchOutcomeRecordRequest {
                    run_id: args.run_id,
                    idempotency_key: normalized_optional(args.idempotency_key),
                    outcome_kind: parse_dispatch_outcome_kind(&args.outcome)?,
                    failure_class: args
                        .failure_class
                        .as_deref()
                        .map(parse_dispatch_failure_class)
                        .transpose()?,
                    failure_detail: normalized_optional(args.failure_reason),
                    task_class: args
                        .task_class
                        .as_deref()
                        .map(parse_dispatch_task_class)
                        .transpose()?,
                    validation_outcome: args
                        .validation_outcome
                        .as_deref()
                        .map(parse_dispatch_validation_outcome)
                        .transpose()?,
                    result_artifact_id: normalized_optional(args.result_artifact_id),
                    metadata_json: json!({ "source": "tool_dispatch_record_outcome" }),
                })
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "ok",
                format!("Recorded dispatch outcome {}.", result.outcome.id),
                json!({ "dispatchOutcome": result }),
            ))
        }
        TOOL_A2A_EXPORT_TASK => {
            let args: A2aExportTaskToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .export_a2a_task(&args.issue)
                .map_err(map_issue_ref_error)?;
            Ok(output(
                "pending_approval",
                format!(
                    "Created A2A task artifact {} from IssueTaskPackage v3 and approval request {}.",
                    result.export_artifact.id, result.approval_request.id
                ),
                json!({ "a2aExport": result }),
            ))
        }
        TOOL_A2A_APPROVE_SEND => {
            let args: A2aSendApprovalToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .approve_a2a_send(&args.approval_request_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "approved",
                format!(
                    "Approved outbound A2A artifact {}.",
                    result.export_artifact.id
                ),
                json!({ "a2aApproval": result }),
            ))
        }
        TOOL_A2A_REJECT_SEND => {
            let args: A2aSendApprovalToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .reject_a2a_send(&args.approval_request_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "rejected",
                format!(
                    "Rejected outbound A2A artifact {}.",
                    result.export_artifact.id
                ),
                json!({ "a2aApproval": result }),
            ))
        }
        TOOL_A2A_IMPORT_RESULT => {
            let args: A2aImportResultToolArgs = parse_arguments(arguments)?;
            let status = args
                .status
                .as_deref()
                .map(parse_dispatch_run_status)
                .transpose()?;
            let outcome = optional_outcome_record_request(
                args.outcome.as_deref(),
                args.failure_class.as_deref(),
                normalized_optional(args.failure_reason),
                args.task_class.as_deref(),
                args.validation_outcome.as_deref(),
                normalized_optional(args.idempotency_key),
                args.run_id.clone(),
            )?;
            let result = runtime
                .import_a2a_result(
                    &args.run_id,
                    &args.path,
                    &args.kind.unwrap_or_else(|| "fix_result".to_string()),
                    &args
                        .content_type
                        .unwrap_or_else(|| "application/json".to_string()),
                    status,
                    outcome,
                )
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "ok",
                format!(
                    "Imported A2A result artifact {} for dispatch run {} against the package outcome contract.",
                    result.artifact.id, result.run.id
                ),
                json!({ "a2aResultImport": result }),
            ))
        }
        TOOL_GITHUB_DRAFT_TRACKING_COMMENT => {
            let args: GithubDraftTrackingCommentToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .draft_github_tracking_comment(&args.issue, normalized_optional(args.body))
                .map_err(map_issue_ref_error)?;
            Ok(github_policy_output(result))
        }
        TOOL_GITHUB_DRAFT_FINAL_COMMENT => {
            let args: GithubDraftFinalCommentToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .draft_github_final_comment(&args.run_id, normalized_optional(args.body))
                .map_err(DispatchToolError::System)?;
            Ok(github_policy_output(result))
        }
        TOOL_GITHUB_APPROVE_COMMENT => {
            let args: GithubInteractionToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .approve_github_interaction(&args.interaction_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "approved",
                format!("Approved GitHub interaction {}.", result.interaction.id),
                json!({ "githubApproval": result }),
            ))
        }
        TOOL_GITHUB_REJECT_COMMENT => {
            let args: GithubInteractionToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .reject_github_interaction(&args.interaction_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "rejected",
                format!("Rejected GitHub interaction {}.", result.interaction.id),
                json!({ "githubApproval": result }),
            ))
        }
        TOOL_GITHUB_POST_COMMENT => {
            let args: GithubInteractionToolArgs = parse_arguments(arguments)?;
            match runtime.post_github_interaction(config, &args.interaction_id) {
                Ok(result) => Ok(output(
                    "posted",
                    format!(
                        "Posted GitHub interaction {} as comment {}.",
                        result.interaction.id, result.posted_comment.id
                    ),
                    json!({ "githubPost": result }),
                )),
                Err(error) if error.to_string().contains("not approved") => Ok(output(
                    "pending_approval",
                    error.to_string(),
                    json!({ "interactionId": args.interaction_id, "approvalRequired": true }),
                )),
                Err(error) => Err(DispatchToolError::System(error)),
            }
        }
        TOOL_GITHUB_RETRY_COMMENT => {
            let args: GithubInteractionToolArgs = parse_arguments(arguments)?;
            let result = runtime
                .retry_github_interaction(config, &args.interaction_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "posted",
                format!(
                    "Retried GitHub interaction {} as comment {}.",
                    result.interaction.id, result.posted_comment.id
                ),
                json!({ "githubPost": result }),
            ))
        }
        TOOL_GITHUB_INTERACTIONS => {
            let args: GithubInteractionsToolArgs = parse_arguments(arguments)?;
            let interactions = runtime
                .list_github_interactions(&args.issue)
                .map_err(map_issue_ref_error)?;
            Ok(output(
                "ok",
                format!("Found {} GitHub interactions.", interactions.len()),
                json!({ "githubInteractions": interactions }),
            ))
        }
        _ => Err(DispatchToolError::InvalidArguments(format!(
            "unknown dispatch tool {tool_name}"
        ))),
    };
    result.or_else(map_business_block)
}

fn output(
    status: impl Into<String>,
    content_text: impl Into<String>,
    fields: Value,
) -> DispatchToolOutput {
    DispatchToolOutput {
        status: status.into(),
        content_text: content_text.into(),
        structured_fields: fields,
    }
}

fn github_policy_output(result: GitHubCommentPolicyResult) -> DispatchToolOutput {
    let status = match result.draft.as_ref() {
        Some(_) => "pending_approval",
        None => result.decision.decision_kind.as_str(),
    };
    let content_text = match result.draft.as_ref() {
        Some(draft) => format!(
            "Drafted {} {} and created GitHub post approval {}.",
            result.issue_task.issue_key,
            draft.interaction.interaction_type,
            draft.approval_request.id
        ),
        None => format!(
            "GitHub interaction policy decided {} for {}.",
            result.decision.decision_kind, result.issue_task.issue_key
        ),
    };
    output(status, content_text, json!({ "githubDecision": result }))
}

fn parse_arguments<T>(arguments: &Value) -> std::result::Result<T, DispatchToolError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(arguments.clone())
        .map_err(|error| DispatchToolError::InvalidArguments(error.to_string()))
}

fn normalized_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_dispatch_run_status(
    value: &str,
) -> std::result::Result<DispatchRunStatus, DispatchToolError> {
    DispatchRunStatus::parse_value(value).ok_or_else(|| {
        DispatchToolError::InvalidArguments(format!("invalid dispatch status {value}"))
    })
}

fn parse_dispatch_outcome_kind(
    value: &str,
) -> std::result::Result<DispatchOutcomeKind, DispatchToolError> {
    DispatchOutcomeKind::parse_value(value).ok_or_else(|| {
        DispatchToolError::InvalidArguments(format!("invalid dispatch outcome kind {value}"))
    })
}

fn parse_dispatch_failure_class(
    value: &str,
) -> std::result::Result<DispatchOutcomeFailureClass, DispatchToolError> {
    DispatchOutcomeFailureClass::parse_value(value).ok_or_else(|| {
        DispatchToolError::InvalidArguments(format!("invalid dispatch failure class {value}"))
    })
}

fn parse_dispatch_task_class(
    value: &str,
) -> std::result::Result<DispatchTaskClass, DispatchToolError> {
    DispatchTaskClass::parse_value(value).ok_or_else(|| {
        DispatchToolError::InvalidArguments(format!("invalid dispatch task class {value}"))
    })
}

fn parse_dispatch_validation_outcome(
    value: &str,
) -> std::result::Result<DispatchValidationOutcome, DispatchToolError> {
    DispatchValidationOutcome::parse_value(value).ok_or_else(|| {
        DispatchToolError::InvalidArguments(format!("invalid dispatch validation outcome {value}"))
    })
}

fn optional_outcome_record_request(
    outcome: Option<&str>,
    failure_class: Option<&str>,
    failure_reason: Option<String>,
    task_class: Option<&str>,
    validation_outcome: Option<&str>,
    idempotency_key: Option<String>,
    run_id: String,
) -> std::result::Result<Option<DispatchOutcomeRecordRequest>, DispatchToolError> {
    let Some(outcome) = outcome else {
        return Ok(None);
    };
    Ok(Some(DispatchOutcomeRecordRequest {
        run_id,
        idempotency_key,
        outcome_kind: parse_dispatch_outcome_kind(outcome)?,
        failure_class: failure_class
            .map(parse_dispatch_failure_class)
            .transpose()?,
        failure_detail: failure_reason,
        task_class: task_class.map(parse_dispatch_task_class).transpose()?,
        validation_outcome: validation_outcome
            .map(parse_dispatch_validation_outcome)
            .transpose()?,
        result_artifact_id: None,
        metadata_json: json!({ "source": "tool_a2a_import_result" }),
    }))
}

fn map_runtime_error(error: anyhow::Error) -> DispatchToolError {
    let message = error.to_string();
    if let Some(output) = business_block_output(&message) {
        DispatchToolError::BusinessBlock(output)
    } else {
        DispatchToolError::System(error)
    }
}

fn map_issue_ref_error(error: anyhow::Error) -> DispatchToolError {
    let message = error.to_string();
    if message.contains("invalid issue reference; expected owner/repo#123") {
        DispatchToolError::InvalidArguments(message)
    } else if let Some(output) = business_block_output(&message) {
        DispatchToolError::BusinessBlock(output)
    } else {
        DispatchToolError::System(error)
    }
}

fn map_business_block(
    error: DispatchToolError,
) -> std::result::Result<DispatchToolOutput, DispatchToolError> {
    match error {
        DispatchToolError::System(error) => {
            let message = error.to_string();
            if let Some(output) = business_block_output(&message) {
                Ok(output)
            } else {
                Err(DispatchToolError::System(error))
            }
        }
        other => Err(other),
    }
}

fn business_block_output(message: &str) -> Option<DispatchToolOutput> {
    if let Some(capability) = unsupported_capability(message) {
        return Some(output(
            "unsupported_capability",
            message.to_string(),
            json!({
                "blocked": true,
                "reason": message,
                "unsupportedCapability": capability
            }),
        ));
    }

    if let Some(issue_key) = missing_task_package(message) {
        return Some(output(
            "missing_task_package",
            message.to_string(),
            json!({
                "blocked": true,
                "reason": message,
                "issueKey": issue_key,
                "missingTaskPackage": true
            }),
        ));
    }

    if let Some((issue_key, approval_request_id)) = pending_issue_review(message) {
        return Some(output(
            "pending_issue_review",
            message.to_string(),
            json!({
                "blocked": true,
                "reason": message,
                "issueKey": issue_key,
                "approvalRequestId": approval_request_id,
                "reviewRequired": true
            }),
        ));
    }

    if let Some((issue_key, approval_request_id)) = rejected_issue_review(message) {
        return Some(output(
            "issue_review_rejected",
            message.to_string(),
            json!({
                "blocked": true,
                "reason": message,
                "issueKey": issue_key,
                "approvalRequestId": approval_request_id,
                "reviewRejected": true
            }),
        ));
    }

    None
}

fn unsupported_capability(message: &str) -> Option<&str> {
    message
        .split_once("does not support capability ")
        .map(|(_, capability)| capability.trim())
        .filter(|capability| !capability.is_empty())
}

fn missing_task_package(message: &str) -> Option<&str> {
    message
        .strip_prefix("issue task ")
        .and_then(|value| {
            value.strip_suffix(" has no task package artifact and no ready inbox handoff was found")
        })
        .filter(|issue_key| !issue_key.is_empty())
}

fn pending_issue_review(message: &str) -> Option<(&str, &str)> {
    let rest = message.strip_prefix("issue task ")?;
    let (issue_key, approval_request_id) = rest.split_once(" is pending issue review approval ")?;
    if issue_key.is_empty() || approval_request_id.is_empty() {
        return None;
    }
    Some((issue_key, approval_request_id))
}

fn rejected_issue_review(message: &str) -> Option<(&str, &str)> {
    let rest = message.strip_prefix("issue task ")?;
    let (issue_key, rest) = rest.split_once(" issue review ")?;
    let approval_request_id = rest.strip_suffix(" was rejected")?;
    if issue_key.is_empty() || approval_request_id.is_empty() {
        return None;
    }
    Some((issue_key, approval_request_id))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmptyToolArgs {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentCapabilitiesToolArgs {
    agent: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentProbeToolArgs {
    agent: String,
    #[serde(default)]
    refresh: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionsListToolArgs {
    #[serde(default)]
    agent: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionsSyncToolArgs {
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionsSearchToolArgs {
    issue: String,
    #[serde(default)]
    agent: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionLinkReadToolArgs {
    session_link_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionsRenameToolArgs {
    session_link_id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionMutationApprovalToolArgs {
    approval_request_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DispatchRunReadToolArgs {
    run_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DispatchImportHandoffToolArgs {
    inbox_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IssueReviewApprovalToolArgs {
    approval_request_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IssueReviewRejectToolArgs {
    approval_request_id: String,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DispatchProposeToolArgs {
    issue: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    new_session: Option<bool>,
    #[serde(default)]
    session: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct A2aExportTaskToolArgs {
    issue: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct A2aSendApprovalToolArgs {
    approval_request_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct A2aImportResultToolArgs {
    run_id: String,
    path: PathBuf,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    outcome: Option<String>,
    #[serde(default)]
    failure_class: Option<String>,
    #[serde(default)]
    failure_reason: Option<String>,
    #[serde(default)]
    task_class: Option<String>,
    #[serde(default)]
    validation_outcome: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DispatchRecordOutcomeToolArgs {
    run_id: String,
    outcome: String,
    #[serde(default)]
    failure_class: Option<String>,
    #[serde(default)]
    failure_reason: Option<String>,
    #[serde(default)]
    task_class: Option<String>,
    #[serde(default)]
    validation_outcome: Option<String>,
    #[serde(default)]
    result_artifact_id: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GithubDraftTrackingCommentToolArgs {
    issue: String,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GithubDraftFinalCommentToolArgs {
    run_id: String,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GithubInteractionToolArgs {
    interaction_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GithubInteractionsToolArgs {
    issue: String,
}
