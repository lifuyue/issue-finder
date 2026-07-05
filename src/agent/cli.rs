use std::time::Duration;

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;

use crate::config::Config;
use crate::paths::IssueFinderPaths;

use super::cli_args::{AgentArgs, AgentCommand};
use super::model::{
    AgentCardEnvelope, AgentEventsEnvelope, AgentTaskAcceptedEnvelope, AgentTaskDetailEnvelope,
    AgentTaskListEnvelope, AgentTaskSendRequest, AgentThreadAcceptedEnvelope,
    AgentThreadDetailEnvelope, AgentThreadEventsEnvelope, AgentThreadInjectRequest,
    AgentThreadListEnvelope, AgentThreadStartRequest, AgentThreadTurnRequest,
};
use super::server::run_daemon;

const CLIENT_TIMEOUT: Duration = Duration::from_secs(20);
const WAIT_INTERVAL: Duration = Duration::from_secs(2);
const WAIT_TIMEOUT: Duration = Duration::from_secs(180);

pub async fn handle_agent_cli(
    paths: &IssueFinderPaths,
    config: Config,
    args: AgentArgs,
) -> Result<String> {
    match args.command {
        AgentCommand::Daemon(args) => {
            run_daemon(paths.clone(), config, args.host, args.port).await?;
            Ok(String::new())
        }
        AgentCommand::Send(args) => {
            let endpoint = AgentEndpoint::new(args.host, args.port)?;
            let request = AgentTaskSendRequest {
                goal: args.goal,
                repo: args.repo,
                limit: args.limit,
                refresh: args.refresh,
                max_turns: args.max_turns,
                run_immediately: !args.queued,
            };
            let accepted = endpoint.send_task(&request).await?;
            if args.wait {
                let detail = endpoint.wait_for_task(&accepted.task.id).await?;
                if args.json {
                    return Ok(serde_json::to_string_pretty(&detail)?);
                }
                return Ok(render_task_detail(&detail));
            }
            if args.json {
                Ok(serde_json::to_string_pretty(&accepted)?)
            } else {
                Ok(render_task_accepted(&accepted))
            }
        }
        AgentCommand::ThreadStart(args) => {
            let endpoint = AgentEndpoint::new(args.host, args.port)?;
            let request = AgentThreadStartRequest {
                goal: args.goal,
                title: args.title,
                repo: args.repo,
                limit: args.limit,
                refresh: args.refresh,
                max_turns: args.max_turns,
                run_immediately: !args.queued,
            };
            let accepted = endpoint.start_thread(&request).await?;
            if args.wait {
                let detail = endpoint
                    .wait_for_thread_turn(&accepted.thread.id, &accepted.turn.id)
                    .await?;
                if args.json {
                    return Ok(serde_json::to_string_pretty(&detail)?);
                }
                return Ok(render_thread_detail(&detail));
            }
            if args.json {
                Ok(serde_json::to_string_pretty(&accepted)?)
            } else {
                Ok(render_thread_accepted(&accepted))
            }
        }
        AgentCommand::ThreadSend(args) => {
            let endpoint = AgentEndpoint::new(args.host, args.port)?;
            let request = AgentThreadTurnRequest {
                input: args.input,
                repo: args.repo,
                limit: args.limit,
                refresh: args.refresh,
                max_turns: args.max_turns,
                run_immediately: !args.queued,
            };
            let accepted = endpoint.send_thread_turn(&args.thread_id, &request).await?;
            if args.wait {
                let detail = endpoint
                    .wait_for_thread_turn(&accepted.thread.id, &accepted.turn.id)
                    .await?;
                if args.json {
                    return Ok(serde_json::to_string_pretty(&detail)?);
                }
                return Ok(render_thread_detail(&detail));
            }
            if args.json {
                Ok(serde_json::to_string_pretty(&accepted)?)
            } else {
                Ok(render_thread_accepted(&accepted))
            }
        }
        AgentCommand::ThreadSteer(args) => {
            let endpoint = AgentEndpoint::new(args.host, args.port)?;
            let request = AgentThreadTurnRequest {
                input: args.input,
                repo: args.repo,
                limit: args.limit,
                refresh: args.refresh,
                max_turns: args.max_turns,
                run_immediately: true,
            };
            let accepted = endpoint
                .steer_thread_turn(&args.thread_id, &args.turn_id, &request)
                .await?;
            if args.json {
                Ok(serde_json::to_string_pretty(&accepted)?)
            } else {
                Ok(render_thread_accepted(&accepted))
            }
        }
        AgentCommand::ThreadInject(args) => {
            let endpoint = AgentEndpoint::new(args.host, args.port)?;
            let request = AgentThreadInjectRequest {
                input: args.input,
                metadata: json!({"source": "cli"}),
            };
            let response = endpoint
                .inject_thread_items(&args.thread_id, &request)
                .await?;
            if args.json {
                Ok(serde_json::to_string_pretty(&response)?)
            } else {
                Ok(format!(
                    "Accepted injected input for thread {}\nevents: /a2a/threads/{}/events",
                    args.thread_id, args.thread_id
                ))
            }
        }
        AgentCommand::ThreadCompact(args) => {
            let endpoint = AgentEndpoint::new(args.host, args.port)?;
            let response = endpoint.compact_thread_context(&args.thread_id).await?;
            if args.json {
                Ok(serde_json::to_string_pretty(&response)?)
            } else {
                Ok(render_compaction_response(&response))
            }
        }
        AgentCommand::ThreadInterrupt(args) => {
            let endpoint = AgentEndpoint::new(args.host, args.port)?;
            let accepted = endpoint
                .interrupt_thread_turn(&args.thread_id, &args.turn_id)
                .await?;
            if args.json {
                Ok(serde_json::to_string_pretty(&accepted)?)
            } else {
                Ok(render_thread_accepted(&accepted))
            }
        }
        AgentCommand::ApprovalApprove(args) => {
            let endpoint = AgentEndpoint::new(args.host, args.port)?;
            let response = endpoint
                .approve_thread_approval(&args.thread_id, &args.approval_request_id)
                .await?;
            if args.json {
                Ok(serde_json::to_string_pretty(&response)?)
            } else {
                Ok(render_approval_response(&response))
            }
        }
        AgentCommand::ApprovalReject(args) => {
            let endpoint = AgentEndpoint::new(args.host, args.port)?;
            let response = endpoint
                .reject_thread_approval(
                    &args.thread_id,
                    &args.approval_request_id,
                    args.reason.as_deref(),
                )
                .await?;
            if args.json {
                Ok(serde_json::to_string_pretty(&response)?)
            } else {
                Ok(render_approval_response(&response))
            }
        }
        AgentCommand::Threads(args) => {
            let endpoint = AgentEndpoint::new(args.host, args.port)?;
            let list = endpoint
                .get::<AgentThreadListEnvelope>("/a2a/threads")
                .await?;
            if args.json {
                Ok(serde_json::to_string_pretty(&list)?)
            } else {
                Ok(render_thread_list(&list))
            }
        }
        AgentCommand::ThreadShow(args) => {
            let endpoint = AgentEndpoint::new(args.host, args.port)?;
            let detail = endpoint.thread_detail(&args.thread_id).await?;
            if args.json {
                Ok(serde_json::to_string_pretty(&detail)?)
            } else {
                Ok(render_thread_detail(&detail))
            }
        }
        AgentCommand::ThreadEvents(args) => {
            let endpoint = AgentEndpoint::new(args.host, args.port)?;
            let events = endpoint.thread_events(&args.thread_id).await?;
            if args.json {
                Ok(serde_json::to_string_pretty(&events)?)
            } else {
                Ok(render_thread_events(&events))
            }
        }
        AgentCommand::List(args) => {
            let endpoint = AgentEndpoint::new(args.host, args.port)?;
            let list = endpoint.get::<AgentTaskListEnvelope>("/a2a/tasks").await?;
            if args.json {
                Ok(serde_json::to_string_pretty(&list)?)
            } else {
                Ok(render_task_list(&list))
            }
        }
        AgentCommand::Show(args) => {
            let endpoint = AgentEndpoint::new(args.host, args.port)?;
            let detail = endpoint.task_detail(&args.task_id).await?;
            if args.json {
                Ok(serde_json::to_string_pretty(&detail)?)
            } else {
                Ok(render_task_detail(&detail))
            }
        }
        AgentCommand::Events(args) => {
            let endpoint = AgentEndpoint::new(args.host, args.port)?;
            let events = endpoint.task_events(&args.task_id).await?;
            if args.json {
                Ok(serde_json::to_string_pretty(&events)?)
            } else {
                Ok(render_task_events(&events))
            }
        }
        AgentCommand::Card(args) => {
            let endpoint = AgentEndpoint::new(args.host, args.port)?;
            let card = endpoint.get::<AgentCardEnvelope>("/a2a/agent-card").await?;
            if args.json {
                Ok(render_agent_card_json(&card)?)
            } else {
                Ok(render_agent_card(&card))
            }
        }
    }
}

fn render_agent_card_json(card: &AgentCardEnvelope) -> Result<String> {
    render_json(card)
}

fn render_json<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)?)
}

#[derive(Debug, Clone)]
struct AgentEndpoint {
    base_url: String,
    client: reqwest::Client,
}

impl AgentEndpoint {
    fn new(host: String, port: u16) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("issue-finder-agent-cli")
            .timeout(CLIENT_TIMEOUT)
            .build()?;
        Ok(Self {
            base_url: format!("http://{host}:{port}"),
            client,
        })
    }

    async fn send_task(&self, request: &AgentTaskSendRequest) -> Result<AgentTaskAcceptedEnvelope> {
        let response = self
            .client
            .post(self.url("/a2a/tasks/send"))
            .json(request)
            .send()
            .await?;
        decode_response(response).await
    }

    async fn start_thread(
        &self,
        request: &AgentThreadStartRequest,
    ) -> Result<AgentThreadAcceptedEnvelope> {
        let response = self
            .client
            .post(self.url("/a2a/threads/start"))
            .json(request)
            .send()
            .await?;
        decode_response(response).await
    }

    async fn send_thread_turn(
        &self,
        thread_id: &str,
        request: &AgentThreadTurnRequest,
    ) -> Result<AgentThreadAcceptedEnvelope> {
        let response = self
            .client
            .post(self.url(&format!("/a2a/threads/{thread_id}/turns/send")))
            .json(request)
            .send()
            .await?;
        decode_response(response).await
    }

    async fn steer_thread_turn(
        &self,
        thread_id: &str,
        turn_id: &str,
        request: &AgentThreadTurnRequest,
    ) -> Result<AgentThreadAcceptedEnvelope> {
        let response = self
            .client
            .post(self.url(&format!("/a2a/threads/{thread_id}/turns/{turn_id}/steer")))
            .json(request)
            .send()
            .await?;
        decode_response(response).await
    }

    async fn inject_thread_items(
        &self,
        thread_id: &str,
        request: &AgentThreadInjectRequest,
    ) -> Result<serde_json::Value> {
        let response = self
            .client
            .post(self.url(&format!("/a2a/threads/{thread_id}/inject-items")))
            .json(request)
            .send()
            .await?;
        decode_response(response).await
    }

    async fn interrupt_thread_turn(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<AgentThreadAcceptedEnvelope> {
        let response = self
            .client
            .post(self.url(&format!(
                "/a2a/threads/{thread_id}/turns/{turn_id}/interrupt"
            )))
            .send()
            .await?;
        decode_response(response).await
    }

    async fn compact_thread_context(&self, thread_id: &str) -> Result<serde_json::Value> {
        let response = self
            .client
            .post(self.url(&format!("/a2a/threads/{thread_id}/compact-context")))
            .send()
            .await?;
        decode_response(response).await
    }

    async fn approve_thread_approval(
        &self,
        thread_id: &str,
        approval_request_id: &str,
    ) -> Result<serde_json::Value> {
        let response = self
            .client
            .post(self.url(&format!(
                "/a2a/threads/{thread_id}/approvals/{approval_request_id}/approve"
            )))
            .send()
            .await?;
        decode_response(response).await
    }

    async fn reject_thread_approval(
        &self,
        thread_id: &str,
        approval_request_id: &str,
        reason: Option<&str>,
    ) -> Result<serde_json::Value> {
        let response = self
            .client
            .post(self.url(&format!(
                "/a2a/threads/{thread_id}/approvals/{approval_request_id}/reject"
            )))
            .json(&json!({ "reason": reason }))
            .send()
            .await?;
        decode_response(response).await
    }

    async fn task_detail(&self, task_id: &str) -> Result<AgentTaskDetailEnvelope> {
        self.get(&format!("/a2a/tasks/{task_id}")).await
    }

    async fn task_events(&self, task_id: &str) -> Result<AgentEventsEnvelope> {
        self.get(&format!("/a2a/tasks/{task_id}/events")).await
    }

    async fn thread_detail(&self, thread_id: &str) -> Result<AgentThreadDetailEnvelope> {
        self.get(&format!("/a2a/threads/{thread_id}")).await
    }

    async fn thread_events(&self, thread_id: &str) -> Result<AgentThreadEventsEnvelope> {
        self.get(&format!("/a2a/threads/{thread_id}/events")).await
    }

    async fn get<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let response = self.client.get(self.url(path)).send().await?;
        decode_response(response).await
    }

    async fn wait_for_task(&self, task_id: &str) -> Result<AgentTaskDetailEnvelope> {
        let started = std::time::Instant::now();
        loop {
            let detail = self.task_detail(task_id).await?;
            if detail.detail.task.status.is_terminal() {
                return Ok(detail);
            }
            if started.elapsed() >= WAIT_TIMEOUT {
                anyhow::bail!("timed out waiting for agent task {task_id}");
            }
            tokio::time::sleep(WAIT_INTERVAL).await;
        }
    }

    async fn wait_for_thread_turn(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<AgentThreadDetailEnvelope> {
        let started = std::time::Instant::now();
        loop {
            let detail = self.thread_detail(thread_id).await?;
            if detail
                .detail
                .turns
                .iter()
                .find(|turn| turn.id == turn_id)
                .map(|turn| turn.status.is_terminal())
                .unwrap_or(false)
            {
                return Ok(detail);
            }
            if started.elapsed() >= WAIT_TIMEOUT {
                anyhow::bail!("timed out waiting for agent turn {turn_id}");
            }
            tokio::time::sleep(WAIT_INTERVAL).await;
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

async fn decode_response<T>(response: reqwest::Response) -> Result<T>
where
    T: DeserializeOwned,
{
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("agent daemon request failed with {status}: {body}");
    }
    serde_json::from_str(&body).with_context(|| format!("invalid agent daemon JSON: {body}"))
}

fn render_task_accepted(envelope: &AgentTaskAcceptedEnvelope) -> String {
    format!(
        "Accepted agent task {}\nstatus: {}\ntask: {}\nevents: {}",
        envelope.task.id,
        envelope.task.status.as_str(),
        envelope.task_url,
        envelope.events_url
    )
}

fn render_task_list(envelope: &AgentTaskListEnvelope) -> String {
    if envelope.tasks.is_empty() {
        return "No agent tasks.".to_string();
    }
    envelope
        .tasks
        .iter()
        .map(|task| {
            format!(
                "{}  {}  {}",
                task.id,
                task.status.as_str(),
                one_line(&task.goal)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_thread_accepted(envelope: &AgentThreadAcceptedEnvelope) -> String {
    let mut rendered = format!(
        "Accepted agent thread {}\nturn: {}\nthread status: {}\nturn status: {}\nthread: {}\nevents: {}",
        envelope.thread.id,
        envelope.turn.id,
        envelope.thread.status.as_str(),
        envelope.turn.status.as_str(),
        envelope.thread_url,
        envelope.events_url
    );
    if let Some(mailbox_item) = &envelope.mailbox_item {
        rendered.push_str(&format!(
            "\nmailbox: {} ({})",
            mailbox_item.id, mailbox_item.delivery
        ));
    }
    rendered
}

fn render_thread_list(envelope: &AgentThreadListEnvelope) -> String {
    if envelope.threads.is_empty() {
        return "No agent threads.".to_string();
    }
    envelope
        .threads
        .iter()
        .map(|thread| {
            format!(
                "{}  {}  {}",
                thread.id,
                thread.status.as_str(),
                one_line(&thread.title)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_thread_detail(envelope: &AgentThreadDetailEnvelope) -> String {
    let thread = &envelope.detail.thread;
    let mut lines = vec![
        format!("Thread {}", thread.id),
        format!("status: {}", thread.status.as_str()),
        format!("title: {}", thread.title),
        format!("goal: {}", thread.goal),
    ];
    if let Some(turn) = envelope.detail.turns.last() {
        lines.push(format!("last turn: {} ({})", turn.id, turn.status.as_str()));
        if let Some(error) = &turn.error {
            lines.push(format!("last error: {error}"));
        }
        if let Some(final_answer) = turn
            .result
            .as_ref()
            .and_then(|result| result.get("finalAnswer"))
            .and_then(|value| value.as_str())
        {
            lines.push(format!("last final: {final_answer}"));
        }
    }
    lines.push(format!("turns: {}", envelope.detail.turns.len()));
    lines.push(format!("items: {}", envelope.detail.items.len()));
    lines.push(format!(
        "approval requests: {}",
        envelope.detail.approval_requests.len()
    ));
    lines.push(format!("events: {}", envelope.detail.events.len()));
    lines.join("\n")
}

fn render_approval_response(value: &serde_json::Value) -> String {
    let approval = &value["approvalRequest"];
    format!(
        "Approval request {}\nstatus: {}\ntool: {}\nthread: {}\nevents: {}",
        approval["id"].as_str().unwrap_or("unknown"),
        approval["status"].as_str().unwrap_or("unknown"),
        approval["toolName"].as_str().unwrap_or("unknown"),
        value["threadUrl"].as_str().unwrap_or(""),
        value["eventsUrl"].as_str().unwrap_or("")
    )
}

fn render_compaction_response(value: &serde_json::Value) -> String {
    format!(
        "Context compaction for {}\ncreated: {}\nitems: {}\nevents: {}",
        value["thread"]["id"].as_str().unwrap_or("unknown"),
        value["created"].as_bool().unwrap_or(false),
        value["itemsUrl"].as_str().unwrap_or(""),
        value["eventsUrl"].as_str().unwrap_or("")
    )
}

fn render_thread_events(envelope: &AgentThreadEventsEnvelope) -> String {
    if envelope.events.is_empty() {
        return format!("No events for {}.", envelope.thread_id);
    }
    envelope
        .events
        .iter()
        .map(|event| {
            let payload = if event.payload == json!({}) {
                String::new()
            } else {
                format!(
                    " {}",
                    serde_json::to_string(&event.payload).unwrap_or_else(|_| "{}".to_string())
                )
            };
            let turn = event
                .turn_id
                .as_deref()
                .map(|turn_id| format!(" {turn_id}"))
                .unwrap_or_default();
            format!(
                "{}{}  {}  {}{}",
                event.sequence, turn, event.kind, event.message, payload
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_task_detail(envelope: &AgentTaskDetailEnvelope) -> String {
    let task = &envelope.detail.task;
    let mut lines = vec![
        format!("Task {}", task.id),
        format!("status: {}", task.status.as_str()),
        format!("goal: {}", task.goal),
    ];
    if let Some(error) = &task.error {
        lines.push(format!("error: {error}"));
    }
    if let Some(result) = &task.result {
        if let Some(final_answer) = result.get("finalAnswer").and_then(|value| value.as_str()) {
            lines.push(format!("final: {final_answer}"));
        } else {
            lines.push(format!(
                "result: {}",
                serde_json::to_string(result).unwrap_or_else(|_| "{}".to_string())
            ));
        }
    }
    lines.push(format!("messages: {}", envelope.detail.messages.len()));
    lines.push(format!("tool calls: {}", envelope.detail.tool_calls.len()));
    lines.push(format!("events: {}", envelope.detail.events.len()));
    lines.join("\n")
}

fn render_task_events(envelope: &AgentEventsEnvelope) -> String {
    if envelope.events.is_empty() {
        return format!("No events for {}.", envelope.task_id);
    }
    envelope
        .events
        .iter()
        .map(|event| {
            let payload = if event.payload == json!({}) {
                String::new()
            } else {
                format!(
                    " {}",
                    serde_json::to_string(&event.payload).unwrap_or_else(|_| "{}".to_string())
                )
            };
            format!(
                "{}  {}  {}{}",
                event.sequence, event.kind, event.message, payload
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_agent_card(card: &AgentCardEnvelope) -> String {
    let endpoints = card
        .endpoints
        .iter()
        .map(|endpoint| format!("{} {}", endpoint.method, endpoint.path))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{}\n{}\n\nTools:\n{}\n\nEndpoints:\n{}",
        card.name,
        card.description,
        card.tools.join("\n"),
        endpoints
    )
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::render_agent_card_json;
    use crate::agent::model::{
        AgentCapability, AgentCardEnvelope, AgentEndpoint, AgentProviderCapabilities,
    };

    #[test]
    fn agent_card_json_output_has_stable_thread_tool_shape() {
        let card = AgentCardEnvelope {
            kind: "issue_finder_agent_card".to_string(),
            version: 1,
            name: "Issue Finder Agent".to_string(),
            description: "Local Issue Finder daemon.".to_string(),
            endpoints: vec![AgentEndpoint {
                method: "POST".to_string(),
                path: "/a2a/threads/start".to_string(),
                description: "Start a resumable thread.".to_string(),
            }],
            input_modes: vec!["natural_language".to_string()],
            output_modes: vec!["json".to_string()],
            tools: vec![
                "issue-finder.status".to_string(),
                "issue-finder.discover_candidates".to_string(),
                "issue-finder.inspect_candidate".to_string(),
                "issue-finder.assess".to_string(),
            ],
            tool_definitions: vec![json!({
                "canonicalName": "issue-finder.discover_candidates",
                "name": "discover_candidates",
                "exposure": "direct"
            })],
            capabilities: vec![AgentCapability {
                name: "thread.start".to_string(),
                status: "supported".to_string(),
                method: Some("POST".to_string()),
                path: Some("/a2a/threads/start".to_string()),
                limits: json!({"maxTurns": 8}),
                provider_requirements: Vec::new(),
            }],
            provider: AgentProviderCapabilities {
                wire_api: "responses".to_string(),
                native_tools: true,
                deferred_tools: true,
            },
        };

        let rendered = render_agent_card_json(&card).unwrap();
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value["kind"], "issue_finder_agent_card");
        assert_eq!(value["tools"][1], "issue-finder.discover_candidates");
        assert_eq!(
            value["toolDefinitions"][0]["canonicalName"],
            "issue-finder.discover_candidates"
        );
        assert_eq!(value["capabilities"][0]["name"], "thread.start");
        assert_eq!(value["capabilities"][0]["path"], "/a2a/threads/start");
        assert_eq!(value["provider"]["wireApi"], "responses");
        assert_eq!(value["provider"]["nativeTools"], true);
    }
}
