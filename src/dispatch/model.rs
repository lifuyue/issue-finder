use serde::{Deserialize, Serialize};
use serde_json::Value;

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }

            pub fn parse_value(value: &str) -> Option<Self> {
                match value {
                    $($value => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

string_enum!(AgentCapabilityName {
    StartSession => "start_session",
    ResumeSession => "resume_session",
    ListSessions => "list_sessions",
    SearchSessions => "search_sessions",
    RenameSession => "rename_session",
    ForkSession => "fork_session",
    ArchiveSession => "archive_session",
    StreamEvents => "stream_events",
    ReadTranscript => "read_transcript",
    SetGoal => "set_goal",
    SetMetadata => "set_metadata",
    InterruptRun => "interrupt_run",
    ReviewMode => "review_mode",
    OpenPr => "open_pr",
});

string_enum!(CapabilityStatus {
    Supported => "supported",
    Unsupported => "unsupported",
    ApprovalBlocked => "approval_blocked",
    Experimental => "experimental",
});

string_enum!(IssueTaskStatus {
    Discovered => "discovered",
    LlmConfirmed => "llm_confirmed",
    UserApproved => "user_approved",
    Dispatched => "dispatched",
    InProgress => "in_progress",
    FixReady => "fix_ready",
    GithubPosted => "github_posted",
    Done => "done",
});

string_enum!(DispatchRunStatus {
    Proposed => "proposed",
    Approved => "approved",
    Queued => "queued",
    Starting => "starting",
    Running => "running",
    NeedsUser => "needs_user",
    Completed => "completed",
    Failed => "failed",
    Canceled => "canceled",
});

string_enum!(DispatchOutcomeKind {
    FixReady => "fix_ready",
    CompletedNoChange => "completed_no_change",
    NeedsUser => "needs_user",
    Blocked => "blocked",
    Failed => "failed",
    Canceled => "canceled",
});

impl DispatchOutcomeKind {
    pub fn is_positive(self) -> bool {
        matches!(self, Self::FixReady | Self::CompletedNoChange)
    }

    pub fn terminal_status(self) -> DispatchRunStatus {
        match self {
            Self::FixReady | Self::CompletedNoChange => DispatchRunStatus::Completed,
            Self::NeedsUser => DispatchRunStatus::NeedsUser,
            Self::Blocked | Self::Failed => DispatchRunStatus::Failed,
            Self::Canceled => DispatchRunStatus::Canceled,
        }
    }
}

string_enum!(DispatchOutcomeFailureClass {
    ValidationFailed => "validation_failed",
    ReproductionFailed => "reproduction_failed",
    DependencyUnavailable => "dependency_unavailable",
    WorkspaceUnavailable => "workspace_unavailable",
    ContextInsufficient => "context_insufficient",
    AgentRuntimeError => "agent_runtime_error",
    ExternalServiceError => "external_service_error",
    PolicyBlocked => "policy_blocked",
    UserCanceled => "user_canceled",
    Unknown => "unknown",
});

string_enum!(DispatchTaskClass {
    RustCliPanic => "rust_cli_panic",
    FrontendUiBug => "frontend_ui_bug",
    DocsUpdate => "docs_update",
    TestCoverage => "test_coverage",
    DependencyUpgrade => "dependency_upgrade",
    UnknownTask => "unknown_task",
});

string_enum!(DispatchValidationOutcome {
    NotRun => "not_run",
    Passed => "passed",
    Failed => "failed",
    Unknown => "unknown",
});

string_enum!(AgentSessionStatus {
    Linked => "linked",
    Active => "active",
    Idle => "idle",
    Archived => "archived",
    Failed => "failed",
});

string_enum!(GitHubInteractionType {
    TrackingComment => "tracking_comment",
    ProgressComment => "progress_comment",
    FinalComment => "final_comment",
});

string_enum!(GitHubInteractionStatus {
    Draft => "draft",
    Approved => "approved",
    Rejected => "rejected",
    Posted => "posted",
    Failed => "failed",
    Retried => "retried",
});

string_enum!(ApprovalType {
    IssueReview => "issue_review",
    Dispatch => "dispatch",
    ContinueSession => "continue_session",
    GithubPost => "github_post",
    A2aSend => "a2a_send",
    OpenPr => "open_pr",
    SessionMutation => "session_mutation",
});

string_enum!(ApprovalStatus {
    Pending => "pending",
    Approved => "approved",
    Rejected => "rejected",
    Canceled => "canceled",
});

string_enum!(MemoryEventType {
    PositiveSignal => "positive_signal",
    NegativeSignal => "negative_signal",
    ProfileAdjustmentCandidate => "profile_adjustment_candidate",
    AgentPerformanceSignal => "agent_performance_signal",
});

string_enum!(DispatchEventKind {
    DispatchApprovalResolved => "dispatch_approval_resolved",
    DispatchOutcomeRecorded => "dispatch_outcome_recorded",
    DispatchOutcomeMemoryIngestFailed => "dispatch_outcome_memory_ingest_failed",
    DispatchStarting => "dispatch_starting",
    DispatchFailed => "dispatch_failed",
    SessionSynced => "session_synced",
    SessionTranscriptRead => "session_transcript_read",
    SessionStarted => "session_started",
    SessionResumed => "session_resumed",
    SessionRenamed => "session_renamed",
    SessionForked => "session_forked",
    SessionArchived => "session_archived",
    TurnStarted => "turn_started",
    A2aResultImported => "a2a_result_imported",
    Legacy => "legacy",
});

string_enum!(DispatchEventSeverity {
    Info => "info",
    Warning => "warning",
    Error => "error",
});

string_enum!(DispatchEventSource {
    Runtime => "runtime",
    Adapter => "adapter",
    Tool => "tool",
    A2a => "a2a",
    Github => "github",
    Migration => "migration",
});

string_enum!(DispatchSubjectType {
    DispatchRun => "dispatch_run",
    Session => "session",
    IssueTask => "issue_task",
    Approval => "approval",
    Artifact => "artifact",
    Adapter => "adapter",
    System => "system",
});

string_enum!(PolicyAction {
    StartDispatch => "start_dispatch",
    ResumeDispatch => "resume_dispatch",
    ReadSessionTranscript => "read_session_transcript",
    RenameSession => "rename_session",
    ForkSession => "fork_session",
    ArchiveSession => "archive_session",
    SendA2aTask => "send_a2a_task",
    PostGithubComment => "post_github_comment",
    OpenPr => "open_pr",
});

string_enum!(PolicyRequirement {
    Allowed => "allowed",
    RequiresApproval => "requires_approval",
    Forbidden => "forbidden",
});

string_enum!(AdapterProbeStatus {
    Supported => "supported",
    Unsupported => "unsupported",
    Failed => "failed",
});

string_enum!(DispatchFailureClass {
    Adapter => "adapter",
    Capability => "capability",
    Policy => "policy",
    External => "external",
    Storage => "storage",
    Validation => "validation",
    Unknown => "unknown",
});

string_enum!(TranscriptPayloadStorage {
    Inline => "inline",
    Artifact => "artifact",
});

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentProfile {
    pub id: String,
    pub kind: String,
    pub display_name: String,
    pub adapter: String,
    pub config_json: Value,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewAgentProfile {
    pub id: Option<String>,
    pub kind: String,
    pub display_name: String,
    pub adapter: String,
    pub config_json: Value,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentCapability {
    pub agent_id: String,
    pub capability: AgentCapabilityName,
    pub status: CapabilityStatus,
    pub details_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewAgentCapability {
    pub agent_id: String,
    pub capability: AgentCapabilityName,
    pub status: CapabilityStatus,
    pub details_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IssueTask {
    pub id: String,
    pub issue_key: String,
    pub repo_full_name: String,
    pub issue_number: u64,
    pub title: String,
    pub url: String,
    pub status: IssueTaskStatus,
    pub priority: Option<i64>,
    pub category: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub current_package_artifact_id: Option<String>,
    pub profile_snapshot_artifact_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewIssueTask {
    pub repo_full_name: String,
    pub issue_number: u64,
    pub title: String,
    pub url: String,
    pub status: IssueTaskStatus,
    pub priority: Option<i64>,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DispatchRun {
    pub id: String,
    pub issue_task_id: String,
    pub agent_id: String,
    pub status: DispatchRunStatus,
    pub requested_by: String,
    pub approval_state: ApprovalStatus,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub selected_session_link_id: Option<String>,
    pub result_artifact_id: Option<String>,
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewDispatchRun {
    pub issue_task_id: String,
    pub agent_id: String,
    pub status: DispatchRunStatus,
    pub requested_by: String,
    pub approval_state: ApprovalStatus,
    pub selected_session_link_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DispatchRunOutcome {
    pub id: String,
    pub run_id: String,
    pub idempotency_key: String,
    pub outcome_kind: DispatchOutcomeKind,
    pub failure_class: Option<DispatchOutcomeFailureClass>,
    pub failure_detail: Option<String>,
    pub task_class: Option<DispatchTaskClass>,
    pub validation_outcome: Option<DispatchValidationOutcome>,
    pub result_artifact_id: Option<String>,
    pub metadata_json: Value,
    pub recorded_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewDispatchRunOutcome {
    pub run_id: String,
    pub idempotency_key: String,
    pub outcome_kind: DispatchOutcomeKind,
    pub failure_class: Option<DispatchOutcomeFailureClass>,
    pub failure_detail: Option<String>,
    pub task_class: Option<DispatchTaskClass>,
    pub validation_outcome: Option<DispatchValidationOutcome>,
    pub result_artifact_id: Option<String>,
    pub metadata_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentSessionLink {
    pub id: String,
    pub agent_id: String,
    pub native_session_id: String,
    pub issue_task_id: Option<String>,
    pub display_name: String,
    pub goal: Option<String>,
    pub status: AgentSessionStatus,
    pub metadata_json: Value,
    pub created_at: String,
    pub last_seen_at: String,
    pub archived_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewAgentSessionLink {
    pub agent_id: String,
    pub native_session_id: String,
    pub issue_task_id: Option<String>,
    pub display_name: String,
    pub goal: Option<String>,
    pub status: AgentSessionStatus,
    pub metadata_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DispatchEvent {
    pub id: String,
    pub sequence: i64,
    pub run_id: Option<String>,
    pub session_link_id: Option<String>,
    pub issue_task_id: Option<String>,
    pub event_kind: DispatchEventKind,
    pub subject_type: DispatchSubjectType,
    pub subject_id: Option<String>,
    pub source: DispatchEventSource,
    pub severity: DispatchEventSeverity,
    pub correlation_id: Option<String>,
    pub causation_id: Option<String>,
    pub native_event_id: Option<String>,
    pub payload_json: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewDispatchEvent {
    pub run_id: Option<String>,
    pub session_link_id: Option<String>,
    pub issue_task_id: Option<String>,
    pub event_kind: DispatchEventKind,
    pub subject_type: DispatchSubjectType,
    pub subject_id: Option<String>,
    pub source: DispatchEventSource,
    pub severity: DispatchEventSeverity,
    pub correlation_id: Option<String>,
    pub causation_id: Option<String>,
    pub native_event_id: Option<String>,
    pub payload_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DispatchFailure {
    pub id: String,
    pub run_id: String,
    pub phase: String,
    pub failure_class: DispatchFailureClass,
    pub code: String,
    pub retryable: bool,
    pub message: String,
    pub details_json: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewDispatchFailure {
    pub run_id: String,
    pub phase: String,
    pub failure_class: DispatchFailureClass,
    pub code: String,
    pub retryable: bool,
    pub message: String,
    pub details_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdapterProbeResult {
    pub id: String,
    pub agent_id: String,
    pub adapter: String,
    pub capability: AgentCapabilityName,
    pub method: Option<String>,
    pub status: AdapterProbeStatus,
    pub protocol_version: Option<String>,
    pub checked_at: String,
    pub expires_at: Option<String>,
    pub error_code: Option<String>,
    pub details_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewAdapterProbeResult {
    pub agent_id: String,
    pub adapter: String,
    pub capability: AgentCapabilityName,
    pub method: Option<String>,
    pub status: AdapterProbeStatus,
    pub protocol_version: Option<String>,
    pub expires_at: Option<String>,
    pub error_code: Option<String>,
    pub details_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionTranscriptItem {
    pub id: String,
    pub session_link_id: String,
    pub turn_id: Option<String>,
    pub item_index: i64,
    pub item_type: String,
    pub text: Option<String>,
    pub payload_artifact_id: Option<String>,
    pub payload_storage: TranscriptPayloadStorage,
    pub metadata_json: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewSessionTranscriptItem {
    pub session_link_id: String,
    pub turn_id: Option<String>,
    pub item_index: i64,
    pub item_type: String,
    pub text: Option<String>,
    pub payload_artifact_id: Option<String>,
    pub payload_storage: TranscriptPayloadStorage,
    pub metadata_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentArtifact {
    pub id: String,
    pub issue_task_id: Option<String>,
    pub run_id: Option<String>,
    pub kind: String,
    pub path: String,
    pub content_type: String,
    pub sha256: String,
    pub created_at: String,
    pub metadata_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewArtifact {
    pub issue_task_id: Option<String>,
    pub run_id: Option<String>,
    pub kind: String,
    pub content_type: String,
    pub metadata_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitHubInteraction {
    pub id: String,
    pub issue_task_id: String,
    pub interaction_type: GitHubInteractionType,
    pub github_comment_id: Option<String>,
    pub body_artifact_id: Option<String>,
    pub status: GitHubInteractionStatus,
    pub created_at: String,
    pub posted_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewGitHubInteraction {
    pub issue_task_id: String,
    pub interaction_type: GitHubInteractionType,
    pub body_artifact_id: Option<String>,
    pub status: GitHubInteractionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApprovalRequest {
    pub id: String,
    pub run_id: Option<String>,
    pub approval_type: ApprovalType,
    pub status: ApprovalStatus,
    pub prompt: String,
    pub details_json: Value,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewApprovalRequest {
    pub run_id: Option<String>,
    pub approval_type: ApprovalType,
    pub status: ApprovalStatus,
    pub prompt: String,
    pub details_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryEvent {
    pub id: String,
    pub issue_task_id: Option<String>,
    pub event_type: MemoryEventType,
    pub source: String,
    pub payload_json: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewMemoryEvent {
    pub issue_task_id: Option<String>,
    pub event_type: MemoryEventType,
    pub source: String,
    pub payload_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct A2aTaskExport {
    pub kind: String,
    pub version: u8,
    pub task: A2aTask,
    pub input_artifacts: Vec<A2aArtifactRef>,
    pub expected_artifacts: Vec<String>,
    pub callback: A2aCallbackPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct A2aTask {
    pub id: String,
    pub task_type: String,
    pub issue_key: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct A2aArtifactRef {
    pub role: String,
    pub name: String,
    pub artifact_id: String,
    pub path: String,
    pub content_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct A2aCallbackPolicy {
    pub expected_result_artifact: String,
    pub import_mode: String,
}
