use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::Config;
use crate::paths::IssueFinderPaths;
use crate::tool_runtime::{IssueFinderToolInvocation, IssueFinderToolRuntime};
use crate::tool_specs::{TOOL_SCOUT, TOOL_STATUS};

use super::model::{
    AgentTaskSendRequest, AgentTaskStatus, AgentThreadItem, AgentThreadStatus,
    AgentThreadTurnRequest,
};
use super::store::{AgentStore, NewAgentThreadItem};

const AGENT_LLM_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_OBSERVATION_CHARS: usize = 12_000;
const MAX_THREAD_CONTEXT_ITEMS: usize = 12;
const MAX_THREAD_CONTEXT_ITEM_CHARS: usize = 4_000;

#[derive(Debug, Clone, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChatChoiceMessage {
    content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDecision {
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub arguments: Option<Value>,
    #[serde(default, alias = "final")]
    pub final_answer: Option<String>,
    #[serde(default)]
    pub rationale: Option<String>,
}

pub async fn run_agent_task(
    paths: IssueFinderPaths,
    config: Config,
    task_id: String,
) -> Result<()> {
    match run_agent_task_inner(paths.clone(), config, task_id.clone()).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = mark_task_failed(&paths, &task_id, error.to_string());
            Err(error)
        }
    }
}

pub async fn run_agent_turn(
    paths: IssueFinderPaths,
    config: Config,
    turn_id: String,
) -> Result<()> {
    match run_agent_turn_inner(paths.clone(), config, turn_id.clone()).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = mark_turn_failed(&paths, &turn_id, error.to_string());
            Err(error)
        }
    }
}

async fn run_agent_task_inner(
    paths: IssueFinderPaths,
    config: Config,
    task_id: String,
) -> Result<()> {
    let (task_goal, input) = {
        let store = AgentStore::open(paths.clone())?;
        let task = store.get_task(&task_id)?;
        if task.status.is_terminal() {
            return Ok(());
        }
        store.update_task_status(&task_id, AgentTaskStatus::Running, None, None)?;
        store.add_event(
            &task_id,
            "task_started",
            "Agent task started.",
            json!({"goal": task.goal}),
        )?;
        let input = task_input_from_metadata(&task.metadata, &task.goal)?;
        (task.goal, input)
    };

    let mut messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: system_prompt(),
        },
        ChatMessage {
            role: "user".to_string(),
            content: user_prompt(&task_goal, &input),
        },
    ];

    let mut last_tool_output: Option<Value> = None;

    for turn_index in 0..input.normalized_max_turns() {
        let raw_decision = request_agent_decision(&config, &messages).await?;
        {
            let store = AgentStore::open(paths.clone())?;
            store.add_message(
                &task_id,
                "assistant",
                &raw_decision,
                json!({"turnIndex": turn_index, "messageType": "agent_decision"}),
            )?;
        }

        let decision = parse_agent_decision(&raw_decision)
            .with_context(|| format!("LLM returned an invalid agent decision: {raw_decision}"))?;

        if let Some(final_answer) = normalized_optional(decision.final_answer.as_deref()) {
            complete_task(
                &paths,
                &task_id,
                &task_goal,
                final_answer,
                decision.rationale.clone(),
                last_tool_output,
            )?;
            return Ok(());
        }

        let Some(tool_name) = normalized_optional(decision.tool.as_deref()) else {
            anyhow::bail!("LLM decision did not include a tool or finalAnswer");
        };
        if !tool_allowed(&tool_name) {
            anyhow::bail!("LLM selected unsupported tool {tool_name}");
        }

        let arguments = normalize_tool_arguments(&tool_name, decision.arguments, &input)?;
        let call_id = {
            let store = AgentStore::open(paths.clone())?;
            let call =
                store.start_tool_call(&task_id, turn_index, &tool_name, arguments.clone())?;
            store.add_event(
                &task_id,
                "tool_call_started",
                &format!("Agent called {tool_name}."),
                json!({
                    "toolName": tool_name,
                    "toolCallId": call.id,
                    "turnIndex": turn_index
                }),
            )?;
            call.id
        };

        let output = IssueFinderToolRuntime::new(paths.clone(), config.clone())
            .execute(IssueFinderToolInvocation {
                call_id: call_id.clone(),
                turn_id: Some(task_id.clone()),
                tool_name: tool_name.clone(),
                arguments,
            })
            .await;
        let output_json = serde_json::to_value(&output)?;
        let output_success = output.success;
        let output_status = output.status.clone();
        let tool_error = (!output_success).then(|| tool_output_error(&output_json));

        {
            let store = AgentStore::open(paths.clone())?;
            store.finish_tool_call(
                &call_id,
                if output_success { "ok" } else { "failed" },
                Some(output_json.clone()),
                tool_error.clone(),
            )?;
            store.add_event(
                &task_id,
                if output_success {
                    "tool_call_completed"
                } else {
                    "tool_call_failed"
                },
                &format!("Agent tool {tool_name} returned {output_status}."),
                json!({
                    "toolName": tool_name,
                    "toolCallId": call_id,
                    "success": output_success,
                    "status": output_status
                }),
            )?;
        }

        last_tool_output = Some(output_json.clone());
        messages.push(ChatMessage {
            role: "assistant".to_string(),
            content: raw_decision,
        });
        messages.push(ChatMessage {
            role: "user".to_string(),
            content: tool_observation(&tool_name, &output_json),
        });
    }

    anyhow::bail!(
        "agent task reached max_turns={} without finalAnswer",
        input.normalized_max_turns()
    )
}

async fn run_agent_turn_inner(
    paths: IssueFinderPaths,
    config: Config,
    turn_id: String,
) -> Result<()> {
    let (thread_id, turn_input, input) = {
        let store = AgentStore::open(paths.clone())?;
        let turn = store.get_turn(&turn_id)?;
        if turn.status.is_terminal() {
            return Ok(());
        }
        let thread = store.get_thread(&turn.thread_id)?;
        if thread.status == AgentThreadStatus::Archived {
            anyhow::bail!("agent thread {} is archived", thread.id);
        }
        store.update_thread_status(&thread.id, AgentThreadStatus::Running)?;
        store.update_turn_status(&turn_id, AgentTaskStatus::Running, None, None)?;
        store.add_thread_event(
            &thread.id,
            Some(&turn_id),
            "turn_started",
            "Agent turn started.",
            json!({"input": turn.input}),
        )?;
        let input = turn_input_from_metadata(&turn.metadata, &turn.input)?;
        (thread.id, turn.input, input)
    };

    let mut messages = {
        let store = AgentStore::open(paths.clone())?;
        thread_messages(&store, &thread_id, &turn_id, &turn_input, &input)?
    };

    let mut last_tool_output: Option<Value> = None;

    for turn_index in 0..input.normalized_max_turns() {
        let raw_decision = request_agent_decision(&config, &messages).await?;
        {
            let store = AgentStore::open(paths.clone())?;
            store.add_thread_item(NewAgentThreadItem {
                thread_id: &thread_id,
                turn_id: Some(&turn_id),
                item_type: "assistant_decision",
                role: Some("assistant"),
                content: Some(&raw_decision),
                tool_name: None,
                payload: json!({"turnIndex": turn_index, "messageType": "agent_decision"}),
            })?;
        }

        let decision = parse_agent_decision(&raw_decision)
            .with_context(|| format!("LLM returned an invalid agent decision: {raw_decision}"))?;

        if let Some(final_answer) = normalized_optional(decision.final_answer.as_deref()) {
            complete_turn(
                &paths,
                &thread_id,
                &turn_id,
                &turn_input,
                final_answer,
                decision.rationale.clone(),
                last_tool_output,
            )?;
            return Ok(());
        }

        let Some(tool_name) = normalized_optional(decision.tool.as_deref()) else {
            anyhow::bail!("LLM decision did not include a tool or finalAnswer");
        };
        if !tool_allowed(&tool_name) {
            anyhow::bail!("LLM selected unsupported tool {tool_name}");
        }

        let arguments = normalize_thread_tool_arguments(&tool_name, decision.arguments, &input)?;
        let call_id = {
            let store = AgentStore::open(paths.clone())?;
            let item = store.add_thread_item(NewAgentThreadItem {
                thread_id: &thread_id,
                turn_id: Some(&turn_id),
                item_type: "tool_call_started",
                role: None,
                content: None,
                tool_name: Some(&tool_name),
                payload: json!({
                    "toolName": tool_name,
                    "arguments": arguments,
                    "turnIndex": turn_index
                }),
            })?;
            store.add_thread_event(
                &thread_id,
                Some(&turn_id),
                "tool_call_started",
                &format!("Agent called {tool_name}."),
                json!({
                    "toolName": tool_name,
                    "toolCallId": item.id,
                    "turnIndex": turn_index
                }),
            )?;
            item.id
        };

        let output = IssueFinderToolRuntime::new(paths.clone(), config.clone())
            .execute(IssueFinderToolInvocation {
                call_id: call_id.clone(),
                turn_id: Some(turn_id.clone()),
                tool_name: tool_name.clone(),
                arguments,
            })
            .await;
        let output_json = serde_json::to_value(&output)?;
        let output_success = output.success;
        let output_status = output.status.clone();
        let tool_error = (!output_success).then(|| tool_output_error(&output_json));
        let observation = tool_observation(&tool_name, &output_json);

        {
            let store = AgentStore::open(paths.clone())?;
            store.add_thread_item(NewAgentThreadItem {
                thread_id: &thread_id,
                turn_id: Some(&turn_id),
                item_type: if output_success {
                    "tool_call_completed"
                } else {
                    "tool_call_failed"
                },
                role: Some("user"),
                content: Some(&observation),
                tool_name: Some(&tool_name),
                payload: json!({
                    "toolName": tool_name,
                    "toolCallId": call_id,
                    "success": output_success,
                    "status": output_status,
                    "error": tool_error,
                    "output": output_json
                }),
            })?;
            store.add_thread_event(
                &thread_id,
                Some(&turn_id),
                if output_success {
                    "tool_call_completed"
                } else {
                    "tool_call_failed"
                },
                &format!("Agent tool {tool_name} returned {output_status}."),
                json!({
                    "toolName": tool_name,
                    "toolCallId": call_id,
                    "success": output_success,
                    "status": output_status
                }),
            )?;
        }

        last_tool_output = Some(output_json);
        messages.push(ChatMessage {
            role: "assistant".to_string(),
            content: raw_decision,
        });
        messages.push(ChatMessage {
            role: "user".to_string(),
            content: observation,
        });
    }

    anyhow::bail!(
        "agent turn reached max_turns={} without finalAnswer",
        input.normalized_max_turns()
    )
}

pub fn parse_agent_decision(raw: &str) -> Result<AgentDecision> {
    let json_text =
        extract_json_object(raw).context("agent decision must contain a JSON object")?;
    let decision = serde_json::from_str::<AgentDecision>(&json_text)?;
    if let Some(arguments) = decision.arguments.as_ref() {
        if !arguments.is_object() {
            anyhow::bail!("agent decision arguments must be a JSON object");
        }
    }
    Ok(decision)
}

fn task_input_from_metadata(metadata: &Value, goal: &str) -> Result<AgentTaskSendRequest> {
    if let Some(input) = metadata.get("input") {
        let mut parsed = serde_json::from_value::<AgentTaskSendRequest>(input.clone())?;
        if parsed.goal.trim().is_empty() {
            parsed.goal = goal.to_string();
        }
        return Ok(parsed);
    }

    Ok(AgentTaskSendRequest {
        goal: goal.to_string(),
        repo: None,
        limit: Some(10),
        refresh: false,
        max_turns: Some(4),
        run_immediately: true,
    })
}

fn turn_input_from_metadata(metadata: &Value, input: &str) -> Result<AgentThreadTurnRequest> {
    if let Some(raw_input) = metadata.get("input") {
        let mut parsed = serde_json::from_value::<AgentThreadTurnRequest>(raw_input.clone())?;
        if parsed.input.trim().is_empty() {
            parsed.input = input.to_string();
        }
        return Ok(parsed);
    }

    Ok(AgentThreadTurnRequest {
        input: input.to_string(),
        repo: None,
        limit: Some(10),
        refresh: false,
        max_turns: Some(4),
        run_immediately: true,
    })
}

async fn request_agent_decision(config: &Config, messages: &[ChatMessage]) -> Result<String> {
    if !config.llm.enabled {
        anyhow::bail!("LLM agent mode requires llm.enabled=true");
    }
    let api_key = config.resolved_llm_api_key();
    if api_key.trim().is_empty() {
        anyhow::bail!("LLM agent mode requires an LLM API key");
    }

    let base_url = config.llm.base_url.trim_end_matches('/');
    let url = format!("{base_url}/chat/completions");
    let client = reqwest::Client::builder()
        .user_agent("issue-finder-agent")
        .timeout(AGENT_LLM_TIMEOUT)
        .build()?;
    let request = ChatCompletionRequest {
        model: config.llm.model.clone(),
        messages: messages.to_vec(),
        temperature: 0.1,
    };

    let response = client
        .post(url)
        .bearer_auth(api_key.trim())
        .json(&request)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("LLM agent request failed with {status}: {body}");
    }

    let response = response.json::<ChatCompletionResponse>().await?;
    Ok(response
        .choices
        .first()
        .map(|choice| choice.message.content.clone())
        .unwrap_or_default())
}

fn system_prompt() -> String {
    format!(
        r#"You are Issue Finder's local agent daemon.
You receive high-level natural-language goals and choose Issue Finder tools.

This first daemon version is read-only. You may use only these tools:
- {TOOL_STATUS}: check local readiness.
- {TOOL_SCOUT}: search and rank GitHub issues. Use repo=null for global discovery.

Return exactly one JSON object and no prose.
To call a tool:
{{"tool":"{TOOL_SCOUT}","arguments":{{"limit":10,"repo":null,"refresh":false,"includeFiltered":false,"recordExposure":true}},"finalAnswer":null,"rationale":"why this tool is needed"}}

To finish:
{{"tool":null,"arguments":{{}},"finalAnswer":"concise recommendation summary for the user","rationale":"why the result answers the goal"}}

Do not invent issue data. Base finalAnswer on tool observations only."#
    )
}

fn user_prompt(goal: &str, input: &AgentTaskSendRequest) -> String {
    let repo = input.repo.as_deref().unwrap_or("null");
    format!(
        "Goal: {goal}\nDefault scout arguments: limit={}, repo={}, refresh={}, includeFiltered=false, recordExposure=true.\nChoose the next action.",
        input.normalized_limit(),
        repo,
        input.refresh
    )
}

fn thread_user_prompt(input_text: &str, input: &AgentThreadTurnRequest) -> String {
    let repo = input.repo.as_deref().unwrap_or("null");
    format!(
        "Thread turn input: {input_text}\nDefault scout arguments: limit={}, repo={}, refresh={}, includeFiltered=false, recordExposure=true.\nChoose the next action.",
        input.normalized_limit(),
        repo,
        input.refresh
    )
}

fn normalize_tool_arguments(
    tool_name: &str,
    arguments: Option<Value>,
    input: &AgentTaskSendRequest,
) -> Result<Value> {
    let mut arguments = arguments.unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        anyhow::bail!("tool arguments must be a JSON object");
    }

    if tool_name == TOOL_SCOUT {
        let object = arguments.as_object_mut().expect("checked object");
        object
            .entry("limit".to_string())
            .or_insert_with(|| json!(input.normalized_limit()));
        object
            .entry("repo".to_string())
            .or_insert_with(|| input.repo.as_ref().map_or(Value::Null, |repo| json!(repo)));
        object
            .entry("refresh".to_string())
            .or_insert_with(|| json!(input.refresh));
        object
            .entry("includeFiltered".to_string())
            .or_insert_with(|| json!(false));
        object
            .entry("recordExposure".to_string())
            .or_insert_with(|| json!(true));
    }

    if tool_name == TOOL_STATUS {
        arguments
            .as_object_mut()
            .expect("checked object")
            .entry("checkAuth".to_string())
            .or_insert_with(|| json!(true));
    }

    Ok(arguments)
}

fn normalize_thread_tool_arguments(
    tool_name: &str,
    arguments: Option<Value>,
    input: &AgentThreadTurnRequest,
) -> Result<Value> {
    let mut arguments = arguments.unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        anyhow::bail!("tool arguments must be a JSON object");
    }

    if tool_name == TOOL_SCOUT {
        let object = arguments.as_object_mut().expect("checked object");
        object
            .entry("limit".to_string())
            .or_insert_with(|| json!(input.normalized_limit()));
        object
            .entry("repo".to_string())
            .or_insert_with(|| input.repo.as_ref().map_or(Value::Null, |repo| json!(repo)));
        object
            .entry("refresh".to_string())
            .or_insert_with(|| json!(input.refresh));
        object
            .entry("includeFiltered".to_string())
            .or_insert_with(|| json!(false));
        object
            .entry("recordExposure".to_string())
            .or_insert_with(|| json!(true));
    }

    if tool_name == TOOL_STATUS {
        arguments
            .as_object_mut()
            .expect("checked object")
            .entry("checkAuth".to_string())
            .or_insert_with(|| json!(true));
    }

    Ok(arguments)
}

fn thread_messages(
    store: &AgentStore,
    thread_id: &str,
    current_turn_id: &str,
    input_text: &str,
    input: &AgentThreadTurnRequest,
) -> Result<Vec<ChatMessage>> {
    let mut messages = vec![ChatMessage {
        role: "system".to_string(),
        content: system_prompt(),
    }];

    let items = store.list_thread_items(thread_id)?;
    let mut history = items
        .into_iter()
        .filter(|item| item.turn_id.as_deref() != Some(current_turn_id))
        .filter_map(thread_item_message)
        .collect::<Vec<_>>();
    if history.len() > MAX_THREAD_CONTEXT_ITEMS {
        history = history.split_off(history.len() - MAX_THREAD_CONTEXT_ITEMS);
    }
    messages.extend(history);
    messages.push(ChatMessage {
        role: "user".to_string(),
        content: thread_user_prompt(input_text, input),
    });
    Ok(messages)
}

fn thread_item_message(item: AgentThreadItem) -> Option<ChatMessage> {
    match item.item_type.as_str() {
        "user_message"
        | "assistant_decision"
        | "tool_call_completed"
        | "tool_call_failed"
        | "final_answer" => {
            let role = item.role?;
            let content = item.content?;
            Some(ChatMessage {
                role,
                content: truncate_chars(&content, MAX_THREAD_CONTEXT_ITEM_CHARS),
            })
        }
        _ => None,
    }
}

fn complete_task(
    paths: &IssueFinderPaths,
    task_id: &str,
    goal: &str,
    final_answer: String,
    rationale: Option<String>,
    last_tool_output: Option<Value>,
) -> Result<()> {
    let result = json!({
        "kind": "issue_finder_agent_result",
        "version": 1,
        "goal": goal,
        "finalAnswer": final_answer,
        "rationale": rationale,
        "lastToolOutput": last_tool_output
    });
    let store = AgentStore::open(paths.clone())?;
    store.add_message(
        task_id,
        "assistant",
        result["finalAnswer"].as_str().unwrap_or_default(),
        json!({"messageType": "final_answer"}),
    )?;
    store.update_task_status(task_id, AgentTaskStatus::Completed, Some(result), None)?;
    store.add_event(
        task_id,
        "task_completed",
        "Agent task completed.",
        json!({}),
    )?;
    Ok(())
}

fn complete_turn(
    paths: &IssueFinderPaths,
    thread_id: &str,
    turn_id: &str,
    input: &str,
    final_answer: String,
    rationale: Option<String>,
    last_tool_output: Option<Value>,
) -> Result<()> {
    let result = json!({
        "kind": "issue_finder_agent_turn_result",
        "version": 1,
        "input": input,
        "finalAnswer": final_answer,
        "rationale": rationale,
        "lastToolOutput": last_tool_output
    });
    let store = AgentStore::open(paths.clone())?;
    store.add_thread_item(NewAgentThreadItem {
        thread_id,
        turn_id: Some(turn_id),
        item_type: "final_answer",
        role: Some("assistant"),
        content: result["finalAnswer"].as_str(),
        tool_name: None,
        payload: result.clone(),
    })?;
    store.update_turn_status(turn_id, AgentTaskStatus::Completed, Some(result), None)?;
    store.update_thread_status(thread_id, AgentThreadStatus::Active)?;
    store.add_thread_event(
        thread_id,
        Some(turn_id),
        "turn_completed",
        "Agent turn completed.",
        json!({}),
    )?;
    Ok(())
}

fn mark_task_failed(paths: &IssueFinderPaths, task_id: &str, error: String) -> Result<()> {
    let store = AgentStore::open(paths.clone())?;
    store.update_task_status(task_id, AgentTaskStatus::Failed, None, Some(error.clone()))?;
    store.add_event(
        task_id,
        "task_failed",
        "Agent task failed.",
        json!({ "error": error }),
    )?;
    Ok(())
}

fn mark_turn_failed(paths: &IssueFinderPaths, turn_id: &str, error: String) -> Result<()> {
    let store = AgentStore::open(paths.clone())?;
    let turn = store.get_turn(turn_id)?;
    store.update_turn_status(turn_id, AgentTaskStatus::Failed, None, Some(error.clone()))?;
    store.update_thread_status(&turn.thread_id, AgentThreadStatus::Active)?;
    store.add_thread_event(
        &turn.thread_id,
        Some(turn_id),
        "turn_failed",
        "Agent turn failed.",
        json!({ "error": error }),
    )?;
    Ok(())
}

fn tool_allowed(tool_name: &str) -> bool {
    matches!(tool_name, TOOL_SCOUT | TOOL_STATUS)
}

fn normalized_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn tool_output_error(output: &Value) -> String {
    output
        .pointer("/structuredContent/error/message")
        .and_then(Value::as_str)
        .or_else(|| output.pointer("/status").and_then(Value::as_str))
        .unwrap_or("tool call failed")
        .to_string()
}

fn tool_observation(tool_name: &str, output: &Value) -> String {
    let raw = serde_json::to_string(output).unwrap_or_else(|_| "{}".to_string());
    format!(
        "Observation from {tool_name}:\n{}\nReturn the next JSON decision. If the goal is answered, finalAnswer must summarize the recommended issues.",
        truncate_chars(&raw, MAX_OBSERVATION_CHARS)
    )
}

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut truncated = value.chars().take(limit).collect::<String>();
    truncated.push_str("\n...[truncated]");
    truncated
}

fn extract_json_object(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed.to_string());
    }

    if let Some(start) = trimmed.find("```") {
        let after_start = &trimmed[start + 3..];
        let after_language = after_start
            .strip_prefix("json")
            .unwrap_or(after_start)
            .trim_start_matches(['\n', '\r', ' ']);
        if let Some(end) = after_language.find("```") {
            let fenced = after_language[..end].trim();
            if fenced.starts_with('{') && fenced.ends_with('}') {
                return Some(fenced.to_string());
            }
        }
    }

    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    (end > start).then(|| trimmed[start..=end].to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{normalize_tool_arguments, parse_agent_decision};
    use crate::agent::model::AgentTaskSendRequest;
    use crate::tool_specs::TOOL_SCOUT;

    #[test]
    fn parses_fenced_agent_decision() {
        let decision = parse_agent_decision(
            r#"```json
{"tool":"issue-finder.scout","arguments":{"limit":3},"finalAnswer":null}
```"#,
        )
        .unwrap();

        assert_eq!(decision.tool.as_deref(), Some("issue-finder.scout"));
        assert_eq!(decision.arguments.unwrap()["limit"], 3);
    }

    #[test]
    fn scout_arguments_fill_goal_defaults() {
        let input = AgentTaskSendRequest {
            goal: "搜索全网仓库并推荐 issue".to_string(),
            repo: None,
            limit: Some(7),
            refresh: true,
            max_turns: Some(3),
            run_immediately: true,
        };

        let arguments = normalize_tool_arguments(TOOL_SCOUT, Some(json!({})), &input).unwrap();

        assert_eq!(arguments["limit"], 7);
        assert!(arguments["repo"].is_null());
        assert_eq!(arguments["refresh"], true);
        assert_eq!(arguments["includeFiltered"], false);
        assert_eq!(arguments["recordExposure"], true);
    }
}
