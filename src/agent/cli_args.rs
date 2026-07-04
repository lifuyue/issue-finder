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
