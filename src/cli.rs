use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::dispatch::cli_args::{AgentsArgs, DispatchArgs};

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
    /// Bootstrap or inspect the local recommendation profile.
    Profile(ProfileArgs),
    /// Inspect configured execution agents and their capabilities.
    Agents(AgentsArgs),
    /// Inspect local dispatch runs, events, and artifacts.
    Dispatch(Box<DispatchArgs>),
    /// Run recommendation evaluation workflows.
    Eval(EvalArgs),
    /// Inspect and control contribution memory.
    Memory(MemoryArgs),
    /// List and call Issue Finder's JSON tool contract.
    Tools(ToolsArgs),
    /// Serve the canonical Issue Finder tool registry over stdio MCP.
    Mcp(McpArgs),
    #[command(hide = true)]
    Supervise(SuperviseArgs),
    /// Check local readiness.
    Doctor,
    /// Verify the selected decision model provider and authentication with a real decision request.
    #[command(visible_alias = "system1-check")]
    DecisionCheck {
        #[arg(long, value_enum)]
        provider: Option<DecisionProviderArg>,
        /// Select Codex and override its executable; incompatible with another explicit provider.
        #[arg(long)]
        codex_binary: Option<String>,
    },
    /// Initialize isolated Codex file authentication from ISSUE_FINDER_CODEX_AUTH_JSON.
    #[command(visible_alias = "system1-auth-init")]
    DecisionAuthInit {
        /// Replace the isolated runtime credential explicitly; otherwise preserve refreshed auth.
        #[arg(long)]
        replace: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DecisionProviderArg {
    AliyunDecision,
    CloudflareClefFlash,
    Codex,
}

impl From<DecisionProviderArg> for crate::config::DecisionProvider {
    fn from(value: DecisionProviderArg) -> Self {
        match value {
            DecisionProviderArg::AliyunDecision => Self::AliyunDecision,
            DecisionProviderArg::CloudflareClefFlash => Self::CloudflareClefFlash,
            DecisionProviderArg::Codex => Self::Codex,
        }
    }
}

#[derive(Debug, Args)]
pub struct SuperviseArgs {
    #[arg(long)]
    pub run_id: String,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum McpProfile {
    Session,
    Control,
    Worker,
}

#[derive(Debug, Args)]
pub struct McpArgs {
    #[arg(long, value_enum, default_value_t = McpProfile::Session)]
    pub profile: McpProfile,
    #[arg(long, requires = "issue_task_id")]
    pub run_id: Option<String>,
    #[arg(long, requires = "package_id")]
    pub issue_task_id: Option<String>,
    #[arg(long, requires = "snapshot_id")]
    pub package_id: Option<String>,
    #[arg(long, requires = "workspace")]
    pub snapshot_id: Option<String>,
    #[arg(long)]
    pub workspace: Option<String>,
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
    /// Restrict recommendation discovery to one repository.
    #[arg(long)]
    pub repo: Option<String>,
    /// Ignore the GitHub discovery cache.
    #[arg(long)]
    pub refresh: bool,
    /// Do not record returned candidates as shown.
    #[arg(long)]
    pub dry_run: bool,
    /// Print ranked candidates as JSON.
    #[arg(long)]
    pub json: bool,
    /// Print ranked candidates plus discovery/filter/API budget stats as JSON.
    #[arg(long)]
    pub stats_json: bool,
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
    /// Restrict recommendation discovery and preparation to one repository.
    #[arg(long)]
    pub repo: Option<String>,
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
pub struct ProfileArgs {
    #[command(subcommand)]
    pub command: ProfileCommand,
}

#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    /// Scan local Agent indexes and project manifests to draft a user profile.
    Bootstrap(ProfileBootstrapArgs),
}

#[derive(Debug, Args)]
pub struct ProfileBootstrapArgs {
    /// Print the profile bootstrap report as JSON.
    #[arg(long)]
    pub json: bool,
    /// Override the OS home scan root. Intended for tests and isolated debugging.
    #[arg(long, hide = true)]
    pub scan_root: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct EvalArgs {
    #[command(subcommand)]
    pub command: EvalCommand,
}

#[derive(Debug, Subcommand)]
pub enum EvalCommand {
    /// Print the versioned external evaluation capability contract.
    Contract(EvalContractArgs),
    /// Generate offline or live recommendation evaluation reports.
    Recommendation(RecommendationEvalArgs),
    /// Generate offline agent loop evaluation reports.
    AgentLoop(AgentLoopEvalArgs),
    /// Validate the default native Codex runtime through a real model turn.
    CodexRuntime(CodexRuntimeEvalArgs),
    /// Prepare an isolated dispatch recovery scenario for an external fault harness.
    RecoveryPrepare(RecoveryEvalPrepareArgs),
}

#[derive(Debug, Args)]
pub struct RecoveryEvalPrepareArgs {
    /// Recovery task identifier: E01, E02, or E03.
    #[arg(long)]
    pub scenario: String,
    /// Isolated workspace referenced by the dispatch task package.
    #[arg(long)]
    pub workspace: PathBuf,
    /// Runtime-randomized marker bound to issue, run, and transcript evidence.
    #[arg(long)]
    pub marker: String,
}

#[derive(Debug, Args)]
pub struct CodexRuntimeEvalArgs {
    /// Isolated workspace bound to the native thread and turn.
    #[arg(long, default_value = "/tmp/issue-finder-codex-runtime-eval")]
    pub workspace: PathBuf,
    /// Maximum wait for the marker-bearing model response.
    #[arg(long, default_value_t = 120)]
    pub timeout_seconds: u64,
    /// Stable marker for deterministic transcript verification.
    #[arg(long)]
    pub marker: Option<String>,
}

#[derive(Debug, Args)]
pub struct EvalContractArgs {
    /// Print the contract as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct RecommendationEvalArgs {
    /// Run deterministic offline fixture evaluation.
    #[arg(long, conflicts_with = "live")]
    pub offline: bool,
    /// Evaluate one externally supplied dataset instead of built-in regression fixtures.
    #[arg(long, value_name = "PATH", requires = "offline")]
    pub dataset: Option<PathBuf>,
    /// Run fixed six-profile live evaluation.
    #[arg(long, conflicts_with = "offline")]
    pub live: bool,
    /// Refresh GitHub data for live evaluation.
    #[arg(long)]
    pub refresh: bool,
    /// Candidate limit for live evaluation.
    #[arg(long, default_value_t = 15)]
    pub limit: usize,
    /// Output directory for metrics.json, report.md, and visible.jsonl.
    #[arg(long)]
    pub output: PathBuf,
}

#[derive(Debug, Args)]
pub struct AgentLoopEvalArgs {
    /// Run deterministic offline fixture evaluation.
    #[arg(long)]
    pub offline: bool,
    /// Output directory for metrics.json and report.md.
    #[arg(long)]
    pub output: PathBuf,
}

#[derive(Debug, Args)]
pub struct MemoryArgs {
    #[command(subcommand)]
    pub command: MemoryCommand,
    /// Print memory output as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum MemoryCommand {
    /// Show memory store status.
    Status,
    /// List memory events without raw payloads.
    Events(MemoryEventsArgs),
    /// Recall memory for an issue.
    Recall(MemoryRecallArgs),
    /// List or inspect memory dreams.
    Dreams(MemoryDreamsArgs),
    /// List or review memory hints.
    Hints(MemoryHintsArgs),
    /// Suppress memory hints for a scope such as repo:owner/repo.
    Suppress(MemorySuppressArgs),
    /// Tombstone a raw event, node, or hint id.
    Tombstone { id: String },
    /// Run deterministic memory dreaming for a scope.
    Dream(MemoryDreamArgs),
    /// Run offline memory evaluation.
    Eval(MemoryEvalArgs),
}

#[derive(Debug, Args)]
pub struct MemoryEventsArgs {
    /// Optional issue reference in owner/repo#123 form.
    #[arg(long)]
    pub issue: Option<String>,
}

#[derive(Debug, Args)]
pub struct MemoryRecallArgs {
    /// Issue reference in owner/repo#123 form.
    #[arg(long)]
    pub issue: String,
    /// Recall kind: scout-ranking, dispatch-planning, github-draft, or profile-review.
    #[arg(long, default_value = "scout-ranking")]
    pub kind: String,
    /// Maximum recalled items.
    #[arg(long, default_value_t = 10)]
    pub limit: usize,
}

#[derive(Debug, Args)]
pub struct MemoryDreamsArgs {
    #[command(subcommand)]
    pub command: MemoryDreamsCommand,
}

#[derive(Debug, Subcommand)]
pub enum MemoryDreamsCommand {
    /// List candidate and reviewed dreams.
    List,
    /// Show one dream and its hints.
    Show { dream_id: String },
}

#[derive(Debug, Args)]
pub struct MemoryHintsArgs {
    #[command(subcommand)]
    pub command: MemoryHintsCommand,
}

#[derive(Debug, Subcommand)]
pub enum MemoryHintsCommand {
    /// List memory hints.
    List,
    /// Approve a candidate hint.
    Approve { hint_id: String },
    /// Reject a candidate hint.
    Reject { hint_id: String },
    /// Pin an approved hint.
    Pin { hint_id: String },
    /// Deprioritize an approved hint.
    Deprioritize { hint_id: String },
}

#[derive(Debug, Args)]
pub struct MemorySuppressArgs {
    /// Scope to suppress, for example repo:owner/repo or global.
    #[arg(long)]
    pub scope: String,
}

#[derive(Debug, Args)]
pub struct MemoryDreamArgs {
    /// Dream scope, for example global or repo:owner/repo.
    #[arg(long, default_value = "global")]
    pub scope: String,
}

#[derive(Debug, Args)]
pub struct MemoryEvalArgs {
    /// Run deterministic offline fixture evaluation.
    #[arg(long)]
    pub offline: bool,
    /// Output directory for metrics.json and report.md.
    #[arg(long)]
    pub output: PathBuf,
}

#[derive(Debug, Args)]
pub struct ToolsArgs {
    /// Tool surface: Codex discovery/assessment or legacy dispatch control plane.
    #[arg(long, value_enum, default_value_t = ToolsProfile::Session, global = true)]
    pub profile: ToolsProfile,
    #[command(subcommand)]
    pub command: ToolsCommand,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ToolsProfile {
    Session,
    Control,
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
    #[arg(long, default_value = "{}")]
    pub arguments: String,
    /// Tool call id to echo in the output envelope.
    #[arg(long)]
    pub call_id: Option<String>,
    /// Optional model turn id to echo in the output envelope.
    #[arg(long)]
    pub turn_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command};
    use crate::dispatch::cli_args::DispatchCommand;

    #[test]
    fn dispatch_direct_issue_parses_as_proposal_entrypoint() {
        let cli = Cli::try_parse_from([
            "issue-finder",
            "dispatch",
            "owner/repo#123",
            "--agent",
            "codex",
            "--new-session",
            "--json",
        ])
        .unwrap();

        let Command::Dispatch(args) = cli.command else {
            panic!("expected dispatch command");
        };
        let args = *args;
        assert!(args.command.is_none());
        assert_eq!(args.issue.as_deref(), Some("owner/repo#123"));
        assert_eq!(args.agent, "codex");
        assert!(args.new_session);
        assert!(args.session.is_none());
        assert!(args.json);
    }

    #[test]
    fn dispatch_status_subcommand_is_not_captured_as_direct_issue() {
        let cli = Cli::try_parse_from(["issue-finder", "dispatch", "status", "run-1"]).unwrap();

        let Command::Dispatch(args) = cli.command else {
            panic!("expected dispatch command");
        };
        let args = *args;
        assert!(args.issue.is_none());
        assert!(
            matches!(args.command, Some(DispatchCommand::Status(status)) if status.run_id == "run-1")
        );
    }
}
