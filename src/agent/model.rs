use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTaskStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl AgentTaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentThreadStatus {
    Idle,
    Running,
    Archived,
    Cancelled,
}

impl AgentThreadStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Archived => "archived",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "idle" => Some(Self::Idle),
            "running" => Some(Self::Running),
            "archived" => Some(Self::Archived),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub fn accepts_turns(self) -> bool {
        matches!(self, Self::Idle | Self::Running)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTurnStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl AgentTurnStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTask {
    pub id: String,
    pub goal: String,
    pub status: AgentTaskStatus,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessage {
    pub id: String,
    pub task_id: String,
    pub role: String,
    pub content: String,
    pub metadata: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolCall {
    pub id: String,
    pub task_id: String,
    pub turn_index: usize,
    pub tool_name: String,
    pub arguments: Value,
    pub output: Option<Value>,
    pub status: String,
    pub error: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub sequence: i64,
    pub id: String,
    pub task_id: String,
    pub kind: String,
    pub message: String,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskDetail {
    pub task: AgentTask,
    pub messages: Vec<AgentMessage>,
    pub tool_calls: Vec<AgentToolCall>,
    pub events: Vec<AgentEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThread {
    pub id: String,
    pub goal: String,
    pub status: AgentThreadStatus,
    pub created_at: String,
    pub updated_at: String,
    pub archived_at: Option<String>,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurn {
    pub id: String,
    pub thread_id: String,
    pub input: String,
    pub status: AgentTurnStatus,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThreadItem {
    pub sequence: i64,
    pub id: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub kind: String,
    pub role: Option<String>,
    pub content: Option<String>,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<String>,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThreadEvent {
    pub sequence: i64,
    pub id: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub kind: String,
    pub message: String,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThreadDetail {
    pub thread: AgentThread,
    pub turns: Vec<AgentTurn>,
    pub items: Vec<AgentThreadItem>,
    pub events: Vec<AgentThreadEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnDetail {
    pub thread: AgentThread,
    pub turn: AgentTurn,
    pub items: Vec<AgentThreadItem>,
    pub events: Vec<AgentThreadEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskSendRequest {
    pub goal: String,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub refresh: bool,
    #[serde(default)]
    pub max_turns: Option<usize>,
    #[serde(default = "default_run_immediately")]
    pub run_immediately: bool,
}

impl AgentTaskSendRequest {
    pub fn normalized_goal(&self) -> String {
        self.goal.trim().to_string()
    }

    pub fn normalized_limit(&self) -> usize {
        self.limit.unwrap_or(10).max(1)
    }

    pub fn normalized_max_turns(&self) -> usize {
        self.max_turns.unwrap_or(4).clamp(1, 8)
    }

    pub fn into_thread_start(self) -> AgentThreadStartRequest {
        AgentThreadStartRequest {
            goal: self.goal,
            repo: self.repo,
            limit: self.limit,
            refresh: self.refresh,
            max_turns: self.max_turns,
            run_immediately: self.run_immediately,
            metadata: Value::Null,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThreadStartRequest {
    pub goal: String,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub refresh: bool,
    #[serde(default)]
    pub max_turns: Option<usize>,
    #[serde(default = "default_run_immediately")]
    pub run_immediately: bool,
    #[serde(default)]
    pub metadata: Value,
}

impl AgentThreadStartRequest {
    pub fn normalized_goal(&self) -> String {
        self.goal.trim().to_string()
    }

    pub fn normalized_limit(&self) -> usize {
        self.limit.unwrap_or(10).max(1)
    }

    pub fn normalized_max_turns(&self) -> usize {
        self.max_turns.unwrap_or(4).clamp(1, 12)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnStartRequest {
    pub input: String,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub refresh: bool,
    #[serde(default)]
    pub max_turns: Option<usize>,
    #[serde(default = "default_run_immediately")]
    pub run_immediately: bool,
    #[serde(default)]
    pub metadata: Value,
}

impl AgentTurnStartRequest {
    pub fn normalized_input(&self) -> String {
        self.input.trim().to_string()
    }

    pub fn normalized_limit(&self) -> usize {
        self.limit.unwrap_or(10).max(1)
    }

    pub fn normalized_max_turns(&self) -> usize {
        self.max_turns.unwrap_or(4).clamp(1, 12)
    }
}

fn default_run_immediately() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskAcceptedEnvelope {
    pub kind: String,
    pub version: u8,
    pub task: AgentTask,
    pub task_url: String,
    pub events_url: String,
    #[serde(default)]
    pub thread: Option<AgentThread>,
    #[serde(default)]
    pub turn: Option<AgentTurn>,
    #[serde(default)]
    pub thread_url: Option<String>,
    #[serde(default)]
    pub turn_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThreadAcceptedEnvelope {
    pub kind: String,
    pub version: u8,
    pub thread: AgentThread,
    pub turn: AgentTurn,
    pub thread_url: String,
    pub turn_url: String,
    pub events_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnAcceptedEnvelope {
    pub kind: String,
    pub version: u8,
    pub thread: AgentThread,
    pub turn: AgentTurn,
    pub thread_url: String,
    pub turn_url: String,
    pub events_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskDetailEnvelope {
    pub kind: String,
    pub version: u8,
    pub detail: AgentTaskDetail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThreadDetailEnvelope {
    pub kind: String,
    pub version: u8,
    pub detail: AgentThreadDetail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnDetailEnvelope {
    pub kind: String,
    pub version: u8,
    pub detail: AgentTurnDetail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEventsEnvelope {
    pub kind: String,
    pub version: u8,
    pub task_id: String,
    pub events: Vec<AgentEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThreadEventsEnvelope {
    pub kind: String,
    pub version: u8,
    pub thread_id: String,
    pub events: Vec<AgentThreadEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskListEnvelope {
    pub kind: String,
    pub version: u8,
    pub tasks: Vec<AgentTask>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThreadListEnvelope {
    pub kind: String,
    pub version: u8,
    pub threads: Vec<AgentThread>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCardEnvelope {
    pub kind: String,
    pub version: u8,
    pub name: String,
    pub description: String,
    pub endpoints: Vec<AgentEndpoint>,
    pub input_modes: Vec<String>,
    pub output_modes: Vec<String>,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEndpoint {
    pub method: String,
    pub path: String,
    pub description: String,
}
