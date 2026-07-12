use std::fs;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::dispatch::adapters::codex_app_server::discover_codex_binary;
use crate::dispatch::native_runtime::AppServerClient;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NativeRuntimeEvalReport {
    pub kind: &'static str,
    pub version: u32,
    pub business_outcome: NativeRuntimeBusinessOutcome,
    pub termination: NativeRuntimeTermination,
    pub protocol_handshake: bool,
    pub authenticated_model_turn: bool,
    pub binary: Option<String>,
    pub connection_mode: &'static str,
    pub workspace: String,
    pub marker: String,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub terminal_turn_status: Option<String>,
    pub observed_item_types: Vec<String>,
    pub user_marker_observed: bool,
    pub agent_marker_observed: bool,
    pub elapsed_millis: u128,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NativeRuntimeBusinessOutcome {
    pub domain: &'static str,
    pub state: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NativeRuntimeTermination {
    pub kind: &'static str,
    pub cause: &'static str,
    pub retryable: bool,
}

pub async fn run_native_runtime_eval(
    workspace: &str,
    timeout_seconds: u64,
    marker: String,
) -> NativeRuntimeEvalReport {
    let started = Instant::now();
    let binary = discover_codex_binary().ok();
    if let Err(error) = fs::create_dir_all(workspace) {
        return unavailable_report(
            workspace,
            marker,
            binary,
            started,
            "harness",
            false,
            format!("unable to create evaluation workspace: {error}"),
        );
    }
    let client = match tokio::time::timeout(
        Duration::from_secs(timeout_seconds.max(1)),
        AppServerClient::connect(),
    )
    .await
    {
        Ok(Ok(client)) => client,
        Ok(Err(error)) => {
            return unavailable_report(
                workspace,
                marker,
                binary,
                started,
                "runtime",
                true,
                error.to_string(),
            );
        }
        Err(_) => {
            return unavailable_report(
                workspace,
                marker,
                binary,
                started,
                "runtime",
                true,
                "timed out connecting to native runtime".to_string(),
            );
        }
    };
    let session = match timed_request(
        &client,
        "thread/start",
        json!({
            "cwd": workspace,
            "runtimeWorkspaceRoots": [workspace],
            "approvalPolicy": "on-request",
            "sandbox": "workspace-write"
        }),
        started,
        timeout_seconds,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            return unavailable_after_handshake(
                workspace, marker, binary, started, None, None, error,
            );
        }
    };
    let Some(thread_id) = session
        .get("thread")
        .and_then(|thread| thread.get("id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return unavailable_after_handshake(
            workspace,
            marker,
            binary,
            started,
            None,
            None,
            "thread/start response missing thread.id".to_string(),
        );
    };
    let prompt = format!(
        "Issue Finder native runtime acceptance. Reply with exactly `{marker} ACK`. Do not use tools, edit files, or perform external actions."
    );
    let turn = match timed_request(
        &client,
        "turn/start",
        json!({
            "threadId": thread_id,
            "clientUserMessageId": format!("issue-finder-native-eval-{marker}"),
            "input": [{ "type": "text", "text": prompt }],
            "cwd": workspace,
            "runtimeWorkspaceRoots": [workspace],
            "approvalPolicy": "on-request",
            "sandboxPolicy": {
                "type": "workspaceWrite",
                "writableRoots": [workspace],
                "networkAccess": false
            }
        }),
        started,
        timeout_seconds,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            return unavailable_after_handshake(
                workspace,
                marker,
                binary,
                started,
                Some(thread_id),
                None,
                error,
            );
        }
    };
    let Some(turn_id) = turn
        .get("turn")
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return unavailable_after_handshake(
            workspace,
            marker,
            binary,
            started,
            Some(thread_id),
            None,
            "turn/start response missing turn.id".to_string(),
        );
    };
    let deadline = started + Duration::from_secs(timeout_seconds);
    let mut last_status = turn
        .get("turn")
        .and_then(|turn| turn.get("status"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    loop {
        match timed_request(
            &client,
            "thread/read",
            json!({"threadId": thread_id, "includeTurns": true}),
            started,
            timeout_seconds,
        )
        .await
        {
            Ok(value) => {
                let (status, turn_error, items) = transcript_observations(&value, &turn_id);
                last_status = status.or(last_status);
                let user_marker_observed = items
                    .iter()
                    .any(|(item_type, text)| item_type == "userMessage" && text.contains(&marker));
                let agent_marker_observed = items
                    .iter()
                    .any(|(item_type, text)| item_type == "agentMessage" && text.contains(&marker));
                let item_types = items
                    .iter()
                    .map(|(item_type, _)| item_type.clone())
                    .collect::<Vec<_>>();
                if user_marker_observed && agent_marker_observed {
                    return NativeRuntimeEvalReport {
                        kind: "issue_finder_native_runtime_eval",
                        version: 1,
                        business_outcome: NativeRuntimeBusinessOutcome {
                            domain: "native_runtime",
                            state: "completed",
                        },
                        termination: NativeRuntimeTermination {
                            kind: "normal",
                            cause: "product",
                            retryable: false,
                        },
                        protocol_handshake: true,
                        authenticated_model_turn: true,
                        binary,
                        connection_mode: requested_connection_mode(),
                        workspace: workspace.to_string(),
                        marker,
                        thread_id: Some(thread_id),
                        turn_id: Some(turn_id),
                        terminal_turn_status: last_status,
                        observed_item_types: item_types,
                        user_marker_observed,
                        agent_marker_observed,
                        elapsed_millis: started.elapsed().as_millis(),
                        error: None,
                    };
                }
                if last_status.as_deref().is_some_and(is_failed_status) {
                    let error = turn_error.map_or_else(
                        || "native turn reached a failure terminal state".to_string(),
                        |detail| format!("native turn reached a failure terminal state: {detail}"),
                    );
                    return unavailable_with_observations(
                        workspace,
                        marker,
                        binary,
                        started,
                        thread_id,
                        turn_id,
                        last_status,
                        item_types,
                        user_marker_observed,
                        agent_marker_observed,
                        error,
                    );
                }
                if Instant::now() >= deadline {
                    return unavailable_with_observations(
                        workspace,
                        marker,
                        binary,
                        started,
                        thread_id,
                        turn_id,
                        last_status,
                        item_types,
                        user_marker_observed,
                        agent_marker_observed,
                        format!(
                            "timed out after {timeout_seconds} seconds waiting for model response"
                        ),
                    );
                }
            }
            Err(error) => {
                return unavailable_after_handshake(
                    workspace,
                    marker,
                    binary,
                    started,
                    Some(thread_id),
                    Some(turn_id),
                    error,
                );
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn timed_request(
    client: &AppServerClient,
    method: &str,
    params: Value,
    started: Instant,
    timeout_seconds: u64,
) -> Result<Value, String> {
    let deadline = started + Duration::from_secs(timeout_seconds);
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(format!(
            "timed out after {timeout_seconds} seconds before {method}"
        ));
    }
    match tokio::time::timeout(
        remaining.min(Duration::from_secs(10)),
        client.request(method, params),
    )
    .await
    {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err(format!("timed out waiting for {method} response")),
    }
}

fn transcript_observations(
    value: &Value,
    expected_turn_id: &str,
) -> (Option<String>, Option<String>, Vec<(String, String)>) {
    let turns = value
        .get("thread")
        .and_then(|thread| thread.get("turns"))
        .and_then(Value::as_array);
    let Some(turn) = turns
        .into_iter()
        .flatten()
        .find(|turn| turn.get("id").and_then(Value::as_str) == Some(expected_turn_id))
    else {
        return (None, None, Vec::new());
    };
    let status = turn
        .get("status")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let error = turn
        .get("error")
        .filter(|error| !error.is_null())
        .map(Value::to_string);
    let items = turn
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let item_type = item.get("type")?.as_str()?.to_string();
            let text = item
                .get("text")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .or_else(|| {
                    item.get("content")
                        .and_then(Value::as_array)
                        .map(|content| {
                            content
                                .iter()
                                .filter_map(|entry| entry.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                })
                .unwrap_or_default();
            Some((item_type, text))
        })
        .collect();
    (status, error, items)
}

fn is_failed_status(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "failed" | "cancelled" | "canceled" | "interrupted"
    )
}

fn unavailable_report(
    workspace: &str,
    marker: String,
    binary: Option<String>,
    started: Instant,
    cause: &'static str,
    retryable: bool,
    error: String,
) -> NativeRuntimeEvalReport {
    NativeRuntimeEvalReport {
        kind: "issue_finder_native_runtime_eval",
        version: 1,
        business_outcome: NativeRuntimeBusinessOutcome {
            domain: "native_runtime",
            state: "capability_unavailable",
        },
        termination: NativeRuntimeTermination {
            kind: "normal",
            cause,
            retryable,
        },
        protocol_handshake: false,
        authenticated_model_turn: false,
        binary,
        connection_mode: requested_connection_mode(),
        workspace: workspace.to_string(),
        marker,
        thread_id: None,
        turn_id: None,
        terminal_turn_status: None,
        observed_item_types: Vec::new(),
        user_marker_observed: false,
        agent_marker_observed: false,
        elapsed_millis: started.elapsed().as_millis(),
        error: Some(error),
    }
}

fn unavailable_after_handshake(
    workspace: &str,
    marker: String,
    binary: Option<String>,
    started: Instant,
    thread_id: Option<String>,
    turn_id: Option<String>,
    error: String,
) -> NativeRuntimeEvalReport {
    NativeRuntimeEvalReport {
        protocol_handshake: true,
        thread_id,
        turn_id,
        ..unavailable_report(workspace, marker, binary, started, "runtime", true, error)
    }
}

#[allow(clippy::too_many_arguments)]
fn unavailable_with_observations(
    workspace: &str,
    marker: String,
    binary: Option<String>,
    started: Instant,
    thread_id: String,
    turn_id: String,
    terminal_turn_status: Option<String>,
    observed_item_types: Vec<String>,
    user_marker_observed: bool,
    agent_marker_observed: bool,
    error: String,
) -> NativeRuntimeEvalReport {
    NativeRuntimeEvalReport {
        protocol_handshake: true,
        thread_id: Some(thread_id),
        turn_id: Some(turn_id),
        terminal_turn_status,
        observed_item_types,
        user_marker_observed,
        agent_marker_observed,
        ..unavailable_report(workspace, marker, binary, started, "runtime", true, error)
    }
}

pub fn native_runtime_eval_contract() -> Value {
    json!({
        "kind": "issue_finder_native_runtime_eval_contract",
        "version": 1,
        "connectionMode": "daemon_socket",
        "successRequires": [
            "protocol_handshake",
            "thread_start",
            "turn_start",
            "user_marker_observed",
            "agent_marker_observed"
        ],
        "unavailableState": "capability_unavailable"
    })
}

fn requested_connection_mode() -> &'static str {
    if std::env::var("ISSUE_FINDER_CODEX_TRANSPORT")
        .ok()
        .as_deref()
        == Some("stdio")
    {
        "stdio"
    } else {
        "daemon_socket"
    }
}
