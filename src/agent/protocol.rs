use anyhow::Result;
use serde_json::{json, Value};

use crate::config::Config;
use crate::tool_specs::{TOOL_DISCOVER_CANDIDATES, TOOL_STATUS};

use super::llm_client::{AgentModelItem, AgentModelRequest, AgentModelResponse, AgentToolCall};
use super::model::{AgentTaskSendRequest, AgentThreadTurnRequest};
use super::tool_registry::AgentToolRegistry;

const MAX_OBSERVATION_CHARS: usize = 12_000;

pub(crate) enum TaskOrTurnInput<'a> {
    Task(&'a AgentTaskSendRequest),
    Turn(&'a AgentThreadTurnRequest),
}

pub(crate) fn model_request(
    config: &Config,
    registry: &AgentToolRegistry,
    input_items: Vec<AgentModelItem>,
) -> AgentModelRequest {
    AgentModelRequest {
        model: config.llm.model.clone(),
        instructions: system_instructions(registry),
        input_items,
        tools: registry.model_visible_responses_tools(),
        parallel_tool_calls: registry.has_parallel_direct_tools(),
        output_schema: None,
    }
}

pub(crate) fn final_model_request(
    config: &Config,
    registry: &AgentToolRegistry,
    input_items: Vec<AgentModelItem>,
) -> AgentModelRequest {
    let mut request = model_request(config, registry, input_items);
    request.tools = Vec::new();
    request.parallel_tool_calls = false;
    request.instructions.push_str(
        "\nThe tool budget for this turn is exhausted. Do not call tools. Produce a final answer grounded only in the observations already present.",
    );
    request
}

pub(crate) fn system_instructions(registry: &AgentToolRegistry) -> String {
    format!(
        "You are Issue Finder's local agent daemon.\n\
You receive high-level natural-language goals and choose from the native direct tools made visible for this turn.\n\
This agent is read-only. Do not prepare workspaces, post to GitHub, mutate memory, or dispatch work unless those tools are explicitly made visible in a future turn.\n\
Current direct tools: {}.\n\
Use lightweight discovery first, inspect individual candidates before deep assessment, and base final answers only on observed tool results.\n\
For recommendation goals, do not produce the final answer until you have called issue-finder.assess on the best observed candidate.",
        registry.direct_tool_names().join(", ")
    )
}

pub(crate) fn user_prompt(goal: &str, input: &AgentTaskSendRequest) -> String {
    let repo = input.repo.as_deref().unwrap_or("null");
    format!(
        "Goal: {goal}\nDefault discover_candidates arguments: limit={}, repo={}, refresh={}, laneLimit=4.\nChoose the next action.",
        input.normalized_limit(),
        repo,
        input.refresh
    )
}

pub(crate) fn requires_assessment_before_final(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    lower.contains("recommend")
        || lower.contains("issue")
        || input.contains("推荐")
        || input.contains("高价值")
        || input.contains("候选")
        || input.contains("评估")
}

pub(crate) fn assessment_required_message() -> String {
    "Before finalizing a recommendation, call issue-finder.assess on the best observed candidate. Final answers for recommendation goals must be grounded in assessment output, not inspection alone.".to_string()
}

pub(crate) fn finalization_prompt() -> String {
    "Tool budget exhausted. Do not request or call more tools. Return a final answer now, explicitly noting any remaining uncertainty or missing evidence.".to_string()
}

pub(crate) fn normalize_tool_call_arguments(
    call: &mut AgentToolCall,
    input: TaskOrTurnInput<'_>,
) -> Result<()> {
    if !call.arguments.is_object() {
        anyhow::bail!("tool arguments must be a JSON object");
    }

    let tool_name = call.canonical_name();
    if tool_name == TOOL_DISCOVER_CANDIDATES {
        let (limit, repo, refresh) = match input {
            TaskOrTurnInput::Task(input) => {
                (input.normalized_limit(), input.repo.as_ref(), input.refresh)
            }
            TaskOrTurnInput::Turn(input) => {
                (input.normalized_limit(), input.repo.as_ref(), input.refresh)
            }
        };
        let object = call.arguments.as_object_mut().expect("checked object");
        object
            .entry("limit".to_string())
            .or_insert_with(|| json!(limit));
        object
            .entry("repo".to_string())
            .or_insert_with(|| repo.map_or(Value::Null, |repo| json!(repo)));
        object
            .entry("refresh".to_string())
            .or_insert_with(|| json!(refresh));
        object
            .entry("laneLimit".to_string())
            .or_insert_with(|| json!(4));
    }

    if tool_name == TOOL_STATUS {
        call.arguments
            .as_object_mut()
            .expect("checked object")
            .entry("checkAuth".to_string())
            .or_insert_with(|| json!(true));
    }

    Ok(())
}

pub(crate) fn model_response_content(response: &AgentModelResponse) -> String {
    let parts = response
        .items
        .iter()
        .map(|item| match item {
            AgentModelItem::SystemMessage { content }
            | AgentModelItem::UserMessage { content }
            | AgentModelItem::AssistantMessage { content }
            | AgentModelItem::ReasoningSummary { content } => content.clone(),
            AgentModelItem::ToolCall(call) => format!("tool_call: {}", call.canonical_name()),
            AgentModelItem::FunctionCallOutput { call_id, .. } => {
                format!("function_call_output: {call_id}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    if parts.trim().is_empty() {
        serde_json::to_string(&response.raw_provider_response).unwrap_or_default()
    } else {
        parts
    }
}

pub(crate) fn model_visible_tool_output(output: &Value) -> Value {
    let raw = serde_json::to_string(output).unwrap_or_else(|_| "{}".to_string());
    Value::String(truncate_chars(&raw, MAX_OBSERVATION_CHARS))
}

pub(crate) fn tool_observation(tool_name: &str, output: &Value) -> String {
    let raw = serde_json::to_string(output).unwrap_or_else(|_| "{}".to_string());
    format!(
        "Observation from {tool_name}:\n{}\nReturn the next JSON decision. If the goal is answered, finalAnswer must summarize the recommended issues.",
        truncate_chars(&raw, MAX_OBSERVATION_CHARS)
    )
}

pub(crate) fn tool_output_error(output: &Value) -> String {
    output
        .pointer("/structuredContent/error/message")
        .and_then(Value::as_str)
        .or_else(|| output.pointer("/status").and_then(Value::as_str))
        .unwrap_or("tool call failed")
        .to_string()
}

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut truncated = value.chars().take(limit).collect::<String>();
    truncated.push_str("\n...[truncated]");
    truncated
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        assessment_required_message, final_model_request, model_request,
        normalize_tool_call_arguments, requires_assessment_before_final, system_instructions,
        TaskOrTurnInput,
    };
    use crate::agent::llm_client::{AgentModelItem, AgentToolCall};
    use crate::agent::model::AgentTaskSendRequest;
    use crate::agent::tool_registry::AgentToolRegistry;
    use crate::config::Config;

    #[test]
    fn discover_candidates_arguments_fill_goal_defaults() {
        let input = AgentTaskSendRequest {
            goal: "搜索全网仓库并推荐 issue".to_string(),
            repo: None,
            limit: Some(7),
            refresh: true,
            max_turns: Some(3),
            run_immediately: true,
        };
        let mut call = AgentToolCall {
            call_id: "call_123".to_string(),
            namespace: Some("issue-finder".to_string()),
            name: "discover_candidates".to_string(),
            arguments: json!({}),
            provider_item_id: Some("fc_123".to_string()),
            provider_call_id_source: Some("provider_responses".to_string()),
        };

        normalize_tool_call_arguments(&mut call, TaskOrTurnInput::Task(&input)).unwrap();

        assert_eq!(call.arguments["limit"], 7);
        assert!(call.arguments["repo"].is_null());
        assert_eq!(call.arguments["refresh"], true);
        assert_eq!(call.arguments["laneLimit"], 4);
    }

    #[test]
    fn system_instructions_are_not_the_tool_schema_transport() {
        let registry = AgentToolRegistry::issue_finder_default();
        let prompt = system_instructions(&registry);

        assert!(prompt.contains("issue-finder.discover_candidates"));
        assert!(prompt.contains("issue-finder.inspect_candidate"));
        assert!(prompt.contains("issue-finder.assess"));
        assert!(!prompt.contains(r#""tool":"issue-finder"#));
        assert!(prompt.contains("read-only"));
    }

    #[test]
    fn model_request_uses_registry_visible_tools() {
        let config = Config::default();
        let registry = AgentToolRegistry::issue_finder_default();
        let request = model_request(
            &config,
            &registry,
            vec![AgentModelItem::UserMessage {
                content: "find an issue".to_string(),
            }],
        );

        assert_eq!(request.tools[0]["type"], "namespace");
        let tool_names = request.tools[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(tool_names.contains(&"discover_candidates"));
        assert!(!tool_names.contains(&"scout"));
        assert!(request.parallel_tool_calls);
    }

    #[test]
    fn recommendation_goals_require_assess_before_final_answer() {
        assert!(requires_assessment_before_final(
            "搜索全网仓库并推荐一个高价值 issue"
        ));
        assert!(assessment_required_message().contains("issue-finder.assess"));
        assert!(!requires_assessment_before_final("check daemon status"));
    }

    #[test]
    fn final_model_request_disables_tools_after_budget_exhaustion() {
        let config = Config::default();
        let registry = AgentToolRegistry::issue_finder_default();
        let request = final_model_request(
            &config,
            &registry,
            vec![AgentModelItem::UserMessage {
                content: "finalize".to_string(),
            }],
        );

        assert!(request.tools.is_empty());
        assert!(!request.parallel_tool_calls);
        assert!(request.instructions.contains("Do not call tools"));
    }
}
