use serde::Serialize;
use serde_json::{json, Value};

pub const TOOL_SCOUT: &str = "issue-finder.scout";
pub const TOOL_ASSESS: &str = "issue-finder.assess";
pub const TOOL_PREPARE: &str = "issue-finder.prepare";
pub const TOOL_READ_CONTEXT: &str = "issue-finder.read_context";
pub const TOOL_STATUS: &str = "issue-finder.status";

pub use crate::dispatch::tool_specs::{
    TOOL_A2A_APPROVE_SEND, TOOL_A2A_EXPORT_TASK, TOOL_A2A_IMPORT_RESULT, TOOL_A2A_REJECT_SEND,
    TOOL_AGENTS_LIST, TOOL_AGENT_CAPABILITIES, TOOL_DISPATCH, TOOL_DISPATCH_APPROVE,
    TOOL_DISPATCH_ARTIFACTS, TOOL_DISPATCH_EVENTS, TOOL_DISPATCH_EXECUTE,
    TOOL_DISPATCH_IMPORT_HANDOFF, TOOL_DISPATCH_PROPOSE, TOOL_DISPATCH_REJECT,
    TOOL_DISPATCH_STATUS, TOOL_GITHUB_APPROVE_COMMENT, TOOL_GITHUB_DRAFT_FINAL_COMMENT,
    TOOL_GITHUB_DRAFT_TRACKING_COMMENT, TOOL_GITHUB_INTERACTIONS, TOOL_GITHUB_POST_COMMENT,
    TOOL_GITHUB_REJECT_COMMENT, TOOL_GITHUB_RETRY_COMMENT, TOOL_SESSIONS_APPROVE_MUTATION,
    TOOL_SESSIONS_ARCHIVE, TOOL_SESSIONS_FORK, TOOL_SESSIONS_LIST, TOOL_SESSIONS_READ,
    TOOL_SESSIONS_REJECT_MUTATION, TOOL_SESSIONS_RENAME, TOOL_SESSIONS_SEARCH, TOOL_SESSIONS_SYNC,
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
            "Discover and rank candidate GitHub issues with gate-aware summaries.",
            scout_schema(),
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
        summary: "Use scout to find candidates, assess the top issue, prepare it if the gate allows, then read deferred context sections as needed.".to_string(),
        first_call: ToolFirstCall {
            default_tool: TOOL_SCOUT.to_string(),
            default_arguments: json!({
                "repo": "owner/repo",
                "limit": 10
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
            TOOL_SCOUT,
            "Find and rank candidates. Use repo when the user named a repository.",
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
    }
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

fn assess_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "issue": { "type": ["string", "null"] },
            "url": { "type": ["string", "null"] },
            "refresh": { "type": "boolean", "default": false },
            "recordRead": { "type": "boolean", "default": true }
        },
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

#[cfg(test)]
mod tests {
    use super::{
        list_tool_specs, TOOL_A2A_APPROVE_SEND, TOOL_A2A_EXPORT_TASK, TOOL_A2A_IMPORT_RESULT,
        TOOL_A2A_REJECT_SEND, TOOL_AGENTS_LIST, TOOL_AGENT_CAPABILITIES, TOOL_ASSESS,
        TOOL_DISPATCH, TOOL_DISPATCH_APPROVE, TOOL_DISPATCH_ARTIFACTS, TOOL_DISPATCH_EVENTS,
        TOOL_DISPATCH_EXECUTE, TOOL_DISPATCH_IMPORT_HANDOFF, TOOL_DISPATCH_REJECT,
        TOOL_DISPATCH_STATUS, TOOL_GITHUB_APPROVE_COMMENT, TOOL_GITHUB_DRAFT_FINAL_COMMENT,
        TOOL_GITHUB_DRAFT_TRACKING_COMMENT, TOOL_GITHUB_INTERACTIONS, TOOL_GITHUB_POST_COMMENT,
        TOOL_GITHUB_REJECT_COMMENT, TOOL_GITHUB_RETRY_COMMENT, TOOL_PREPARE, TOOL_READ_CONTEXT,
        TOOL_SCOUT, TOOL_SESSIONS_APPROVE_MUTATION, TOOL_SESSIONS_ARCHIVE, TOOL_SESSIONS_FORK,
        TOOL_SESSIONS_LIST, TOOL_SESSIONS_READ, TOOL_SESSIONS_REJECT_MUTATION,
        TOOL_SESSIONS_RENAME, TOOL_SESSIONS_SEARCH, TOOL_SESSIONS_SYNC, TOOL_STATUS,
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
                TOOL_ASSESS,
                TOOL_PREPARE,
                TOOL_READ_CONTEXT,
                TOOL_AGENTS_LIST,
                TOOL_AGENT_CAPABILITIES,
                TOOL_SESSIONS_LIST,
                TOOL_SESSIONS_SYNC,
                TOOL_SESSIONS_SEARCH,
                TOOL_SESSIONS_READ,
                TOOL_SESSIONS_RENAME,
                TOOL_SESSIONS_FORK,
                TOOL_SESSIONS_ARCHIVE,
                TOOL_SESSIONS_APPROVE_MUTATION,
                TOOL_SESSIONS_REJECT_MUTATION,
                TOOL_DISPATCH_STATUS,
                TOOL_DISPATCH_EVENTS,
                TOOL_DISPATCH_ARTIFACTS,
                TOOL_DISPATCH_IMPORT_HANDOFF,
                TOOL_DISPATCH,
                TOOL_DISPATCH_APPROVE,
                TOOL_DISPATCH_REJECT,
                TOOL_DISPATCH_EXECUTE,
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

        assert_eq!(specs.quick_start.first_call.default_tool, TOOL_SCOUT);
        assert_eq!(specs.quick_start.first_call.when_ready_unknown, TOOL_STATUS);

        let workflow_tools = specs
            .recommended_workflow
            .iter()
            .map(|step| step.tool.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            workflow_tools,
            vec![TOOL_SCOUT, TOOL_ASSESS, TOOL_PREPARE, TOOL_READ_CONTEXT]
        );
    }
}
