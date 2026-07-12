use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::paths::IssueFinderPaths;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NativeThread {
    pub id: String,
    pub name: Option<String>,
    pub cwd: Option<String>,
    pub status: Value,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NativeTurn {
    pub id: String,
    pub thread_id: String,
    pub status: String,
    pub payload: Value,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NativeItem {
    pub id: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub item_type: String,
    pub payload: Value,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NativePendingRequest {
    pub id: String,
    pub method: String,
    pub payload: Value,
}

pub struct NativeThreadStore {
    conn: Connection,
}

impl NativeThreadStore {
    pub fn open(paths: &IssueFinderPaths) -> Result<Self> {
        paths.ensure_layout()?;
        std::fs::create_dir_all(paths.dispatch_dir())?;
        let conn = Connection::open(paths.dispatch_db_path())?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        let store = Self { conn };
        store.initialize()?;
        Ok(store)
    }
    fn initialize(&self) -> Result<()> {
        self.conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS native_threads(id TEXT PRIMARY KEY,name TEXT,cwd TEXT,status_json TEXT NOT NULL,payload_json TEXT NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS native_turns(id TEXT PRIMARY KEY,thread_id TEXT NOT NULL,status TEXT NOT NULL,payload_json TEXT NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,FOREIGN KEY(thread_id) REFERENCES native_threads(id) ON DELETE CASCADE);
        CREATE TABLE IF NOT EXISTS native_items(id TEXT NOT NULL,thread_id TEXT NOT NULL,turn_id TEXT NOT NULL DEFAULT '',item_type TEXT NOT NULL,payload_json TEXT NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,PRIMARY KEY(thread_id,turn_id,id),FOREIGN KEY(thread_id) REFERENCES native_threads(id) ON DELETE CASCADE);
        CREATE TABLE IF NOT EXISTS native_runtime_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT NOT NULL UNIQUE,thread_id TEXT,turn_id TEXT,method TEXT NOT NULL,delivery TEXT NOT NULL,payload_json TEXT NOT NULL,created_at TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS native_outbox(id TEXT PRIMARY KEY,thread_id TEXT NOT NULL,turn_id TEXT,method TEXT NOT NULL,client_message_id TEXT,payload_json TEXT NOT NULL,status TEXT NOT NULL,attempts INTEGER NOT NULL DEFAULT 0,last_error TEXT,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,UNIQUE(client_message_id));
        CREATE TABLE IF NOT EXISTS native_pending_server_requests(id TEXT PRIMARY KEY,thread_id TEXT,method TEXT NOT NULL,payload_json TEXT NOT NULL,status TEXT NOT NULL,response_json TEXT,created_at TEXT NOT NULL,resolved_at TEXT);
        CREATE INDEX IF NOT EXISTS idx_native_turns_thread ON native_turns(thread_id,updated_at);
        CREATE INDEX IF NOT EXISTS idx_native_items_thread ON native_items(thread_id,updated_at);
        CREATE INDEX IF NOT EXISTS idx_native_events_thread ON native_runtime_events(thread_id,sequence);
    "#)?;
        self.migrate_native_items_identity()?;
        Ok(())
    }
    fn migrate_native_items_identity(&self) -> Result<()> {
        let mut statement = self.conn.prepare("PRAGMA table_info(native_items)")?;
        let primary_key_columns = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|(_, position)| *position > 0)
            .map(|(name, position)| (position, name))
            .collect::<Vec<_>>();
        let mut primary_key_columns = primary_key_columns;
        primary_key_columns.sort_by_key(|(position, _)| *position);
        let names = primary_key_columns
            .into_iter()
            .map(|(_, name)| name)
            .collect::<Vec<_>>();
        if names == ["thread_id", "turn_id", "id"] {
            return Ok(());
        }
        self.conn.execute_batch(
            r#"
            PRAGMA foreign_keys=OFF;
            BEGIN IMMEDIATE;
            ALTER TABLE native_items RENAME TO native_items_legacy_identity;
            CREATE TABLE native_items(
                id TEXT NOT NULL,
                thread_id TEXT NOT NULL,
                turn_id TEXT NOT NULL DEFAULT '',
                item_type TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(thread_id,turn_id,id),
                FOREIGN KEY(thread_id) REFERENCES native_threads(id) ON DELETE CASCADE
            );
            INSERT INTO native_items(id,thread_id,turn_id,item_type,payload_json,created_at,updated_at)
                SELECT id,thread_id,COALESCE(turn_id,''),item_type,payload_json,created_at,updated_at
                FROM native_items_legacy_identity;
            DROP TABLE native_items_legacy_identity;
            CREATE INDEX idx_native_items_thread ON native_items(thread_id,created_at);
            COMMIT;
            PRAGMA foreign_keys=ON;
            "#,
        )?;
        Ok(())
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
        self.conn.execute("INSERT INTO native_runtime_events(id,thread_id,turn_id,method,delivery,payload_json,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![id,thread_id,turn_id,method,delivery,params_json.to_string(),now])?;
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
        self.conn.execute("INSERT INTO native_threads(id,name,cwd,status_json,payload_json,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?6) ON CONFLICT(id) DO UPDATE SET name=excluded.name,cwd=excluded.cwd,status_json=excluded.status_json,payload_json=excluded.payload_json,updated_at=excluded.updated_at",params![id,value.get("name").and_then(Value::as_str),value.get("cwd").and_then(Value::as_str),value.get("status").unwrap_or(&Value::Null).to_string(),value.to_string(),now])?;
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
        self.conn.execute("INSERT INTO native_turns(id,thread_id,status,payload_json,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?5) ON CONFLICT(id) DO UPDATE SET status=excluded.status,payload_json=excluded.payload_json,updated_at=excluded.updated_at",params![id,thread_id,status,value.to_string(),now])?;
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
        self.conn.execute("INSERT INTO native_items(id,thread_id,turn_id,item_type,payload_json,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?6) ON CONFLICT(thread_id,turn_id,id) DO UPDATE SET item_type=excluded.item_type,payload_json=excluded.payload_json,updated_at=excluded.updated_at",params![id,thread_id,turn_id.unwrap_or(""),kind,value.to_string(),now])?;
        Ok(())
    }
    pub fn enqueue(
        &self,
        id: &str,
        thread_id: &str,
        method: &str,
        client_id: Option<&str>,
        payload: &Value,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute("INSERT OR IGNORE INTO native_outbox(id,thread_id,method,client_message_id,payload_json,status,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,'pending',?6,?6)",params![id,thread_id,method,client_id,payload.to_string(),now])?;
        Ok(())
    }
    pub fn mark_sent(&self, id: &str, turn_id: Option<&str>) -> Result<()> {
        self.conn.execute("UPDATE native_outbox SET status='sent',turn_id=?2,attempts=attempts+1,updated_at=?3 WHERE id=?1",params![id,turn_id,Utc::now().to_rfc3339()])?;
        Ok(())
    }
    pub fn record_server_request(
        &self,
        request_id: &str,
        method: &str,
        payload: &Value,
    ) -> Result<()> {
        self.conn.execute("INSERT OR REPLACE INTO native_pending_server_requests(id,method,payload_json,status,created_at) VALUES(?1,?2,?3,'pending',?4)",params![request_id,method,payload.to_string(),Utc::now().to_rfc3339()])?;
        Ok(())
    }
    pub fn resolve_server_request(&self, request_id: &str, response: &Value) -> Result<()> {
        self.conn.execute("UPDATE native_pending_server_requests SET status='resolved',response_json=?2,resolved_at=?3 WHERE id=?1", params![request_id,response.to_string(),Utc::now().to_rfc3339()])?;
        Ok(())
    }
    pub fn thread(&self, id: &str) -> Result<Option<NativeThread>> {
        self.conn
            .query_row(
                "SELECT id,name,cwd,status_json,updated_at FROM native_threads WHERE id=?1",
                params![id],
                |r| {
                    Ok(NativeThread {
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
    pub fn items(&self, thread_id: &str) -> Result<Vec<NativeItem>> {
        let mut statement = self.conn.prepare("SELECT id,thread_id,turn_id,item_type,payload_json,updated_at FROM native_items WHERE thread_id=?1 ORDER BY created_at,id")?;
        let rows = statement.query_map(params![thread_id], |r| {
            Ok(NativeItem {
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
    pub fn pending_server_requests(&self) -> Result<Vec<NativePendingRequest>> {
        let mut statement=self.conn.prepare("SELECT id,method,payload_json FROM native_pending_server_requests WHERE status='pending' ORDER BY created_at,id")?;
        let rows = statement.query_map([], |r| {
            Ok(NativePendingRequest {
                id: r.get(0)?,
                method: r.get(1)?,
                payload: serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or(Value::Null),
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
}
