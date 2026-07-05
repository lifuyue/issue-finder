use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, Row};
use serde_json::Value;

use crate::paths::IssueFinderPaths;

use super::model::{
    AgentEvent, AgentMessage, AgentTask, AgentTaskDetail, AgentTaskStatus, AgentThread,
    AgentThreadDetail, AgentThreadEvent, AgentThreadItem, AgentThreadStatus, AgentToolCall,
    AgentTurn,
};

static ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub struct AgentStore {
    paths: IssueFinderPaths,
    conn: Connection,
}

impl AgentStore {
    pub fn open(paths: IssueFinderPaths) -> Result<Self> {
        let db_path = paths.agent_db_path();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("unable to create {}", parent.display()))?;
        }
        let conn = Connection::open(&db_path)
            .with_context(|| format!("unable to open {}", db_path.display()))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        initialize_schema(&conn)?;
        Ok(Self { paths, conn })
    }

    pub fn db_path(&self) -> PathBuf {
        self.paths.agent_db_path()
    }

    pub fn create_task(&self, goal: &str, metadata: Value) -> Result<AgentTask> {
        let id = next_id("agent-task");
        let now = now();
        self.conn.execute(
            "INSERT INTO agent_tasks (
                id, goal, status, created_at, updated_at, completed_at,
                result_json, error, metadata_json
             )
             VALUES (?1, ?2, ?3, ?4, ?4, NULL, NULL, NULL, ?5)",
            params![
                id,
                goal,
                AgentTaskStatus::Queued.as_str(),
                now,
                json_text(&metadata)?
            ],
        )?;
        self.get_task(&id)
    }

    pub fn get_task(&self, id: &str) -> Result<AgentTask> {
        self.conn
            .query_row(
                "SELECT id, goal, status, created_at, updated_at, completed_at,
                        result_json, error, metadata_json
                 FROM agent_tasks
                 WHERE id = ?1",
                params![id],
                agent_task_from_row,
            )
            .with_context(|| format!("agent task {id} not found"))
    }

    pub fn list_tasks(&self, limit: usize) -> Result<Vec<AgentTask>> {
        let mut statement = self.conn.prepare(
            "SELECT id, goal, status, created_at, updated_at, completed_at,
                    result_json, error, metadata_json
             FROM agent_tasks
             ORDER BY created_at DESC, id DESC
             LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit.max(1) as i64], agent_task_from_row)?;
        collect_rows(rows)
    }

    pub fn update_task_status(
        &self,
        id: &str,
        status: AgentTaskStatus,
        result: Option<Value>,
        error: Option<String>,
    ) -> Result<AgentTask> {
        let now = now();
        let completed_at = status.is_terminal().then_some(now.as_str());
        self.conn.execute(
            "UPDATE agent_tasks
             SET status = ?2,
                 updated_at = ?3,
                 completed_at = COALESCE(?4, completed_at),
                 result_json = COALESCE(?5, result_json),
                 error = ?6
             WHERE id = ?1",
            params![
                id,
                status.as_str(),
                now,
                completed_at,
                optional_json_text(result.as_ref())?,
                error
            ],
        )?;
        self.get_task(id)
    }

    pub fn add_message(
        &self,
        task_id: &str,
        role: &str,
        content: &str,
        metadata: Value,
    ) -> Result<AgentMessage> {
        let id = next_id("agent-message");
        let created_at = now();
        self.conn.execute(
            "INSERT INTO agent_messages (id, task_id, role, content, metadata_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                task_id,
                role,
                content,
                json_text(&metadata)?,
                created_at
            ],
        )?;
        self.get_message(&id)
    }

    pub fn add_event(
        &self,
        task_id: &str,
        kind: &str,
        message: &str,
        payload: Value,
    ) -> Result<AgentEvent> {
        let id = next_id("agent-event");
        let created_at = now();
        self.conn.execute(
            "INSERT INTO agent_events (id, task_id, kind, message, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, task_id, kind, message, json_text(&payload)?, created_at],
        )?;
        self.get_event(&id)
    }

    pub fn start_tool_call(
        &self,
        task_id: &str,
        turn_index: usize,
        tool_name: &str,
        arguments: Value,
    ) -> Result<AgentToolCall> {
        let id = next_id("agent-tool-call");
        let created_at = now();
        self.conn.execute(
            "INSERT INTO agent_tool_calls (
                id, task_id, turn_index, tool_name, arguments_json, output_json,
                status, error, created_at, completed_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'running', NULL, ?6, NULL)",
            params![
                id,
                task_id,
                turn_index as i64,
                tool_name,
                json_text(&arguments)?,
                created_at
            ],
        )?;
        self.get_tool_call(&id)
    }

    pub fn finish_tool_call(
        &self,
        id: &str,
        status: &str,
        output: Option<Value>,
        error: Option<String>,
    ) -> Result<AgentToolCall> {
        let completed_at = now();
        self.conn.execute(
            "UPDATE agent_tool_calls
             SET status = ?2,
                 output_json = ?3,
                 error = ?4,
                 completed_at = ?5
             WHERE id = ?1",
            params![
                id,
                status,
                optional_json_text(output.as_ref())?,
                error,
                completed_at
            ],
        )?;
        self.get_tool_call(id)
    }

    pub fn detail(&self, task_id: &str) -> Result<AgentTaskDetail> {
        Ok(AgentTaskDetail {
            task: self.get_task(task_id)?,
            messages: self.list_messages(task_id)?,
            tool_calls: self.list_tool_calls(task_id)?,
            events: self.list_events(task_id)?,
        })
    }

    pub fn create_thread(&self, goal: &str, title: &str, metadata: Value) -> Result<AgentThread> {
        let id = next_id("agent-thread");
        let now = now();
        self.conn.execute(
            "INSERT INTO agent_threads (
                id, title, goal, status, created_at, updated_at,
                last_turn_id, metadata_json
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, NULL, ?6)",
            params![
                id,
                title,
                goal,
                AgentThreadStatus::Active.as_str(),
                now,
                json_text(&metadata)?
            ],
        )?;
        self.get_thread(&id)
    }

    pub fn get_thread(&self, id: &str) -> Result<AgentThread> {
        self.conn
            .query_row(
                "SELECT id, title, goal, status, created_at, updated_at,
                        last_turn_id, metadata_json
                 FROM agent_threads
                 WHERE id = ?1",
                params![id],
                agent_thread_from_row,
            )
            .with_context(|| format!("agent thread {id} not found"))
    }

    pub fn list_threads(&self, limit: usize) -> Result<Vec<AgentThread>> {
        let mut statement = self.conn.prepare(
            "SELECT id, title, goal, status, created_at, updated_at,
                    last_turn_id, metadata_json
             FROM agent_threads
             ORDER BY updated_at DESC, id DESC
             LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit.max(1) as i64], agent_thread_from_row)?;
        collect_rows(rows)
    }

    pub fn update_thread_status(&self, id: &str, status: AgentThreadStatus) -> Result<AgentThread> {
        let now = now();
        self.conn.execute(
            "UPDATE agent_threads
             SET status = ?2, updated_at = ?3
             WHERE id = ?1",
            params![id, status.as_str(), now],
        )?;
        self.get_thread(id)
    }

    pub fn create_turn(&self, thread_id: &str, input: &str, metadata: Value) -> Result<AgentTurn> {
        let thread = self.get_thread(thread_id)?;
        if !thread.status.accepts_new_turn() {
            anyhow::bail!(
                "agent thread {thread_id} cannot accept a new turn while status is {}",
                thread.status.as_str()
            );
        }

        let id = next_id("agent-turn");
        let now = now();
        self.conn.execute(
            "INSERT INTO agent_turns (
                id, thread_id, input, status, created_at, updated_at,
                completed_at, result_json, error, metadata_json
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, NULL, NULL, NULL, ?6)",
            params![
                id,
                thread_id,
                input,
                AgentTaskStatus::Queued.as_str(),
                now,
                json_text(&metadata)?
            ],
        )?;
        self.conn.execute(
            "UPDATE agent_threads
             SET last_turn_id = ?2, updated_at = ?3
             WHERE id = ?1",
            params![thread_id, id, now],
        )?;
        self.get_turn(&id)
    }

    pub fn get_turn(&self, id: &str) -> Result<AgentTurn> {
        self.conn
            .query_row(
                "SELECT id, thread_id, input, status, created_at, updated_at,
                        completed_at, result_json, error, metadata_json
                 FROM agent_turns
                 WHERE id = ?1",
                params![id],
                agent_turn_from_row,
            )
            .with_context(|| format!("agent turn {id} not found"))
    }

    pub fn update_turn_status(
        &self,
        id: &str,
        status: AgentTaskStatus,
        result: Option<Value>,
        error: Option<String>,
    ) -> Result<AgentTurn> {
        let now = now();
        let completed_at = status.is_terminal().then_some(now.as_str());
        self.conn.execute(
            "UPDATE agent_turns
             SET status = ?2,
                 updated_at = ?3,
                 completed_at = COALESCE(?4, completed_at),
                 result_json = COALESCE(?5, result_json),
                 error = ?6
             WHERE id = ?1",
            params![
                id,
                status.as_str(),
                now,
                completed_at,
                optional_json_text(result.as_ref())?,
                error
            ],
        )?;
        let turn = self.get_turn(id)?;
        self.conn.execute(
            "UPDATE agent_threads SET updated_at = ?2 WHERE id = ?1",
            params![turn.thread_id, now],
        )?;
        Ok(turn)
    }

    pub fn add_thread_item(&self, input: NewAgentThreadItem<'_>) -> Result<AgentThreadItem> {
        let id = next_id("agent-item");
        let created_at = now();
        self.conn.execute(
            "INSERT INTO agent_thread_items (
                id, thread_id, turn_id, item_type, role, content,
                tool_name, payload_json, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                input.thread_id,
                input.turn_id,
                input.item_type,
                input.role,
                input.content,
                input.tool_name,
                json_text(&input.payload)?,
                created_at
            ],
        )?;
        self.get_thread_item(&id)
    }

    pub fn add_thread_event(
        &self,
        thread_id: &str,
        turn_id: Option<&str>,
        kind: &str,
        message: &str,
        payload: Value,
    ) -> Result<AgentThreadEvent> {
        let id = next_id("agent-thread-event");
        let created_at = now();
        self.conn.execute(
            "INSERT INTO agent_thread_events (
                id, thread_id, turn_id, kind, message, payload_json, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                thread_id,
                turn_id,
                kind,
                message,
                json_text(&payload)?,
                created_at
            ],
        )?;
        self.get_thread_event(&id)
    }

    pub fn thread_detail(&self, thread_id: &str) -> Result<AgentThreadDetail> {
        Ok(AgentThreadDetail {
            thread: self.get_thread(thread_id)?,
            turns: self.list_thread_turns(thread_id)?,
            items: self.list_thread_items(thread_id)?,
            events: self.list_thread_events(thread_id)?,
        })
    }

    pub fn list_thread_turns(&self, thread_id: &str) -> Result<Vec<AgentTurn>> {
        let mut statement = self.conn.prepare(
            "SELECT id, thread_id, input, status, created_at, updated_at,
                    completed_at, result_json, error, metadata_json
             FROM agent_turns
             WHERE thread_id = ?1
             ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![thread_id], agent_turn_from_row)?;
        collect_rows(rows)
    }

    pub fn list_thread_items(&self, thread_id: &str) -> Result<Vec<AgentThreadItem>> {
        let mut statement = self.conn.prepare(
            "SELECT sequence, id, thread_id, turn_id, item_type, role, content,
                    tool_name, payload_json, created_at
             FROM agent_thread_items
             WHERE thread_id = ?1
             ORDER BY sequence",
        )?;
        let rows = statement.query_map(params![thread_id], agent_thread_item_from_row)?;
        collect_rows(rows)
    }

    pub fn list_thread_events(&self, thread_id: &str) -> Result<Vec<AgentThreadEvent>> {
        let mut statement = self.conn.prepare(
            "SELECT sequence, id, thread_id, turn_id, kind, message, payload_json, created_at
             FROM agent_thread_events
             WHERE thread_id = ?1
             ORDER BY sequence",
        )?;
        let rows = statement.query_map(params![thread_id], agent_thread_event_from_row)?;
        collect_rows(rows)
    }

    pub fn list_events(&self, task_id: &str) -> Result<Vec<AgentEvent>> {
        let mut statement = self.conn.prepare(
            "SELECT sequence, id, task_id, kind, message, payload_json, created_at
             FROM agent_events
             WHERE task_id = ?1
             ORDER BY sequence",
        )?;
        let rows = statement.query_map(params![task_id], agent_event_from_row)?;
        collect_rows(rows)
    }

    fn get_message(&self, id: &str) -> Result<AgentMessage> {
        self.conn
            .query_row(
                "SELECT id, task_id, role, content, metadata_json, created_at
                 FROM agent_messages
                 WHERE id = ?1",
                params![id],
                agent_message_from_row,
            )
            .with_context(|| format!("agent message {id} not found"))
    }

    fn list_messages(&self, task_id: &str) -> Result<Vec<AgentMessage>> {
        let mut statement = self.conn.prepare(
            "SELECT id, task_id, role, content, metadata_json, created_at
             FROM agent_messages
             WHERE task_id = ?1
             ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![task_id], agent_message_from_row)?;
        collect_rows(rows)
    }

    fn get_tool_call(&self, id: &str) -> Result<AgentToolCall> {
        self.conn
            .query_row(
                "SELECT id, task_id, turn_index, tool_name, arguments_json, output_json,
                        status, error, created_at, completed_at
                 FROM agent_tool_calls
                 WHERE id = ?1",
                params![id],
                agent_tool_call_from_row,
            )
            .with_context(|| format!("agent tool call {id} not found"))
    }

    fn list_tool_calls(&self, task_id: &str) -> Result<Vec<AgentToolCall>> {
        let mut statement = self.conn.prepare(
            "SELECT id, task_id, turn_index, tool_name, arguments_json, output_json,
                    status, error, created_at, completed_at
             FROM agent_tool_calls
             WHERE task_id = ?1
             ORDER BY turn_index, created_at, id",
        )?;
        let rows = statement.query_map(params![task_id], agent_tool_call_from_row)?;
        collect_rows(rows)
    }

    fn get_event(&self, id: &str) -> Result<AgentEvent> {
        self.conn
            .query_row(
                "SELECT sequence, id, task_id, kind, message, payload_json, created_at
                 FROM agent_events
                 WHERE id = ?1",
                params![id],
                agent_event_from_row,
            )
            .with_context(|| format!("agent event {id} not found"))
    }

    fn get_thread_item(&self, id: &str) -> Result<AgentThreadItem> {
        self.conn
            .query_row(
                "SELECT sequence, id, thread_id, turn_id, item_type, role, content,
                        tool_name, payload_json, created_at
                 FROM agent_thread_items
                 WHERE id = ?1",
                params![id],
                agent_thread_item_from_row,
            )
            .with_context(|| format!("agent thread item {id} not found"))
    }

    fn get_thread_event(&self, id: &str) -> Result<AgentThreadEvent> {
        self.conn
            .query_row(
                "SELECT sequence, id, thread_id, turn_id, kind, message, payload_json, created_at
                 FROM agent_thread_events
                 WHERE id = ?1",
                params![id],
                agent_thread_event_from_row,
            )
            .with_context(|| format!("agent thread event {id} not found"))
    }
}

pub struct NewAgentThreadItem<'a> {
    pub thread_id: &'a str,
    pub turn_id: Option<&'a str>,
    pub item_type: &'a str,
    pub role: Option<&'a str>,
    pub content: Option<&'a str>,
    pub tool_name: Option<&'a str>,
    pub payload: Value,
}

fn initialize_schema(conn: &Connection) -> Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match version {
        0 => {
            create_schema_v1(conn)?;
            create_schema_v2(conn)?;
        }
        1 => create_schema_v2(conn)?,
        2 => create_schema_v2(conn)?,
        other => anyhow::bail!("unsupported agent database schema version {other}"),
    }
    Ok(())
}

fn create_schema_v2(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS agent_threads (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            goal TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            last_turn_id TEXT,
            metadata_json TEXT NOT NULL,
            FOREIGN KEY (last_turn_id) REFERENCES agent_turns(id) ON DELETE SET NULL
        );

        CREATE TABLE IF NOT EXISTS agent_turns (
            id TEXT PRIMARY KEY,
            thread_id TEXT NOT NULL,
            input TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            completed_at TEXT,
            result_json TEXT,
            error TEXT,
            metadata_json TEXT NOT NULL,
            FOREIGN KEY (thread_id) REFERENCES agent_threads(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_thread_items (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
            thread_id TEXT NOT NULL,
            turn_id TEXT,
            item_type TEXT NOT NULL,
            role TEXT,
            content TEXT,
            tool_name TEXT,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY (thread_id) REFERENCES agent_threads(id) ON DELETE CASCADE,
            FOREIGN KEY (turn_id) REFERENCES agent_turns(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_thread_events (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
            thread_id TEXT NOT NULL,
            turn_id TEXT,
            kind TEXT NOT NULL,
            message TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY (thread_id) REFERENCES agent_threads(id) ON DELETE CASCADE,
            FOREIGN KEY (turn_id) REFERENCES agent_turns(id) ON DELETE SET NULL
        );

        CREATE INDEX IF NOT EXISTS idx_agent_threads_status ON agent_threads(status, updated_at);
        CREATE INDEX IF NOT EXISTS idx_agent_turns_thread ON agent_turns(thread_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_agent_turns_status ON agent_turns(status, updated_at);
        CREATE INDEX IF NOT EXISTS idx_agent_thread_items_thread ON agent_thread_items(thread_id, sequence);
        CREATE INDEX IF NOT EXISTS idx_agent_thread_items_turn ON agent_thread_items(turn_id, sequence);
        CREATE INDEX IF NOT EXISTS idx_agent_thread_events_thread ON agent_thread_events(thread_id, sequence);
        PRAGMA user_version = 2;
        "#,
    )?;
    Ok(())
}

fn create_schema_v1(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS agent_tasks (
            id TEXT PRIMARY KEY,
            goal TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            completed_at TEXT,
            result_json TEXT,
            error TEXT,
            metadata_json TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS agent_messages (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            metadata_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY (task_id) REFERENCES agent_tasks(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_tool_calls (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL,
            turn_index INTEGER NOT NULL,
            tool_name TEXT NOT NULL,
            arguments_json TEXT NOT NULL,
            output_json TEXT,
            status TEXT NOT NULL,
            error TEXT,
            created_at TEXT NOT NULL,
            completed_at TEXT,
            FOREIGN KEY (task_id) REFERENCES agent_tasks(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_events (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
            task_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            message TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY (task_id) REFERENCES agent_tasks(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_agent_tasks_status ON agent_tasks(status, updated_at);
        CREATE INDEX IF NOT EXISTS idx_agent_messages_task ON agent_messages(task_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_agent_tool_calls_task ON agent_tool_calls(task_id, turn_index);
        CREATE INDEX IF NOT EXISTS idx_agent_events_task ON agent_events(task_id, sequence);
        PRAGMA user_version = 1;
        "#,
    )?;
    Ok(())
}

fn agent_task_from_row(row: &Row<'_>) -> rusqlite::Result<AgentTask> {
    let status: String = row.get(2)?;
    Ok(AgentTask {
        id: row.get(0)?,
        goal: row.get(1)?,
        status: AgentTaskStatus::parse(&status).unwrap_or(AgentTaskStatus::Failed),
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        completed_at: row.get(5)?,
        result: optional_json_column(row, 6)?,
        error: row.get(7)?,
        metadata: json_column(row, 8)?,
    })
}

fn agent_message_from_row(row: &Row<'_>) -> rusqlite::Result<AgentMessage> {
    Ok(AgentMessage {
        id: row.get(0)?,
        task_id: row.get(1)?,
        role: row.get(2)?,
        content: row.get(3)?,
        metadata: json_column(row, 4)?,
        created_at: row.get(5)?,
    })
}

fn agent_tool_call_from_row(row: &Row<'_>) -> rusqlite::Result<AgentToolCall> {
    let turn_index: i64 = row.get(2)?;
    Ok(AgentToolCall {
        id: row.get(0)?,
        task_id: row.get(1)?,
        turn_index: turn_index.max(0) as usize,
        tool_name: row.get(3)?,
        arguments: json_column(row, 4)?,
        output: optional_json_column(row, 5)?,
        status: row.get(6)?,
        error: row.get(7)?,
        created_at: row.get(8)?,
        completed_at: row.get(9)?,
    })
}

fn agent_event_from_row(row: &Row<'_>) -> rusqlite::Result<AgentEvent> {
    Ok(AgentEvent {
        sequence: row.get(0)?,
        id: row.get(1)?,
        task_id: row.get(2)?,
        kind: row.get(3)?,
        message: row.get(4)?,
        payload: json_column(row, 5)?,
        created_at: row.get(6)?,
    })
}

fn agent_thread_from_row(row: &Row<'_>) -> rusqlite::Result<AgentThread> {
    let status: String = row.get(3)?;
    Ok(AgentThread {
        id: row.get(0)?,
        title: row.get(1)?,
        goal: row.get(2)?,
        status: AgentThreadStatus::parse(&status).unwrap_or(AgentThreadStatus::Active),
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        last_turn_id: row.get(6)?,
        metadata: json_column(row, 7)?,
    })
}

fn agent_turn_from_row(row: &Row<'_>) -> rusqlite::Result<AgentTurn> {
    let status: String = row.get(3)?;
    Ok(AgentTurn {
        id: row.get(0)?,
        thread_id: row.get(1)?,
        input: row.get(2)?,
        status: AgentTaskStatus::parse(&status).unwrap_or(AgentTaskStatus::Failed),
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        completed_at: row.get(6)?,
        result: optional_json_column(row, 7)?,
        error: row.get(8)?,
        metadata: json_column(row, 9)?,
    })
}

fn agent_thread_item_from_row(row: &Row<'_>) -> rusqlite::Result<AgentThreadItem> {
    Ok(AgentThreadItem {
        sequence: row.get(0)?,
        id: row.get(1)?,
        thread_id: row.get(2)?,
        turn_id: row.get(3)?,
        item_type: row.get(4)?,
        role: row.get(5)?,
        content: row.get(6)?,
        tool_name: row.get(7)?,
        payload: json_column(row, 8)?,
        created_at: row.get(9)?,
    })
}

fn agent_thread_event_from_row(row: &Row<'_>) -> rusqlite::Result<AgentThreadEvent> {
    Ok(AgentThreadEvent {
        sequence: row.get(0)?,
        id: row.get(1)?,
        thread_id: row.get(2)?,
        turn_id: row.get(3)?,
        kind: row.get(4)?,
        message: row.get(5)?,
        payload: json_column(row, 6)?,
        created_at: row.get(7)?,
    })
}

fn json_text(value: &Value) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

fn optional_json_text(value: Option<&Value>) -> Result<Option<String>> {
    value.map(json_text).transpose()
}

fn json_column(row: &Row<'_>, index: usize) -> rusqlite::Result<Value> {
    let raw: String = row.get(index)?;
    serde_json::from_str(&raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn optional_json_column(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<Value>> {
    let raw: Option<String> = row.get(index)?;
    raw.map(|value| {
        serde_json::from_str(&value).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
    })
    .transpose()
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>> {
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn next_id(prefix: &str) -> String {
    let sequence = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "{}-{}-{sequence}",
        prefix,
        Utc::now().format("%Y%m%d%H%M%S%3f")
    )
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::tempdir;

    use super::{AgentStore, NewAgentThreadItem};
    use crate::agent::model::{AgentTaskStatus, AgentThreadStatus};
    use crate::paths::IssueFinderPaths;

    #[test]
    fn store_persists_task_history_and_tool_calls() {
        let dir = tempdir().unwrap();
        let paths = IssueFinderPaths {
            home: dir.path().to_path_buf(),
            config: dir.path().join("config.toml"),
            cache_dir: dir.path().join("cache"),
            workspaces_dir: dir.path().join("workspaces"),
            inbox_dir: dir.path().join("inbox"),
            reports_dir: dir.path().join("reports"),
        };
        let store = AgentStore::open(paths).unwrap();

        let task = store
            .create_task("搜索全网仓库并推荐 issue", json!({"input": {"limit": 3}}))
            .unwrap();
        store
            .add_message(&task.id, "user", &task.goal, json!({}))
            .unwrap();
        store
            .add_event(&task.id, "task_queued", "Task queued.", json!({}))
            .unwrap();
        let call = store
            .start_tool_call(&task.id, 0, "issue-finder.scout", json!({"limit": 3}))
            .unwrap();
        store
            .finish_tool_call(&call.id, "ok", Some(json!({"success": true})), None)
            .unwrap();
        store
            .update_task_status(
                &task.id,
                AgentTaskStatus::Completed,
                Some(json!({"finalAnswer": "done"})),
                None,
            )
            .unwrap();

        let detail = store.detail(&task.id).unwrap();
        assert_eq!(detail.task.status, AgentTaskStatus::Completed);
        assert_eq!(detail.messages.len(), 1);
        assert_eq!(detail.tool_calls.len(), 1);
        assert_eq!(detail.events.len(), 1);
        assert_eq!(detail.tool_calls[0].status, "ok");
    }

    #[test]
    fn store_persists_resumable_thread_turns_items_and_events() {
        let dir = tempdir().unwrap();
        let paths = IssueFinderPaths {
            home: dir.path().to_path_buf(),
            config: dir.path().join("config.toml"),
            cache_dir: dir.path().join("cache"),
            workspaces_dir: dir.path().join("workspaces"),
            inbox_dir: dir.path().join("inbox"),
            reports_dir: dir.path().join("reports"),
        };
        let store = AgentStore::open(paths.clone()).unwrap();

        let thread = store
            .create_thread(
                "搜索全网仓库并推荐 issue",
                "搜索 issue",
                json!({"transport": "local_http_a2a"}),
            )
            .unwrap();
        let turn = store
            .create_turn(&thread.id, "先 scout", json!({"limit": 3}))
            .unwrap();
        store
            .add_thread_item(NewAgentThreadItem {
                thread_id: &thread.id,
                turn_id: Some(&turn.id),
                item_type: "user_message",
                role: Some("user"),
                content: Some("先 scout"),
                tool_name: None,
                payload: json!({}),
            })
            .unwrap();
        store
            .add_thread_event(
                &thread.id,
                Some(&turn.id),
                "turn_queued",
                "Turn queued.",
                json!({}),
            )
            .unwrap();
        store
            .update_thread_status(&thread.id, AgentThreadStatus::Running)
            .unwrap();
        store
            .update_turn_status(
                &turn.id,
                AgentTaskStatus::Completed,
                Some(json!({"finalAnswer": "done"})),
                None,
            )
            .unwrap();
        store
            .update_thread_status(&thread.id, AgentThreadStatus::Active)
            .unwrap();

        let reopened = AgentStore::open(paths).unwrap();
        let detail = reopened.thread_detail(&thread.id).unwrap();
        assert_eq!(detail.thread.status, AgentThreadStatus::Active);
        assert_eq!(
            detail.thread.last_turn_id.as_deref(),
            Some(turn.id.as_str())
        );
        assert_eq!(detail.turns.len(), 1);
        assert_eq!(detail.items.len(), 1);
        assert_eq!(detail.events.len(), 1);
    }
}
