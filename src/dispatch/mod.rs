pub mod a2a_gateway;
pub mod adapters;
pub mod cli;
pub mod cli_args;
pub mod execution;
pub mod github_projection;
pub mod memory;
pub mod model;
mod output;
pub mod packaging;
pub mod runtime;
pub mod session_approvals;
pub mod session_ops;
pub mod store;
pub mod tool_specs;
pub mod tools;

pub use a2a_gateway::{A2aApprovalResult, A2aExportResult, A2aResultImport};
pub use execution::DispatchExecutionResult;
pub use github_projection::{
    GitHubApprovalResult, GitHubCommentDraftResult, GitHubCommentWriter, GitHubPostResult,
    PostedGitHubComment, ReqwestGitHubCommentWriter,
};
pub use model::{
    A2aArtifactRef, A2aCallbackPolicy, A2aTask, A2aTaskExport, AgentArtifact, AgentCapability,
    AgentCapabilityName, AgentEvent, AgentProfile, AgentSessionLink, AgentSessionStatus,
    ApprovalRequest, ApprovalStatus, ApprovalType, CapabilityStatus, DispatchRun,
    DispatchRunStatus, GitHubInteraction, GitHubInteractionStatus, GitHubInteractionType,
    IssueTask, IssueTaskPackage, IssueTaskPackageIssue, IssueTaskStatus, MemoryEvent,
    MemoryEventType, NewAgentCapability, NewAgentEvent, NewAgentProfile, NewAgentSessionLink,
    NewApprovalRequest, NewArtifact, NewDispatchRun, NewGitHubInteraction, NewIssueTask,
    NewMemoryEvent,
};
pub use packaging::PackageImportResult;
pub use runtime::{
    AgentCapabilitiesView, DispatchApprovalResolution, DispatchProposal, DispatchProposalRequest,
    DispatchRuntime, DispatchStatusSnapshot, SessionSearchResult,
};
pub use session_approvals::{
    PendingSessionMutation, SessionMutationApprovalResolution, SessionMutationProposal,
};
pub use session_ops::{
    SessionMutationResult, SessionTranscriptResult, SessionsSyncRequest, SessionsSyncResult,
};
pub use store::DispatchStore;

pub use cli::{handle_agents_cli, handle_dispatch_cli, handle_sessions_cli};
