use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::config::Config;
use crate::paths::IssueFinderPaths;

use super::context::{compact_thread_context_now, AgentContextBudget};
use super::llm_client::{AgentLlmClient, AgentWireApi, OpenAiCompatibleAgentLlmClient};
use super::llm_loop::{run_agent_task, run_agent_turn};
use super::model::{
    AgentCapability, AgentCardEnvelope, AgentEndpoint, AgentEventsEnvelope,
    AgentProviderCapabilities, AgentTaskAcceptedEnvelope, AgentTaskDetailEnvelope,
    AgentTaskListEnvelope, AgentTaskSendRequest, AgentTaskStatus, AgentThreadAcceptedEnvelope,
    AgentThreadDetailEnvelope, AgentThreadEventsEnvelope, AgentThreadInjectRequest,
    AgentThreadListEnvelope, AgentThreadStartRequest, AgentThreadStatus, AgentThreadTurnRequest,
};
use super::runtime::{approve_agent_approval_request, reject_agent_approval_request};
use super::store::{AgentStore, NewAgentThreadItem};
use super::tool_registry::{AgentToolExposure, AgentToolRegistry};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_SUBSCRIBE_REPLAY_EVENTS: usize = 256;

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
            "sendTaskUrl": format!("http://{address}/a2a/tasks/send"),
            "startThreadUrl": format!("http://{address}/a2a/threads/start")
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
    let full_path = request.path.clone();
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
        ("GET", "/a2a/agent-card") => to_value(agent_card(&config)),
        ("GET", "/a2a/tasks") => {
            let store = open_store(paths)?;
            let tasks = store.list_tasks(50).map_err(HttpError::internal)?;
            to_value(AgentTaskListEnvelope {
                kind: "issue_finder_agent_task_list".to_string(),
                version: 1,
                tasks,
            })
        }
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
                        "transport": "local_http_a2a",
                        "legacy": true,
                        "threadRef": null
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
        _ if path.starts_with("/a2a/threads/") => {
            route_thread_request(
                &request.method,
                &full_path,
                path,
                request.body,
                paths,
                config,
            )
            .await
        }
        _ => route_task_request(&request.method, path, paths),
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
    let turn_input = input.turn_input();
    let title = input.normalized_title();
    let store = open_store(paths.clone())?;
    let thread = store
        .create_thread(
            &goal,
            &title,
            json!({
                "input": input,
                "transport": "local_http_a2a"
            }),
        )
        .map_err(HttpError::internal)?;
    store
        .add_thread_event(
            &thread.id,
            None,
            "thread_started",
            "Agent thread started.",
            json!({"goal": goal}),
        )
        .map_err(HttpError::internal)?;
    let turn = create_thread_turn(&store, &thread.id, turn_input).map_err(HttpError::internal)?;

    maybe_spawn_turn(&turn.id, &turn.metadata, paths, config);

    let thread = store.get_thread(&thread.id).map_err(HttpError::internal)?;
    to_value(thread_accepted_envelope(thread, turn, None))
}

async fn route_thread_request(
    method: &str,
    full_path: &str,
    path: &str,
    body: Vec<u8>,
    paths: IssueFinderPaths,
    config: Config,
) -> std::result::Result<Value, HttpError> {
    let Some(rest) = path.strip_prefix("/a2a/threads/") else {
        return Err(HttpError::not_found("unknown agent thread endpoint"));
    };

    if method == "POST" && rest.ends_with("/turns/send") {
        let thread_id = rest.trim_end_matches("/turns/send");
        let input = serde_json::from_slice::<AgentThreadTurnRequest>(&body)
            .map_err(|error| HttpError::bad_request(format!("invalid turn JSON: {error}")))?;
        let normalized = input.normalized_input();
        if normalized.is_empty() {
            return Err(HttpError::bad_request("input must not be empty"));
        }
        let store = open_store(paths.clone())?;
        let thread = store.get_thread(thread_id).map_err(|error| {
            if error.to_string().contains("not found") {
                HttpError::not_found(error.to_string())
            } else {
                HttpError::internal(error)
            }
        })?;
        if thread.status == AgentThreadStatus::Running {
            let turn_id = thread
                .last_turn_id
                .clone()
                .ok_or_else(|| HttpError::bad_request("running thread has no active turn"))?;
            let mailbox = store
                .enqueue_mailbox_item(
                    thread_id,
                    Some(&turn_id),
                    "steer",
                    &normalized,
                    json!({
                        "input": input,
                        "source": "turns_send_running_thread"
                    }),
                )
                .map_err(HttpError::internal)?;
            store
                .add_thread_event(
                    thread_id,
                    Some(&turn_id),
                    "mailbox_item_queued",
                    "Queued running-turn steer input.",
                    json!({
                        "mailboxItemId": mailbox.id,
                        "delivery": mailbox.delivery,
                        "input": normalized
                    }),
                )
                .map_err(HttpError::internal)?;
            let turn = store.get_turn(&turn_id).map_err(HttpError::internal)?;
            return to_value(thread_accepted_envelope(thread, turn, Some(mailbox)));
        }

        let turn = create_thread_turn(&store, thread_id, input).map_err(|error| {
            if error.to_string().contains("not found")
                || error.to_string().contains("cannot accept")
            {
                HttpError::bad_request(error.to_string())
            } else {
                HttpError::internal(error)
            }
        })?;
        maybe_spawn_turn(&turn.id, &turn.metadata, paths, config);
        let thread = store.get_thread(thread_id).map_err(HttpError::internal)?;
        return to_value(thread_accepted_envelope(thread, turn, None));
    }

    if method == "POST" && rest.ends_with("/inject-items") {
        let thread_id = rest.trim_end_matches("/inject-items");
        let input = serde_json::from_slice::<AgentThreadInjectRequest>(&body).map_err(|error| {
            HttpError::bad_request(format!("invalid inject item JSON: {error}"))
        })?;
        let normalized = input.normalized_input();
        if normalized.is_empty() {
            return Err(HttpError::bad_request("input must not be empty"));
        }
        let store = open_store(paths)?;
        let thread = store.get_thread(thread_id).map_err(HttpError::internal)?;
        let mailbox = store
            .enqueue_mailbox_item(
                thread_id,
                thread.last_turn_id.as_deref(),
                "inject_only",
                &normalized,
                json!({
                    "metadata": input.metadata,
                    "source": "inject_items"
                }),
            )
            .map_err(HttpError::internal)?;
        store
            .add_thread_event(
                thread_id,
                thread.last_turn_id.as_deref(),
                "mailbox_item_queued",
                "Queued injected thread context item.",
                json!({
                    "mailboxItemId": mailbox.id,
                    "delivery": mailbox.delivery,
                    "input": normalized
                }),
            )
            .map_err(HttpError::internal)?;
        return to_value(json!({
            "kind": "issue_finder_agent_mailbox_item_accepted",
            "version": 1,
            "thread": thread,
            "mailboxItem": mailbox,
            "threadUrl": format!("/a2a/threads/{thread_id}"),
            "eventsUrl": format!("/a2a/threads/{thread_id}/events"),
            "subscribeUrl": format!("/a2a/threads/{thread_id}/subscribe")
        }));
    }

    if method == "POST" && rest.ends_with("/compact-context") {
        let thread_id = rest.trim_end_matches("/compact-context");
        let store = open_store(paths)?;
        let thread = store.get_thread(thread_id).map_err(HttpError::internal)?;
        let summary = compact_thread_context_now(
            &store,
            thread_id,
            AgentContextBudget::from_config(&config.agent),
        )
        .map_err(HttpError::internal)?;
        store
            .add_thread_event(
                thread_id,
                thread.last_turn_id.as_deref(),
                "context_compaction_requested",
                "Manual context compaction requested.",
                json!({"created": summary.is_some()}),
            )
            .map_err(HttpError::internal)?;
        return to_value(json!({
            "kind": "issue_finder_agent_context_compaction",
            "version": 1,
            "thread": thread,
            "created": summary.is_some(),
            "summary": summary,
            "threadUrl": format!("/a2a/threads/{thread_id}"),
            "itemsUrl": format!("/a2a/threads/{thread_id}/items"),
            "eventsUrl": format!("/a2a/threads/{thread_id}/events")
        }));
    }

    if method == "POST" && rest.contains("/turns/") && rest.ends_with("/steer") {
        let (thread_id, turn_id) = parse_thread_turn_suffix(rest, "/steer")?;
        let input = serde_json::from_slice::<AgentThreadTurnRequest>(&body)
            .map_err(|error| HttpError::bad_request(format!("invalid steer JSON: {error}")))?;
        let normalized = input.normalized_input();
        if normalized.is_empty() {
            return Err(HttpError::bad_request("input must not be empty"));
        }
        let store = open_store(paths)?;
        let thread = store.get_thread(thread_id).map_err(HttpError::internal)?;
        let turn = store.get_turn(turn_id).map_err(HttpError::internal)?;
        if turn.thread_id != thread_id {
            return Err(HttpError::bad_request("turn does not belong to thread"));
        }
        let mailbox = store
            .enqueue_mailbox_item(
                thread_id,
                Some(turn_id),
                "steer",
                &normalized,
                json!({
                    "input": input,
                    "source": "turn_steer"
                }),
            )
            .map_err(HttpError::internal)?;
        store
            .add_thread_event(
                thread_id,
                Some(turn_id),
                "mailbox_item_queued",
                "Queued running-turn steer input.",
                json!({
                    "mailboxItemId": mailbox.id,
                    "delivery": mailbox.delivery,
                    "input": normalized
                }),
            )
            .map_err(HttpError::internal)?;
        return to_value(thread_accepted_envelope(thread, turn, Some(mailbox)));
    }

    if method == "POST" && rest.contains("/turns/") && rest.ends_with("/interrupt") {
        let (thread_id, turn_id) = parse_thread_turn_suffix(rest, "/interrupt")?;
        let store = open_store(paths)?;
        let turn = store.get_turn(turn_id).map_err(HttpError::internal)?;
        if turn.thread_id != thread_id {
            return Err(HttpError::bad_request("turn does not belong to thread"));
        }
        let interrupt = store
            .enqueue_mailbox_item(
                thread_id,
                Some(turn_id),
                "interrupt",
                "interrupt requested",
                json!({"source": "turn_interrupt"}),
            )
            .map_err(HttpError::internal)?;
        store
            .consume_mailbox_item(&interrupt.id)
            .map_err(HttpError::internal)?;
        store
            .update_turn_status(turn_id, AgentTaskStatus::Cancelled, None, None)
            .map_err(HttpError::internal)?;
        store
            .update_thread_status(thread_id, AgentThreadStatus::Active)
            .map_err(HttpError::internal)?;
        store
            .add_thread_event(
                thread_id,
                Some(turn_id),
                "turn_interrupted",
                "Agent turn interrupt requested.",
                json!({}),
            )
            .map_err(HttpError::internal)?;
        let thread = store.get_thread(thread_id).map_err(HttpError::internal)?;
        let turn = store.get_turn(turn_id).map_err(HttpError::internal)?;
        return to_value(thread_accepted_envelope(thread, turn, None));
    }

    if method == "POST" && rest.contains("/approvals/") && rest.ends_with("/approve") {
        let (thread_id, approval_request_id) = parse_thread_approval_suffix(rest, "/approve")?;
        let approval = approve_agent_approval_request(
            paths,
            config,
            thread_id.to_string(),
            approval_request_id.to_string(),
        )
        .await
        .map_err(HttpError::internal)?;
        return to_value(json!({
            "kind": "issue_finder_agent_approval_request",
            "version": 1,
            "approvalRequest": approval,
            "threadUrl": format!("/a2a/threads/{thread_id}"),
            "itemsUrl": format!("/a2a/threads/{thread_id}/items"),
            "eventsUrl": format!("/a2a/threads/{thread_id}/events")
        }));
    }

    if method == "POST" && rest.contains("/approvals/") && rest.ends_with("/reject") {
        let (thread_id, approval_request_id) = parse_thread_approval_suffix(rest, "/reject")?;
        let input = if body.is_empty() {
            AgentApprovalDecisionRequest::default()
        } else {
            serde_json::from_slice::<AgentApprovalDecisionRequest>(&body).map_err(|error| {
                HttpError::bad_request(format!("invalid approval JSON: {error}"))
            })?
        };
        let approval = reject_agent_approval_request(
            paths,
            thread_id.to_string(),
            approval_request_id.to_string(),
            input.reason,
        )
        .map_err(HttpError::internal)?;
        return to_value(json!({
            "kind": "issue_finder_agent_approval_request",
            "version": 1,
            "approvalRequest": approval,
            "threadUrl": format!("/a2a/threads/{thread_id}"),
            "itemsUrl": format!("/a2a/threads/{thread_id}/items"),
            "eventsUrl": format!("/a2a/threads/{thread_id}/events")
        }));
    }

    let store = open_store(paths)?;

    if method == "GET" && (rest.ends_with("/events") || rest.ends_with("/subscribe")) {
        let is_subscribe = rest.ends_with("/subscribe");
        let thread_id = rest
            .trim_end_matches("/events")
            .trim_end_matches("/subscribe");
        let since_sequence = query_param(full_path, "since")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0);
        let events = store
            .list_thread_events_since(thread_id, since_sequence)
            .map_err(HttpError::internal)?;
        if is_subscribe && events.len() > MAX_SUBSCRIBE_REPLAY_EVENTS {
            return Err(HttpError::conflict(format!(
                "lagged: subscribe replay has {} events after sequence {since_sequence}, exceeding the {} event limit; read /a2a/threads/{thread_id}/events or resume from a newer sequence",
                events.len(),
                MAX_SUBSCRIBE_REPLAY_EVENTS
            )));
        }
        return to_value(AgentThreadEventsEnvelope {
            kind: "issue_finder_agent_thread_events".to_string(),
            version: 1,
            thread_id: thread_id.to_string(),
            events,
        });
    }

    if method == "GET" && rest.ends_with("/items") {
        let thread_id = rest.trim_end_matches("/items");
        let detail = store
            .thread_detail(thread_id)
            .map_err(HttpError::internal)?;
        return to_value(json!({
            "kind": "issue_finder_agent_thread_items",
            "version": 1,
            "threadId": thread_id,
            "items": detail.items,
            "mailboxItems": detail.mailbox_items
        }));
    }

    if method == "GET" && rest.contains("/turns/") {
        let (thread_id, turn_id) = parse_thread_turn_suffix(rest, "")?;
        let thread = store.get_thread(thread_id).map_err(HttpError::internal)?;
        let turn = store.get_turn(turn_id).map_err(HttpError::internal)?;
        if turn.thread_id != thread_id {
            return Err(HttpError::bad_request("turn does not belong to thread"));
        }
        return to_value(json!({
            "kind": "issue_finder_agent_thread_turn_detail",
            "version": 1,
            "thread": thread,
            "turn": turn,
            "threadUrl": format!("/a2a/threads/{thread_id}"),
            "itemsUrl": format!("/a2a/threads/{thread_id}/items"),
            "eventsUrl": format!("/a2a/threads/{thread_id}/events"),
            "subscribeUrl": format!("/a2a/threads/{thread_id}/subscribe")
        }));
    }

    if method == "GET" {
        let detail = store.thread_detail(rest).map_err(|error| {
            if error.to_string().contains("not found") {
                HttpError::not_found(error.to_string())
            } else {
                HttpError::internal(error)
            }
        })?;
        return to_value(AgentThreadDetailEnvelope {
            kind: "issue_finder_agent_thread_detail".to_string(),
            version: 1,
            detail,
        });
    }

    Err(HttpError::not_found("unknown agent thread endpoint"))
}

fn thread_accepted_envelope(
    thread: super::model::AgentThread,
    turn: super::model::AgentTurn,
    mailbox_item: Option<super::model::AgentThreadMailboxItem>,
) -> AgentThreadAcceptedEnvelope {
    AgentThreadAcceptedEnvelope {
        kind: "issue_finder_agent_thread_accepted".to_string(),
        version: 1,
        thread_url: format!("/a2a/threads/{}", thread.id),
        turn_url: format!("/a2a/threads/{}/turns/{}", thread.id, turn.id),
        events_url: format!("/a2a/threads/{}/events", thread.id),
        subscribe_url: format!("/a2a/threads/{}/subscribe", thread.id),
        items_url: format!("/a2a/threads/{}/items", thread.id),
        result_url: format!("/a2a/threads/{}/turns/{}", thread.id, turn.id),
        thread,
        turn,
        mailbox_item,
    }
}

fn parse_thread_turn_suffix<'a>(
    rest: &'a str,
    suffix: &str,
) -> std::result::Result<(&'a str, &'a str), HttpError> {
    let trimmed = if suffix.is_empty() {
        rest
    } else {
        rest.trim_end_matches(suffix)
    };
    let Some((thread_id, turn_path)) = trimmed.split_once("/turns/") else {
        return Err(HttpError::not_found("unknown agent thread turn endpoint"));
    };
    if thread_id.is_empty() || turn_path.is_empty() {
        return Err(HttpError::bad_request("thread id and turn id are required"));
    }
    Ok((thread_id, turn_path))
}

fn parse_thread_approval_suffix<'a>(
    rest: &'a str,
    suffix: &str,
) -> std::result::Result<(&'a str, &'a str), HttpError> {
    let trimmed = rest.trim_end_matches(suffix);
    let Some((thread_id, approval_path)) = trimmed.split_once("/approvals/") else {
        return Err(HttpError::not_found(
            "unknown agent thread approval endpoint",
        ));
    };
    if thread_id.is_empty() || approval_path.is_empty() {
        return Err(HttpError::bad_request(
            "thread id and approval request id are required",
        ));
    }
    Ok((thread_id, approval_path))
}

fn query_param(path: &str, key: &str) -> Option<String> {
    let query = path.split_once('?')?.1;
    for part in query.split('&') {
        let (name, value) = part.split_once('=').unwrap_or((part, ""));
        if name == key {
            return Some(value.to_string());
        }
    }
    None
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentApprovalDecisionRequest {
    #[serde(default)]
    reason: Option<String>,
}

fn create_thread_turn(
    store: &AgentStore,
    thread_id: &str,
    input: AgentThreadTurnRequest,
) -> Result<super::model::AgentTurn> {
    let normalized = input.normalized_input();
    let turn = store.create_turn(
        thread_id,
        &normalized,
        json!({
            "input": input,
            "transport": "local_http_a2a"
        }),
    )?;
    store.add_thread_item(NewAgentThreadItem {
        thread_id,
        turn_id: Some(&turn.id),
        item_type: "user_message",
        role: Some("user"),
        content: Some(&normalized),
        tool_name: None,
        payload: json!({"messageType": "turn_input"}),
    })?;
    store.add_thread_event(
        thread_id,
        Some(&turn.id),
        "turn_queued",
        "Agent turn queued.",
        json!({"input": normalized}),
    )?;
    Ok(turn)
}

fn maybe_spawn_turn(turn_id: &str, metadata: &Value, paths: IssueFinderPaths, config: Config) {
    if metadata["input"]["runImmediately"]
        .as_bool()
        .unwrap_or(true)
    {
        let turn_id = turn_id.to_string();
        tokio::spawn(async move {
            if let Err(error) = run_agent_turn(paths, config, turn_id.clone()).await {
                tracing::warn!("agent turn {turn_id} failed: {error}");
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

fn agent_card(config: &Config) -> AgentCardEnvelope {
    let registry = AgentToolRegistry::issue_finder_default();
    let wire_api =
        AgentWireApi::parse(&config.llm.wire_api).unwrap_or(AgentWireApi::ChatCompletions);
    let native_tools = OpenAiCompatibleAgentLlmClient::new(config.clone())
        .map(|client| client.supports_native_tools())
        .unwrap_or(false);

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
                method: "POST".to_string(),
                path: "/a2a/threads/start".to_string(),
                description: "Start a resumable natural-language agent thread.".to_string(),
            },
            AgentEndpoint {
                method: "POST".to_string(),
                path: "/a2a/threads/{threadId}/turns/send".to_string(),
                description: "Append a turn to a resumable agent thread.".to_string(),
            },
            AgentEndpoint {
                method: "POST".to_string(),
                path: "/a2a/threads/{threadId}/turns/{turnId}/steer".to_string(),
                description: "Queue steering input for a running turn.".to_string(),
            },
            AgentEndpoint {
                method: "POST".to_string(),
                path: "/a2a/threads/{threadId}/turns/{turnId}/interrupt".to_string(),
                description: "Request cancellation for a running turn.".to_string(),
            },
            AgentEndpoint {
                method: "POST".to_string(),
                path: "/a2a/threads/{threadId}/inject-items".to_string(),
                description: "Inject thread context through the runtime mailbox.".to_string(),
            },
            AgentEndpoint {
                method: "POST".to_string(),
                path: "/a2a/threads/{threadId}/compact-context".to_string(),
                description: "Request deterministic context compaction for a thread.".to_string(),
            },
            AgentEndpoint {
                method: "POST".to_string(),
                path: "/a2a/threads/{threadId}/approvals/{approvalRequestId}/approve"
                    .to_string(),
                description: "Approve and execute one approval-gated agent tool request."
                    .to_string(),
            },
            AgentEndpoint {
                method: "POST".to_string(),
                path: "/a2a/threads/{threadId}/approvals/{approvalRequestId}/reject".to_string(),
                description: "Reject one approval-gated agent tool request.".to_string(),
            },
            AgentEndpoint {
                method: "GET".to_string(),
                path: "/a2a/threads/{threadId}".to_string(),
                description: "Read one persisted agent thread.".to_string(),
            },
            AgentEndpoint {
                method: "GET".to_string(),
                path: "/a2a/threads/{threadId}/turns/{turnId}".to_string(),
                description: "Read one persisted agent turn.".to_string(),
            },
            AgentEndpoint {
                method: "GET".to_string(),
                path: "/a2a/threads/{threadId}/items".to_string(),
                description: "Read persisted thread items and mailbox state.".to_string(),
            },
            AgentEndpoint {
                method: "GET".to_string(),
                path: "/a2a/threads/{threadId}/events".to_string(),
                description: "Read ordered persisted thread events.".to_string(),
            },
            AgentEndpoint {
                method: "GET".to_string(),
                path: "/a2a/threads/{threadId}/subscribe".to_string(),
                description: "Read events after a sequence for resumable polling.".to_string(),
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
        tools: registry.direct_tool_names(),
        tool_definitions: registry
            .direct_tool_definitions()
            .into_iter()
            .filter_map(|definition| serde_json::to_value(definition).ok())
            .collect(),
        capabilities: capability_matrix(native_tools),
        provider: AgentProviderCapabilities {
            wire_api: wire_api.as_str().to_string(),
            native_tools,
            deferred_tools: !registry
                .responses_tools_for_exposures(&[AgentToolExposure::Deferred])
                .is_empty(),
        },
    }
}

fn capability_matrix(native_tools: bool) -> Vec<AgentCapability> {
    vec![
        capability(
            "thread.start",
            "supported",
            Some("POST"),
            Some("/a2a/threads/start"),
            json!({}),
            Vec::new(),
        ),
        capability(
            "thread.read",
            "supported",
            Some("GET"),
            Some("/a2a/threads/{threadId}"),
            json!({}),
            Vec::new(),
        ),
        capability(
            "thread.list",
            "supported",
            Some("GET"),
            Some("/a2a/threads"),
            json!({"limit": 50}),
            Vec::new(),
        ),
        capability(
            "turn.start",
            "supported",
            Some("POST"),
            Some("/a2a/threads/{threadId}/turns/send"),
            json!({"runningThreadDelivery": "steer"}),
            Vec::new(),
        ),
        capability(
            "turn.steer",
            "supported",
            Some("POST"),
            Some("/a2a/threads/{threadId}/turns/{turnId}/steer"),
            json!({"delivery": "steer"}),
            Vec::new(),
        ),
        capability(
            "turn.interrupt",
            "supported",
            Some("POST"),
            Some("/a2a/threads/{threadId}/turns/{turnId}/interrupt"),
            json!({}),
            Vec::new(),
        ),
        capability(
            "thread.inject_items",
            "supported",
            Some("POST"),
            Some("/a2a/threads/{threadId}/inject-items"),
            json!({"delivery": "inject_only"}),
            Vec::new(),
        ),
        capability(
            "approval.approve",
            "supported",
            Some("POST"),
            Some("/a2a/threads/{threadId}/approvals/{approvalRequestId}/approve"),
            json!({"executesThrough": "IssueFinderToolRuntime"}),
            Vec::new(),
        ),
        capability(
            "approval.reject",
            "supported",
            Some("POST"),
            Some("/a2a/threads/{threadId}/approvals/{approvalRequestId}/reject"),
            json!({}),
            Vec::new(),
        ),
        capability(
            "events.subscribe",
            "supported",
            Some("GET"),
            Some("/a2a/threads/{threadId}/subscribe?since=<sequence>"),
            json!({"transport": "resumable_poll", "also": "/a2a/threads/{threadId}/events?since=<sequence>"}),
            Vec::new(),
        ),
        capability(
            "tools.native_responses",
            if native_tools {
                "supported"
            } else {
                "unsupported"
            },
            None,
            None,
            json!({}),
            vec!["llm.wire_api = \"responses\"".to_string()],
        ),
        capability(
            "tools.deferred",
            "experimental",
            None,
            None,
            json!({"modelVisibleByDefault": false}),
            Vec::new(),
        ),
        capability(
            "context.compaction",
            "supported",
            Some("POST"),
            Some("/a2a/threads/{threadId}/compact-context"),
            json!({"strategy": "deterministic_summary", "rawHistoryRetained": true, "triggers": ["budget", "manual", "turn_completed"]}),
            Vec::new(),
        ),
    ]
}

fn capability(
    name: &str,
    status: &str,
    method: Option<&str>,
    path: Option<&str>,
    limits: serde_json::Value,
    provider_requirements: Vec<String>,
) -> AgentCapability {
    AgentCapability {
        name: name.to_string(),
        status: status.to_string(),
        method: method.map(ToOwned::to_owned),
        path: path.map(ToOwned::to_owned),
        limits,
        provider_requirements,
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use chrono::Utc;
    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        agent_card, find_header_end, route_request, HttpRequest, MAX_SUBSCRIBE_REPLAY_EVENTS,
    };
    use crate::agent::model::{AgentTaskStatus, AgentThreadStatus};
    use crate::agent::runtime::run_agent_turn;
    use crate::agent::store::AgentStore;
    use crate::config::Config;
    use crate::paths::IssueFinderPaths;
    use crate::tool_specs::TOOL_STATUS;

    #[test]
    fn agent_card_lists_read_only_assessment_tools() {
        let card = agent_card(&Config::default());

        assert_eq!(
            card.tools,
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
        assert!(card.tool_definitions.iter().any(|definition| {
            definition["canonicalName"] == "issue-finder.discover_candidates"
                && definition["exposure"] == "direct"
                && definition["inputSchema"].is_object()
        }));
        assert!(!card
            .tool_definitions
            .iter()
            .any(|definition| definition["canonicalName"] == "issue-finder.scout"));
        assert!(card.capabilities.iter().any(|capability| {
            capability.name == "turn.steer" && capability.status == "supported"
        }));
        assert!(card.capabilities.iter().any(|capability| {
            capability.name == "context.compaction" && capability.status == "supported"
        }));
        assert_eq!(card.provider.wire_api, "chat_completions");
    }

    #[tokio::test]
    async fn running_thread_send_is_queued_as_steer_mailbox_item() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let store = AgentStore::open(paths.clone()).unwrap();
        let thread = store.create_thread("goal", "goal", json!({})).unwrap();
        let turn = store
            .create_turn(
                &thread.id,
                "first",
                json!({"input": {"input": "first", "runImmediately": false}}),
            )
            .unwrap();
        store
            .update_thread_status(&thread.id, AgentThreadStatus::Running)
            .unwrap();
        store
            .update_turn_status(&turn.id, AgentTaskStatus::Running, None, None)
            .unwrap();
        drop(store);

        let value = route_request(
            HttpRequest {
                method: "POST".to_string(),
                path: format!("/a2a/threads/{}/turns/send", thread.id),
                body: serde_json::to_vec(&json!({
                    "input": "please inspect the second candidate",
                    "runImmediately": true
                }))
                .unwrap(),
            },
            paths.clone(),
            Config::default(),
        )
        .await
        .unwrap();

        assert_eq!(value["mailboxItem"]["delivery"], "steer");
        assert_eq!(value["turn"]["id"], turn.id);
        let detail = AgentStore::open(paths)
            .unwrap()
            .thread_detail(&thread.id)
            .unwrap();
        assert_eq!(detail.mailbox_items.len(), 1);
        assert_eq!(detail.mailbox_items[0].status, "pending");
    }

    #[tokio::test]
    async fn legacy_task_send_is_marked_as_legacy_state() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let value = route_request(
            HttpRequest {
                method: "POST".to_string(),
                path: "/a2a/tasks/send".to_string(),
                body: serde_json::to_vec(&json!({
                    "goal": "search issues",
                    "runImmediately": false
                }))
                .unwrap(),
            },
            paths,
            Config::default(),
        )
        .await
        .unwrap();

        assert_eq!(value["task"]["metadata"]["legacy"], true);
        assert!(value["task"]["metadata"]["threadRef"].is_null());
    }

    #[tokio::test]
    async fn thread_subscribe_returns_events_after_sequence() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let store = AgentStore::open(paths.clone()).unwrap();
        let thread = store.create_thread("goal", "goal", json!({})).unwrap();
        let first = store
            .add_thread_event(&thread.id, None, "first", "First.", json!({}))
            .unwrap();
        store
            .add_thread_event(&thread.id, None, "second", "Second.", json!({}))
            .unwrap();
        drop(store);

        let value = route_request(
            HttpRequest {
                method: "GET".to_string(),
                path: format!(
                    "/a2a/threads/{}/subscribe?since={}",
                    thread.id, first.sequence
                ),
                body: Vec::new(),
            },
            paths,
            Config::default(),
        )
        .await
        .unwrap();

        assert_eq!(value["events"].as_array().unwrap().len(), 1);
        assert_eq!(value["events"][0]["kind"], "second");
    }

    #[tokio::test]
    async fn thread_subscribe_returns_lagged_when_replay_exceeds_limit() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let store = AgentStore::open(paths.clone()).unwrap();
        let thread = store.create_thread("goal", "goal", json!({})).unwrap();
        for index in 0..=MAX_SUBSCRIBE_REPLAY_EVENTS {
            store
                .add_thread_event(
                    &thread.id,
                    None,
                    "event",
                    &format!("Event {index}."),
                    json!({"index": index}),
                )
                .unwrap();
        }
        drop(store);

        let error = route_request(
            HttpRequest {
                method: "GET".to_string(),
                path: format!("/a2a/threads/{}/subscribe?since=0", thread.id),
                body: Vec::new(),
            },
            paths,
            Config::default(),
        )
        .await
        .unwrap_err();

        assert_eq!(error.status, 409);
        assert!(error.message.contains("lagged"));
        assert!(error.message.contains("exceeding"));
    }

    #[tokio::test]
    async fn thread_interrupt_cancels_turn_and_records_consumed_mailbox_item() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let store = AgentStore::open(paths.clone()).unwrap();
        let thread = store.create_thread("goal", "goal", json!({})).unwrap();
        let turn = store
            .create_turn(
                &thread.id,
                "first",
                json!({"input": {"input": "first", "runImmediately": false}}),
            )
            .unwrap();
        store
            .update_thread_status(&thread.id, AgentThreadStatus::Running)
            .unwrap();
        store
            .update_turn_status(&turn.id, AgentTaskStatus::Running, None, None)
            .unwrap();
        drop(store);

        let value = route_request(
            HttpRequest {
                method: "POST".to_string(),
                path: format!("/a2a/threads/{}/turns/{}/interrupt", thread.id, turn.id),
                body: Vec::new(),
            },
            paths.clone(),
            Config::default(),
        )
        .await
        .unwrap();

        assert_eq!(value["turn"]["status"], "cancelled");
        let detail = AgentStore::open(paths)
            .unwrap()
            .thread_detail(&thread.id)
            .unwrap();
        assert_eq!(detail.thread.status, AgentThreadStatus::Active);
        assert_eq!(detail.mailbox_items.len(), 1);
        assert_eq!(detail.mailbox_items[0].delivery, "interrupt");
        assert_eq!(detail.mailbox_items[0].status, "consumed");
    }

    #[tokio::test]
    async fn approval_approve_endpoint_executes_owner_runtime_and_records_result() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let store = AgentStore::open(paths.clone()).unwrap();
        let thread = store.create_thread("goal", "goal", json!({})).unwrap();
        let turn = store.create_turn(&thread.id, "input", json!({})).unwrap();
        let approval = store
            .create_agent_approval_request(
                "agent-approval-status",
                &thread.id,
                Some(&turn.id),
                "call_status",
                TOOL_STATUS,
                json!({"checkAuth": false}),
            )
            .unwrap();
        drop(store);

        let response = route_request(
            HttpRequest {
                method: "POST".to_string(),
                path: format!(
                    "/a2a/threads/{}/approvals/{}/approve",
                    thread.id, approval.id
                ),
                body: Vec::new(),
            },
            paths.clone(),
            Config::default(),
        )
        .await
        .unwrap();

        assert_eq!(response["approvalRequest"]["status"], "executed");
        assert_eq!(response["approvalRequest"]["toolName"], TOOL_STATUS);
        let detail = AgentStore::open(paths)
            .unwrap()
            .thread_detail(&thread.id)
            .unwrap();
        assert!(detail
            .events
            .iter()
            .any(|event| event.kind == "approval_tool_completed"));
        assert!(detail
            .mailbox_items
            .iter()
            .any(|item| item.payload["source"] == "approval_execution"));
    }

    #[tokio::test]
    async fn fake_responses_llm_drives_thread_tools_and_persists_sqlite_history() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let github = start_agent_mock_github();
        let llm = start_fake_responses_llm();
        let mut config = Config::default();
        config.github.token = "fake-github-token".to_string();
        config.github.api_base_url = github.base_url().to_string();
        config.llm.enabled = true;
        config.llm.base_url = llm.base_url();
        config.llm.api_key = "fake-llm-token".to_string();
        config.llm.model = "fake-responses-model".to_string();
        config.llm.wire_api = "responses".to_string();
        config.agent.context_max_items = 4;
        config.agent.context_max_chars = 2_000;
        config.agent.context_max_item_chars = 800;
        config.agent.context_recent_items_after_compaction = 2;

        let accepted = route_request(
            HttpRequest {
                method: "POST".to_string(),
                path: "/a2a/threads/start".to_string(),
                body: serde_json::to_vec(&json!({
                    "goal": "搜索全网仓库并推荐 issue",
                    "limit": 3,
                    "maxTurns": 4,
                    "runImmediately": false
                }))
                .unwrap(),
            },
            paths.clone(),
            config.clone(),
        )
        .await
        .unwrap();
        let thread_id = accepted["thread"]["id"].as_str().unwrap().to_string();
        let first_turn_id = accepted["turn"]["id"].as_str().unwrap().to_string();
        run_agent_turn(paths.clone(), config.clone(), first_turn_id.clone())
            .await
            .unwrap();
        let first_turn = AgentStore::open(paths.clone())
            .unwrap()
            .get_turn(&first_turn_id)
            .unwrap();
        assert_eq!(first_turn.status, AgentTaskStatus::Completed);

        let accepted_follow_up = route_request(
            HttpRequest {
                method: "POST".to_string(),
                path: format!("/a2a/threads/{thread_id}/turns/send"),
                body: serde_json::to_vec(&json!({
                    "input": "继续在同一线程进一步评估第一个候选",
                    "maxTurns": 3,
                    "runImmediately": false
                }))
                .unwrap(),
            },
            paths.clone(),
            config.clone(),
        )
        .await
        .unwrap();
        assert_eq!(accepted_follow_up["thread"]["id"], thread_id);
        let second_turn_id = accepted_follow_up["turn"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        run_agent_turn(paths.clone(), config.clone(), second_turn_id.clone())
            .await
            .unwrap();
        let second_turn = AgentStore::open(paths.clone())
            .unwrap()
            .get_turn(&second_turn_id)
            .unwrap();
        assert_eq!(second_turn.status, AgentTaskStatus::Completed);

        let store = AgentStore::open(paths.clone()).unwrap();
        let detail = store.thread_detail(&thread_id).unwrap();
        assert_eq!(detail.turns.len(), 2);
        assert!(detail
            .turns
            .iter()
            .all(|turn| turn.status == AgentTaskStatus::Completed));

        let completed_tools = detail
            .items
            .iter()
            .filter(|item| item.item_type == "tool_call_completed")
            .map(|item| {
                (
                    item.tool_name.as_deref().unwrap(),
                    item.payload["toolCallId"].as_str().unwrap(),
                    item.payload["resultArtifactId"].as_str().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            completed_tools
                .iter()
                .map(|(tool, _, _)| *tool)
                .collect::<Vec<_>>(),
            vec![
                "issue-finder.discover_candidates",
                "issue-finder.inspect_candidate",
                "issue-finder.assess",
                "issue-finder.inspect_candidate",
                "issue-finder.assess"
            ]
        );
        assert_eq!(
            completed_tools
                .iter()
                .map(|(_, call_id, _)| *call_id)
                .collect::<Vec<_>>(),
            vec![
                "call_discover",
                "call_inspect",
                "call_assess",
                "call_follow_inspect",
                "call_follow_assess"
            ]
        );
        for (_, _, artifact_id) in &completed_tools {
            let artifact = store.read_artifact_bytes(artifact_id).unwrap();
            let output = serde_json::from_slice::<serde_json::Value>(&artifact).unwrap();
            assert_eq!(output["success"], true);
            assert_eq!(
                output["structured_content"]["kind"],
                "issue_finder_tool_output"
            );
        }
        assert!(detail
            .items
            .iter()
            .any(|item| item.item_type == "context_compaction"));
        assert!(detail.items.iter().any(|item| {
            item.item_type == "final_answer"
                && item
                    .content
                    .as_deref()
                    .unwrap_or_default()
                    .contains("owner/ready#1")
        }));
        assert!(detail.events.iter().any(|event| {
            event.kind == "tool_call_completed"
                && event.payload["providerItemId"] == "fc_call_discover"
                && event.payload["resultArtifactId"].as_str().is_some()
        }));

        let llm_requests = llm.requests();
        assert!(
            llm_requests
                .first()
                .and_then(|request| request["tools"].as_array())
                .is_some_and(|tools| !tools.is_empty()),
            "first fake LLM request should include native tool schemas"
        );
        assert_eq!(llm_requests[0]["parallel_tool_calls"], true);
        assert!(llm_requests[0].to_string().contains("discover_candidates"));
        assert!(llm_requests.iter().any(|request| {
            request["input"].as_array().unwrap().iter().any(|item| {
                item["type"] == "function_call_output" && item["call_id"] == "call_discover"
            })
        }));

        github.stop();
        llm.stop();
    }

    struct FakeResponsesLlm {
        base_url: String,
        requests: Arc<Mutex<Vec<serde_json::Value>>>,
        shutdown: Arc<AtomicBool>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl FakeResponsesLlm {
        fn base_url(&self) -> String {
            self.base_url.clone()
        }

        fn requests(&self) -> Vec<serde_json::Value> {
            self.requests.lock().unwrap().clone()
        }

        fn stop(mut self) {
            self.shutdown.store(true, Ordering::SeqCst);
            if let Some(handle) = self.handle.take() {
                handle.join().unwrap();
            }
        }
    }

    fn start_fake_responses_llm() -> FakeResponsesLlm {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_for_thread = Arc::clone(&shutdown);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_thread = Arc::clone(&requests);
        let request_count = Arc::new(AtomicUsize::new(0));
        let request_count_for_thread = Arc::clone(&request_count);

        let handle = thread::spawn(move || {
            let started = Instant::now();
            while !shutdown_for_thread.load(Ordering::SeqCst)
                && started.elapsed() < Duration::from_secs(30)
            {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_blocking_request(&mut stream);
                        let index = request_count_for_thread.fetch_add(1, Ordering::SeqCst);
                        if request.target == "/responses" {
                            if let Ok(value) =
                                serde_json::from_slice::<serde_json::Value>(&request.body)
                            {
                                requests_for_thread.lock().unwrap().push(value);
                            }
                            write_blocking_json(&mut stream, 200, fake_llm_response(index));
                        } else {
                            write_blocking_json(
                                &mut stream,
                                404,
                                json!({"error": "unknown fake llm route"}),
                            );
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        FakeResponsesLlm {
            base_url,
            requests,
            shutdown,
            handle: Some(handle),
        }
    }

    fn fake_llm_response(index: usize) -> serde_json::Value {
        match index {
            0 => fake_tool_call(
                "fc_call_discover",
                "call_discover",
                "discover_candidates",
                json!({"limit": 3, "laneLimit": 2, "refresh": true}),
            ),
            1 => fake_tool_call(
                "fc_call_inspect",
                "call_inspect",
                "inspect_candidate",
                json!({"issue": "owner/ready#1"}),
            ),
            2 => fake_tool_call(
                "fc_call_assess",
                "call_assess",
                "assess",
                json!({"issue": "owner/ready#1", "recordRead": false}),
            ),
            3 => fake_message("推荐 owner/ready#1：已完成 inspect 和 assess。"),
            4 => fake_tool_call(
                "fc_call_follow_inspect",
                "call_follow_inspect",
                "inspect_candidate",
                json!({"issue": "owner/ready#1"}),
            ),
            5 => fake_tool_call(
                "fc_call_follow_assess",
                "call_follow_assess",
                "assess",
                json!({"issue": "owner/ready#1", "recordRead": false}),
            ),
            6 => fake_message("继续同一线程后，owner/ready#1 仍是最佳候选。"),
            _ => fake_message("没有更多 fake response。"),
        }
    }

    fn fake_tool_call(
        provider_item_id: &str,
        call_id: &str,
        name: &str,
        arguments: serde_json::Value,
    ) -> serde_json::Value {
        json!({
            "id": format!("resp_{call_id}"),
            "output": [{
                "type": "function_call",
                "id": provider_item_id,
                "call_id": call_id,
                "namespace": "issue-finder",
                "name": name,
                "arguments": arguments.to_string()
            }],
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })
    }

    fn fake_message(content: &str) -> serde_json::Value {
        json!({
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": content}]
            }],
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })
    }

    struct AgentMockGithub {
        base_url: String,
        shutdown: Arc<AtomicBool>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl AgentMockGithub {
        fn base_url(&self) -> &str {
            &self.base_url
        }

        fn stop(mut self) {
            self.shutdown.store(true, Ordering::SeqCst);
            if let Some(handle) = self.handle.take() {
                handle.join().unwrap();
            }
        }
    }

    fn start_agent_mock_github() -> AgentMockGithub {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let base_url_for_thread = base_url.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_for_thread = Arc::clone(&shutdown);

        let handle = thread::spawn(move || {
            let started = Instant::now();
            while !shutdown_for_thread.load(Ordering::SeqCst)
                && started.elapsed() < Duration::from_secs(30)
            {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_blocking_request(&mut stream);
                        let response = github_response_body(&request.target, &base_url_for_thread);
                        write_blocking_response(&mut stream, response.status, &response.body);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        AgentMockGithub {
            base_url,
            shutdown,
            handle: Some(handle),
        }
    }

    struct BlockingRequest {
        target: String,
        body: Vec<u8>,
    }

    fn read_blocking_request(stream: &mut std::net::TcpStream) -> BlockingRequest {
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let read = stream.read(&mut chunk).unwrap_or(0);
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
            if let Some(header_end) = find_header_end(&buffer) {
                let content_length = blocking_content_length(&buffer[..header_end]);
                let body_start = header_end + 4;
                while buffer.len() < body_start + content_length {
                    let read = stream.read(&mut chunk).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                }
                let target = blocking_request_target(&buffer[..header_end]).to_string();
                let body = buffer[body_start..body_start + content_length].to_vec();
                return BlockingRequest { target, body };
            }
        }
        BlockingRequest {
            target: String::new(),
            body: Vec::new(),
        }
    }

    fn blocking_content_length(headers: &[u8]) -> usize {
        String::from_utf8_lossy(headers)
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(key, value)| {
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
            })
            .unwrap_or(0)
    }

    fn blocking_request_target(headers: &[u8]) -> &str {
        std::str::from_utf8(headers)
            .ok()
            .and_then(|headers| headers.lines().next())
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("")
    }

    fn write_blocking_json(
        stream: &mut std::net::TcpStream,
        status: u16,
        value: serde_json::Value,
    ) {
        write_blocking_response(stream, status, &value.to_string());
    }

    fn write_blocking_response(stream: &mut std::net::TcpStream, status: u16, body: &str) {
        let reason = if status == 200 { "OK" } else { "Error" };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    }

    struct AgentMockResponse {
        status: u16,
        body: String,
    }

    fn github_response_body(target: &str, base_url: &str) -> AgentMockResponse {
        if target == "/user" {
            return agent_ok_response(r#"{"login":"agent-test"}"#);
        }

        if target.starts_with("/search/issues") {
            return agent_ok_response(&agent_search_body(base_url));
        }

        for repo in ["ready", "backup"] {
            let prefix = format!("/repos/owner/{repo}");
            if target.starts_with(&format!("{prefix}/issues/1/comments")) {
                return agent_ok_response(&agent_comments_body());
            }
            if target.starts_with(&format!("{prefix}/issues/1/timeline")) {
                return agent_ok_response("[]");
            }
            if target.starts_with(&format!("{prefix}/issues?"))
                || target == format!("{prefix}/issues")
            {
                return agent_ok_response(&format!("[{}]", agent_issue_body(repo)));
            }
            if target.starts_with(&format!("{prefix}/stargazers")) {
                return agent_ok_response(&agent_stargazers_body());
            }
            if target.starts_with(&format!("{prefix}/forks")) {
                return agent_ok_response(&agent_forks_body());
            }
            if target.starts_with(&format!("{prefix}/issues/1")) {
                return agent_ok_response(&agent_issue_body(repo));
            }
            if target == prefix {
                return agent_ok_response(&agent_repo_body(repo));
            }
        }

        AgentMockResponse {
            status: 404,
            body: json!({"message": "mock route not found", "target": target}).to_string(),
        }
    }

    fn agent_ok_response(body: &str) -> AgentMockResponse {
        AgentMockResponse {
            status: 200,
            body: body.to_string(),
        }
    }

    fn agent_search_body(base_url: &str) -> String {
        format!(
            r#"{{"items":[{},{}]}}"#,
            agent_search_item(base_url, "ready"),
            agent_search_item(base_url, "backup")
        )
    }

    fn agent_search_item(base_url: &str, repo: &str) -> String {
        let title = if repo == "ready" {
            "Fix parser panic on nested CLI flags"
        } else {
            "Improve docs for backup workflow"
        };
        format!(
            r#"{{
  "id": 1,
  "number": 1,
  "title": "{title}",
  "body": "Small focused Rust CLI issue with reproducer and tests.",
  "html_url": "https://github.com/owner/{repo}/issues/1",
  "repository_url": "{base_url}/repos/owner/{repo}",
  "labels": [{{"name":"good first issue"}}],
  "pull_request": null,
  "locked": false,
  "created_at": "{timestamp}",
  "updated_at": "{timestamp}"
}}"#,
            timestamp = Utc::now().to_rfc3339()
        )
    }

    fn agent_issue_body(repo: &str) -> String {
        format!(
            r#"{{
  "id": 1,
  "number": 1,
  "title": "Fix parser panic on nested CLI flags",
  "body": "The CLI panics when nested flags are parsed twice. Repro steps and expected behavior are included. Add a regression test.",
  "html_url": "https://github.com/owner/{repo}/issues/1",
  "labels": [{{"name":"good first issue"}}, {{"name":"bug"}}],
  "pull_request": null,
  "locked": false,
  "assignee": null,
  "assignees": [],
  "created_at": "{timestamp}",
  "updated_at": "{timestamp}",
  "comments": 1,
  "author_association": "CONTRIBUTOR",
  "user": {{"login":"issue-author"}}
}}"#,
            timestamp = Utc::now().to_rfc3339()
        )
    }

    fn agent_repo_body(repo: &str) -> String {
        format!(
            r#"{{
  "full_name": "owner/{repo}",
  "name": "{repo}",
  "description": "Rust CLI parser developer tools",
  "stargazers_count": 2500,
  "forks_count": 220,
  "subscribers_count": 50,
  "open_issues_count": 12,
  "pushed_at": "{timestamp}",
  "created_at": "2025-01-01T00:00:00Z",
  "updated_at": "{timestamp}",
  "default_branch": "main",
  "topics": ["rust", "cli", "parser"],
  "language": "Rust",
  "archived": false
}}"#,
            timestamp = Utc::now().to_rfc3339()
        )
    }

    fn agent_comments_body() -> String {
        format!(
            r#"[{{
  "body": "Maintainer confirmed this is a good first contribution.",
  "author_association": "MEMBER",
  "created_at": "{}",
  "user": {{"login":"maintainer"}}
}}]"#,
            Utc::now().to_rfc3339()
        )
    }

    fn agent_stargazers_body() -> String {
        let items = (0..8)
            .map(|index| {
                format!(
                    r#"{{"starred_at":"{}","user":{{"login":"star-{index}"}}}}"#,
                    Utc::now().to_rfc3339()
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("[{items}]")
    }

    fn agent_forks_body() -> String {
        let items = (0..8)
            .map(|index| {
                format!(
                    r#"{{"created_at":"{}","owner":{{"login":"fork-{index}"}}}}"#,
                    Utc::now().to_rfc3339()
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("[{items}]")
    }

    fn test_paths(home: &std::path::Path) -> IssueFinderPaths {
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
        409 => "Conflict",
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

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: 409,
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
