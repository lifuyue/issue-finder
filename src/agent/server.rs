use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::config::Config;
use crate::paths::IssueFinderPaths;

use super::llm_loop::run_agent_task;
use super::model::{
    AgentCardEnvelope, AgentEndpoint, AgentEventsEnvelope, AgentTaskAcceptedEnvelope,
    AgentTaskDetailEnvelope, AgentTaskListEnvelope, AgentTaskSendRequest,
};
use super::store::AgentStore;

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
            "version": 1,
            "address": address,
            "agentCardUrl": format!("http://{address}/a2a/agent-card"),
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
            "version": 1,
            "status": "ok"
        })),
        ("GET", "/a2a/agent-card") => to_value(agent_card()),
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
            let goal = input.normalized_goal();
            if goal.is_empty() {
                return Err(HttpError::bad_request("goal must not be empty"));
            }
            let store = open_store(paths.clone())?;
            let task = store
                .create_task(
                    &goal,
                    json!({
                        "input": input,
                        "transport": "local_http_a2a"
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
                    json!({"goal": goal}),
                )
                .map_err(HttpError::internal)?;

            if task.metadata["input"]["runImmediately"]
                .as_bool()
                .unwrap_or(true)
            {
                let task_id = task.id.clone();
                let paths_for_task = paths.clone();
                let config_for_task = config.clone();
                tokio::spawn(async move {
                    if let Err(error) =
                        run_agent_task(paths_for_task, config_for_task, task_id.clone()).await
                    {
                        tracing::warn!("agent task {task_id} failed: {error}");
                    }
                });
            }

            to_value(AgentTaskAcceptedEnvelope {
                kind: "issue_finder_agent_task_accepted".to_string(),
                version: 1,
                task: task.clone(),
                task_url: format!("/a2a/tasks/{}", task.id),
                events_url: format!("/a2a/tasks/{}/events", task.id),
            })
        }
        _ => route_task_request(&request.method, path, paths),
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
        let detail = store.detail(rest).map_err(|error| {
            if error.to_string().contains("not found") {
                HttpError::not_found(error.to_string())
            } else {
                HttpError::internal(error)
            }
        })?;
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
        version: 1,
        name: "Issue Finder Agent".to_string(),
        description:
            "Local Issue Finder daemon that accepts natural-language discovery goals and runs read-only Issue Finder tools through an LLM loop."
                .to_string(),
        endpoints: vec![
            AgentEndpoint {
                method: "GET".to_string(),
                path: "/a2a/agent-card".to_string(),
                description: "Describe daemon capabilities.".to_string(),
            },
            AgentEndpoint {
                method: "POST".to_string(),
                path: "/a2a/tasks/send".to_string(),
                description: "Queue a natural-language agent task.".to_string(),
            },
            AgentEndpoint {
                method: "GET".to_string(),
                path: "/a2a/tasks/{taskId}".to_string(),
                description: "Read task state, messages, tool calls, and events.".to_string(),
            },
            AgentEndpoint {
                method: "GET".to_string(),
                path: "/a2a/tasks/{taskId}/events".to_string(),
                description: "Read ordered task events.".to_string(),
            },
        ],
        input_modes: vec!["natural_language".to_string()],
        output_modes: vec!["json".to_string(), "natural_language".to_string()],
        tools: vec![
            "issue-finder.status".to_string(),
            "issue-finder.scout".to_string(),
        ],
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
