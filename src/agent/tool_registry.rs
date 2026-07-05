use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::paths::IssueFinderPaths;
use crate::tool_runtime::{
    IssueFinderContentItem, IssueFinderToolInvocation, IssueFinderToolOutput,
    IssueFinderToolRuntime,
};
use crate::tool_specs::{
    list_tool_specs, IssueFinderToolSpec, TOOL_ASSESS, TOOL_DISCOVER_CANDIDATES,
    TOOL_INSPECT_CANDIDATE, TOOL_INSPECT_DISCUSSION, TOOL_INSPECT_REPO_HEALTH, TOOL_RANK_SHORTLIST,
    TOOL_STATUS,
};

use super::llm_client::AgentToolCall;

const ISSUE_FINDER_NAMESPACE: &str = "issue-finder";
const DIRECT_AGENT_TOOLS: &[&str] = &[
    TOOL_STATUS,
    TOOL_DISCOVER_CANDIDATES,
    TOOL_INSPECT_CANDIDATE,
    TOOL_INSPECT_DISCUSSION,
    TOOL_INSPECT_REPO_HEALTH,
    TOOL_RANK_SHORTLIST,
    TOOL_ASSESS,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolExposure {
    Direct,
    Deferred,
    Hidden,
    ApprovalRequired,
}

impl AgentToolExposure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Deferred => "deferred",
            Self::Hidden => "hidden",
            Self::ApprovalRequired => "approval_required",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "direct" => Self::Direct,
            "approval_required" => Self::ApprovalRequired,
            "hidden" => Self::Hidden,
            _ => Self::Deferred,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolDefinition {
    pub canonical_name: String,
    pub namespace: String,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub exposure: AgentToolExposure,
    pub supports_parallel: bool,
}

impl AgentToolDefinition {
    pub fn responses_function_json(&self) -> Value {
        json!({
            "type": "function",
            "name": self.name,
            "description": self.description,
            "strict": false,
            "parameters": self.input_schema,
        })
    }
}

#[derive(Debug, Clone)]
pub struct AgentToolRegistry {
    tools: BTreeMap<String, AgentToolDefinition>,
}

impl AgentToolRegistry {
    pub fn issue_finder_default() -> Self {
        let mut tools = BTreeMap::new();
        for spec in list_tool_specs().tools {
            let Some(definition) = definition_from_spec(spec) else {
                continue;
            };
            tools.insert(definition.canonical_name.clone(), definition);
        }
        Self { tools }
    }

    pub fn definition(&self, canonical_name: &str) -> Option<&AgentToolDefinition> {
        self.tools.get(canonical_name)
    }

    pub fn direct_tool_names(&self) -> Vec<String> {
        DIRECT_AGENT_TOOLS
            .iter()
            .filter(|tool| self.tools.contains_key(**tool))
            .map(|tool| (*tool).to_string())
            .collect()
    }

    pub fn direct_tool_definitions(&self) -> Vec<AgentToolDefinition> {
        DIRECT_AGENT_TOOLS
            .iter()
            .filter_map(|tool| self.tools.get(*tool).cloned())
            .collect()
    }

    pub fn model_visible_responses_tools(&self) -> Vec<Value> {
        self.responses_tools_for_exposures(&[AgentToolExposure::Direct])
    }

    pub fn has_parallel_direct_tools(&self) -> bool {
        self.tools
            .values()
            .any(|tool| tool.exposure == AgentToolExposure::Direct && tool.supports_parallel)
    }

    pub fn calls_support_parallel(&self, calls: &[AgentToolCall]) -> bool {
        calls.iter().all(|call| {
            self.definition(&call.canonical_name()).is_some_and(|tool| {
                tool.exposure == AgentToolExposure::Direct && tool.supports_parallel
            })
        })
    }

    pub fn responses_tools_for_exposures(&self, exposures: &[AgentToolExposure]) -> Vec<Value> {
        let child_tools = self
            .tools
            .values()
            .filter(|tool| exposures.contains(&tool.exposure))
            .filter(|tool| tool.exposure != AgentToolExposure::Hidden)
            .map(AgentToolDefinition::responses_function_json)
            .collect::<Vec<_>>();

        if child_tools.is_empty() {
            return Vec::new();
        }

        vec![json!({
            "type": "namespace",
            "name": ISSUE_FINDER_NAMESPACE,
            "description": "Issue Finder local read-only and approval-gated tools.",
            "tools": child_tools,
        })]
    }

    pub fn tool_summaries_for_exposures(
        &self,
        exposures: &[AgentToolExposure],
        limit: usize,
    ) -> Vec<String> {
        self.tools
            .values()
            .filter(|tool| exposures.contains(&tool.exposure))
            .filter(|tool| tool.exposure != AgentToolExposure::Hidden)
            .take(limit)
            .map(|tool| {
                format!(
                    "{} ({}) - {}",
                    tool.canonical_name,
                    tool.exposure.as_str(),
                    truncate_words(&tool.description, 18)
                )
            })
            .collect()
    }

    pub fn tool_summary(&self, canonical_name: &str) -> Option<String> {
        self.tools.get(canonical_name).map(|tool| {
            format!(
                "{} ({}) - {}",
                tool.canonical_name,
                tool.exposure.as_str(),
                truncate_words(&tool.description, 18)
            )
        })
    }

    pub async fn execute_call(
        &self,
        runtime: &IssueFinderToolRuntime,
        paths: &IssueFinderPaths,
        turn_id: Option<String>,
        call: &AgentToolCall,
    ) -> IssueFinderToolOutput {
        let canonical_name = call.canonical_name();
        let Some(definition) = self.definition(&canonical_name) else {
            return IssueFinderToolOutput::failure(
                call.call_id.clone(),
                turn_id,
                canonical_name,
                "unknown_tool",
                "tool is not registered in the Issue Finder agent registry",
            );
        };

        if definition.exposure == AgentToolExposure::ApprovalRequired {
            return approval_required_output(definition, turn_id, call);
        }

        if definition.exposure != AgentToolExposure::Direct {
            return IssueFinderToolOutput::failure(
                call.call_id.clone(),
                turn_id,
                canonical_name,
                "tool_not_direct",
                format!(
                    "tool exposure is {}; this agent turn may call only direct tools",
                    definition.exposure.as_str()
                ),
            );
        }

        let arguments =
            normalize_arguments(&definition.canonical_name, call.arguments.clone(), paths);
        runtime
            .execute(IssueFinderToolInvocation {
                call_id: call.call_id.clone(),
                turn_id,
                tool_name: definition.canonical_name.clone(),
                arguments,
            })
            .await
    }
}

fn approval_required_output(
    definition: &AgentToolDefinition,
    turn_id: Option<String>,
    call: &AgentToolCall,
) -> IssueFinderToolOutput {
    let approval_request_id = match &turn_id {
        Some(turn_id) => format!("agent-approval-{turn_id}-{}", call.call_id),
        None => format!("agent-approval-{}", call.call_id),
    };
    IssueFinderToolOutput {
        call_id: call.call_id.clone(),
        turn_id,
        tool_name: definition.canonical_name.clone(),
        success: true,
        status: "pending_approval".to_string(),
        content_items: vec![IssueFinderContentItem::InputText {
            text: format!(
                "{} requires explicit user approval before Issue Finder can execute it.",
                definition.canonical_name
            ),
        }],
        structured_content: json!({
            "kind": "issue_finder_tool_output",
            "tool": definition.canonical_name,
            "status": "pending_approval",
            "success": true,
            "approvalRequired": {
                "approvalRequestId": approval_request_id,
                "toolName": definition.canonical_name,
                "exposure": definition.exposure.as_str(),
                "arguments": call.arguments,
                "message": "This tool is approval-gated and was not executed."
            }
        }),
    }
}

fn definition_from_spec(spec: IssueFinderToolSpec) -> Option<AgentToolDefinition> {
    let namespace = spec.namespace?;
    let canonical_name = format!("{namespace}.{}", spec.name);
    let exposure = AgentToolExposure::parse(&spec.agent.exposure);

    Some(AgentToolDefinition {
        canonical_name,
        namespace,
        name: spec.name,
        description: spec.description,
        input_schema: spec.input_schema,
        exposure,
        supports_parallel: spec.agent.supports_parallel,
    })
}

fn normalize_arguments(tool_name: &str, mut arguments: Value, _paths: &IssueFinderPaths) -> Value {
    if !arguments.is_object() {
        return json!({});
    }

    if tool_name == TOOL_STATUS {
        arguments
            .as_object_mut()
            .expect("checked object")
            .entry("checkAuth".to_string())
            .or_insert_with(|| json!(true));
    }

    arguments
}

fn truncate_words(value: &str, limit: usize) -> String {
    let words = value.split_whitespace().collect::<Vec<_>>();
    if words.len() <= limit {
        return value.to_string();
    }
    format!("{}...", words[..limit].join(" "))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;
    use tempfile::tempdir;

    use super::{AgentToolExposure, AgentToolRegistry};
    use crate::agent::llm_client::AgentToolCall;
    use crate::config::Config;
    use crate::paths::IssueFinderPaths;
    use crate::tool_runtime::IssueFinderToolRuntime;
    use crate::tool_specs::{
        TOOL_ASSESS, TOOL_INSPECT_CANDIDATE, TOOL_PREPARE, TOOL_SCOUT, TOOL_STATUS,
    };

    #[test]
    fn registry_exposes_lightweight_tools_directly() {
        let registry = AgentToolRegistry::issue_finder_default();

        assert_eq!(
            registry.direct_tool_names(),
            vec![
                "issue-finder.status",
                "issue-finder.discover_candidates",
                "issue-finder.inspect_candidate",
                "issue-finder.inspect_discussion",
                "issue-finder.inspect_repo_health",
                "issue-finder.rank_shortlist",
                "issue-finder.assess"
            ]
        );
        assert_eq!(
            registry.definition(TOOL_SCOUT).unwrap().exposure,
            AgentToolExposure::Deferred
        );
        assert_eq!(
            registry.definition(TOOL_PREPARE).unwrap().exposure,
            AgentToolExposure::ApprovalRequired
        );
        assert!(registry.definition(TOOL_STATUS).unwrap().supports_parallel);
        assert!(
            registry
                .definition(TOOL_INSPECT_CANDIDATE)
                .unwrap()
                .supports_parallel
        );
        assert!(!registry.definition(TOOL_ASSESS).unwrap().supports_parallel);
    }

    #[test]
    fn responses_tools_use_namespace_shape_and_hide_deferred_from_direct_view() {
        let registry = AgentToolRegistry::issue_finder_default();

        let direct = registry.model_visible_responses_tools();
        assert_eq!(direct[0]["type"], "namespace");
        assert_eq!(direct[0]["name"], "issue-finder");
        let direct_names = direct[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(direct_names.contains(&"discover_candidates"));
        assert!(direct_names.contains(&"inspect_discussion"));
        assert!(direct_names.contains(&"inspect_repo_health"));
        assert!(direct_names.contains(&"rank_shortlist"));
        assert!(!direct_names.contains(&"scout"));

        let deferred = registry.responses_tools_for_exposures(&[AgentToolExposure::Deferred]);
        assert_eq!(
            deferred[0]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == "scout")
                .unwrap()["parameters"]["type"],
            json!("object")
        );
    }

    #[tokio::test]
    async fn unknown_and_deferred_tool_calls_return_model_facing_errors() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let runtime = IssueFinderToolRuntime::new(paths.clone(), Config::default());
        let registry = AgentToolRegistry::issue_finder_default();

        let unknown = registry
            .execute_call(
                &runtime,
                &paths,
                Some("turn-1".to_string()),
                &AgentToolCall {
                    call_id: "call_unknown".to_string(),
                    namespace: Some("issue-finder".to_string()),
                    name: "not_registered".to_string(),
                    arguments: json!({}),
                    provider_item_id: Some("fc_unknown".to_string()),
                    provider_call_id_source: Some("provider_responses".to_string()),
                },
            )
            .await;
        assert_eq!(unknown.call_id, "call_unknown");
        assert_eq!(unknown.status, "unknown_tool");
        assert!(!unknown.success);

        let deferred = registry
            .execute_call(
                &runtime,
                &paths,
                Some("turn-1".to_string()),
                &AgentToolCall {
                    call_id: "call_scout".to_string(),
                    namespace: Some("issue-finder".to_string()),
                    name: "scout".to_string(),
                    arguments: json!({"limit": 1}),
                    provider_item_id: Some("fc_scout".to_string()),
                    provider_call_id_source: Some("provider_responses".to_string()),
                },
            )
            .await;
        assert_eq!(deferred.call_id, "call_scout");
        assert_eq!(deferred.status, "tool_not_direct");
        assert!(!deferred.success);
    }

    #[tokio::test]
    async fn approval_required_tool_calls_return_pending_approval_without_execution() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let runtime = IssueFinderToolRuntime::new(paths.clone(), Config::default());
        let registry = AgentToolRegistry::issue_finder_default();

        let output = registry
            .execute_call(
                &runtime,
                &paths,
                Some("turn-1".to_string()),
                &AgentToolCall {
                    call_id: "call_prepare".to_string(),
                    namespace: Some("issue-finder".to_string()),
                    name: "prepare".to_string(),
                    arguments: json!({"issue": "owner/repo#1"}),
                    provider_item_id: Some("fc_prepare".to_string()),
                    provider_call_id_source: Some("provider_responses".to_string()),
                },
            )
            .await;

        assert!(output.success);
        assert_eq!(output.status, "pending_approval");
        assert_eq!(
            output.structured_content["approvalRequired"]["toolName"],
            "issue-finder.prepare"
        );
        assert_eq!(
            output.structured_content["approvalRequired"]["message"],
            "This tool is approval-gated and was not executed."
        );
    }

    fn test_paths(home: &Path) -> IssueFinderPaths {
        IssueFinderPaths {
            home: home.to_path_buf(),
            config: home.join("config.toml"),
            cache_dir: home.join("cache"),
            workspaces_dir: home.join("workspaces"),
            inbox_dir: home.join("inbox"),
            reports_dir: home.join("reports"),
        }
    }
}
