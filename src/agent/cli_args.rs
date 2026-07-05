use clap::{Args, Subcommand};

pub const DEFAULT_AGENT_HOST: &str = "127.0.0.1";
pub const DEFAULT_AGENT_PORT: u16 = 8787;

#[derive(Debug, Args)]
pub struct AgentArgs {
    #[command(subcommand)]
    pub command: AgentCommand,
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// Start the local Issue Finder A2A agent daemon.
    Daemon(AgentDaemonArgs),
    /// Send a natural-language task to a running agent daemon.
    Send(AgentSendArgs),
    /// Start a resumable natural-language thread.
    ThreadStart(AgentThreadStartArgs),
    /// Append a turn to a resumable thread.
    ThreadSend(AgentThreadSendArgs),
    /// Steer a running turn without starting a new thread.
    ThreadSteer(AgentThreadSteerArgs),
    /// Inject context into a thread mailbox.
    ThreadInject(AgentThreadInjectArgs),
    /// Request deterministic context compaction for a thread.
    ThreadCompact(AgentThreadCompactArgs),
    /// Interrupt a running turn.
    ThreadInterrupt(AgentThreadInterruptArgs),
    /// Approve and execute an approval-gated tool request.
    ApprovalApprove(AgentApprovalDecisionArgs),
    /// Reject an approval-gated tool request.
    ApprovalReject(AgentApprovalRejectArgs),
    /// List recent resumable threads from a running agent daemon.
    Threads(AgentEndpointArgs),
    /// Show one resumable thread from a running agent daemon.
    ThreadShow(AgentThreadQueryArgs),
    /// Show ordered events for one resumable thread from a running agent daemon.
    ThreadEvents(AgentThreadQueryArgs),
    /// List recent tasks from a running agent daemon.
    List(AgentEndpointArgs),
    /// Show one task from a running agent daemon.
    Show(AgentTaskQueryArgs),
    /// Show ordered events for one task from a running agent daemon.
    Events(AgentTaskQueryArgs),
    /// Print the running daemon's A2A agent card.
    Card(AgentEndpointArgs),
}

#[derive(Debug, Args)]
pub struct AgentDaemonArgs {
    /// Host interface to bind.
    #[arg(long, default_value = DEFAULT_AGENT_HOST)]
    pub host: String,
    /// TCP port to bind.
    #[arg(long, default_value_t = DEFAULT_AGENT_PORT)]
    pub port: u16,
}

#[derive(Debug, Args)]
pub struct AgentSendArgs {
    /// Natural-language goal for the Issue Finder agent.
    pub goal: String,
    /// Running daemon host.
    #[arg(long, default_value = DEFAULT_AGENT_HOST)]
    pub host: String,
    /// Running daemon port.
    #[arg(long, default_value_t = DEFAULT_AGENT_PORT)]
    pub port: u16,
    /// Restrict discovery to one repository.
    #[arg(long)]
    pub repo: Option<String>,
    /// Candidate limit for scout-style tasks.
    #[arg(long)]
    pub limit: Option<usize>,
    /// Ignore the GitHub discovery cache.
    #[arg(long)]
    pub refresh: bool,
    /// Maximum LLM/tool turns for this task.
    #[arg(long)]
    pub max_turns: Option<usize>,
    /// Queue the task without running it immediately.
    #[arg(long)]
    pub queued: bool,
    /// Poll until the task reaches a terminal state.
    #[arg(long)]
    pub wait: bool,
    /// Print raw JSON response.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentThreadStartArgs {
    /// Natural-language goal for the Issue Finder agent thread.
    pub goal: String,
    /// Optional display title for the thread.
    #[arg(long)]
    pub title: Option<String>,
    /// Running daemon host.
    #[arg(long, default_value = DEFAULT_AGENT_HOST)]
    pub host: String,
    /// Running daemon port.
    #[arg(long, default_value_t = DEFAULT_AGENT_PORT)]
    pub port: u16,
    /// Restrict discovery to one repository.
    #[arg(long)]
    pub repo: Option<String>,
    /// Candidate limit for scout-style tasks.
    #[arg(long)]
    pub limit: Option<usize>,
    /// Ignore the GitHub discovery cache.
    #[arg(long)]
    pub refresh: bool,
    /// Maximum LLM/tool turns for this turn.
    #[arg(long)]
    pub max_turns: Option<usize>,
    /// Queue the first turn without running it immediately.
    #[arg(long)]
    pub queued: bool,
    /// Poll until the first turn reaches a terminal state.
    #[arg(long)]
    pub wait: bool,
    /// Print raw JSON response.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentThreadSendArgs {
    /// Agent thread id.
    pub thread_id: String,
    /// Natural-language follow-up input for the thread.
    pub input: String,
    /// Running daemon host.
    #[arg(long, default_value = DEFAULT_AGENT_HOST)]
    pub host: String,
    /// Running daemon port.
    #[arg(long, default_value_t = DEFAULT_AGENT_PORT)]
    pub port: u16,
    /// Restrict discovery to one repository.
    #[arg(long)]
    pub repo: Option<String>,
    /// Candidate limit for scout-style tasks.
    #[arg(long)]
    pub limit: Option<usize>,
    /// Ignore the GitHub discovery cache.
    #[arg(long)]
    pub refresh: bool,
    /// Maximum LLM/tool turns for this turn.
    #[arg(long)]
    pub max_turns: Option<usize>,
    /// Queue the turn without running it immediately.
    #[arg(long)]
    pub queued: bool,
    /// Poll until this turn reaches a terminal state.
    #[arg(long)]
    pub wait: bool,
    /// Print raw JSON response.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentThreadSteerArgs {
    /// Agent thread id.
    pub thread_id: String,
    /// Agent turn id.
    pub turn_id: String,
    /// Natural-language steering input for the running turn.
    pub input: String,
    /// Running daemon host.
    #[arg(long, default_value = DEFAULT_AGENT_HOST)]
    pub host: String,
    /// Running daemon port.
    #[arg(long, default_value_t = DEFAULT_AGENT_PORT)]
    pub port: u16,
    /// Restrict discovery to one repository.
    #[arg(long)]
    pub repo: Option<String>,
    /// Candidate limit for scout-style tasks.
    #[arg(long)]
    pub limit: Option<usize>,
    /// Ignore the GitHub discovery cache.
    #[arg(long)]
    pub refresh: bool,
    /// Maximum LLM/tool turns for this turn.
    #[arg(long)]
    pub max_turns: Option<usize>,
    /// Print raw JSON response.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentThreadInjectArgs {
    /// Agent thread id.
    pub thread_id: String,
    /// Context text to inject into the thread mailbox.
    pub input: String,
    /// Running daemon host.
    #[arg(long, default_value = DEFAULT_AGENT_HOST)]
    pub host: String,
    /// Running daemon port.
    #[arg(long, default_value_t = DEFAULT_AGENT_PORT)]
    pub port: u16,
    /// Print raw JSON response.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentThreadCompactArgs {
    /// Agent thread id.
    pub thread_id: String,
    /// Running daemon host.
    #[arg(long, default_value = DEFAULT_AGENT_HOST)]
    pub host: String,
    /// Running daemon port.
    #[arg(long, default_value_t = DEFAULT_AGENT_PORT)]
    pub port: u16,
    /// Print raw JSON response.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentThreadInterruptArgs {
    /// Agent thread id.
    pub thread_id: String,
    /// Agent turn id.
    pub turn_id: String,
    /// Running daemon host.
    #[arg(long, default_value = DEFAULT_AGENT_HOST)]
    pub host: String,
    /// Running daemon port.
    #[arg(long, default_value_t = DEFAULT_AGENT_PORT)]
    pub port: u16,
    /// Print raw JSON response.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentApprovalDecisionArgs {
    /// Agent thread id.
    pub thread_id: String,
    /// Agent approval request id.
    pub approval_request_id: String,
    /// Running daemon host.
    #[arg(long, default_value = DEFAULT_AGENT_HOST)]
    pub host: String,
    /// Running daemon port.
    #[arg(long, default_value_t = DEFAULT_AGENT_PORT)]
    pub port: u16,
    /// Print raw JSON response.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentApprovalRejectArgs {
    /// Agent thread id.
    pub thread_id: String,
    /// Agent approval request id.
    pub approval_request_id: String,
    /// Reason for rejecting the request.
    #[arg(long)]
    pub reason: Option<String>,
    /// Running daemon host.
    #[arg(long, default_value = DEFAULT_AGENT_HOST)]
    pub host: String,
    /// Running daemon port.
    #[arg(long, default_value_t = DEFAULT_AGENT_PORT)]
    pub port: u16,
    /// Print raw JSON response.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentEndpointArgs {
    /// Running daemon host.
    #[arg(long, default_value = DEFAULT_AGENT_HOST)]
    pub host: String,
    /// Running daemon port.
    #[arg(long, default_value_t = DEFAULT_AGENT_PORT)]
    pub port: u16,
    /// Print raw JSON response.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentTaskQueryArgs {
    /// Agent task id.
    pub task_id: String,
    /// Running daemon host.
    #[arg(long, default_value = DEFAULT_AGENT_HOST)]
    pub host: String,
    /// Running daemon port.
    #[arg(long, default_value_t = DEFAULT_AGENT_PORT)]
    pub port: u16,
    /// Print raw JSON response.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentThreadQueryArgs {
    /// Agent thread id.
    pub thread_id: String,
    /// Running daemon host.
    #[arg(long, default_value = DEFAULT_AGENT_HOST)]
    pub host: String,
    /// Running daemon port.
    #[arg(long, default_value_t = DEFAULT_AGENT_PORT)]
    pub port: u16,
    /// Print raw JSON response.
    #[arg(long)]
    pub json: bool,
}
