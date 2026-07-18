use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::Config;
use crate::paths::IssueFinderPaths;
use crate::tool_specs::{
    TOOL_A2A_APPROVE_SEND, TOOL_A2A_EXPORT_TASK, TOOL_A2A_IMPORT_RESULT, TOOL_A2A_REJECT_SEND,
    TOOL_AGENTS_LIST, TOOL_AGENT_CAPABILITIES, TOOL_AGENT_PROBE, TOOL_DISPATCH,
    TOOL_DISPATCH_APPROVE, TOOL_DISPATCH_ARTIFACTS, TOOL_DISPATCH_EVENTS, TOOL_DISPATCH_EXECUTE,
    TOOL_DISPATCH_IMPORT_HANDOFF, TOOL_DISPATCH_INTERRUPT, TOOL_DISPATCH_PENDING_REQUESTS,
    TOOL_DISPATCH_REJECT, TOOL_DISPATCH_RESPOND, TOOL_DISPATCH_REVIEW_APPROVE,
    TOOL_DISPATCH_REVIEW_LIST, TOOL_DISPATCH_REVIEW_REJECT, TOOL_DISPATCH_REVIEW_SHOW,
    TOOL_DISPATCH_STATUS, TOOL_DISPATCH_STEER, TOOL_DISPATCH_SYNC, TOOL_DISPATCH_TIMELINE,
    TOOL_DISPATCH_TRACE, TOOL_GITHUB_APPROVE_COMMENT, TOOL_GITHUB_DRAFT_FINAL_COMMENT,
    TOOL_GITHUB_DRAFT_TRACKING_COMMENT, TOOL_GITHUB_INTERACTIONS, TOOL_GITHUB_POST_COMMENT,
    TOOL_GITHUB_REJECT_COMMENT, TOOL_GITHUB_RETRY_COMMENT,
};

use super::github_projection::GitHubCommentPolicyResult;
use super::model::ApprovalStatus;
use super::runtime::{DispatchProposalRequest, DispatchRuntime};

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
            | TOOL_DISPATCH_APPROVE
            | TOOL_DISPATCH_REJECT
            | TOOL_DISPATCH_EXECUTE
            | TOOL_DISPATCH_PENDING_REQUESTS
            | TOOL_DISPATCH_RESPOND
            | TOOL_DISPATCH_STEER
            | TOOL_DISPATCH_INTERRUPT
            | TOOL_DISPATCH_SYNC
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
        TOOL_DISPATCH => {
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
                    selected_thread_id: normalized_optional(args.session),
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
                    format!("Dispatch run {} supervisor started.", result.run.id),
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
        TOOL_DISPATCH_PENDING_REQUESTS => {
            let args: DispatchRunReadToolArgs = parse_arguments(arguments)?;
            let requests = runtime
                .pending_requests(&args.run_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "ok",
                format!("Found {} pending requests.", requests.len()),
                json!({"pendingRequests":requests}),
            ))
        }
        TOOL_DISPATCH_RESPOND => {
            let args: DispatchRespondToolArgs = parse_arguments(arguments)?;
            runtime
                .respond_pending_request(&args.request_id, args.response)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "response_queued",
                "Queued the pending request response.",
                json!({"requestId":args.request_id}),
            ))
        }
        TOOL_DISPATCH_STEER => {
            let args: DispatchSteerToolArgs = parse_arguments(arguments)?;
            runtime
                .steer_dispatch(&args.run_id, &args.message)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "queued",
                "Queued guidance for the active Codex turn.",
                json!({"runId":args.run_id}),
            ))
        }
        TOOL_DISPATCH_INTERRUPT => {
            let args: DispatchRunReadToolArgs = parse_arguments(arguments)?;
            runtime
                .interrupt_dispatch(&args.run_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "queued",
                "Queued interruption for the active Codex turn.",
                json!({"runId":args.run_id}),
            ))
        }
        TOOL_DISPATCH_SYNC => {
            let args: DispatchRunReadToolArgs = parse_arguments(arguments)?;
            let status = runtime
                .sync_dispatch(&args.run_id)
                .map_err(DispatchToolError::System)?;
            Ok(output(
                "ok",
                "Read supervisor-projected run state.",
                json!({"dispatchStatus":status}),
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
                    "Created A2A task artifact {} from TaskPackage and approval request {}.",
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
            let result = runtime
                .import_a2a_result(&args.run_id, &args.path)
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
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DispatchRespondToolArgs {
    request_id: String,
    response: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DispatchSteerToolArgs {
    run_id: String,
    message: String,
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
