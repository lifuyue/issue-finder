use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::dispatch::cli_args::{AgentsArgs, DispatchArgs, SessionsArgs};

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
    /// Inspect local links to native execution agent sessions.
    Sessions(SessionsArgs),
    /// Inspect local dispatch runs, events, and artifacts.
    Dispatch(DispatchArgs),
    /// Run recommendation evaluation workflows.
    Eval(EvalArgs),
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
    /// Generate offline or live recommendation evaluation reports.
    Recommendation(RecommendationEvalArgs),
}

#[derive(Debug, Args)]
pub struct RecommendationEvalArgs {
    /// Run deterministic offline fixture evaluation.
    #[arg(long, conflicts_with = "live")]
    pub offline: bool,
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
        assert!(args.command.is_none());
        assert_eq!(args.issue.as_deref(), Some("owner/repo#123"));
        assert_eq!(args.agent, "codex");
        assert!(args.new_session);
        assert!(args.session.is_none());
        assert!(args.json);
    }

    #[test]
    fn dispatch_propose_subcommand_remains_supported() {
        let cli = Cli::try_parse_from([
            "issue-finder",
            "dispatch",
            "propose",
            "owner/repo#123",
            "--agent",
            "codex",
            "--session",
            "thread-1",
        ])
        .unwrap();

        let Command::Dispatch(args) = cli.command else {
            panic!("expected dispatch command");
        };
        assert!(args.issue.is_none());
        let Some(DispatchCommand::Propose(propose)) = args.command else {
            panic!("expected dispatch propose subcommand");
        };
        assert_eq!(propose.issue, "owner/repo#123");
        assert_eq!(propose.agent, "codex");
        assert_eq!(propose.session.as_deref(), Some("thread-1"));
    }

    #[test]
    fn dispatch_status_subcommand_is_not_captured_as_direct_issue() {
        let cli = Cli::try_parse_from(["issue-finder", "dispatch", "status", "run-1"]).unwrap();

        let Command::Dispatch(args) = cli.command else {
            panic!("expected dispatch command");
        };
        assert!(args.issue.is_none());
        assert!(
            matches!(args.command, Some(DispatchCommand::Status(status)) if status.run_id == "run-1")
        );
    }
}
