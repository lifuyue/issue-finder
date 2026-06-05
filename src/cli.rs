use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "issue-finder")]
#[command(about = "Local-first handoff prep for developers using coding agents")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize Issue Finder config and local state directories.
    Init(InitArgs),
    /// Discover and rank good-first-issue tasks.
    Scout(ScoutArgs),
    /// Assess one issue without preparing workspace or handoff state.
    Assess(AssessArgs),
    /// Prepare one issue and write a handoff into the inbox.
    Prepare(PrepareArgs),
    /// Display or print an existing handoff.
    Handoff(HandoffArgs),
    /// List or lightly update local inbox status.
    Inbox(InboxArgs),
    /// Record or inspect recommendation feedback for any issue.
    Feedback(FeedbackArgs),
    /// Run scout, prepare Top N, and write today's report.
    Daily(DailyArgs),
    /// Display local daily reports.
    Report(ReportArgs),
    /// List and call Issue Finder's JSON tool contract.
    Tools(ToolsArgs),
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
    /// Do not record returned candidates as shown.
    #[arg(long)]
    pub dry_run: bool,
    /// Print ranked candidates as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AssessArgs {
    /// Issue reference in owner/repo#123 form.
    pub issue: Option<String>,
    /// GitHub issue URL.
    #[arg(long)]
    pub url: Option<String>,
    /// Ignore the GitHub enrichment cache.
    #[arg(long)]
    pub refresh: bool,
    /// Do not record the issue as read.
    #[arg(long)]
    pub dry_run: bool,
    /// Print assessment as JSON.
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
pub struct FeedbackArgs {
    #[command(subcommand)]
    pub command: FeedbackCommand,
}

#[derive(Debug, Subcommand)]
pub enum FeedbackCommand {
    /// Mark an issue as read.
    Read { issue: String },
    /// Hide an issue from future recommendation feed results.
    Dismiss { issue: String },
    /// Restore a done or dismissed issue to the recommendation feed.
    Restore { issue: String },
    /// Show derived recommendation feedback state for an issue.
    Show { issue: String },
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

#[derive(Debug, Args)]
pub struct ToolsArgs {
    #[command(subcommand)]
    pub command: ToolsCommand,
}

#[derive(Debug, Subcommand)]
pub enum ToolsCommand {
    /// Print Issue Finder tool specs as JSON.
    List,
    /// Call one Issue Finder tool with a JSON object argument payload.
    Call(ToolsCallArgs),
}

#[derive(Debug, Args)]
pub struct ToolsCallArgs {
    /// Tool name, for example issue-finder.scout.
    pub tool: String,
    /// Tool arguments as a JSON object.
    #[arg(long)]
    pub arguments: String,
    /// Tool call id to echo in the output envelope.
    #[arg(long)]
    pub call_id: Option<String>,
    /// Optional model turn id to echo in the output envelope.
    #[arg(long)]
    pub turn_id: Option<String>,
}
