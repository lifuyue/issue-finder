use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::config::Config;
use crate::paths::IssueFinderPaths;

use super::llm_loop::{allowed_agent_tool_names, run_agent_turn};
use super::model::{
    AgentCardEnvelope, AgentEndpoint, AgentEventsEnvelope, AgentTaskAcceptedEnvelope,
    AgentTaskDetailEnvelope, AgentTaskListEnvelope, AgentTaskSendRequest,
    AgentThreadAcceptedEnvelope, AgentThreadDetailEnvelope, AgentThreadEventsEnvelope,
    AgentThreadListEnvelope, AgentThreadStartRequest, AgentTurnAcceptedEnvelope,
    AgentTurnDetailEnvelope, AgentTurnStartRequest,
};
use super::store::{AgentStore, NewAgentThreadItem};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

pub async fn run_daemon(
    paths: IssueFinderPaths,
    config: Config,
    host: String,
    port: u16,
) -> Result<()> {
    let address = format!("{host}:{port}");
    let listener = TcpListener::bind(&address)
        .await
        .with_context(|| format!("unable to bind agent daemon at {address}"))?;
    println!(
        "{}",
        serde_json::to_string(&json!({
            "kind": "issue_finder_agent_daemon_started",
            "version": 2,
            "address": address,
            "agentCardUrl": format!("http://{address}/a2a/agent-card"),
            "startThreadUrl": format!("http://{address}/a2a/threads/start"),
            "sendTaskUrl": format!("http://{address}/a2a/tasks/send")
        }))?
    );

    loop {
        let (stream, _) = listener.accept().await?;
        let paths = paths.clone();
        let config = config.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, paths, config).await {
                tracing::warn!("agent daemon request failed: {error}");
            }
        });
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    paths: IssueFinderPaths,
    config: Config,
) -> Result<()> {
    let request = read_request(&mut stream).await?;
    let response = route_request(request, paths, config).await;
    match response {
        Ok(response) => write_json(&mut stream, 200, &response).await?,
        Err(HttpError { status, message }) => {
            write_json(
                &mut stream,
                status,
                &json!({
                    "kind": "issue_finder_agent_error",
                    "version": 1,
                    "error": { "message": message }
                }),
            )
            .await?;
        }
    }
    Ok(())
}

async fn route_request(
    request: HttpRequest,
    paths: IssueFinderPaths,
    config: Config,
) -> std::result::Result<Value, HttpError> {
    let path = request
        .path
        .split('?')
        .next()
        .unwrap_or(request.path.as_str());

    match (request.method.as_str(), path) {
        ("GET", "/health") => Ok(json!({
            "kind": "issue_finder_agent_health",
            "version": 2,
            "status": "ok"
        })),
        ("GET", "/a2a/agent-card") => to_value(agent_card()),
        ("GET", "/a2a/threads") => {
            let store = open_store(paths)?;
            let threads = store.list_threads(50).map_err(HttpError::internal)?;
            to_value(AgentThreadListEnvelope {
                kind: "issue_finder_agent_thread_list".to_string(),
                version: 1,
                threads,
            })
        }
        ("POST", "/a2a/threads/start") => {
            let input = serde_json::from_slice::<AgentThreadStartRequest>(&request.body)
                .map_err(|error| HttpError::bad_request(format!("invalid thread JSON: {error}")))?;
            start_thread(paths, config, input).await
        }
        ("GET", "/a2a/tasks") => {
            let store = open_store(paths)?;
            let tasks = store.list_tasks(50).map_err(HttpError::internal)?;
            to_value(AgentTaskListEnvelope {
                kind: "issue_finder_agent_task_list".to_string(),
                version: 1,
                tasks,
            })
        }
        ("POST", "/a2a/tasks/send") => {
            let input = serde_json::from_slice::<AgentTaskSendRequest>(&request.body)
                .map_err(|error| HttpError::bad_request(format!("invalid task JSON: {error}")))?;
            send_legacy_task(paths, config, input).await
        }
        _ if path.starts_with("/a2a/threads/") => {
            route_thread_request(&request.method, path, &request.body, paths, config).await
        }
        _ if path.starts_with("/a2a/tasks/") => route_task_request(&request.method, path, paths),
        _ => Err(HttpError::not_found("unknown agent endpoint")),
    }
}

async fn start_thread(
    paths: IssueFinderPaths,
    config: Config,
    input: AgentThreadStartRequest,
) -> std::result::Result<Value, HttpError> {
    let goal = input.normalized_goal();
    if goal.is_empty() {
        return Err(HttpError::bad_request("goal must not be empty"));
    }
    let turn_request = AgentTurnStartRequest {
        input: goal.clone(),
        repo: input.repo.clone(),
        limit: input.limit,
        refresh: input.refresh,
        max_turns: input.max_turns,
        run_immediately: input.run_immediately,
        metadata: input.metadata.clone(),
    };
    let store = open_store(paths.clone())?;
    let thread = store
        .create_thread(
            &goal,
            json!({
                "input": input,
                "transport": "local_http_a2a"
            }),
        )
        .map_err(HttpError::internal)?;
    let turn = create_turn_record(&store, &thread.id, &goal, turn_request, None)?;
    spawn_turn_if_requested(
        &turn.metadata,
        paths.clone(),
        config,
        thread.id.clone(),
        turn.id.clone(),
    );

    to_value(AgentThreadAcceptedEnvelope {
        kind: "issue_finder_agent_thread_accepted".to_string(),
        version: 1,
        thread: thread.clone(),
        turn: turn.clone(),
        thread_url: format!("/a2a/threads/{}", thread.id),
        turn_url: format!("/a2a/threads/{}/turns/{}", thread.id, turn.id),
        events_url: format!("/a2a/threads/{}/events", thread.id),
    })
}

async fn send_legacy_task(
    paths: IssueFinderPaths,
    config: Config,
    input: AgentTaskSendRequest,
) -> std::result::Result<Value, HttpError> {
    let goal = input.normalized_goal();
    if goal.is_empty() {
        return Err(HttpError::bad_request("goal must not be empty"));
    }
    let store = open_store(paths.clone())?;
    let thread = store
        .create_thread(
            &goal,
            json!({
                "input": input.clone().into_thread_start(),
                "transport": "local_http_a2a",
                "legacyTaskShim": true
            }),
        )
        .map_err(HttpError::internal)?;
    let task = store
        .create_task(
            &goal,
            json!({
                "input": input,
                "transport": "local_http_a2a",
                "threadId": thread.id
            }),
        )
        .map_err(HttpError::internal)?;
    store
        .add_message(&task.id, "user", &goal, json!({"messageType": "task_goal"}))
        .map_err(HttpError::internal)?;
    store
        .add_event(
            &task.id,
            "task_queued",
            "Agent task queued.",
            json!({"goal": goal, "threadId": thread.id}),
        )
        .map_err(HttpError::internal)?;

    let turn_request = AgentTurnStartRequest {
        input: goal.clone(),
        repo: task.metadata["input"]["repo"]
            .as_str()
            .map(ToOwned::to_owned),
        limit: task.metadata["input"]["limit"]
            .as_u64()
            .map(|value| value as usize),
        refresh: task.metadata["input"]["refresh"].as_bool().unwrap_or(false),
        max_turns: task.metadata["input"]["maxTurns"]
            .as_u64()
            .map(|value| value as usize),
        run_immediately: task.metadata["input"]["runImmediately"]
            .as_bool()
            .unwrap_or(true),
        metadata: json!({"legacyTaskId": task.id}),
    };
    let turn = create_turn_record(&store, &thread.id, &goal, turn_request, Some(&task.id))?;
    spawn_turn_if_requested(
        &turn.metadata,
        paths.clone(),
        config,
        thread.id.clone(),
        turn.id.clone(),
    );

    to_value(AgentTaskAcceptedEnvelope {
        kind: "issue_finder_agent_task_accepted".to_string(),
        version: 2,
        task: task.clone(),
        task_url: format!("/a2a/tasks/{}", task.id),
        events_url: format!("/a2a/tasks/{}/events", task.id),
        thread: Some(thread.clone()),
        turn: Some(turn.clone()),
        thread_url: Some(format!("/a2a/threads/{}", thread.id)),
        turn_url: Some(format!("/a2a/threads/{}/turns/{}", thread.id, turn.id)),
    })
}

async fn route_thread_request(
    method: &str,
    path: &str,
    body: &[u8],
    paths: IssueFinderPaths,
    config: Config,
) -> std::result::Result<Value, HttpError> {
    let rest = path
        .strip_prefix("/a2a/threads/")
        .ok_or_else(|| HttpError::not_found("unknown thread endpoint"))?;
    let parts = rest.split('/').collect::<Vec<_>>();
    if parts.is_empty() || parts[0].is_empty() {
        return Err(HttpError::not_found("unknown thread endpoint"));
    }

    match (method, parts.as_slice()) {
        ("GET", [thread_id]) => {
            let store = open_store(paths)?;
            let detail = store.thread_detail(thread_id).map_err(map_not_found)?;
            to_value(AgentThreadDetailEnvelope {
                kind: "issue_finder_agent_thread_detail".to_string(),
                version: 1,
                detail,
            })
        }
        ("GET", [thread_id, "events"]) => {
            let store = open_store(paths)?;
            let events = store
                .list_thread_events(thread_id)
                .map_err(HttpError::internal)?;
            to_value(AgentThreadEventsEnvelope {
                kind: "issue_finder_agent_thread_events".to_string(),
                version: 1,
                thread_id: (*thread_id).to_string(),
                events,
            })
        }
        ("POST", [thread_id, "turns", "start"]) => {
            let input = serde_json::from_slice::<AgentTurnStartRequest>(body)
                .map_err(|error| HttpError::bad_request(format!("invalid turn JSON: {error}")))?;
            start_turn(paths, config, thread_id, input).await
        }
        ("GET", [thread_id, "turns", turn_id]) => {
            let store = open_store(paths)?;
            let detail = store.turn_detail(turn_id).map_err(map_not_found)?;
            if detail.thread.id != *thread_id {
                return Err(HttpError::not_found("turn does not belong to thread"));
            }
            to_value(AgentTurnDetailEnvelope {
                kind: "issue_finder_agent_turn_detail".to_string(),
                version: 1,
                detail,
            })
        }
        _ => Err(HttpError::not_found("unknown thread endpoint")),
    }
}

async fn start_turn(
    paths: IssueFinderPaths,
    config: Config,
    thread_id: &str,
    input: AgentTurnStartRequest,
) -> std::result::Result<Value, HttpError> {
    let turn_input = input.normalized_input();
    if turn_input.is_empty() {
        return Err(HttpError::bad_request("input must not be empty"));
    }
    let store = open_store(paths.clone())?;
    let thread = store.get_thread(thread_id).map_err(map_not_found)?;
    if !thread.status.accepts_turns() {
        return Err(HttpError::bad_request(format!(
            "thread {} does not accept turns while status={}",
            thread.id,
            thread.status.as_str()
        )));
    }
    let turn = create_turn_record(&store, &thread.id, &turn_input, input, None)?;
    spawn_turn_if_requested(
        &turn.metadata,
        paths.clone(),
        config,
        thread.id.clone(),
        turn.id.clone(),
    );

    to_value(AgentTurnAcceptedEnvelope {
        kind: "issue_finder_agent_turn_accepted".to_string(),
        version: 1,
        thread: thread.clone(),
        turn: turn.clone(),
        thread_url: format!("/a2a/threads/{}", thread.id),
        turn_url: format!("/a2a/threads/{}/turns/{}", thread.id, turn.id),
        events_url: format!("/a2a/threads/{}/events", thread.id),
    })
}

fn create_turn_record(
    store: &AgentStore,
    thread_id: &str,
    input_text: &str,
    input: AgentTurnStartRequest,
    legacy_task_id: Option<&str>,
) -> std::result::Result<super::model::AgentTurn, HttpError> {
    let mut metadata = json!({
        "input": input,
        "transport": "local_http_a2a"
    });
    if let Some(task_id) = legacy_task_id {
        metadata["legacyTaskId"] = json!(task_id);
    }
    let turn = store
        .create_turn(thread_id, input_text, metadata)
        .map_err(HttpError::internal)?;
    store
        .add_thread_item(NewAgentThreadItem {
            id: None,
            thread_id,
            turn_id: Some(&turn.id),
            kind: "user_message",
            role: Some("user"),
            content: Some(input_text),
            tool_name: None,
            tool_call_id: None,
            payload: json!({
                "messageType": "turn_input",
                "legacyTaskId": legacy_task_id
            }),
        })
        .map_err(HttpError::internal)?;
    store
        .add_thread_event(
            thread_id,
            Some(&turn.id),
            "turn_queued",
            "Agent turn queued.",
            json!({"input": input_text, "legacyTaskId": legacy_task_id}),
        )
        .map_err(HttpError::internal)?;
    Ok(turn)
}

fn spawn_turn_if_requested(
    turn_metadata: &Value,
    paths: IssueFinderPaths,
    config: Config,
    thread_id: String,
    turn_id: String,
) {
    if turn_metadata["input"]["runImmediately"]
        .as_bool()
        .unwrap_or(true)
    {
        tokio::spawn(async move {
            if let Err(error) =
                run_agent_turn(paths, config, thread_id.clone(), turn_id.clone()).await
            {
                tracing::warn!("agent turn {thread_id}/{turn_id} failed: {error}");
            }
        });
    }
}

fn route_task_request(
    method: &str,
    path: &str,
    paths: IssueFinderPaths,
) -> std::result::Result<Value, HttpError> {
    let Some(rest) = path.strip_prefix("/a2a/tasks/") else {
        return Err(HttpError::not_found("unknown agent endpoint"));
    };
    let store = open_store(paths)?;

    if method == "GET" && rest.ends_with("/events") {
        let task_id = rest.trim_end_matches("/events");
        let events = store.list_events(task_id).map_err(HttpError::internal)?;
        return to_value(AgentEventsEnvelope {
            kind: "issue_finder_agent_events".to_string(),
            version: 1,
            task_id: task_id.to_string(),
            events,
        });
    }

    if method == "GET" {
        let detail = store.detail(rest).map_err(map_not_found)?;
        return to_value(AgentTaskDetailEnvelope {
            kind: "issue_finder_agent_task_detail".to_string(),
            version: 1,
            detail,
        });
    }

    Err(HttpError::not_found("unknown agent endpoint"))
}

fn agent_card() -> AgentCardEnvelope {
    AgentCardEnvelope {
        kind: "issue_finder_agent_card".to_string(),
        version: 2,
        name: "Issue Finder Agent".to_string(),
        description:
            "Local Issue Finder daemon that accepts resumable natural-language threads and runs safe Issue Finder tools through an LLM loop."
                .to_string(),
        endpoints: vec![
            AgentEndpoint {
                method: "GET".to_string(),
                path: "/a2a/agent-card".to_string(),
                description: "Describe daemon capabilities.".to_string(),
            },
            AgentEndpoint {
                method: "POST".to_string(),
                path: "/a2a/threads/start".to_string(),
                description: "Create a persistent Issue Finder agent thread and first turn."
                    .to_string(),
            },
            AgentEndpoint {
                method: "POST".to_string(),
                path: "/a2a/threads/{threadId}/turns/start".to_string(),
                description: "Append a user turn to an existing thread.".to_string(),
            },
            AgentEndpoint {
                method: "GET".to_string(),
                path: "/a2a/threads/{threadId}".to_string(),
                description: "Read thread state, turns, transcript items, and events.".to_string(),
            },
            AgentEndpoint {
                method: "GET".to_string(),
                path: "/a2a/threads/{threadId}/events".to_string(),
                description: "Read ordered thread events.".to_string(),
            },
            AgentEndpoint {
                method: "POST".to_string(),
                path: "/a2a/tasks/send".to_string(),
                description: "Compatibility endpoint that creates a thread plus first turn."
                    .to_string(),
            },
        ],
        input_modes: vec!["natural_language".to_string()],
        output_modes: vec!["json".to_string(), "natural_language".to_string()],
        tools: allowed_agent_tool_names()
            .iter()
            .map(|tool| (*tool).to_string())
            .collect(),
    }
}

async fn read_request(stream: &mut TcpStream) -> Result<HttpRequest> {
    let mut buffer = Vec::new();
    let mut header_end = None;

    while header_end.is_none() {
        if buffer.len() > MAX_HEADER_BYTES {
            anyhow::bail!("HTTP request headers are too large");
        }
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            anyhow::bail!("connection closed before HTTP headers");
        }
        buffer.extend_from_slice(&chunk[..read]);
        header_end = find_header_end(&buffer);
    }

    let header_end = header_end.expect("checked");
    let header_text =
        std::str::from_utf8(&buffer[..header_end]).context("HTTP headers must be valid UTF-8")?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().context("missing HTTP request line")?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .context("missing HTTP method")?
        .to_string();
    let path = request_parts
        .next()
        .context("missing HTTP path")?
        .to_string();
    let mut content_length = 0_usize;
    for line in lines {
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = value
                .trim()
                .parse::<usize>()
                .context("invalid Content-Length")?;
        } else if let Some(value) = line.strip_prefix("content-length:") {
            content_length = value
                .trim()
                .parse::<usize>()
                .context("invalid Content-Length")?;
        }
    }
    if content_length > MAX_BODY_BYTES {
        anyhow::bail!("HTTP request body is too large");
    }

    let body_start = header_end + 4;
    let mut body = buffer[body_start..].to_vec();
    while body.len() < content_length {
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            anyhow::bail!("connection closed before HTTP body");
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);

    Ok(HttpRequest { method, path, body })
}

async fn write_json(stream: &mut TcpStream, status: u16, value: &impl Serialize) -> Result<()> {
    let body = serde_json::to_string(value)?;
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn open_store(paths: IssueFinderPaths) -> std::result::Result<AgentStore, HttpError> {
    AgentStore::open(paths).map_err(HttpError::internal)
}

fn to_value<T: Serialize>(value: T) -> std::result::Result<Value, HttpError> {
    serde_json::to_value(value).map_err(HttpError::internal)
}

fn map_not_found(error: anyhow::Error) -> HttpError {
    if error.to_string().contains("not found") {
        HttpError::not_found(error.to_string())
    } else {
        HttpError::internal(error)
    }
}

#[derive(Debug)]
struct HttpError {
    status: u16,
    message: String,
}

impl HttpError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: 404,
            message: message.into(),
        }
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        Self {
            status: 500,
            message: error.to_string(),
        }
    }
}
