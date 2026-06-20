pub mod a2a_gateway;
pub mod adapters;
pub mod capability_probe;
pub mod cli;
pub mod cli_args;
pub mod events;
pub mod execution;
pub mod failure;
pub mod github_projection;
pub mod memory;
pub mod model;
mod output;
pub mod packaging;
pub mod policy;
pub mod runtime;
pub mod session_approvals;
pub mod session_ops;
pub mod store;
pub mod timeline;
pub mod tool_specs;
pub mod tools;

pub use a2a_gateway::{A2aApprovalResult, A2aExportResult, A2aResultImport};
pub use capability_probe::AgentProbeReport;
pub use execution::DispatchExecutionResult;
pub use github_projection::{
    GitHubApprovalResult, GitHubCommentDraftResult, GitHubCommentWriter, GitHubPostResult,
    PostedGitHubComment, ReqwestGitHubCommentWriter,
};
pub use model::{
    A2aArtifactRef, A2aCallbackPolicy, A2aTask, A2aTaskExport, AdapterProbeResult,
    AdapterProbeStatus, AgentArtifact, AgentCapability, AgentCapabilityName, AgentProfile,
    AgentSessionLink, AgentSessionStatus, ApprovalRequest, ApprovalStatus, ApprovalType,
    CapabilityStatus, DispatchEvent, DispatchEventKind, DispatchEventSeverity, DispatchEventSource,
    DispatchFailure, DispatchFailureClass, DispatchRun, DispatchRunStatus, DispatchSubjectType,
    GitHubInteraction, GitHubInteractionStatus, GitHubInteractionType, IssueTask, IssueTaskPackage,
    IssueTaskPackageIssue, IssueTaskStatus, MemoryEvent, MemoryEventType, NewAdapterProbeResult,
    NewAgentCapability, NewAgentProfile, NewAgentSessionLink, NewApprovalRequest, NewArtifact,
    NewDispatchEvent, NewDispatchFailure, NewDispatchRun, NewGitHubInteraction, NewIssueTask,
    NewMemoryEvent, NewSessionTranscriptItem, PolicyAction, PolicyRequirement,
    SessionTranscriptItem, TranscriptPayloadStorage,
};
pub use packaging::{IssueReviewDetail, IssueReviewResolution, PackageImportResult};
pub use policy::PolicyDecision;
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
pub use timeline::{ApprovalLatency, DispatchTimeline, DispatchTrace, TimelineItem};

pub use cli::{handle_agents_cli, handle_dispatch_cli, handle_sessions_cli};
