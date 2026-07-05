use serde::Serialize;
use serde_json::{json, Value};

pub const TOOL_SCOUT: &str = "issue-finder.scout";
pub const TOOL_DISCOVER_CANDIDATES: &str = "issue-finder.discover_candidates";
pub const TOOL_INSPECT_CANDIDATE: &str = "issue-finder.inspect_candidate";
pub const TOOL_INSPECT_DISCUSSION: &str = "issue-finder.inspect_discussion";
pub const TOOL_INSPECT_REPO_HEALTH: &str = "issue-finder.inspect_repo_health";
pub const TOOL_RANK_SHORTLIST: &str = "issue-finder.rank_shortlist";
pub const TOOL_ASSESS: &str = "issue-finder.assess";
pub const TOOL_PREPARE: &str = "issue-finder.prepare";
pub const TOOL_READ_CONTEXT: &str = "issue-finder.read_context";
pub const TOOL_STATUS: &str = "issue-finder.status";
pub const TOOL_MEMORY_STATUS: &str = "issue-finder.memory_status";
pub const TOOL_MEMORY_RECALL: &str = "issue-finder.memory_recall";
pub const TOOL_MEMORY_DREAMS_LIST: &str = "issue-finder.memory_dreams_list";
pub const TOOL_MEMORY_DREAM_SHOW: &str = "issue-finder.memory_dream_show";
pub const TOOL_MEMORY_HINTS_LIST: &str = "issue-finder.memory_hints_list";
pub const TOOL_MEMORY_HINT_UPDATE: &str = "issue-finder.memory_hint_update";
pub const TOOL_MEMORY_TOMBSTONE: &str = "issue-finder.memory_tombstone";

pub use crate::dispatch::tool_specs::{
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

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IssueFinderToolSpecsEnvelope {
    pub kind: String,
    pub version: u8,
    pub quick_start: ToolQuickStart,
    pub recommended_workflow: Vec<ToolWorkflowStep>,
    pub tools: Vec<IssueFinderToolSpec>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolQuickStart {
    pub summary: String,
    pub first_call: ToolFirstCall,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolFirstCall {
    pub default_tool: String,
    pub default_arguments: Value,
    pub when_ready_unknown: String,
    pub fallback_after_setup_failure: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolWorkflowStep {
    pub step: String,
    pub tool: String,
    pub purpose: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deferred: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub first_sections: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IssueFinderToolSpec {
    pub namespace: Option<String>,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub defer_loading: bool,
    pub agent: ToolAgentMetadata,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolAgentMetadata {
    pub exposure: String,
    pub visible_by_default: bool,
    pub supports_parallel: bool,
    pub side_effects: Vec<String>,
}

pub fn list_tool_specs() -> IssueFinderToolSpecsEnvelope {
    let mut tools = vec![
        tool_spec(
            "status",
            "Report Issue Finder config, GitHub token source, and auth readiness without exposing tokens.",
            status_schema(),
            false,
        ),
        tool_spec(
            "scout",
            "Run the batch discovery, enrichment, ranking, and filtering pipeline for GitHub issues.",
            scout_schema(),
            true,
        ),
        tool_spec(
            "discover_candidates",
            "Lightly discover candidate GitHub issues for agent loops without enrichment or ranking.",
            discover_candidates_schema(),
            false,
        ),
        tool_spec(
            "inspect_candidate",
            "Fetch one GitHub issue's core details before deciding whether to assess it.",
            inspect_candidate_schema(),
            false,
        ),
        tool_spec(
            "inspect_discussion",
            "Fetch one candidate issue's comments, participant, and competition signals as a bounded read-only observation.",
            inspect_discussion_schema(),
            false,
        ),
        tool_spec(
            "inspect_repo_health",
            "Inspect repository activity and lightweight issue supply signals before committing to a candidate.",
            inspect_repo_health_schema(),
            false,
        ),
        tool_spec(
            "rank_shortlist",
            "Score and compare an explicit shortlist of GitHub issues; still call assess on the selected best candidate before a final recommendation.",
            rank_shortlist_schema(),
            false,
        ),
        tool_spec(
            "assess",
            "Assess one GitHub issue without preparing workspace or handoff state.",
            assess_schema(),
            false,
        ),
        tool_spec(
            "prepare",
            "Prepare a workspace and handoff for one issue after the prepare gate passes.",
            prepare_schema(),
            false,
        ),
        tool_spec(
            "read_context",
            "Read one fixed section from a prepared Issue Finder handoff context pack.",
            read_context_schema(),
            true,
        ),
        tool_spec(
            "memory_status",
            "Report local contribution memory status without exposing raw payloads.",
            memory_status_schema(),
            false,
        ),
        tool_spec(
            "memory_recall",
            "Recall memory for an issue and return compact evidence references.",
            memory_recall_schema(),
            false,
        ),
        tool_spec(
            "memory_dreams_list",
            "List reviewable memory dreams.",
            empty_schema(),
            false,
        ),
        tool_spec(
            "memory_dream_show",
            "Show one memory dream and its candidate hints.",
            memory_dream_show_schema(),
            false,
        ),
        tool_spec(
            "memory_hints_list",
            "List memory hints and decision eligibility metadata.",
            empty_schema(),
            false,
        ),
        tool_spec(
            "memory_hint_update",
            "Review or control one memory hint using a structured state transition.",
            memory_hint_update_schema(),
            false,
        ),
        tool_spec(
            "memory_tombstone",
            "Tombstone a memory raw event, node, or hint id.",
            memory_tombstone_schema(),
            false,
        ),
    ];
    tools.extend(crate::dispatch::tool_specs::dispatch_tool_specs());

    IssueFinderToolSpecsEnvelope {
        kind: "issue_finder_tool_specs".to_string(),
        version: 1,
        quick_start: quick_start(),
        recommended_workflow: recommended_workflow(),
        tools,
    }
}

fn quick_start() -> ToolQuickStart {
    ToolQuickStart {
        summary: "For agent loops, use discover_candidates to recall a small shortlist, inspect one candidate, assess it, then prepare only if the gate allows. Use scout for batch CLI-style discovery.".to_string(),
        first_call: ToolFirstCall {
            default_tool: TOOL_DISCOVER_CANDIDATES.to_string(),
            default_arguments: json!({
                "repo": null,
                "limit": 5,
                "laneLimit": 4
            }),
            when_ready_unknown: TOOL_STATUS.to_string(),
            fallback_after_setup_failure: TOOL_STATUS.to_string(),
        },
    }
}

fn recommended_workflow() -> Vec<ToolWorkflowStep> {
    vec![
        workflow_step(
            "discover",
            TOOL_DISCOVER_CANDIDATES,
            "Recall a compact candidate shortlist. Use repo when the user named a repository.",
        ),
        workflow_step(
            "inspect",
            TOOL_INSPECT_CANDIDATE,
            "Inspect one candidate issue before spending a heavier assessment call.",
        ),
        workflow_step(
            "inspect_discussion",
            TOOL_INSPECT_DISCUSSION,
            "Pull comments and competition signals only when inspection leaves uncertainty.",
        ),
        workflow_step(
            "inspect_repo_health",
            TOOL_INSPECT_REPO_HEALTH,
            "Check repository activity or candidate supply when repo health matters to the decision.",
        ),
        workflow_step(
            "rank_shortlist",
            TOOL_RANK_SHORTLIST,
            "Compare 2-5 explicit candidates before final recommendation.",
        ),
        workflow_step(
            "assess",
            TOOL_ASSESS,
            "Assess the best candidate before preparing workspace state.",
        ),
        workflow_step(
            "prepare",
            TOOL_PREPARE,
            "Prepare workspace and handoff only when the prepare gate allows.",
        ),
        ToolWorkflowStep {
            step: "read_context".to_string(),
            tool: TOOL_READ_CONTEXT.to_string(),
            purpose: "After prepare, read entry first, then safety and probe; read larger sections only when needed.".to_string(),
            deferred: Some(true),
            first_sections: vec![
                "entry".to_string(),
                "safety".to_string(),
                "probe".to_string(),
            ],
        },
    ]
}

fn workflow_step(step: &str, tool: &str, purpose: &str) -> ToolWorkflowStep {
    ToolWorkflowStep {
        step: step.to_string(),
        tool: tool.to_string(),
        purpose: purpose.to_string(),
        deferred: None,
        first_sections: Vec::new(),
    }
}

fn tool_spec(
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
        agent: agent_metadata_for_tool(&format!("issue-finder.{name}")),
    }
}

pub fn agent_metadata_for_tool(canonical_name: &str) -> ToolAgentMetadata {
    let exposure = agent_exposure_for_tool(canonical_name);
    ToolAgentMetadata {
        exposure: exposure.to_string(),
        visible_by_default: exposure == "direct",
        supports_parallel: supports_parallel_for_tool(canonical_name),
        side_effects: side_effects_for_tool(canonical_name),
    }
}

pub fn agent_exposure_for_tool(canonical_name: &str) -> &'static str {
    match canonical_name {
        TOOL_STATUS
        | TOOL_DISCOVER_CANDIDATES
        | TOOL_INSPECT_CANDIDATE
        | TOOL_INSPECT_DISCUSSION
        | TOOL_INSPECT_REPO_HEALTH
        | TOOL_RANK_SHORTLIST
        | TOOL_ASSESS => "direct",
        TOOL_PREPARE => "approval_required",
        TOOL_SCOUT | TOOL_READ_CONTEXT => "deferred",
        name if name.contains("approve")
            || name.contains("reject")
            || name.contains("execute")
            || name.contains("post_comment")
            || name.contains("retry_comment")
            || name.contains("hint_update")
            || name.contains("tombstone")
            || name.contains("archive")
            || name.contains("rename")
            || name.contains("fork") =>
        {
            "approval_required"
        }
        _ => "deferred",
    }
}

fn side_effects_for_tool(canonical_name: &str) -> Vec<String> {
    match canonical_name {
        TOOL_STATUS
        | TOOL_INSPECT_CANDIDATE
        | TOOL_INSPECT_DISCUSSION
        | TOOL_INSPECT_REPO_HEALTH
        | TOOL_READ_CONTEXT => Vec::new(),
        TOOL_DISCOVER_CANDIDATES => vec!["may_refresh_cache".to_string()],
        TOOL_SCOUT => vec![
            "may_refresh_cache".to_string(),
            "records_local_exposure_by_default".to_string(),
            "large_observation".to_string(),
        ],
        TOOL_RANK_SHORTLIST | TOOL_ASSESS => vec![
            "may_refresh_cache".to_string(),
            "records_local_read_by_default".to_string(),
        ],
        TOOL_PREPARE => vec![
            "approval_required".to_string(),
            "writes_local_workspace".to_string(),
            "writes_handoff".to_string(),
        ],
        name if name.contains("github_post_comment") || name.contains("github_retry_comment") => {
            vec![
                "approval_required".to_string(),
                "posts_to_github".to_string(),
            ]
        }
        name if agent_exposure_for_tool(name) == "approval_required" => {
            vec![
                "approval_required".to_string(),
                "mutates_local_state".to_string(),
            ]
        }
        name if name.contains("memory_status")
            || name.contains("memory_recall")
            || name.contains("dispatch_status")
            || name.contains("dispatch_events")
            || name.contains("dispatch_timeline")
            || name.contains("dispatch_trace")
            || name.contains("dispatch_artifacts")
            || name.contains("dispatch_review_list")
            || name.contains("dispatch_review_show")
            || name.contains("github_interactions")
            || name.contains("agents_list")
            || name.contains("agent_capabilities")
            || name.contains("sessions_list")
            || name.contains("sessions_search")
            || name.contains("sessions_read")
            || name.contains("sessions_replay") =>
        {
            Vec::new()
        }
        _ => vec!["mutates_local_state_or_large_context".to_string()],
    }
}

fn supports_parallel_for_tool(canonical_name: &str) -> bool {
    matches!(canonical_name, TOOL_STATUS | TOOL_INSPECT_CANDIDATE)
}

fn status_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "checkAuth": { "type": "boolean", "default": true }
        },
        "additionalProperties": false
    })
}

fn scout_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "limit": { "type": "integer", "minimum": 1, "default": 10 },
            "repo": { "type": ["string", "null"], "default": null },
            "refresh": { "type": "boolean", "default": false },
            "includeFiltered": { "type": "boolean", "default": false },
            "recordExposure": { "type": "boolean", "default": true }
        },
        "additionalProperties": false
    })
}

fn discover_candidates_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "limit": { "type": "integer", "minimum": 1, "maximum": 20, "default": 5 },
            "repo": { "type": ["string", "null"], "default": null },
            "refresh": { "type": "boolean", "default": false },
            "laneLimit": { "type": "integer", "minimum": 1, "maximum": 10, "default": 4 }
        },
        "additionalProperties": false
    })
}

fn inspect_candidate_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "issue": { "type": "string" },
            "url": { "type": "string" },
            "maxBodyChars": { "type": "integer", "minimum": 0, "maximum": 12000, "default": 4000 }
        },
        "oneOf": [
            { "required": ["issue"] },
            { "required": ["url"] }
        ],
        "additionalProperties": false
    })
}

fn inspect_discussion_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "issue": { "type": "string" },
            "url": { "type": "string" },
            "refresh": { "type": "boolean", "default": false },
            "maxComments": { "type": "integer", "minimum": 0, "maximum": 30, "default": 12 }
        },
        "oneOf": [
            { "required": ["issue"] },
            { "required": ["url"] }
        ],
        "additionalProperties": false
    })
}

fn inspect_repo_health_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "repo": { "type": ["string", "null"] },
            "issue": { "type": "string" },
            "url": { "type": "string" },
            "refresh": { "type": "boolean", "default": false },
            "recentWindow": { "type": "integer", "minimum": 1, "maximum": 100, "default": 30 }
        },
        "oneOf": [
            { "required": ["repo"] },
            { "required": ["issue"] },
            { "required": ["url"] }
        ],
        "additionalProperties": false
    })
}

fn rank_shortlist_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "issues": {
                "type": "array",
                "items": { "type": "string" },
                "minItems": 1,
                "maxItems": 5
            },
            "refresh": { "type": "boolean", "default": false },
            "recordRead": { "type": "boolean", "default": true }
        },
        "required": ["issues"],
        "additionalProperties": false
    })
}

fn assess_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "issue": { "type": "string" },
            "url": { "type": "string" },
            "refresh": { "type": "boolean", "default": false },
            "recordRead": { "type": "boolean", "default": true }
        },
        "oneOf": [
            { "required": ["issue"] },
            { "required": ["url"] }
        ],
        "additionalProperties": false
    })
}

fn prepare_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "issue": { "type": ["string", "null"] },
            "url": { "type": ["string", "null"] },
            "refresh": { "type": "boolean", "default": false },
            "allowGateBypass": { "type": "boolean", "default": false },
            "bypassReason": { "type": ["string", "null"], "default": null }
        },
        "additionalProperties": false
    })
}

fn read_context_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "handoffId": { "type": "string" },
            "section": {
                "type": "string",
                "enum": [
                    "entry",
                    "safety",
                    "probe",
                    "value",
                    "issue",
                    "repo",
                    "validation",
                    "handoff_json",
                    "agent_policy",
                    "probe_json"
                ]
            },
            "maxBytes": {
                "type": "integer",
                "minimum": 0,
                "maximum": 50000,
                "default": 12000
            }
        },
        "required": ["handoffId", "section"],
        "additionalProperties": false
    })
}

fn empty_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

fn memory_status_schema() -> Value {
    empty_schema()
}

fn memory_recall_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "issue": { "type": "string" },
            "kind": {
                "type": "string",
                "enum": ["scout-ranking", "dispatch-planning", "github-draft", "profile-review"],
                "default": "scout-ranking"
            },
            "limit": { "type": "integer", "minimum": 1, "default": 10 }
        },
        "required": ["issue"],
        "additionalProperties": false
    })
}

fn memory_dream_show_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "dreamId": { "type": "string" }
        },
        "required": ["dreamId"],
        "additionalProperties": false
    })
}

fn memory_hint_update_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "hintId": { "type": "string" },
            "action": {
                "type": "string",
                "enum": ["approve", "reject", "pin", "deprioritize", "suppress", "stale", "tombstone"]
            },
            "reason": { "type": ["string", "null"], "default": null }
        },
        "required": ["hintId", "action"],
        "additionalProperties": false
    })
}

fn memory_tombstone_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string" }
        },
        "required": ["id"],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::{
        list_tool_specs, TOOL_A2A_APPROVE_SEND, TOOL_A2A_EXPORT_TASK, TOOL_A2A_IMPORT_RESULT,
        TOOL_A2A_REJECT_SEND, TOOL_AGENTS_LIST, TOOL_AGENT_CAPABILITIES, TOOL_AGENT_PROBE,
        TOOL_ASSESS, TOOL_DISCOVER_CANDIDATES, TOOL_DISPATCH, TOOL_DISPATCH_APPROVE,
        TOOL_DISPATCH_ARTIFACTS, TOOL_DISPATCH_EVENTS, TOOL_DISPATCH_EXECUTE,
        TOOL_DISPATCH_IMPORT_HANDOFF, TOOL_DISPATCH_RECORD_OUTCOME, TOOL_DISPATCH_REJECT,
        TOOL_DISPATCH_REVIEW_APPROVE, TOOL_DISPATCH_REVIEW_LIST, TOOL_DISPATCH_REVIEW_REJECT,
        TOOL_DISPATCH_REVIEW_SHOW, TOOL_DISPATCH_STATUS, TOOL_DISPATCH_TIMELINE,
        TOOL_DISPATCH_TRACE, TOOL_GITHUB_APPROVE_COMMENT, TOOL_GITHUB_DRAFT_FINAL_COMMENT,
        TOOL_GITHUB_DRAFT_TRACKING_COMMENT, TOOL_GITHUB_INTERACTIONS, TOOL_GITHUB_POST_COMMENT,
        TOOL_GITHUB_REJECT_COMMENT, TOOL_GITHUB_RETRY_COMMENT, TOOL_INSPECT_CANDIDATE,
        TOOL_INSPECT_DISCUSSION, TOOL_INSPECT_REPO_HEALTH, TOOL_MEMORY_DREAMS_LIST,
        TOOL_MEMORY_DREAM_SHOW, TOOL_MEMORY_HINTS_LIST, TOOL_MEMORY_HINT_UPDATE,
        TOOL_MEMORY_RECALL, TOOL_MEMORY_STATUS, TOOL_MEMORY_TOMBSTONE, TOOL_PREPARE,
        TOOL_RANK_SHORTLIST, TOOL_READ_CONTEXT, TOOL_SCOUT, TOOL_SESSIONS_APPROVE_MUTATION,
        TOOL_SESSIONS_ARCHIVE, TOOL_SESSIONS_FORK, TOOL_SESSIONS_LIST, TOOL_SESSIONS_READ,
        TOOL_SESSIONS_REJECT_MUTATION, TOOL_SESSIONS_RENAME, TOOL_SESSIONS_REPLAY,
        TOOL_SESSIONS_SEARCH, TOOL_SESSIONS_SYNC, TOOL_STATUS,
    };

    #[test]
    fn lists_issue_finder_tool_specs_with_workflow_metadata() {
        let specs = list_tool_specs();
        let names = specs
            .tools
            .iter()
            .map(|tool| {
                format!(
                    "{}.{}",
                    tool.namespace.as_deref().unwrap_or_default(),
                    tool.name
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                TOOL_STATUS,
                TOOL_SCOUT,
                TOOL_DISCOVER_CANDIDATES,
                TOOL_INSPECT_CANDIDATE,
                TOOL_INSPECT_DISCUSSION,
                TOOL_INSPECT_REPO_HEALTH,
                TOOL_RANK_SHORTLIST,
                TOOL_ASSESS,
                TOOL_PREPARE,
                TOOL_READ_CONTEXT,
                TOOL_MEMORY_STATUS,
                TOOL_MEMORY_RECALL,
                TOOL_MEMORY_DREAMS_LIST,
                TOOL_MEMORY_DREAM_SHOW,
                TOOL_MEMORY_HINTS_LIST,
                TOOL_MEMORY_HINT_UPDATE,
                TOOL_MEMORY_TOMBSTONE,
                TOOL_AGENTS_LIST,
                TOOL_AGENT_CAPABILITIES,
                TOOL_AGENT_PROBE,
                TOOL_SESSIONS_LIST,
                TOOL_SESSIONS_SYNC,
                TOOL_SESSIONS_SEARCH,
                TOOL_SESSIONS_READ,
                TOOL_SESSIONS_REPLAY,
                TOOL_SESSIONS_RENAME,
                TOOL_SESSIONS_FORK,
                TOOL_SESSIONS_ARCHIVE,
                TOOL_SESSIONS_APPROVE_MUTATION,
                TOOL_SESSIONS_REJECT_MUTATION,
                TOOL_DISPATCH_STATUS,
                TOOL_DISPATCH_EVENTS,
                TOOL_DISPATCH_TIMELINE,
                TOOL_DISPATCH_TRACE,
                TOOL_DISPATCH_ARTIFACTS,
                TOOL_DISPATCH_IMPORT_HANDOFF,
                TOOL_DISPATCH_REVIEW_LIST,
                TOOL_DISPATCH_REVIEW_SHOW,
                TOOL_DISPATCH_REVIEW_APPROVE,
                TOOL_DISPATCH_REVIEW_REJECT,
                TOOL_DISPATCH,
                TOOL_DISPATCH_APPROVE,
                TOOL_DISPATCH_REJECT,
                TOOL_DISPATCH_EXECUTE,
                TOOL_DISPATCH_RECORD_OUTCOME,
                TOOL_A2A_EXPORT_TASK,
                TOOL_A2A_APPROVE_SEND,
                TOOL_A2A_REJECT_SEND,
                TOOL_A2A_IMPORT_RESULT,
                TOOL_GITHUB_DRAFT_TRACKING_COMMENT,
                TOOL_GITHUB_DRAFT_FINAL_COMMENT,
                TOOL_GITHUB_APPROVE_COMMENT,
                TOOL_GITHUB_REJECT_COMMENT,
                TOOL_GITHUB_POST_COMMENT,
                TOOL_GITHUB_RETRY_COMMENT,
                TOOL_GITHUB_INTERACTIONS
            ]
        );

        assert_eq!(
            specs.quick_start.first_call.default_tool,
            TOOL_DISCOVER_CANDIDATES
        );
        assert_eq!(specs.quick_start.first_call.when_ready_unknown, TOOL_STATUS);

        let workflow_tools = specs
            .recommended_workflow
            .iter()
            .map(|step| step.tool.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            workflow_tools,
            vec![
                TOOL_DISCOVER_CANDIDATES,
                TOOL_INSPECT_CANDIDATE,
                TOOL_INSPECT_DISCUSSION,
                TOOL_INSPECT_REPO_HEALTH,
                TOOL_RANK_SHORTLIST,
                TOOL_ASSESS,
                TOOL_PREPARE,
                TOOL_READ_CONTEXT
            ]
        );
    }
}
