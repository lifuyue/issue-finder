use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::{AppServerClient, AppServerEvent, NativeThreadStore};

pub struct SendTurnRequest {
    pub thread_id: String,
    pub prompt: String,
    pub cwd: String,
    pub client_user_message_id: String,
}
pub struct StartedTurn {
    pub thread_id: String,
    pub turn_id: String,
    pub client_user_message_id: String,
}
pub struct NativeThreadManager {
    client: AppServerClient,
    store: NativeThreadStore,
    connection_epoch: String,
}

impl NativeThreadManager {
    pub async fn connect(store: NativeThreadStore) -> Result<Self> {
        store.orphan_pending_server_requests()?;
        Ok(Self {
            client: AppServerClient::connect().await?,
            store,
            connection_epoch: connection_epoch(),
        })
    }
    pub async fn reconnect(&mut self, thread_ids: &[String]) -> Result<()> {
        self.store.orphan_pending_server_requests()?;
        self.client = AppServerClient::connect().await?;
        self.connection_epoch = connection_epoch();
        for thread_id in thread_ids {
            self.resume(thread_id).await?;
            self.reconcile(thread_id).await?;
        }
        Ok(())
    }
    pub async fn start_thread(&self, name: &str, cwd: &str) -> Result<String> {
        let value=self.client.request("thread/start",json!({"cwd":cwd,"runtimeWorkspaceRoots":[cwd],"approvalPolicy":"on-request","sandbox":"workspace-write"})).await?;
        let thread = value.get("thread").context("thread/start missing thread")?;
        self.store.upsert_thread(thread)?;
        let id = thread
            .get("id")
            .and_then(Value::as_str)
            .context("thread/start missing id")?
            .to_string();
        self.client
            .request("thread/name/set", json!({"threadId":id,"name":name}))
            .await?;
        Ok(id)
    }
    pub async fn resume(&self, thread_id: &str) -> Result<()> {
        let value = self
            .client
            .request(
                "thread/resume",
                json!({"threadId":thread_id,"excludeTurns":true}),
            )
            .await?;
        if let Some(thread) = value.get("thread") {
            self.store.upsert_thread(thread)?;
        }
        Ok(())
    }
    pub async fn rename(&self, thread_id: &str, name: &str) -> Result<()> {
        self.client
            .request("thread/name/set", json!({"threadId":thread_id,"name":name}))
            .await?;
        Ok(())
    }
    pub async fn set_goal(&self, thread_id: &str, objective: &str) -> Result<()> {
        self.client
            .request(
                "thread/goal/set",
                json!({"threadId":thread_id,"objective":objective}),
            )
            .await?;
        Ok(())
    }
    pub async fn send(&self, request: SendTurnRequest) -> Result<StartedTurn> {
        let outbox_id = format!("outbox:{}", request.client_user_message_id);
        let payload = json!({"threadId":request.thread_id,"clientUserMessageId":request.client_user_message_id,"input":[{"type":"text","text":request.prompt}],"cwd":request.cwd,"runtimeWorkspaceRoots":[request.cwd],"approvalPolicy":"on-request","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[request.cwd],"networkAccess":false}});
        let inserted = self.store.enqueue(
            &outbox_id,
            &request.thread_id,
            "turn/start",
            Some(&request.client_user_message_id),
            &payload,
        )?;
        if !inserted {
            let existing = self
                .store
                .outbox_entry(&outbox_id)?
                .context("native outbox entry disappeared")?;
            if existing.thread_id != request.thread_id || existing.payload != payload {
                anyhow::bail!("native outbox idempotency conflict for {outbox_id}");
            }
            if existing.status == "sent" {
                let turn_id = existing
                    .turn_id
                    .context("sent native outbox entry has no turn id")?;
                return Ok(StartedTurn {
                    thread_id: request.thread_id,
                    turn_id,
                    client_user_message_id: request.client_user_message_id,
                });
            }
            anyhow::bail!(
                "native outbox {outbox_id} has uncertain status {}; reconcile before retry",
                existing.status
            );
        }
        let value = self.client.request("turn/start", payload).await?;
        crate::eval_fault::crash_at("after_turn_accept_before_projection");
        let turn = value.get("turn").context("turn/start missing turn")?;
        self.store.upsert_turn(&request.thread_id, turn)?;
        let turn_id = turn
            .get("id")
            .and_then(Value::as_str)
            .context("turn/start missing id")?
            .to_string();
        self.store.mark_sent(&outbox_id, Some(&turn_id))?;
        Ok(StartedTurn {
            thread_id: request.thread_id,
            turn_id,
            client_user_message_id: request.client_user_message_id,
        })
    }
    pub async fn steer(
        &self,
        thread_id: &str,
        turn_id: &str,
        prompt: &str,
        client_id: &str,
    ) -> Result<()> {
        self.client.request("turn/steer",json!({"threadId":thread_id,"expectedTurnId":turn_id,"clientUserMessageId":client_id,"input":[{"type":"text","text":prompt}]})).await?;
        Ok(())
    }
    pub async fn interrupt(&self, thread_id: &str, turn_id: &str) -> Result<()> {
        self.client
            .request(
                "turn/interrupt",
                json!({"threadId":thread_id,"turnId":turn_id}),
            )
            .await?;
        Ok(())
    }
    pub async fn reconcile(&self, thread_id: &str) -> Result<()> {
        let value = self
            .client
            .request(
                "thread/read",
                json!({"threadId":thread_id,"includeTurns":true}),
            )
            .await?;
        let thread = value.get("thread").context("thread/read missing thread")?;
        self.store.upsert_thread(thread)?;
        for turn in thread
            .get("turns")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.store.upsert_turn(thread_id, turn)?;
            let tid = turn.get("id").and_then(Value::as_str);
            for item in turn
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                self.store.upsert_item(thread_id, tid, item)?;
            }
        }
        self.reconcile_pending_outbox(thread_id, thread)?;
        Ok(())
    }

    fn reconcile_pending_outbox(&self, thread_id: &str, thread: &Value) -> Result<()> {
        let turns = thread
            .get("turns")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for entry in self.store.pending_outbox_entries(thread_id)? {
            let client_message_id = entry
                .payload
                .get("clientUserMessageId")
                .and_then(Value::as_str);
            let prompt = entry
                .payload
                .get("input")
                .and_then(Value::as_array)
                .and_then(|input| input.first())
                .and_then(|item| item.get("text"))
                .and_then(Value::as_str);
            let matches = turns
                .iter()
                .filter(|turn| {
                    client_message_id.is_some_and(|expected| {
                        turn.get("clientUserMessageId").and_then(Value::as_str) == Some(expected)
                            || turn
                                .get("items")
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                                .any(|item| {
                                    item.get("clientUserMessageId").and_then(Value::as_str)
                                        == Some(expected)
                                        || item.get("id").and_then(Value::as_str) == Some(expected)
                                })
                    }) || prompt.is_some_and(|expected| turn_contains_user_prompt(turn, expected))
                })
                .collect::<Vec<_>>();
            if matches.len() > 1 {
                anyhow::bail!(
                    "native outbox {} matches multiple persisted turns and cannot be reconciled",
                    entry.id
                );
            }
            if let Some(turn) = matches.first() {
                let turn_id = turn
                    .get("id")
                    .and_then(Value::as_str)
                    .context("reconciled native turn is missing id")?;
                self.store.mark_sent(&entry.id, Some(turn_id))?;
            }
        }
        Ok(())
    }
    pub async fn pump_once(&mut self) -> Result<bool> {
        let Some(event) = self.client.next_event().await else {
            return Ok(false);
        };
        match event {
            AppServerEvent::Notification { method, params } => self.store.project_notification(
                &method,
                &params,
                if lossless(&method) {
                    "lossless"
                } else {
                    "best_effort"
                },
            )?,
            AppServerEvent::ServerRequest(request) => {
                let scoped_id = format!("{}:{}", self.connection_epoch, request.id);
                self.store.record_server_request(
                    &scoped_id,
                    &self.connection_epoch,
                    &request.id,
                    &request.method,
                    &request.params,
                )?;
            }
            AppServerEvent::Lagged { skipped } => self.store.project_notification(
                "runtime/lagged",
                &json!({"skipped":skipped}),
                "lossless",
            )?,
            AppServerEvent::Disconnected { message } => self.store.project_notification(
                "runtime/disconnected",
                &json!({"message":message}),
                "lossless",
            )?,
        };
        Ok(true)
    }
    pub async fn respond_server_request(
        &self,
        id: Value,
        result: std::result::Result<Value, Value>,
    ) -> Result<()> {
        self.client.respond(id, result).await
    }
    pub async fn decide_approval(&self, id: Value, decision: &str) -> Result<()> {
        let response = json!({"decision": decision});
        self.client
            .respond(id.clone(), Ok(response.clone()))
            .await?;
        self.store
            .resolve_server_request(&format!("{}:{}", self.connection_epoch, id), &response)
    }
    pub async fn answer_user_input(&self, id: Value, answers: Value) -> Result<()> {
        let response = json!({"answers": answers});
        self.client
            .respond(id.clone(), Ok(response.clone()))
            .await?;
        self.store
            .resolve_server_request(&format!("{}:{}", self.connection_epoch, id), &response)
    }
    pub async fn resolve_mcp_elicitation(&self, id: Value, response: Value) -> Result<()> {
        self.client
            .respond(id.clone(), Ok(response.clone()))
            .await?;
        self.store
            .resolve_server_request(&format!("{}:{}", self.connection_epoch, id), &response)
    }
    pub async fn shutdown(self) {
        self.client.shutdown().await
    }
}

fn turn_contains_user_prompt(turn: &Value, expected: &str) -> bool {
    turn.get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|item| {
            item.get("type").and_then(Value::as_str) == Some("userMessage")
                && item.get("text").and_then(Value::as_str).or_else(|| {
                    item.get("content")
                        .and_then(Value::as_array)
                        .and_then(|content| content.first())
                        .and_then(|content| content.get("text"))
                        .and_then(Value::as_str)
                }) == Some(expected)
        })
}
fn connection_epoch() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    )
}
fn lossless(method: &str) -> bool {
    matches!(
        method,
        "turn/completed"
            | "item/completed"
            | "item/agentMessage/delta"
            | "item/reasoning/textDelta"
            | "item/reasoning/summaryTextDelta"
    )
}
