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
}

impl NativeThreadManager {
    pub async fn connect(store: NativeThreadStore) -> Result<Self> {
        Ok(Self {
            client: AppServerClient::connect().await?,
            store,
        })
    }
    pub async fn reconnect(&mut self, thread_ids: &[String]) -> Result<()> {
        self.client = AppServerClient::connect().await?;
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
        self.store.enqueue(
            &outbox_id,
            &request.thread_id,
            "turn/start",
            Some(&request.client_user_message_id),
            &payload,
        )?;
        let value = self.client.request("turn/start", payload).await?;
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
                self.store.record_server_request(
                    &request.id.to_string(),
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
            .resolve_server_request(&id.to_string(), &response)
    }
    pub async fn answer_user_input(&self, id: Value, answers: Value) -> Result<()> {
        let response = json!({"answers": answers});
        self.client
            .respond(id.clone(), Ok(response.clone()))
            .await?;
        self.store
            .resolve_server_request(&id.to_string(), &response)
    }
    pub async fn resolve_mcp_elicitation(&self, id: Value, response: Value) -> Result<()> {
        self.client
            .respond(id.clone(), Ok(response.clone()))
            .await?;
        self.store
            .resolve_server_request(&id.to_string(), &response)
    }
    pub async fn shutdown(self) {
        self.client.shutdown().await
    }
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
