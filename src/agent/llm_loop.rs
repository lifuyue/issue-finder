use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::Config;
use crate::paths::IssueFinderPaths;
use crate::tool_runtime::{IssueFinderToolInvocation, IssueFinderToolRuntime};
use crate::tool_specs::{
    list_tool_specs, IssueFinderToolSpec, TOOL_ASSESS, TOOL_DISPATCH_ARTIFACTS,
    TOOL_DISPATCH_EVENTS, TOOL_DISPATCH_REVIEW_LIST, TOOL_DISPATCH_REVIEW_SHOW,
    TOOL_DISPATCH_STATUS, TOOL_DISPATCH_TIMELINE, TOOL_GITHUB_DRAFT_FINAL_COMMENT,
    TOOL_GITHUB_DRAFT_TRACKING_COMMENT, TOOL_GITHUB_INTERACTIONS, TOOL_MEMORY_RECALL,
    TOOL_MEMORY_STATUS, TOOL_PREPARE, TOOL_READ_CONTEXT, TOOL_SCOUT, TOOL_STATUS,
};

use super::model::{
    AgentTaskSendRequest, AgentTaskStatus, AgentThread, AgentThreadItem, AgentThreadStartRequest,
    AgentThreadStatus, AgentTurn, AgentTurnStartRequest, AgentTurnStatus,
};
use super::store::{AgentStore, NewAgentThreadItem};

const AGENT_LLM_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_OBSERVATION_CHARS: usize = 12_000;
const DEFAULT_AGENT_MAX_TURNS: usize = 6;
const REQUIRED_ASSESSMENTS_AFTER_SCOUT: usize = 3;
const AGENT_ALLOWED_TOOLS: &[&str] = &[
    TOOL_STATUS,
    TOOL_SCOUT,
    TOOL_ASSESS,
    TOOL_PREPARE,
    TOOL_READ_CONTEXT,
    TOOL_MEMORY_STATUS,
    TOOL_MEMORY_RECALL,
    TOOL_DISPATCH_STATUS,
    TOOL_DISPATCH_TIMELINE,
    TOOL_DISPATCH_EVENTS,
    TOOL_DISPATCH_ARTIFACTS,
    TOOL_DISPATCH_REVIEW_LIST,
    TOOL_DISPATCH_REVIEW_SHOW,
    TOOL_GITHUB_INTERACTIONS,
    TOOL_GITHUB_DRAFT_TRACKING_COMMENT,
    TOOL_GITHUB_DRAFT_FINAL_COMMENT,
];

#[derive(Debug, Clone, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

pub fn allowed_agent_tool_names() -> &'static [&'static str] {
    AGENT_ALLOWED_TOOLS
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TurnRuntimeInput {
    thread_goal: String,
    user_input: String,
    repo: Option<String>,
    limit: Option<usize>,
    refresh: bool,
    max_turns: Option<usize>,
}

impl TurnRuntimeInput {
    fn normalized_limit(&self) -> usize {
        self.limit.unwrap_or(10).max(1)
    }

    fn normalized_max_turns(&self) -> usize {
        self.max_turns
            .unwrap_or(DEFAULT_AGENT_MAX_TURNS)
            .clamp(1, 12)
    }
}

pub async fn run_agent_task(
    paths: IssueFinderPaths,
    config: Config,
    task_id: String,
) -> Result<()> {
    let (thread_id, turn_id) = {
        let store = AgentStore::open(paths.clone())?;
        let task = store.get_task(&task_id)?;
        if task.status.is_terminal() {
            return Ok(());
        }
        let thread_id = task
            .metadata
            .get("threadId")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let turn_id = task
            .metadata
            .get("turnId")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        match (thread_id, turn_id) {
            (Some(thread_id), Some(turn_id)) => (thread_id, turn_id),
            _ => create_thread_for_legacy_task(&store, &task_id, &task.goal, &task.metadata)?,
        }
    };

    run_agent_turn(paths, config, thread_id, turn_id).await
}

pub async fn run_agent_turn(
    paths: IssueFinderPaths,
    config: Config,
    thread_id: String,
    turn_id: String,
) -> Result<()> {
    match run_agent_turn_inner(paths.clone(), config, thread_id.clone(), turn_id.clone()).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = mark_turn_failed(&paths, &thread_id, &turn_id, error.to_string());
            Err(error)
        }
    }
}

async fn run_agent_turn_inner(
    paths: IssueFinderPaths,
    config: Config,
    thread_id: String,
    turn_id: String,
) -> Result<()> {
    let (thread, turn, input, mut messages) = {
        let store = AgentStore::open(paths.clone())?;
        let thread = store.get_thread(&thread_id)?;
        if !thread.status.accepts_turns() {
            anyhow::bail!(
                "agent thread {} does not accept turns while status={}",
                thread.id,
                thread.status.as_str()
            );
        }
        let turn = store.get_turn(&turn_id)?;
        if turn.status.is_terminal() {
            return Ok(());
        }
        store.update_turn_status(&turn_id, AgentTurnStatus::Running, None, None)?;
        store.update_thread_status(&thread_id, AgentThreadStatus::Running, None, None)?;
        mirror_legacy_task_started(&store, &turn)?;
        store.add_thread_event(
            &thread_id,
            Some(&turn_id),
            "turn_started",
            "Agent turn started.",
            json!({"input": turn.input}),
        )?;
        ensure_user_item(&store, &thread_id, &turn)?;
        let input = turn_input_from_metadata(&thread, &turn)?;
        let items = store.list_thread_items(&thread_id)?;
        let messages = build_thread_messages(&thread, &turn, &items, &input);
        (thread, turn, input, messages)
    };

    let mut last_tool_output: Option<Value> = None;
    let mut required_assessments_after_scout = 0usize;
    let mut completed_assessments_after_scout = 0usize;
    let mut scout_issue_refs = Vec::<String>::new();

    for turn_index in 0..input.normalized_max_turns() {
        let raw_decision = request_agent_decision(&config, &messages).await?;
        {
            let store = AgentStore::open(paths.clone())?;
            store.add_thread_item(NewAgentThreadItem {
                id: None,
                thread_id: &thread_id,
                turn_id: Some(&turn_id),
                kind: "assistant_decision",
                role: Some("assistant"),
                content: Some(&raw_decision),
                tool_name: None,
                tool_call_id: None,
                payload: json!({"turnIndex": turn_index, "messageType": "agent_decision"}),
            })?;
        }

        let decision = parse_agent_decision(&raw_decision)
            .with_context(|| format!("LLM returned an invalid agent decision: {raw_decision}"))?;

        if let Some(final_answer) = normalized_optional(decision.final_answer.as_deref()) {
            if completed_assessments_after_scout < required_assessments_after_scout {
                let store = AgentStore::open(paths.clone())?;
                store.add_thread_event(
                    &thread_id,
                    Some(&turn_id),
                    "agent_final_deferred",
                    "Agent final answer was deferred until top scout candidates are assessed.",
                    json!({
                        "completedAssessments": completed_assessments_after_scout,
                        "requiredAssessments": required_assessments_after_scout,
                        "candidateIssues": scout_issue_refs
                    }),
                )?;
                messages.push(ChatMessage {
                    role: "assistant".to_string(),
                    content: raw_decision,
                });
                messages.push(ChatMessage {
                    role: "user".to_string(),
                    content: final_deferred_prompt(
                        completed_assessments_after_scout,
                        required_assessments_after_scout,
                        &scout_issue_refs,
                    ),
                });
                continue;
            }
            complete_turn(
                &paths,
                &thread,
                &turn,
                &input,
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
            let call_id = crate::agent::store::new_id("agent-tool-call");
            store.add_thread_item(NewAgentThreadItem {
                id: Some(&call_id),
                thread_id: &thread_id,
                turn_id: Some(&turn_id),
                kind: "tool_call",
                role: Some("assistant"),
                content: Some(&format!("Calling {tool_name}.")),
                tool_name: Some(&tool_name),
                tool_call_id: Some(&call_id),
                payload: json!({
                    "arguments": arguments,
                    "status": "running",
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
                    "toolCallId": call_id,
                    "turnIndex": turn_index
                }),
            )?;
            call_id
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
                id: None,
                thread_id: &thread_id,
                turn_id: Some(&turn_id),
                kind: "tool_result",
                role: Some("tool"),
                content: Some(&observation),
                tool_name: Some(&tool_name),
                tool_call_id: Some(&call_id),
                payload: json!({
                    "output": output_json,
                    "success": output_success,
                    "status": output_status,
                    "error": tool_error
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

        if tool_name == TOOL_SCOUT && output_success {
            scout_issue_refs = scout_candidate_issue_refs(&output_json)
                .into_iter()
                .take(REQUIRED_ASSESSMENTS_AFTER_SCOUT)
                .collect();
            required_assessments_after_scout =
                required_assessments_after_scout.max(scout_issue_refs.len());
        } else if tool_name == TOOL_ASSESS && output_success && required_assessments_after_scout > 0
        {
            completed_assessments_after_scout += 1;
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

fn create_thread_for_legacy_task(
    store: &AgentStore,
    task_id: &str,
    goal: &str,
    metadata: &Value,
) -> Result<(String, String)> {
    let input = task_input_from_metadata(metadata, goal)?;
    let thread = store.create_thread(
        goal,
        json!({
            "transport": "legacy_task_runner",
            "legacyTaskId": task_id
        }),
    )?;
    let turn_request = AgentTurnStartRequest {
        input: goal.to_string(),
        repo: input.repo,
        limit: input.limit,
        refresh: input.refresh,
        max_turns: input.max_turns,
        run_immediately: true,
        metadata: json!({"legacyTaskId": task_id}),
    };
    let turn = store.create_turn(
        &thread.id,
        goal,
        json!({
            "input": turn_request,
            "legacyTaskId": task_id
        }),
    )?;
    store.add_thread_item(NewAgentThreadItem {
        id: None,
        thread_id: &thread.id,
        turn_id: Some(&turn.id),
        kind: "user_message",
        role: Some("user"),
        content: Some(goal),
        tool_name: None,
        tool_call_id: None,
        payload: json!({"messageType": "turn_input", "legacyTaskId": task_id}),
    })?;
    store.add_thread_event(
        &thread.id,
        Some(&turn.id),
        "turn_queued",
        "Agent turn queued.",
        json!({"input": goal, "legacyTaskId": task_id}),
    )?;
    Ok((thread.id, turn.id))
}

fn task_input_from_metadata(metadata: &Value, goal: &str) -> Result<AgentTaskSendRequest> {
    if let Some(input) = metadata.get("input") {
        let mut parsed = serde_json::from_value::<AgentTaskSendRequest>(input.clone())?;
        if parsed.goal.trim().is_empty() {
            parsed.goal = goal.to_string();
        }
        if parsed.max_turns.is_none() {
            parsed.max_turns = Some(DEFAULT_AGENT_MAX_TURNS);
        }
        return Ok(parsed);
    }

    Ok(AgentTaskSendRequest {
        goal: goal.to_string(),
        repo: None,
        limit: Some(10),
        refresh: false,
        max_turns: Some(DEFAULT_AGENT_MAX_TURNS),
        run_immediately: true,
    })
}

fn turn_input_from_metadata(thread: &AgentThread, turn: &AgentTurn) -> Result<TurnRuntimeInput> {
    if let Some(input) = turn.metadata.get("input") {
        let mut parsed = serde_json::from_value::<AgentTurnStartRequest>(input.clone())?;
        if parsed.input.trim().is_empty() {
            parsed.input = turn.input.clone();
        }
        if parsed.max_turns.is_none() {
            parsed.max_turns = Some(DEFAULT_AGENT_MAX_TURNS);
        }
        return Ok(TurnRuntimeInput {
            thread_goal: thread.goal.clone(),
            user_input: parsed.input,
            repo: parsed.repo,
            limit: parsed.limit,
            refresh: parsed.refresh,
            max_turns: parsed.max_turns,
        });
    }

    if let Some(input) = turn.metadata.get("threadStart") {
        let mut parsed = serde_json::from_value::<AgentThreadStartRequest>(input.clone())?;
        if parsed.goal.trim().is_empty() {
            parsed.goal = turn.input.clone();
        }
        if parsed.max_turns.is_none() {
            parsed.max_turns = Some(DEFAULT_AGENT_MAX_TURNS);
        }
        return Ok(TurnRuntimeInput {
            thread_goal: thread.goal.clone(),
            user_input: parsed.goal,
            repo: parsed.repo,
            limit: parsed.limit,
            refresh: parsed.refresh,
            max_turns: parsed.max_turns,
        });
    }

    Ok(TurnRuntimeInput {
        thread_goal: thread.goal.clone(),
        user_input: turn.input.clone(),
        repo: None,
        limit: Some(10),
        refresh: false,
        max_turns: Some(DEFAULT_AGENT_MAX_TURNS),
    })
}

fn build_thread_messages(
    thread: &AgentThread,
    turn: &AgentTurn,
    items: &[AgentThreadItem],
    input: &TurnRuntimeInput,
) -> Vec<ChatMessage> {
    let mut messages = vec![ChatMessage {
        role: "system".to_string(),
        content: system_prompt(),
    }];

    messages.push(ChatMessage {
        role: "user".to_string(),
        content: format!(
            "Thread goal: {}\nCurrent turn: {}\nDefault scout arguments: limit={}, repo={}, refresh={}, includeFiltered=false, recordExposure=true.",
            thread.goal,
            turn.input,
            input.normalized_limit(),
            input.repo.as_deref().unwrap_or("null"),
            input.refresh
        ),
    });

    for item in items {
        let Some(content) = item
            .content
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        match item.kind.as_str() {
            "user_message" => messages.push(ChatMessage {
                role: "user".to_string(),
                content: format!(
                    "User turn {}: {content}",
                    item.turn_id.as_deref().unwrap_or("-")
                ),
            }),
            "assistant_decision" => messages.push(ChatMessage {
                role: "assistant".to_string(),
                content: content.to_string(),
            }),
            "tool_result" => messages.push(ChatMessage {
                role: "user".to_string(),
                content: content.to_string(),
            }),
            "final_answer" => messages.push(ChatMessage {
                role: "assistant".to_string(),
                content: format!(
                    "Final answer for turn {}: {content}",
                    item.turn_id.as_deref().unwrap_or("-")
                ),
            }),
            _ => {}
        }
    }

    messages.push(ChatMessage {
        role: "user".to_string(),
        content: "Choose the next JSON action for the current turn. Continue the existing thread context instead of starting over.".to_string(),
    });

    messages
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
    let tool_catalog = allowed_tool_catalog();
    format!(
        r#"You are Issue Finder's local agent daemon.
You receive a persistent thread transcript. For each user turn, either call exactly one allowed Issue Finder tool or finish the turn with finalAnswer.

Use the transcript as durable context. Continue the existing thread instead of starting over. If a prior tool observation already answers the current turn, finish without calling another tool.

Allowed tool catalog. Use these exact tool names and obey each compact JSON input schema:
{tool_catalog}

Tool selection guide:
- Readiness: use {TOOL_STATUS} when setup, auth, or local readiness is unknown.
- Discovery: use {TOOL_SCOUT} for candidate search; for recommendation goals, assess up to the top 3 visible candidates with {TOOL_ASSESS} before finalAnswer.
- Preparation: use {TOOL_PREPARE} only for a specific issue. It may write Issue Finder local workspace/handoff state, but must not modify target repository source.
- Context: after prepare, use {TOOL_READ_CONTEXT}; read entry, safety, and probe before larger sections unless the user asked for a specific section.
- Memory: use {TOOL_MEMORY_STATUS} or {TOOL_MEMORY_RECALL} to inspect contribution memory; do not invent memory.
- Dispatch inspection: use dispatch status/timeline/events/artifacts/review list/show only for local state inspection.
- GitHub interactions: use {TOOL_GITHUB_INTERACTIONS} to inspect local interaction records. Use draft tools only to create local drafts/approval requests.

Safety boundary:
- Allowed local writes: Issue Finder state, handoff artifacts, local workspace metadata, local GitHub comment drafts and approval requests.
- Forbidden actions: post GitHub comments, approve comment posting, reject or approve dispatch decisions, execute dispatch runs, mutate native sessions, export/approve A2A sends, modify target repository source, install dependencies, commit, push, or open pull requests.

Missing information rule:
- If a required schema field cannot be inferred from the current user turn, transcript, or tool observations, do not invent it.
- Finish with finalAnswer asking for the exact missing identifier, such as issue `owner/repo#123`, handoffId, runId, or interactionId.

Output protocol:
- Return exactly one JSON object and no prose.
- To call a tool, set finalAnswer to null and arguments to a JSON object that matches the tool schema.
- To finish, set tool to null and arguments to an empty object.

Examples:
{{"tool":"{TOOL_SCOUT}","arguments":{{"limit":10,"repo":null,"refresh":false,"includeFiltered":false,"recordExposure":true}},"finalAnswer":null,"rationale":"why this tool is needed"}}
{{"tool":"{TOOL_ASSESS}","arguments":{{"issue":"owner/repo#123","refresh":false,"recordRead":false}},"finalAnswer":null,"rationale":"why this candidate needs verification"}}
{{"tool":"{TOOL_PREPARE}","arguments":{{"issue":"owner/repo#123","refresh":false,"allowGateBypass":false,"bypassReason":null}},"finalAnswer":null,"rationale":"why local handoff context is needed"}}
{{"tool":"{TOOL_READ_CONTEXT}","arguments":{{"handoffId":"handoff-id","section":"entry","maxBytes":12000}},"finalAnswer":null,"rationale":"why this context section is needed"}}
{{"tool":"{TOOL_GITHUB_DRAFT_TRACKING_COMMENT}","arguments":{{"issue":"owner/repo#123","body":null}},"finalAnswer":null,"rationale":"why a local comment draft is useful"}}

{{"tool":null,"arguments":{{}},"finalAnswer":"concise answer for the user","rationale":"why the result answers the current turn"}}

Do not invent issue data. Base finalAnswer on transcript and tool observations only."#,
        tool_catalog = tool_catalog
    )
}

fn allowed_tool_catalog() -> String {
    let specs = list_tool_specs().tools;
    AGENT_ALLOWED_TOOLS
        .iter()
        .map(|tool_name| {
            specs
                .iter()
                .find(|spec| qualified_tool_name(spec) == *tool_name)
                .map(|spec| render_tool_catalog_entry(tool_name, spec))
                .unwrap_or_else(|| {
                    format!(
                        "- `{tool_name}`: canonical tool spec unavailable; arguments must be an object."
                    )
                })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_tool_catalog_entry(tool_name: &str, spec: &IssueFinderToolSpec) -> String {
    let schema = serde_json::to_string(&spec.input_schema).unwrap_or_else(|_| "{}".to_string());
    format!(
        "- `{tool_name}`: {}\n  input_schema: {schema}",
        spec.description
    )
}

fn qualified_tool_name(spec: &IssueFinderToolSpec) -> String {
    match spec.namespace.as_deref() {
        Some(namespace) if !namespace.is_empty() => format!("{namespace}.{}", spec.name),
        _ => spec.name.clone(),
    }
}

fn normalize_tool_arguments(
    tool_name: &str,
    arguments: Option<Value>,
    input: &TurnRuntimeInput,
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

    if tool_name == TOOL_ASSESS {
        let object = arguments.as_object_mut().expect("checked object");
        object
            .entry("refresh".to_string())
            .or_insert_with(|| json!(input.refresh));
        object
            .entry("recordRead".to_string())
            .or_insert_with(|| json!(false));
    }

    Ok(arguments)
}

fn ensure_user_item(store: &AgentStore, thread_id: &str, turn: &AgentTurn) -> Result<()> {
    let has_user_item = store
        .list_turn_items(thread_id, &turn.id)?
        .iter()
        .any(|item| item.kind == "user_message");
    if has_user_item {
        return Ok(());
    }
    store.add_thread_item(NewAgentThreadItem {
        id: None,
        thread_id,
        turn_id: Some(&turn.id),
        kind: "user_message",
        role: Some("user"),
        content: Some(&turn.input),
        tool_name: None,
        tool_call_id: None,
        payload: json!({"messageType": "turn_input"}),
    })?;
    Ok(())
}

fn complete_turn(
    paths: &IssueFinderPaths,
    thread: &AgentThread,
    turn: &AgentTurn,
    input: &TurnRuntimeInput,
    final_answer: String,
    rationale: Option<String>,
    last_tool_output: Option<Value>,
) -> Result<()> {
    let result = json!({
        "kind": "issue_finder_agent_turn_result",
        "version": 1,
        "threadId": thread.id,
        "turnId": turn.id,
        "threadGoal": input.thread_goal,
        "input": input.user_input,
        "finalAnswer": final_answer,
        "rationale": rationale,
        "lastToolOutput": last_tool_output
    });
    let store = AgentStore::open(paths.clone())?;
    store.add_thread_item(NewAgentThreadItem {
        id: None,
        thread_id: &thread.id,
        turn_id: Some(&turn.id),
        kind: "final_answer",
        role: Some("assistant"),
        content: result["finalAnswer"].as_str(),
        tool_name: None,
        tool_call_id: None,
        payload: result.clone(),
    })?;
    store.update_turn_status(
        &turn.id,
        AgentTurnStatus::Completed,
        Some(result.clone()),
        None,
    )?;
    store.update_thread_status(
        &thread.id,
        AgentThreadStatus::Idle,
        Some(result.clone()),
        None,
    )?;
    store.add_thread_event(
        &thread.id,
        Some(&turn.id),
        "turn_completed",
        "Agent turn completed.",
        json!({}),
    )?;
    mirror_legacy_task_completed(&store, turn, result)?;
    Ok(())
}

fn mark_turn_failed(
    paths: &IssueFinderPaths,
    thread_id: &str,
    turn_id: &str,
    error: String,
) -> Result<()> {
    let store = AgentStore::open(paths.clone())?;
    let turn = store.get_turn(turn_id)?;
    store.update_turn_status(turn_id, AgentTurnStatus::Failed, None, Some(error.clone()))?;
    store.update_thread_status(
        thread_id,
        AgentThreadStatus::Idle,
        None,
        Some(error.clone()),
    )?;
    store.add_thread_event(
        thread_id,
        Some(turn_id),
        "turn_failed",
        "Agent turn failed.",
        json!({ "error": error }),
    )?;
    mirror_legacy_task_failed(&store, &turn, error)?;
    Ok(())
}

fn mirror_legacy_task_started(store: &AgentStore, turn: &AgentTurn) -> Result<()> {
    if let Some(task_id) = legacy_task_id(turn) {
        store.update_task_status(&task_id, AgentTaskStatus::Running, None, None)?;
        store.add_event(
            &task_id,
            "task_started",
            "Agent task started through thread runtime.",
            json!({"threadId": turn.thread_id, "turnId": turn.id}),
        )?;
    }
    Ok(())
}

fn mirror_legacy_task_completed(store: &AgentStore, turn: &AgentTurn, result: Value) -> Result<()> {
    if let Some(task_id) = legacy_task_id(turn) {
        store.add_message(
            &task_id,
            "assistant",
            result["finalAnswer"].as_str().unwrap_or_default(),
            json!({"messageType": "final_answer", "threadId": turn.thread_id, "turnId": turn.id}),
        )?;
        store.update_task_status(&task_id, AgentTaskStatus::Completed, Some(result), None)?;
        store.add_event(
            &task_id,
            "task_completed",
            "Agent task completed through thread runtime.",
            json!({"threadId": turn.thread_id, "turnId": turn.id}),
        )?;
    }
    Ok(())
}

fn mirror_legacy_task_failed(store: &AgentStore, turn: &AgentTurn, error: String) -> Result<()> {
    if let Some(task_id) = legacy_task_id(turn) {
        store.update_task_status(&task_id, AgentTaskStatus::Failed, None, Some(error.clone()))?;
        store.add_event(
            &task_id,
            "task_failed",
            "Agent task failed through thread runtime.",
            json!({"threadId": turn.thread_id, "turnId": turn.id, "error": error}),
        )?;
    }
    Ok(())
}

fn legacy_task_id(turn: &AgentTurn) -> Option<String> {
    turn.metadata
        .get("legacyTaskId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            turn.metadata
                .pointer("/metadata/legacyTaskId")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
}

fn tool_allowed(tool_name: &str) -> bool {
    AGENT_ALLOWED_TOOLS.contains(&tool_name)
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
    let next_instruction = if tool_name == TOOL_SCOUT {
        let issue_refs = scout_candidate_issue_refs(output)
            .into_iter()
            .take(REQUIRED_ASSESSMENTS_AFTER_SCOUT)
            .collect::<Vec<_>>();
        if issue_refs.is_empty() {
            "Return the next JSON decision. If the current turn is answered, finalAnswer must summarize the result.".to_string()
        } else {
            format!(
                "Before finalAnswer, assess these top scout candidates with recordRead=false: {}.",
                issue_refs.join(", ")
            )
        }
    } else {
        "Return the next JSON decision. If the current turn is answered, finalAnswer must summarize the useful result and any remaining risks.".to_string()
    };
    format!(
        "Observation from {tool_name}:\n{}\n{}",
        truncate_chars(&raw, MAX_OBSERVATION_CHARS),
        next_instruction
    )
}

fn final_deferred_prompt(completed: usize, required: usize, issue_refs: &[String]) -> String {
    let remaining = issue_refs
        .iter()
        .skip(completed)
        .take(required.saturating_sub(completed))
        .cloned()
        .collect::<Vec<_>>();
    format!(
        "Do not produce finalAnswer yet. You have assessed {completed}/{required} required scout candidates. Next, call {TOOL_ASSESS} with recordRead=false for: {}.",
        remaining.join(", ")
    )
}

fn scout_candidate_issue_refs(output: &Value) -> Vec<String> {
    output
        .pointer("/structured_content/candidates")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|candidate| {
            let issue = candidate.get("issue")?;
            let repo = issue.get("repoFullName").and_then(Value::as_str)?;
            let number = issue.get("number").and_then(Value::as_u64)?;
            Some(format!("{repo}#{number}"))
        })
        .collect()
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

    use super::{
        allowed_agent_tool_names, allowed_tool_catalog, build_thread_messages,
        normalize_tool_arguments, parse_agent_decision, scout_candidate_issue_refs, tool_allowed,
        TurnRuntimeInput,
    };
    use crate::agent::model::{
        AgentThread, AgentThreadItem, AgentThreadStatus, AgentTurn, AgentTurnStatus,
    };
    use crate::tool_specs::{
        TOOL_ASSESS, TOOL_DISPATCH_EXECUTE, TOOL_GITHUB_APPROVE_COMMENT,
        TOOL_GITHUB_DRAFT_TRACKING_COMMENT, TOOL_GITHUB_POST_COMMENT, TOOL_PREPARE, TOOL_SCOUT,
    };

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
        let input = test_input();

        let arguments = normalize_tool_arguments(TOOL_SCOUT, Some(json!({})), &input).unwrap();

        assert_eq!(arguments["limit"], 7);
        assert!(arguments["repo"].is_null());
        assert_eq!(arguments["refresh"], true);
        assert_eq!(arguments["includeFiltered"], false);
        assert_eq!(arguments["recordExposure"], true);
    }

    #[test]
    fn assess_arguments_disable_read_feedback_and_fill_refresh() {
        let input = test_input();

        let arguments =
            normalize_tool_arguments(TOOL_ASSESS, Some(json!({"issue":"owner/repo#1"})), &input)
                .unwrap();

        assert_eq!(arguments["issue"], "owner/repo#1");
        assert_eq!(arguments["refresh"], true);
        assert_eq!(arguments["recordRead"], false);
    }

    #[test]
    fn new_thread_context_injects_tool_catalog_and_safety_boundary() {
        let thread = test_thread("Search global repositories and recommend issues");
        let turn = test_turn(&thread, "Search global repositories and recommend issues");

        let messages = build_thread_messages(&thread, &turn, &[], &test_input());

        assert_eq!(messages[0].role, "system");
        let system = &messages[0].content;
        for tool_name in allowed_agent_tool_names() {
            assert!(
                system.contains(tool_name),
                "system prompt should describe {tool_name}"
            );
        }
        assert!(system.contains("input_schema:"));
        assert!(system.contains("\"required\":[\"handoffId\",\"section\"]"));
        assert!(system.contains("\"required\":[\"runId\"]"));
        assert!(system.contains("\"required\":[\"issue\"]"));
        assert!(system.contains("Forbidden actions: post GitHub comments"));
        assert!(system.contains("If a required schema field cannot be inferred"));
        assert!(system.contains("Return exactly one JSON object"));
        assert!(!system.contains(TOOL_GITHUB_POST_COMMENT));
        assert!(!system.contains(TOOL_GITHUB_APPROVE_COMMENT));
        assert!(!system.contains(TOOL_DISPATCH_EXECUTE));
        assert!(messages.iter().any(|message| message
            .content
            .contains("Current turn: Search global repositories")));
    }

    #[test]
    fn generated_tool_catalog_matches_allowlist_and_schema_sources() {
        let catalog = allowed_tool_catalog();

        for tool_name in allowed_agent_tool_names() {
            assert!(catalog.contains(tool_name));
            assert!(tool_allowed(tool_name));
        }
        assert!(catalog.contains("Report Issue Finder config"));
        assert!(catalog.contains("\"checkAuth\""));
        assert!(catalog.contains("\"handoffId\""));
        assert!(catalog.contains("\"section\""));
        assert!(catalog.contains("\"runId\""));
        assert!(!catalog.contains(TOOL_GITHUB_POST_COMMENT));
        assert!(!tool_allowed(TOOL_GITHUB_POST_COMMENT));
    }

    #[test]
    fn thread_context_replays_previous_turn_items_before_next_decision() {
        let thread = test_thread("find issues");
        let turn = test_turn(&thread, "draft a tracking comment for the first one");
        let items = vec![
            item(1, "turn-1", "user_message", Some("user"), "search globally"),
            item(
                2,
                "turn-1",
                "tool_result",
                Some("tool"),
                "Observation from issue-finder.scout: owner/repo#1",
            ),
            item(
                3,
                "turn-1",
                "final_answer",
                Some("assistant"),
                "Recommended owner/repo#1",
            ),
            item(
                4,
                "turn-2",
                "user_message",
                Some("user"),
                "draft a tracking comment for the first one",
            ),
        ];

        let messages = build_thread_messages(&thread, &turn, &items, &test_input());
        let joined = messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("owner/repo#1"));
        assert!(joined.contains("Recommended owner/repo#1"));
        assert!(joined.contains("draft a tracking comment"));
    }

    #[test]
    fn safe_followup_tools_are_allowed() {
        assert!(tool_allowed(TOOL_ASSESS));
        assert!(tool_allowed(TOOL_PREPARE));
        assert!(tool_allowed(TOOL_GITHUB_DRAFT_TRACKING_COMMENT));
    }

    #[test]
    fn extracts_scout_candidate_issue_refs() {
        let output = json!({
            "structured_content": {
                "candidates": [
                    {"issue": {"repoFullName": "owner/one", "number": 1}},
                    {"issue": {"repoFullName": "owner/two", "number": 2}}
                ]
            }
        });

        assert_eq!(
            scout_candidate_issue_refs(&output),
            vec!["owner/one#1".to_string(), "owner/two#2".to_string()]
        );
    }

    fn test_input() -> TurnRuntimeInput {
        TurnRuntimeInput {
            thread_goal: "搜索全网仓库并推荐 issue".to_string(),
            user_input: "搜索全网仓库并推荐 issue".to_string(),
            repo: None,
            limit: Some(7),
            refresh: true,
            max_turns: Some(6),
        }
    }

    fn test_thread(goal: &str) -> AgentThread {
        AgentThread {
            id: "thread-1".to_string(),
            goal: goal.to_string(),
            status: AgentThreadStatus::Idle,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
            archived_at: None,
            result: None,
            error: None,
            metadata: json!({}),
        }
    }

    fn test_turn(thread: &AgentThread, input: &str) -> AgentTurn {
        AgentTurn {
            id: "turn-2".to_string(),
            thread_id: thread.id.clone(),
            input: input.to_string(),
            status: AgentTurnStatus::Queued,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
            completed_at: None,
            result: None,
            error: None,
            metadata: json!({}),
        }
    }

    fn item(
        sequence: i64,
        turn_id: &str,
        kind: &str,
        role: Option<&str>,
        content: &str,
    ) -> AgentThreadItem {
        AgentThreadItem {
            sequence,
            id: format!("item-{sequence}"),
            thread_id: "thread-1".to_string(),
            turn_id: Some(turn_id.to_string()),
            kind: kind.to_string(),
            role: role.map(ToOwned::to_owned),
            content: Some(content.to_string()),
            tool_name: None,
            tool_call_id: None,
            payload: json!({}),
            created_at: "now".to_string(),
        }
    }
}
