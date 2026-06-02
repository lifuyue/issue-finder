use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "patchbay")]
#[command(about = "Prepare local-first contribution handoffs for coding agents")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize Patchbay config and local state directories.
    Init(InitArgs),
    /// Discover and rank good-first-issue tasks.
    Scout(ScoutArgs),
    /// Prepare one issue and write a handoff into the inbox.
    Prepare(PrepareArgs),
    /// Display or print an existing handoff.
    Handoff(HandoffArgs),
    /// List or lightly update local inbox status.
    Inbox(InboxArgs),
    /// Run scout, prepare Top N, and write today's report.
    Daily(DailyArgs),
    /// Display local daily reports.
    Report(ReportArgs),
    /// Check local readiness.
    Doctor,
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Overwrite an existing config file.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct ScoutArgs {
    /// Number of ranked candidates to show.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    /// Ignore the GitHub discovery cache.
    #[arg(long)]
    pub refresh: bool,
    /// Print ranked candidates as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct PrepareArgs {
    /// Issue reference in owner/repo#123 form.
    pub issue: Option<String>,
    /// GitHub issue URL.
    #[arg(long)]
    pub url: Option<String>,
}

#[derive(Debug, Args)]
pub struct HandoffArgs {
    /// Inbox item id.
    pub inbox_id: String,
    /// Print canonical handoff JSON.
    #[arg(long)]
    pub json: bool,
    /// Print human-readable handoff markdown.
    #[arg(long)]
    pub print: bool,
}

#[derive(Debug, Args)]
pub struct InboxArgs {
    #[command(subcommand)]
    pub command: Option<InboxCommand>,
    /// Print inbox index as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum InboxCommand {
    /// Mark an inbox item archived.
    Archive { inbox_id: String },
    /// Mark an inbox item done.
    Done { inbox_id: String },
}

#[derive(Debug, Args)]
pub struct DailyArgs {
    /// Number of top issues to prepare.
    #[arg(long)]
    pub top: Option<usize>,
    /// Ignore the GitHub discovery cache.
    #[arg(long)]
    pub refresh: bool,
}

#[derive(Debug, Args)]
pub struct ReportArgs {
    /// Local date in YYYY-MM-DD form.
    #[arg(long)]
    pub date: Option<String>,
}
