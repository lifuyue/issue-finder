use serde_json::{json, Value};

use crate::tool_specs::IssueFinderToolSpec;

pub const TOOL_AGENTS_LIST: &str = "issue-finder.agents_list";
pub const TOOL_AGENT_CAPABILITIES: &str = "issue-finder.agent_capabilities";
pub const TOOL_AGENT_PROBE: &str = "issue-finder.agent_probe";
pub const TOOL_DISPATCH_STATUS: &str = "issue-finder.dispatch_status";
pub const TOOL_DISPATCH_EVENTS: &str = "issue-finder.dispatch_events";
pub const TOOL_DISPATCH_TIMELINE: &str = "issue-finder.dispatch_timeline";
pub const TOOL_DISPATCH_TRACE: &str = "issue-finder.dispatch_trace";
pub const TOOL_DISPATCH_ARTIFACTS: &str = "issue-finder.dispatch_artifacts";
pub const TOOL_DISPATCH_IMPORT_HANDOFF: &str = "issue-finder.dispatch_import_handoff";
pub const TOOL_DISPATCH_REVIEW_LIST: &str = "issue-finder.dispatch_review_list";
pub const TOOL_DISPATCH_REVIEW_SHOW: &str = "issue-finder.dispatch_review_show";
pub const TOOL_DISPATCH_REVIEW_APPROVE: &str = "issue-finder.dispatch_review_approve";
pub const TOOL_DISPATCH_REVIEW_REJECT: &str = "issue-finder.dispatch_review_reject";
pub const TOOL_DISPATCH: &str = "issue-finder.dispatch";
pub const TOOL_DISPATCH_APPROVE: &str = "issue-finder.dispatch_approve";
pub const TOOL_DISPATCH_REJECT: &str = "issue-finder.dispatch_reject";
pub const TOOL_DISPATCH_EXECUTE: &str = "issue-finder.dispatch_execute";
pub const TOOL_DISPATCH_PENDING_REQUESTS: &str = "issue-finder.dispatch_pending_requests";
pub const TOOL_DISPATCH_RESPOND: &str = "issue-finder.dispatch_respond";
pub const TOOL_DISPATCH_STEER: &str = "issue-finder.dispatch_steer";
pub const TOOL_DISPATCH_INTERRUPT: &str = "issue-finder.dispatch_interrupt";
pub const TOOL_DISPATCH_SYNC: &str = "issue-finder.dispatch_sync";
pub const TOOL_A2A_EXPORT_TASK: &str = "issue-finder.a2a_export_task";
pub const TOOL_A2A_APPROVE_SEND: &str = "issue-finder.a2a_approve_send";
pub const TOOL_A2A_REJECT_SEND: &str = "issue-finder.a2a_reject_send";
pub const TOOL_A2A_IMPORT_RESULT: &str = "issue-finder.a2a_import_result";
pub const TOOL_GITHUB_DRAFT_TRACKING_COMMENT: &str = "issue-finder.github_draft_tracking_comment";
pub const TOOL_GITHUB_DRAFT_FINAL_COMMENT: &str = "issue-finder.github_draft_final_comment";
pub const TOOL_GITHUB_APPROVE_COMMENT: &str = "issue-finder.github_approve_comment";
pub const TOOL_GITHUB_REJECT_COMMENT: &str = "issue-finder.github_reject_comment";
pub const TOOL_GITHUB_POST_COMMENT: &str = "issue-finder.github_post_comment";
pub const TOOL_GITHUB_RETRY_COMMENT: &str = "issue-finder.github_retry_comment";
pub const TOOL_GITHUB_INTERACTIONS: &str = "issue-finder.github_interactions";

pub(crate) fn dispatch_tool_specs() -> Vec<IssueFinderToolSpec> {
    vec![
        dispatch_tool_spec(
            "agents_list",
            "List local execution agent profiles from the dispatch store.",
            empty_schema(),
            false,
        ),
        dispatch_tool_spec(
            "agent_capabilities",
            "List one execution agent's declared native capabilities.",
            agent_capabilities_schema(),
            false,
        ),
        dispatch_tool_spec(
            "agent_probe",
            "Probe one execution agent's adapter capabilities and cache the result.",
            agent_probe_schema(),
            false,
        ),
        dispatch_tool_spec(
            "dispatch_status",
            "Read one local dispatch run summary.",
            dispatch_run_read_schema(),
            false,
        ),
        dispatch_tool_spec(
            "dispatch_events",
            "List persisted events for a local dispatch run.",
            dispatch_run_read_schema(),
            false,
        ),
        dispatch_tool_spec(
            "dispatch_timeline",
            "List a merged chronological timeline for a local dispatch run.",
            dispatch_run_read_schema(),
            false,
        ),
        dispatch_tool_spec(
            "dispatch_trace",
            "Read diagnostic trace records for a local dispatch run.",
            dispatch_run_read_schema(),
            false,
        ),
        dispatch_tool_spec(
            "dispatch_artifacts",
            "List persisted artifacts for a local dispatch run.",
            dispatch_run_read_schema(),
            false,
        ),
        dispatch_tool_spec(
            "dispatch_import_handoff",
            "Import an existing inbox handoff as a review-gated local issue task candidate.",
            dispatch_import_handoff_schema(),
            false,
        ),
        dispatch_tool_spec(
            "dispatch_review_list",
            "List issue review approval requests created from imported handoffs.",
            empty_schema(),
            false,
        ),
        dispatch_tool_spec(
            "dispatch_review_show",
            "Show one issue review approval request and its local artifacts.",
            issue_review_approval_schema(),
            false,
        ),
        dispatch_tool_spec(
            "dispatch_review_approve",
            "Approve an issue review and create the immutable TaskPackage artifact.",
            issue_review_approval_schema(),
            false,
        ),
        dispatch_tool_spec(
            "dispatch_review_reject",
            "Reject an issue review without dismissing the recommendation.",
            issue_review_reject_schema(),
            false,
        ),
        dispatch_tool_spec(
            "dispatch",
            "Create a pending dispatch approval without starting a native agent session; imports a matching ready handoff when needed and returns pending_issue_review until human review approves the package.",
            dispatch_request_schema(),
            false,
        ),
        dispatch_tool_spec(
            "dispatch_approve",
            "Approve a pending local dispatch proposal without directly executing an agent.",
            dispatch_run_read_schema(),
            false,
        ),
        dispatch_tool_spec(
            "dispatch_reject",
            "Reject a pending local dispatch proposal and cancel the dispatch run.",
            dispatch_run_read_schema(),
            false,
        ),
        dispatch_tool_spec(
            "dispatch_execute",
            "Start or resume the native execution agent for an already approved dispatch run.",
            dispatch_run_read_schema(),
            false,
        ),
        dispatch_tool_spec("dispatch_pending_requests", "List durable app-server requests awaiting user action.", dispatch_run_read_schema(), false),
        dispatch_tool_spec("dispatch_respond", "Queue an idempotent response to a durable pending request.", dispatch_respond_schema(), false),
        dispatch_tool_spec("dispatch_steer", "Queue in-flight guidance for the active Codex turn.", dispatch_steer_schema(), false),
        dispatch_tool_spec("dispatch_interrupt", "Queue interruption of the active Codex turn.", dispatch_run_read_schema(), false),
        dispatch_tool_spec("dispatch_sync", "Read supervisor-projected durable run state without opening a second app-server connection.", dispatch_run_read_schema(), false),
        dispatch_tool_spec(
            "a2a_export_task",
            "Create a local A2A gateway artifact from the approved TaskPackage and require approval before external use.",
            a2a_export_task_schema(),
            false,
        ),
        dispatch_tool_spec(
            "a2a_approve_send",
            "Approve a pending outbound A2A task artifact for external use.",
            a2a_send_approval_schema(),
            false,
        ),
        dispatch_tool_spec(
            "a2a_reject_send",
            "Reject a pending outbound A2A task artifact.",
            a2a_send_approval_schema(),
            false,
        ),
        dispatch_tool_spec(
            "a2a_import_result",
            "Import a local A2A candidate result through the same evaluator used by submit_result.",
            a2a_import_result_schema(),
            false,
        ),
        dispatch_tool_spec(
            "github_draft_tracking_comment",
            "Evaluate GitHub tracking-comment policy for an imported issue; default policy returns no_comment, while an allowed draft creates a post approval request.",
            github_draft_tracking_comment_schema(),
            false,
        ),
        dispatch_tool_spec(
            "github_draft_final_comment",
            "Evaluate GitHub final/clarification comment policy from a dispatch result artifact; only explicit suggested replies create post approval requests.",
            github_draft_final_comment_schema(),
            false,
        ),
        dispatch_tool_spec(
            "github_approve_comment",
            "Approve a local GitHub comment draft for posting.",
            github_interaction_read_schema(),
            false,
        ),
        dispatch_tool_spec(
            "github_reject_comment",
            "Reject a local GitHub comment draft.",
            github_interaction_read_schema(),
            false,
        ),
        dispatch_tool_spec(
            "github_post_comment",
            "Post an approved GitHub comment draft through the configured GitHub token.",
            github_interaction_read_schema(),
            false,
        ),
        dispatch_tool_spec(
            "github_retry_comment",
            "Retry posting a failed GitHub comment interaction through the configured GitHub token.",
            github_interaction_read_schema(),
            false,
        ),
        dispatch_tool_spec(
            "github_interactions",
            "List local GitHub comment interactions for an imported issue task.",
            github_interactions_schema(),
            false,
        ),
    ]
}

fn dispatch_tool_spec(
    name: &str,
    description: &str,
    input_schema: Value,
    defer_loading: bool,
) -> IssueFinderToolSpec {
    IssueFinderToolSpec {
        namespace: Some("issue-finder".to_string()),
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        defer_loading,
    }
}

fn empty_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

fn agent_capabilities_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "agent": { "type": "string" }
        },
        "required": ["agent"],
        "additionalProperties": false
    })
}

fn agent_probe_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "agent": { "type": "string" },
            "refresh": { "type": "boolean", "default": false }
        },
        "required": ["agent"],
        "additionalProperties": false
    })
}

fn dispatch_run_read_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "runId": { "type": "string" }
        },
        "required": ["runId"],
        "additionalProperties": false
    })
}

fn dispatch_import_handoff_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "inboxId": { "type": "string" }
        },
        "required": ["inboxId"],
        "additionalProperties": false
    })
}

fn issue_review_approval_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "approvalRequestId": { "type": "string" }
        },
        "required": ["approvalRequestId"],
        "additionalProperties": false
    })
}

fn issue_review_reject_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "approvalRequestId": { "type": "string" },
            "reason": { "type": ["string", "null"], "default": null }
        },
        "required": ["approvalRequestId"],
        "additionalProperties": false
    })
}

fn dispatch_request_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "issue": { "type": "string" },
            "agent": { "type": ["string", "null"], "default": "codex" },
            "newSession": { "type": "boolean", "default": false },
            "session": { "type": ["string", "null"], "default": null }
        },
        "required": ["issue"],
        "additionalProperties": false
    })
}

fn a2a_export_task_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "issue": { "type": "string" }
        },
        "required": ["issue"],
        "additionalProperties": false
    })
}

fn a2a_send_approval_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "approvalRequestId": { "type": "string" }
        },
        "required": ["approvalRequestId"],
        "additionalProperties": false
    })
}

fn a2a_import_result_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "runId": { "type": "string" },
            "path": { "type": "string" }
        },
        "required": ["runId", "path"],
        "additionalProperties": false
    })
}

fn dispatch_respond_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "requestId": { "type": "string" },
            "response": {}
        },
        "required": ["requestId", "response"],
        "additionalProperties": false
    })
}

fn dispatch_steer_schema() -> Value {
    json!({
        "type":"object",
        "properties":{"runId":{"type":"string"},"message":{"type":"string"}},
        "required":["runId","message"],
        "additionalProperties":false
    })
}

fn github_draft_tracking_comment_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "issue": { "type": "string" },
            "body": { "type": ["string", "null"], "default": null }
        },
        "required": ["issue"],
        "additionalProperties": false
    })
}

fn github_draft_final_comment_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "runId": { "type": "string" },
            "body": { "type": ["string", "null"], "default": null }
        },
        "required": ["runId"],
        "additionalProperties": false
    })
}

fn github_interaction_read_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "interactionId": { "type": "string" }
        },
        "required": ["interactionId"],
        "additionalProperties": false
    })
}

fn github_interactions_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "issue": { "type": "string" }
        },
        "required": ["issue"],
        "additionalProperties": false
    })
}
