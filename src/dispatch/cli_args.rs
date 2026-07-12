use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct AgentsArgs {
    #[command(subcommand)]
    pub command: AgentsCommand,
}

#[derive(Debug, Subcommand)]
pub enum AgentsCommand {
    /// List configured execution agent profiles.
    List(AgentsListArgs),
    /// List one agent's declared native capabilities.
    Capabilities(AgentCapabilitiesArgs),
    /// Probe and cache one agent's adapter capabilities.
    Probe(AgentProbeArgs),
}

#[derive(Debug, Args)]
pub struct AgentsListArgs {
    /// Print agent profiles as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentCapabilitiesArgs {
    /// Agent id, for example codex.
    pub agent: String,
    /// Print capabilities as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentProbeArgs {
    /// Agent id, for example codex.
    pub agent: String,
    /// Force a fresh probe instead of using a successful cache entry.
    #[arg(long)]
    pub refresh: bool,
    /// Print probe result as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct DispatchArgs {
    #[command(subcommand)]
    pub command: Option<DispatchCommand>,
    /// Issue reference in owner/repo#123 form. Without a subcommand, dispatch creates a proposal.
    pub issue: Option<String>,
    /// Agent id, for example codex. Used by direct `dispatch <issue>`.
    #[arg(long, default_value = "codex")]
    pub agent: String,
    /// Use a new native session after approval. Used by direct `dispatch <issue>`.
    #[arg(long, conflicts_with = "session")]
    pub new_session: bool,
    /// Existing local session link id or native session id to continue after approval.
    #[arg(long, conflicts_with = "new_session")]
    pub session: Option<String>,
    /// Print direct dispatch proposal as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum DispatchCommand {
    /// Import prepared handoffs into dispatch task packages.
    Package(DispatchPackageArgs),
    /// Review imported handoffs before creating task packages.
    Review(DispatchReviewArgs),
    /// Create an approval-gated dispatch proposal without starting an agent.
    Propose(DispatchProposeArgs),
    /// Approve a pending dispatch proposal.
    Approve(DispatchApprovalArgs),
    /// Reject a pending dispatch proposal.
    Reject(DispatchApprovalArgs),
    /// Execute an approved dispatch through the run's native adapter.
    Execute(DispatchExecuteArgs),
    /// Reconcile one non-terminal dispatch with its native Codex thread.
    Sync(DispatchStatusArgs),
    /// Map task packages and results to local A2A artifacts.
    A2a(DispatchA2aArgs),
    /// Record a normalized dispatch outcome.
    Outcome(DispatchOutcomeArgs),
    /// Draft, approve, and post GitHub issue comments from dispatch state.
    Github(DispatchGithubArgs),
    /// Show one dispatch run summary.
    Status(DispatchStatusArgs),
    /// List persisted events for a dispatch run.
    Events(DispatchRunReadArgs),
    /// Show merged dispatch timeline for a run.
    Timeline(DispatchRunReadArgs),
    /// Show diagnostic dispatch trace for a run.
    Trace(DispatchRunReadArgs),
    /// List persisted artifacts for a dispatch run.
    Artifacts(DispatchRunReadArgs),
}

#[derive(Debug, Args)]
pub struct DispatchStatusArgs {
    /// Dispatch run id.
    pub run_id: String,
    /// Print status as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DispatchRunReadArgs {
    /// Dispatch run id.
    pub run_id: String,
    /// Print results as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DispatchPackageArgs {
    #[command(subcommand)]
    pub command: DispatchPackageCommand,
}

#[derive(Debug, Subcommand)]
pub enum DispatchPackageCommand {
    /// Import an existing inbox handoff as an issue review candidate.
    ImportHandoff(DispatchImportHandoffArgs),
}

#[derive(Debug, Args)]
pub struct DispatchImportHandoffArgs {
    /// Inbox item id.
    pub inbox_id: String,
    /// Print import result as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DispatchReviewArgs {
    #[command(subcommand)]
    pub command: DispatchReviewCommand,
}

#[derive(Debug, Subcommand)]
pub enum DispatchReviewCommand {
    /// List issue review requests.
    List(DispatchReviewListArgs),
    /// Show one issue review request.
    Show(DispatchReviewReadArgs),
    /// Approve one issue review and create a task package.
    Approve(DispatchReviewReadArgs),
    /// Reject one issue review without dismissing the recommendation.
    Reject(DispatchReviewRejectArgs),
}

#[derive(Debug, Args)]
pub struct DispatchReviewListArgs {
    /// Print reviews as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DispatchReviewReadArgs {
    /// Issue review approval request id.
    pub approval_request_id: String,
    /// Print result as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DispatchReviewRejectArgs {
    /// Issue review approval request id.
    pub approval_request_id: String,
    /// Optional rejection reason for memory signal payload.
    #[arg(long)]
    pub reason: Option<String>,
    /// Print result as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DispatchProposeArgs {
    /// Issue reference in owner/repo#123 form.
    pub issue: String,
    /// Agent id, for example codex.
    #[arg(long, default_value = "codex")]
    pub agent: String,
    /// Use a new native session after approval.
    #[arg(long, conflicts_with = "session")]
    pub new_session: bool,
    /// Existing local session link id or native session id to continue after approval.
    #[arg(long, conflicts_with = "new_session")]
    pub session: Option<String>,
    /// Print proposal as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DispatchApprovalArgs {
    /// Dispatch run id.
    pub run_id: String,
    /// Print approval result as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DispatchExecuteArgs {
    /// Approved dispatch run id.
    pub run_id: String,
    /// Print execution result as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DispatchA2aArgs {
    #[command(subcommand)]
    pub command: DispatchA2aCommand,
}

#[derive(Debug, Subcommand)]
pub enum DispatchA2aCommand {
    /// Export an imported IssueTaskPackage v3 as a local A2A task artifact.
    Export(DispatchA2aExportArgs),
    /// Approve an outbound A2A task artifact for external use.
    Approve(DispatchA2aApprovalArgs),
    /// Reject an outbound A2A task artifact.
    Reject(DispatchA2aApprovalArgs),
    /// Import a local A2A result file that satisfies the package outcome contract.
    ImportResult(Box<DispatchA2aImportResultArgs>),
}

#[derive(Debug, Args)]
pub struct DispatchA2aExportArgs {
    /// Issue reference in owner/repo#123 form.
    pub issue: String,
    /// Print export result as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DispatchA2aApprovalArgs {
    /// A2A send approval request id.
    pub approval_request_id: String,
    /// Print approval result as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DispatchA2aImportResultArgs {
    /// Dispatch run id.
    pub run_id: String,
    /// Local result file path.
    #[arg(long)]
    pub path: PathBuf,
    /// Artifact kind, for example fix_result.
    #[arg(long, default_value = "fix_result")]
    pub kind: String,
    /// Artifact content type.
    #[arg(long, default_value = "application/json")]
    pub content_type: String,
    /// Optional dispatch run status to set after import.
    #[arg(long)]
    pub status: Option<String>,
    /// Optional normalized outcome kind to record.
    #[arg(long)]
    pub outcome: Option<String>,
    /// Optional normalized failure class.
    #[arg(long)]
    pub failure_class: Option<String>,
    /// Optional human-readable failure detail.
    #[arg(long)]
    pub failure_reason: Option<String>,
    /// Optional normalized task class.
    #[arg(long)]
    pub task_class: Option<String>,
    /// Optional normalized validation outcome.
    #[arg(long)]
    pub validation_outcome: Option<String>,
    /// Optional idempotency key for outcome recording.
    #[arg(long)]
    pub idempotency_key: Option<String>,
    /// Print import result as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DispatchOutcomeArgs {
    #[command(subcommand)]
    pub command: DispatchOutcomeCommand,
}

#[derive(Debug, Subcommand)]
pub enum DispatchOutcomeCommand {
    /// Record a normalized terminal or blocked dispatch outcome.
    Record(DispatchOutcomeRecordArgs),
}

#[derive(Debug, Args)]
pub struct DispatchOutcomeRecordArgs {
    /// Dispatch run id.
    pub run_id: String,
    /// Normalized outcome kind.
    #[arg(long)]
    pub outcome: String,
    /// Optional normalized failure class.
    #[arg(long)]
    pub failure_class: Option<String>,
    /// Optional human-readable failure detail.
    #[arg(long)]
    pub failure_reason: Option<String>,
    /// Optional normalized task class.
    #[arg(long)]
    pub task_class: Option<String>,
    /// Optional normalized validation outcome.
    #[arg(long)]
    pub validation_outcome: Option<String>,
    /// Optional dispatch artifact id associated with this outcome.
    #[arg(long)]
    pub result_artifact_id: Option<String>,
    /// Optional idempotency key.
    #[arg(long)]
    pub idempotency_key: Option<String>,
    /// Print result as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DispatchGithubArgs {
    #[command(subcommand)]
    pub command: DispatchGithubCommand,
}

#[derive(Debug, Subcommand)]
pub enum DispatchGithubCommand {
    /// Draft a tracking comment for an imported issue task.
    DraftTracking(DispatchGithubDraftTrackingArgs),
    /// Draft a final comment from a dispatch result artifact.
    DraftFinal(DispatchGithubDraftFinalArgs),
    /// Approve a drafted GitHub comment for posting.
    Approve(DispatchGithubInteractionArgs),
    /// Reject a drafted GitHub comment.
    Reject(DispatchGithubInteractionArgs),
    /// Post an approved GitHub comment through the configured GitHub token.
    Post(DispatchGithubInteractionArgs),
    /// Retry posting a failed GitHub comment interaction.
    Retry(DispatchGithubInteractionArgs),
    /// List local GitHub comment interactions for an issue task.
    List(DispatchGithubListArgs),
}

#[derive(Debug, Args)]
pub struct DispatchGithubDraftTrackingArgs {
    /// Issue reference in owner/repo#123 form.
    pub issue: String,
    /// Override the generated tracking comment body.
    #[arg(long)]
    pub body: Option<String>,
    /// Print draft result as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DispatchGithubDraftFinalArgs {
    /// Dispatch run id with an imported fix result artifact.
    pub run_id: String,
    /// Override the generated final comment body.
    #[arg(long)]
    pub body: Option<String>,
    /// Print draft result as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DispatchGithubInteractionArgs {
    /// Local GitHub interaction id.
    pub interaction_id: String,
    /// Print result as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DispatchGithubListArgs {
    /// Issue reference in owner/repo#123 form.
    #[arg(long)]
    pub issue: String,
    /// Print interactions as JSON.
    #[arg(long)]
    pub json: bool,
}
