use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::paths::IssueFinderPaths;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexThread {
    pub id: String,
    pub name: Option<String>,
    pub cwd: Option<String>,
    pub status: Value,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexRuntimeTurn {
    pub id: String,
    pub thread_id: String,
    pub status: String,
    pub payload: Value,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexRuntimeItem {
    pub id: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub item_type: String,
    pub payload: Value,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PendingRequest {
    pub id: String,
    pub connection_epoch: String,
    pub wire_id: Value,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub request_type: String,
    pub method: String,
    pub payload: Value,
    pub status: String,
    pub response: Option<Value>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeOutboxEntry {
    pub id: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub status: String,
    pub method: String,
    pub payload: Value,
}

pub struct CodexRuntimeStore {
    conn: Connection,
}

impl CodexRuntimeStore {
    pub fn open(paths: &IssueFinderPaths) -> Result<Self> {
        paths.ensure_layout()?;
        std::fs::create_dir_all(paths.dispatch_dir())?;
        let _ = crate::dispatch::DispatchStore::open(paths.clone())?;
        let conn = Connection::open(paths.dispatch_db_path())?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        Ok(Self { conn })
    }
    pub fn project_notification(
        &self,
        method: &str,
        params_json: &Value,
        delivery: &str,
    ) -> Result<()> {
        let thread_id = params_json
            .get("threadId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let turn_id = params_json
            .get("turnId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let now = Utc::now().to_rfc3339();
        let id = format!(
            "native-event-{}-{}",
            Utc::now().timestamp_micros(),
            self.conn.last_insert_rowid() + 1
        );
        self.conn.execute("INSERT INTO codex_runtime_events(id,thread_id,turn_id,method,delivery,payload_json,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![id,thread_id,turn_id,method,delivery,params_json.to_string(),now])?;
        if let Some(thread) = params_json.get("thread") {
            self.upsert_thread(thread)?;
        }
        if let Some(turn) = params_json.get("turn") {
            if let Some(thread_id) = thread_id
                .as_deref()
                .or_else(|| turn.get("threadId").and_then(Value::as_str))
            {
                self.upsert_turn(thread_id, turn)?;
            }
        }
        if let Some(item) = params_json.get("item") {
            if let Some(thread_id) = thread_id.as_deref() {
                self.upsert_item(thread_id, turn_id.as_deref(), item)?;
            }
        }
        Ok(())
    }
    pub fn upsert_thread(&self, value: &Value) -> Result<()> {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("thread missing id"))?;
        let now = Utc::now().to_rfc3339();
        self.conn.execute("INSERT INTO codex_threads(id,name,cwd,status_json,payload_json,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?6) ON CONFLICT(id) DO UPDATE SET name=excluded.name,cwd=excluded.cwd,status_json=excluded.status_json,payload_json=excluded.payload_json,updated_at=excluded.updated_at",params![id,value.get("name").and_then(Value::as_str),value.get("cwd").and_then(Value::as_str),value.get("status").unwrap_or(&Value::Null).to_string(),value.to_string(),now])?;
        Ok(())
    }
    pub fn upsert_turn(&self, thread_id: &str, value: &Value) -> Result<()> {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("turn missing id"))?;
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let now = Utc::now().to_rfc3339();
        self.conn.execute("INSERT INTO codex_turns(id,thread_id,status,payload_json,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?5) ON CONFLICT(id) DO UPDATE SET status=excluded.status,payload_json=excluded.payload_json,updated_at=excluded.updated_at",params![id,thread_id,status,value.to_string(),now])?;
        Ok(())
    }
    pub fn upsert_item(&self, thread_id: &str, turn_id: Option<&str>, value: &Value) -> Result<()> {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("item missing id"))?;
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let now = Utc::now().to_rfc3339();
        self.conn.execute("INSERT INTO codex_items(id,thread_id,turn_id,item_type,payload_json,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?6) ON CONFLICT(thread_id,turn_id,id) DO UPDATE SET item_type=excluded.item_type,payload_json=excluded.payload_json,updated_at=excluded.updated_at",params![id,thread_id,turn_id.unwrap_or(""),kind,value.to_string(),now])?;
        Ok(())
    }
    pub fn enqueue(
        &self,
        id: &str,
        thread_id: &str,
        method: &str,
        client_id: Option<&str>,
        payload: &Value,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        let changed = self.conn.execute("INSERT OR IGNORE INTO codex_outbox(id,thread_id,method,client_message_id,payload_json,status,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,'pending',?6,?6)",params![id,thread_id,method,client_id,payload.to_string(),now])?;
        Ok(changed == 1)
    }
    pub fn outbox_entry(&self, id: &str) -> Result<Option<RuntimeOutboxEntry>> {
        self.conn
            .query_row(
                "SELECT id,thread_id,turn_id,status,method,payload_json FROM codex_outbox WHERE id=?1",
                params![id],
                |row| {
                    Ok(RuntimeOutboxEntry {
                        id: row.get(0)?,
                        thread_id: row.get(1)?,
                        turn_id: row.get(2)?,
                        status: row.get(3)?,
                        method: row.get(4)?,
                        payload: serde_json::from_str(&row.get::<_, String>(5)?)
                            .unwrap_or(Value::Null),
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }
    pub fn pending_outbox_entries(&self, thread_id: &str) -> Result<Vec<RuntimeOutboxEntry>> {
        let mut statement = self.conn.prepare(
            "SELECT id,thread_id,turn_id,status,method,payload_json
             FROM codex_outbox
             WHERE thread_id=?1 AND status='pending'
             ORDER BY created_at,id",
        )?;
        let rows = statement.query_map(params![thread_id], |row| {
            Ok(RuntimeOutboxEntry {
                id: row.get(0)?,
                thread_id: row.get(1)?,
                turn_id: row.get(2)?,
                status: row.get(3)?,
                method: row.get(4)?,
                payload: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or(Value::Null),
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn mark_sent(&self, id: &str, turn_id: Option<&str>) -> Result<()> {
        self.conn.execute("UPDATE codex_outbox SET status='sent',turn_id=?2,attempts=attempts+1,updated_at=?3 WHERE id=?1",params![id,turn_id,Utc::now().to_rfc3339()])?;
        Ok(())
    }
    pub fn enqueue_control(
        &self,
        id: &str,
        thread_id: &str,
        method: &str,
        payload: &Value,
    ) -> Result<bool> {
        self.enqueue(id, thread_id, method, None, payload)
    }

    pub fn pending_control_entries(&self, thread_id: &str) -> Result<Vec<RuntimeOutboxEntry>> {
        let mut statement = self.conn.prepare(
            "SELECT id,thread_id,turn_id,status,method,payload_json FROM codex_outbox WHERE thread_id=?1 AND status='pending' AND method != 'turn/start' ORDER BY created_at,id",
        )?;
        let rows = statement.query_map(params![thread_id], |row| {
            Ok(RuntimeOutboxEntry {
                id: row.get(0)?,
                thread_id: row.get(1)?,
                turn_id: row.get(2)?,
                status: row.get(3)?,
                method: row.get(4)?,
                payload: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or(Value::Null),
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn queue_pending_response(&self, request_id: &str, response: &Value) -> Result<()> {
        let response_json = response.to_string();
        let changed = self.conn.execute(
            "UPDATE pending_requests SET status='response_ready',response_json=?2 WHERE id=?1 AND status='pending'",
            params![request_id, response_json],
        )?;
        if changed == 0 {
            let existing: Option<(String, Option<String>)> = self
                .conn
                .query_row(
                    "SELECT status,response_json FROM pending_requests WHERE id=?1",
                    params![request_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            match existing {
                Some((_, Some(value))) if value == response_json => {}
                Some((status, _)) => anyhow::bail!(
                    "pending request {request_id} cannot be answered from status {status}"
                ),
                None => anyhow::bail!("pending request {request_id} not found"),
            }
        }
        Ok(())
    }

    pub fn ready_responses(&self) -> Result<Vec<PendingRequest>> {
        self.requests_with_status("response_ready")
    }
    pub fn record_server_request(
        &self,
        scoped_id: &str,
        connection_epoch: &str,
        wire_id: &Value,
        method: &str,
        payload: &Value,
    ) -> Result<()> {
        let thread_id = payload.get("threadId").and_then(Value::as_str);
        let turn_id = payload.get("turnId").and_then(Value::as_str);
        let request_type = request_type(method);
        self.conn.execute("INSERT INTO pending_requests(id,connection_epoch,wire_id_json,thread_id,turn_id,request_type,method,payload_json,status,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'pending',?9) ON CONFLICT(id) DO NOTHING",params![scoped_id,connection_epoch,wire_id.to_string(),thread_id,turn_id,request_type,method,payload.to_string(),Utc::now().to_rfc3339()])?;
        Ok(())
    }
    pub fn record_candidate_user_request(
        &self,
        id: &str,
        thread_id: Option<&str>,
        turn_id: Option<&str>,
        payload: &Value,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO pending_requests(id,connection_epoch,wire_id_json,thread_id,turn_id,request_type,method,payload_json,status,created_at) VALUES(?1,'candidate_result','null',?2,?3,'user_input','issue-finder.submit_result/needs_user',?4,'pending',?5) ON CONFLICT(id) DO NOTHING",
            params![id, thread_id, turn_id, payload.to_string(), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }
    pub fn resolve_server_request(&self, request_id: &str, response: &Value) -> Result<()> {
        self.conn.execute("UPDATE pending_requests SET status='resolved',response_json=?2,resolved_at=?3 WHERE id=?1", params![request_id,response.to_string(),Utc::now().to_rfc3339()])?;
        Ok(())
    }
    pub fn thread(&self, id: &str) -> Result<Option<CodexThread>> {
        self.conn
            .query_row(
                "SELECT id,name,cwd,status_json,updated_at FROM codex_threads WHERE id=?1",
                params![id],
                |r| {
                    Ok(CodexThread {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        cwd: r.get(2)?,
                        status: serde_json::from_str(&r.get::<_, String>(3)?)
                            .unwrap_or(Value::Null),
                        updated_at: r.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }
    pub fn items(&self, thread_id: &str) -> Result<Vec<CodexRuntimeItem>> {
        let mut statement = self.conn.prepare("SELECT id,thread_id,turn_id,item_type,payload_json,updated_at FROM codex_items WHERE thread_id=?1 ORDER BY created_at,id")?;
        let rows = statement.query_map(params![thread_id], |r| {
            Ok(CodexRuntimeItem {
                id: r.get(0)?,
                thread_id: r.get(1)?,
                turn_id: r
                    .get::<_, String>(2)
                    .map(|value| (!value.is_empty()).then_some(value))?,
                item_type: r.get(3)?,
                payload: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
                updated_at: r.get(5)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn latest_turn(&self, thread_id: &str) -> Result<Option<CodexRuntimeTurn>> {
        self.conn.query_row("SELECT id,thread_id,status,payload_json,updated_at FROM codex_turns WHERE thread_id=?1 ORDER BY updated_at DESC,id DESC LIMIT 1",params![thread_id],|row| Ok(CodexRuntimeTurn { id: row.get(0)?, thread_id: row.get(1)?, status: row.get(2)?, payload: serde_json::from_str(&row.get::<_,String>(3)?).unwrap_or(Value::Null), updated_at: row.get(4)? })).optional().map_err(Into::into)
    }
    pub fn pending_server_requests(&self) -> Result<Vec<PendingRequest>> {
        self.requests_with_status("pending")
    }

    fn requests_with_status(&self, expected_status: &str) -> Result<Vec<PendingRequest>> {
        let mut statement=self.conn.prepare("SELECT id,connection_epoch,wire_id_json,thread_id,turn_id,request_type,method,payload_json,status,response_json FROM pending_requests WHERE status=?1 ORDER BY created_at,id")?;
        let rows = statement.query_map(params![expected_status], |r| {
            Ok(PendingRequest {
                id: r.get(0)?,
                connection_epoch: r.get(1)?,
                wire_id: serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or(Value::Null),
                thread_id: r.get(3)?,
                turn_id: r.get(4)?,
                request_type: r.get(5)?,
                method: r.get(6)?,
                payload: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or(Value::Null),
                status: r.get(8)?,
                response: r
                    .get::<_, Option<String>>(9)?
                    .and_then(|value| serde_json::from_str(&value).ok()),
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
}

fn request_type(method: &str) -> &'static str {
    if method.contains("requestUserInput") || method.contains("elicitation") {
        "user_input"
    } else if method.contains("approval") || method.contains("requestApproval") {
        "approval"
    } else if method.contains("permission") {
        "permission"
    } else {
        "server_request"
    }
}
