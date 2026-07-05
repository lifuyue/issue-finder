use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Map;
use serde_json::{json, Value};

use crate::config::Config;

const AGENT_LLM_TIMEOUT: Duration = Duration::from_secs(60);
const ISSUE_FINDER_NAMESPACE: &str = "issue-finder";

static CHAT_CALL_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentWireApi {
    Responses,
    ChatCompletions,
}

impl AgentWireApi {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim() {
            "responses" => Ok(Self::Responses),
            "chat_completions" | "chat-completions" | "chat" => Ok(Self::ChatCompletions),
            other => anyhow::bail!(
                "unsupported llm.wire_api {other:?}; expected responses or chat_completions"
            ),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::ChatCompletions => "chat_completions",
        }
    }

    pub fn supports_native_tools(self) -> bool {
        matches!(self, Self::Responses)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelRequest {
    pub model: String,
    pub instructions: String,
    pub input_items: Vec<AgentModelItem>,
    pub tools: Vec<Value>,
    pub parallel_tool_calls: bool,
    pub output_schema: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentModelResponse {
    pub items: Vec<AgentModelItem>,
    pub usage: Option<Value>,
    pub raw_provider_response: Value,
}

impl AgentModelResponse {
    pub fn tool_calls(&self) -> Vec<AgentToolCall> {
        self.items
            .iter()
            .filter_map(|item| match item {
                AgentModelItem::ToolCall(call) => Some(call.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn final_assistant_message(&self) -> Option<String> {
        self.items.iter().find_map(|item| match item {
            AgentModelItem::AssistantMessage { content } if !content.trim().is_empty() => {
                Some(content.trim().to_string())
            }
            _ => None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentModelItem {
    SystemMessage { content: String },
    UserMessage { content: String },
    AssistantMessage { content: String },
    ToolCall(AgentToolCall),
    FunctionCallOutput { call_id: String, output: Value },
    ReasoningSummary { content: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolCall {
    pub call_id: String,
    pub namespace: Option<String>,
    pub name: String,
    pub arguments: Value,
    #[serde(default)]
    pub provider_item_id: Option<String>,
    #[serde(default)]
    pub provider_call_id_source: Option<String>,
}

impl AgentToolCall {
    pub fn canonical_name(&self) -> String {
        self.namespace
            .as_deref()
            .map(|namespace| format!("{namespace}.{}", self.name))
            .unwrap_or_else(|| self.name.clone())
    }
}

pub trait AgentLlmClient: Send + Sync {
    fn sample_turn<'a>(
        &'a self,
        request: AgentModelRequest,
    ) -> BoxFuture<'a, Result<AgentModelResponse>>;

    fn wire_api(&self) -> AgentWireApi;

    fn supports_native_tools(&self) -> bool {
        self.wire_api().supports_native_tools()
    }
}

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleAgentLlmClient {
    config: Config,
    client: reqwest::Client,
    wire_api: AgentWireApi,
}

impl OpenAiCompatibleAgentLlmClient {
    pub fn new(config: Config) -> Result<Self> {
        let wire_api = AgentWireApi::parse(&config.llm.wire_api)?;
        let client = reqwest::Client::builder()
            .user_agent("issue-finder-agent")
            .timeout(AGENT_LLM_TIMEOUT)
            .build()?;
        Ok(Self {
            config,
            client,
            wire_api,
        })
    }

    async fn sample_turn_inner(&self, request: AgentModelRequest) -> Result<AgentModelResponse> {
        if !self.config.llm.enabled {
            anyhow::bail!("LLM agent mode requires llm.enabled=true");
        }
        let api_key = self.config.resolved_llm_api_key();
        if api_key.trim().is_empty() {
            anyhow::bail!("LLM agent mode requires an LLM API key");
        }

        match self.wire_api {
            AgentWireApi::Responses => self.sample_responses(request, api_key).await,
            AgentWireApi::ChatCompletions => self.sample_chat_completions(request, api_key).await,
        }
    }

    async fn sample_responses(
        &self,
        request: AgentModelRequest,
        api_key: String,
    ) -> Result<AgentModelResponse> {
        let url = format!(
            "{}/responses",
            self.config.llm.base_url.trim_end_matches('/')
        );
        let request_json = responses_request_json(&request);
        let response = self
            .client
            .post(url)
            .bearer_auth(api_key.trim())
            .json(&request_json)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "LLM agent request failed with {status} using wire_api={}: {body}",
                self.wire_api.as_str()
            );
        }

        let raw = response.json::<Value>().await?;
        responses_value_to_model_response(raw)
    }

    async fn sample_chat_completions(
        &self,
        request: AgentModelRequest,
        api_key: String,
    ) -> Result<AgentModelResponse> {
        let url = format!(
            "{}/chat/completions",
            self.config.llm.base_url.trim_end_matches('/')
        );
        let request_json = chat_completions_request_json(&request);
        let response = self
            .client
            .post(url)
            .bearer_auth(api_key.trim())
            .json(&request_json)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "LLM agent request failed with {status} using wire_api={}: {body}",
                self.wire_api.as_str()
            );
        }

        let raw = response.json::<Value>().await?;
        chat_value_to_model_response(raw)
    }
}

impl AgentLlmClient for OpenAiCompatibleAgentLlmClient {
    fn sample_turn<'a>(
        &'a self,
        request: AgentModelRequest,
    ) -> BoxFuture<'a, Result<AgentModelResponse>> {
        Box::pin(async move { self.sample_turn_inner(request).await })
    }

    fn wire_api(&self) -> AgentWireApi {
        self.wire_api
    }
}

pub fn responses_request_json(request: &AgentModelRequest) -> Value {
    let mut body = json!({
        "model": request.model,
        "instructions": request.instructions,
        "input": request.input_items.iter().map(model_item_to_responses_input).collect::<Vec<_>>(),
        "tools": request.tools,
        "tool_choice": "auto",
        "parallel_tool_calls": request.parallel_tool_calls,
        "store": false,
        "stream": false,
        "include": []
    });

    if let Some(schema) = request.output_schema.as_ref() {
        body.as_object_mut()
            .expect("responses request body is an object")
            .insert(
                "text".to_string(),
                json!({
                    "format": {
                        "type": "json_schema",
                        "name": "issue_finder_agent_output",
                        "strict": false,
                        "schema": schema
                    }
                }),
            );
    }

    body
}

fn chat_completions_request_json(request: &AgentModelRequest) -> Value {
    json!({
        "model": request.model,
        "temperature": 0.1,
        "messages": model_items_to_chat_messages(request),
    })
}

fn model_item_to_responses_input(item: &AgentModelItem) -> Value {
    match item {
        AgentModelItem::SystemMessage { content } => {
            json!({"type": "message", "role": "system", "content": content})
        }
        AgentModelItem::UserMessage { content } => {
            json!({"type": "message", "role": "user", "content": content})
        }
        AgentModelItem::AssistantMessage { content } => {
            json!({"type": "message", "role": "assistant", "content": content})
        }
        AgentModelItem::ToolCall(call) => json!({
            "type": "function_call",
            "call_id": call.call_id,
            "namespace": call.namespace,
            "name": call.name,
            "arguments": serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_string())
        }),
        AgentModelItem::FunctionCallOutput { call_id, output } => json!({
            "type": "function_call_output",
            "call_id": call_id,
            "output": model_visible_output_text(output)
        }),
        AgentModelItem::ReasoningSummary { content } => {
            json!({"type": "message", "role": "assistant", "content": content})
        }
    }
}

fn model_items_to_chat_messages(request: &AgentModelRequest) -> Vec<Value> {
    let mut messages = vec![json!({
        "role": "system",
        "content": chat_compat_instructions(request)
    })];

    for item in &request.input_items {
        match item {
            AgentModelItem::SystemMessage { content } => {
                messages.push(json!({"role": "system", "content": content}));
            }
            AgentModelItem::UserMessage { content } => {
                messages.push(json!({"role": "user", "content": content}));
            }
            AgentModelItem::AssistantMessage { content } => {
                messages.push(json!({"role": "assistant", "content": content}));
            }
            AgentModelItem::ToolCall(call) => {
                messages.push(json!({
                    "role": "assistant",
                    "content": serde_json::to_string(&json!({
                        "tool": call.canonical_name(),
                        "arguments": call.arguments,
                        "finalAnswer": null
                    })).unwrap_or_else(|_| "{}".to_string())
                }));
            }
            AgentModelItem::FunctionCallOutput { call_id: _, output } => {
                messages.push(json!({
                    "role": "user",
                    "content": format!(
                        "Observation from tool call:\n{}\nReturn the next JSON decision. If the goal is answered, finalAnswer must summarize the recommended issues.",
                        model_visible_output_text(output)
                    )
                }));
            }
            AgentModelItem::ReasoningSummary { content } => {
                messages.push(json!({"role": "assistant", "content": content}));
            }
        }
    }

    messages
}

fn chat_compat_instructions(request: &AgentModelRequest) -> String {
    format!(
        "{}\n\nThis provider is using chat_completions compatibility mode. Native tools are unavailable, so choose tools by returning exactly one JSON object with fields tool, arguments, finalAnswer, and rationale. Available direct tool schemas:\n{}\n\nTo finish, return {{\"tool\":null,\"arguments\":{{}},\"finalAnswer\":\"answer grounded in tool observations\",\"rationale\":\"why the result answers the user\"}}.",
        request.instructions,
        serde_json::to_string(&request.tools).unwrap_or_else(|_| "[]".to_string())
    )
}

fn responses_value_to_model_response(raw: Value) -> Result<AgentModelResponse> {
    let mut items = Vec::new();
    for item in raw
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(parsed) = parse_responses_output_item(item)? {
            items.push(parsed);
        }
    }

    if items.is_empty() {
        if let Some(text) = raw
            .get("output_text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            items.push(AgentModelItem::AssistantMessage {
                content: text.to_string(),
            });
        }
    }

    Ok(AgentModelResponse {
        items,
        usage: raw.get("usage").cloned(),
        raw_provider_response: sanitize_provider_response(raw),
    })
}

fn parse_responses_output_item(item: &Value) -> Result<Option<AgentModelItem>> {
    match item.get("type").and_then(Value::as_str) {
        Some("function_call") => {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .context("Responses function_call item missing name")?
                .to_string();
            let namespace = item
                .get("namespace")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let arguments = item
                .get("arguments")
                .and_then(Value::as_str)
                .map(parse_arguments_string)
                .transpose()?
                .unwrap_or_else(|| json!({}));
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .context("Responses function_call item missing call_id")?
                .to_string();
            let provider_item_id = item
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            Ok(Some(AgentModelItem::ToolCall(AgentToolCall {
                call_id,
                namespace,
                name,
                arguments,
                provider_item_id,
                provider_call_id_source: Some("provider_responses".to_string()),
            })))
        }
        Some("message") => {
            Ok(message_item_text(item).map(|content| AgentModelItem::AssistantMessage { content }))
        }
        Some("reasoning") => {
            Ok(message_item_text(item).map(|content| AgentModelItem::ReasoningSummary { content }))
        }
        _ => Ok(None),
    }
}

fn message_item_text(item: &Value) -> Option<String> {
    if let Some(content) = item.get("content").and_then(Value::as_str) {
        return Some(content.trim().to_string()).filter(|content| !content.is_empty());
    }

    let text = item
        .get("content")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|part| {
            part.get("text")
                .or_else(|| part.get("content"))
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("");
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

fn chat_value_to_model_response(raw: Value) -> Result<AgentModelResponse> {
    let content = raw
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let decision = parse_chat_agent_decision(&content)
        .with_context(|| format!("LLM returned an invalid agent decision: {content}"))?;

    let mut items = Vec::new();
    if let Some(final_answer) = normalized_optional(decision.final_answer.as_deref()) {
        items.push(AgentModelItem::AssistantMessage {
            content: final_answer,
        });
    } else if let Some(tool_name) = normalized_optional(decision.tool.as_deref()) {
        let (namespace, name) = split_tool_name(&tool_name);
        items.push(AgentModelItem::AssistantMessage { content });
        items.push(AgentModelItem::ToolCall(AgentToolCall {
            call_id: next_chat_call_id(),
            namespace,
            name,
            arguments: decision.arguments.unwrap_or_else(|| json!({})),
            provider_item_id: None,
            provider_call_id_source: Some("synthesized_chat_fallback".to_string()),
        }));
    } else {
        anyhow::bail!("LLM decision did not include a tool or finalAnswer");
    }

    Ok(AgentModelResponse {
        items,
        usage: raw.get("usage").cloned(),
        raw_provider_response: sanitize_provider_response(raw),
    })
}

fn parse_arguments_string(raw: &str) -> Result<Value> {
    let value = serde_json::from_str::<Value>(raw)
        .with_context(|| format!("function call arguments must be valid JSON: {raw}"))?;
    if !value.is_object() {
        anyhow::bail!("function call arguments must be a JSON object");
    }
    Ok(value)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAgentDecision {
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub arguments: Option<Value>,
    #[serde(default, alias = "final")]
    pub final_answer: Option<String>,
    #[serde(default)]
    pub rationale: Option<String>,
}

pub fn parse_chat_agent_decision(raw: &str) -> Result<ChatAgentDecision> {
    let json_text =
        extract_json_object(raw).context("agent decision must contain a JSON object")?;
    let decision = serde_json::from_str::<ChatAgentDecision>(&json_text)?;
    if let Some(arguments) = decision.arguments.as_ref() {
        if !arguments.is_object() {
            anyhow::bail!("agent decision arguments must be a JSON object");
        }
    }
    Ok(decision)
}

fn split_tool_name(tool_name: &str) -> (Option<String>, String) {
    if let Some((namespace, name)) = tool_name.rsplit_once('.') {
        if namespace == ISSUE_FINDER_NAMESPACE {
            return (Some(namespace.to_string()), name.to_string());
        }
    }
    (None, tool_name.to_string())
}

fn normalized_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn next_chat_call_id() -> String {
    let counter = CHAT_CALL_COUNTER.fetch_add(1, Ordering::Relaxed);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("chat-call-{millis}-{counter}")
}

fn model_visible_output_text(output: &Value) -> String {
    if let Some(text) = output.as_str() {
        return text.to_string();
    }
    serde_json::to_string(output).unwrap_or_else(|_| "{}".to_string())
}

fn extract_json_object(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if let Some(object) = first_json_object(trimmed) {
        return Some(object);
    }

    if let Some(start) = trimmed.find("```") {
        let after_start = &trimmed[start + 3..];
        let after_language = after_start
            .strip_prefix("json")
            .unwrap_or(after_start)
            .trim_start_matches(['\n', '\r', ' ']);
        if let Some(end) = after_language.find("```") {
            let fenced = after_language[..end].trim();
            if let Some(object) = first_json_object(fenced) {
                return Some(object);
            }
        }
    }

    first_json_object_after_prefix(trimmed)
}

fn first_json_object(raw: &str) -> Option<String> {
    let mut stream = serde_json::Deserializer::from_str(raw).into_iter::<Value>();
    match stream.next() {
        Some(Ok(Value::Object(_))) => {
            let end = stream.byte_offset();
            Some(raw[..end].trim().to_string())
        }
        _ => None,
    }
}

fn first_json_object_after_prefix(raw: &str) -> Option<String> {
    raw.char_indices()
        .filter(|(_, char)| *char == '{')
        .find_map(|(start, _)| first_json_object(&raw[start..]))
}

fn sanitize_provider_response(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(sanitize_provider_object(object)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(sanitize_provider_response)
                .collect::<Vec<_>>(),
        ),
        other => other,
    }
}

fn sanitize_provider_object(object: Map<String, Value>) -> Map<String, Value> {
    object
        .into_iter()
        .filter_map(|(key, value)| {
            if matches!(
                key.as_str(),
                "reasoning_content" | "reasoning_details" | "chain_of_thought"
            ) {
                return None;
            }
            Some((key, sanitize_provider_response(value)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        chat_value_to_model_response, parse_chat_agent_decision, responses_request_json,
        responses_value_to_model_response, AgentModelItem, AgentModelRequest, AgentWireApi,
    };
    use crate::agent::tool_registry::AgentToolRegistry;

    #[test]
    fn parses_wire_api_values() {
        assert_eq!(
            AgentWireApi::parse("responses").unwrap(),
            AgentWireApi::Responses
        );
        assert_eq!(
            AgentWireApi::parse("chat_completions").unwrap(),
            AgentWireApi::ChatCompletions
        );
        assert!(AgentWireApi::parse("other").is_err());
    }

    #[test]
    fn responses_request_contains_native_tools_without_json_prompt_examples() {
        let registry = AgentToolRegistry::issue_finder_default();
        let request = AgentModelRequest {
            model: "test-model".to_string(),
            instructions: "Use native tools. Do not invent issue data.".to_string(),
            input_items: vec![AgentModelItem::UserMessage {
                content: "find issues".to_string(),
            }],
            tools: registry.model_visible_responses_tools(),
            parallel_tool_calls: false,
            output_schema: None,
        };

        let value = responses_request_json(&request);
        assert_eq!(value["tools"][0]["type"], "namespace");
        assert_eq!(value["tools"][0]["tools"][0]["type"], "function");
        assert!(value.get("text").is_none());
        assert!(!value["instructions"]
            .as_str()
            .unwrap()
            .contains(r#""tool":"issue-finder"#));
    }

    #[test]
    fn chat_fallback_maps_json_decision_to_tool_call() {
        let response = chat_value_to_model_response(json!({
            "choices": [{
                "message": {
                    "content": "{\"tool\":\"issue-finder.discover_candidates\",\"arguments\":{\"limit\":3},\"finalAnswer\":null}"
                }
            }]
        }))
        .unwrap();

        let calls = response.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].namespace.as_deref(), Some("issue-finder"));
        assert_eq!(calls[0].name, "discover_candidates");
        assert_eq!(calls[0].arguments["limit"], 3);
        assert!(calls[0].call_id.starts_with("chat-call-"));
        assert_eq!(
            calls[0].provider_call_id_source.as_deref(),
            Some("synthesized_chat_fallback")
        );
    }

    #[test]
    fn responses_function_call_keeps_provider_call_id_and_namespace() {
        let response = responses_value_to_model_response(json!({
            "output": [{
                "type": "function_call",
                "id": "fc_123",
                "call_id": "call_123",
                "namespace": "issue-finder",
                "name": "inspect_candidate",
                "arguments": "{\"issue\":\"owner/repo#1\"}"
            }]
        }))
        .unwrap();

        let calls = response.tool_calls();
        assert_eq!(calls[0].call_id, "call_123");
        assert_eq!(calls[0].provider_item_id.as_deref(), Some("fc_123"));
        assert_eq!(calls[0].canonical_name(), "issue-finder.inspect_candidate");
    }

    #[test]
    fn parses_fenced_chat_agent_decision() {
        let decision = parse_chat_agent_decision(
            r#"```json
{"tool":"issue-finder.discover_candidates","arguments":{"limit":3},"finalAnswer":null}
```"#,
        )
        .unwrap();

        assert_eq!(
            decision.tool.as_deref(),
            Some("issue-finder.discover_candidates")
        );
        assert_eq!(decision.arguments.unwrap()["limit"], 3);
    }

    #[test]
    fn parses_first_valid_json_decision_when_provider_duplicates_objects() {
        let decision = parse_chat_agent_decision(
            r#"{"tool":"issue-finder.inspect_candidate","arguments":{"issue":"owner/repo#1"},"finalAnswer":""}{"tool":"issue-finder.inspect_candidate","arguments":{"issue":"owner/repo#1"},"finalAnswer":null}"#,
        )
        .unwrap();

        assert_eq!(
            decision.tool.as_deref(),
            Some("issue-finder.inspect_candidate")
        );
        assert_eq!(decision.arguments.unwrap()["issue"], "owner/repo#1");
    }

    #[test]
    fn raw_provider_response_drops_hidden_reasoning_content() {
        let response = chat_value_to_model_response(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "{\"tool\":null,\"arguments\":{},\"finalAnswer\":\"done\"}",
                    "reasoning_content": "hidden chain of thought"
                }
            }]
        }))
        .unwrap();

        assert!(response
            .raw_provider_response
            .pointer("/choices/0/message/reasoning_content")
            .is_none());
        assert_eq!(response.final_assistant_message().as_deref(), Some("done"));
    }
}
