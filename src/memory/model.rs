use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MemoryModelError {
    #[error("unknown memory enum value `{value}` for {kind}")]
    UnknownEnumValue { kind: &'static str, value: String },
}

macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $value:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }

            pub fn parse(value: &str) -> Result<Self, MemoryModelError> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    other => Err(MemoryModelError::UnknownEnumValue {
                        kind: stringify!($name),
                        value: other.to_string(),
                    }),
                }
            }
        }
    };
}

string_enum! {
    pub enum MemorySourceType {
        RecommendationEvent => "recommendation_event",
        DispatchEvent => "dispatch_event",
        GithubInteraction => "github_interaction",
        ProfileBootstrap => "profile_bootstrap",
        Manual => "manual",
    }
}

string_enum! {
    pub enum MemoryTrustLevel {
        UserExplicit => "user_explicit",
        SystemObserved => "system_observed",
        ExternalGithub => "external_github",
        AgentObserved => "agent_observed",
        LlmInferred => "llm_inferred",
    }
}

string_enum! {
    pub enum MemoryRawEventType {
        Approve => "approve",
        Reject => "reject",
        Dismiss => "dismiss",
        DispatchSuccess => "dispatch_success",
        DispatchFailure => "dispatch_failure",
        MaintainerReply => "maintainer_reply",
        ValidationPass => "validation_pass",
        ValidationFail => "validation_fail",
        Manual => "manual",
    }
}

string_enum! {
    pub enum MemoryRole {
        User => "user",
        System => "system",
        Agent => "agent",
        Github => "github",
        Llm => "llm",
    }
}

string_enum! {
    pub enum MemorySubjectType {
        Issue => "issue",
        Repo => "repo",
        Agent => "agent",
        Maintainer => "maintainer",
        Label => "label",
        Validation => "validation",
        Profile => "profile",
        Manual => "manual",
    }
}

string_enum! {
    pub enum MemoryNodeType {
        RawEvent => "raw_event",
        Entity => "entity",
        Episode => "episode",
        ClaimCandidate => "claim_candidate",
        Dream => "dream",
        Hint => "hint",
    }
}

string_enum! {
    pub enum MemoryIndexType {
        Fts => "fts",
        Embedding => "embedding",
        RareToken => "rare_token",
        Entity => "entity",
    }
}

string_enum! {
    pub enum MemoryEdgeRelation {
        CoActivated => "co_activated",
        PredictsSuccess => "predicts_success",
        PredictsFailure => "predicts_failure",
        Prefers => "prefers",
        Avoids => "avoids",
        FailsDueTo => "fails_due_to",
        ValidatesWith => "validates_with",
        MaintainerStyle => "maintainer_style",
        AgentSucceedsOn => "agent_succeeds_on",
        AgentFailsOn => "agent_fails_on",
        RepoHasPattern => "repo_has_pattern",
    }
}

string_enum! {
    pub enum MemoryQueryKind {
        ScoutRanking => "scout_ranking",
        DispatchPlanning => "dispatch_planning",
        GithubDraft => "github_draft",
        ProfileReview => "profile_review",
    }
}

string_enum! {
    pub enum MemorySourceChannel {
        Fts => "fts",
        Embedding => "embedding",
        Entity => "entity",
        Recent => "recent",
        NearRipple => "near_ripple",
        FarRipple => "far_ripple",
    }
}

string_enum! {
    pub enum MemoryWritebackAction {
        Recalled => "recalled",
        Reinforced => "reinforced",
        EdgeReinforced => "edge_reinforced",
        ResourceDecremented => "resource_decremented",
    }
}

string_enum! {
    pub enum MemoryDreamTrigger {
        Scheduled => "scheduled",
        Manual => "manual",
        AfterDispatch => "after_dispatch",
        AfterFeedback => "after_feedback",
        AfterEval => "after_eval",
        AfterProfileBootstrap => "after_profile_bootstrap",
    }
}

string_enum! {
    pub enum MemoryDreamScope {
        Global => "global",
        Repo => "repo",
        Agent => "agent",
        Profile => "profile",
        IssueType => "issue_type",
    }
}

string_enum! {
    pub enum MemoryModelStatus {
        Disabled => "disabled",
        Success => "success",
        Failed => "failed",
    }
}

string_enum! {
    pub enum MemoryDreamType {
        ProfileAdjustment => "profile_adjustment",
        RepoSummary => "repo_summary",
        AgentPerformance => "agent_performance",
        DiscoveryPolicy => "discovery_policy",
        StaleMemory => "stale_memory",
        Conflict => "conflict",
    }
}

string_enum! {
    pub enum MemoryDreamStatus {
        Candidate => "candidate",
        Approved => "approved",
        Rejected => "rejected",
        Stale => "stale",
        Tombstoned => "tombstoned",
    }
}

string_enum! {
    pub enum MemoryHintType {
        Ranking => "ranking",
        Dispatch => "dispatch",
        GithubDraft => "github_draft",
        ProfileCandidate => "profile_candidate",
    }
}

string_enum! {
    pub enum MemoryHintScopeType {
        Global => "global",
        Repo => "repo",
        Agent => "agent",
        IssueType => "issue_type",
        Maintainer => "maintainer",
    }
}

string_enum! {
    pub enum MemoryHintStatus {
        Candidate => "candidate",
        Approved => "approved",
        Rejected => "rejected",
        Pinned => "pinned",
        Deprioritized => "deprioritized",
        Suppressed => "suppressed",
        Stale => "stale",
        Tombstoned => "tombstoned",
    }
}

impl MemoryHintStatus {
    pub fn is_active_decision_status(self) -> bool {
        matches!(self, Self::Approved | Self::Pinned | Self::Deprioritized)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemorySource {
    pub id: String,
    pub source_type: MemorySourceType,
    pub source_ref: String,
    pub trust_level: MemoryTrustLevel,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewMemorySource {
    pub id: String,
    pub source_type: MemorySourceType,
    pub source_ref: String,
    pub trust_level: MemoryTrustLevel,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryRawEvent {
    pub id: String,
    pub source_id: String,
    pub event_type: MemoryRawEventType,
    pub role: MemoryRole,
    pub trust_level: MemoryTrustLevel,
    pub subject_type: MemorySubjectType,
    pub subject_ref: String,
    pub payload_json: Value,
    pub confidence: f64,
    pub occurred_at: String,
    pub created_at: String,
    pub tombstoned_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewMemoryRawEvent {
    pub id: String,
    pub source_id: String,
    pub event_type: MemoryRawEventType,
    pub role: MemoryRole,
    pub trust_level: MemoryTrustLevel,
    pub subject_type: MemorySubjectType,
    pub subject_ref: String,
    pub payload_json: Value,
    pub confidence: f64,
    pub occurred_at: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryNode {
    pub id: String,
    pub node_type: MemoryNodeType,
    pub raw_event_id: Option<String>,
    pub entity_type: Option<String>,
    pub entity_value: Option<String>,
    pub normalized_value: Option<String>,
    pub text_ref: Option<String>,
    pub metadata_json: Value,
    pub created_at: String,
    pub tombstoned_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewMemoryNode {
    pub id: String,
    pub node_type: MemoryNodeType,
    pub raw_event_id: Option<String>,
    pub entity_type: Option<String>,
    pub entity_value: Option<String>,
    pub normalized_value: Option<String>,
    pub text_ref: Option<String>,
    pub metadata_json: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryNodeState {
    pub node_id: String,
    pub salience: f64,
    pub strength: f64,
    pub resource: f64,
    pub recall_count: i64,
    pub reinforce_count: i64,
    pub fan_in: i64,
    pub fan_out: i64,
    pub last_recalled_at: Option<String>,
    pub last_reinforced_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryIndex {
    pub node_id: String,
    pub index_type: MemoryIndexType,
    pub index_ref_or_payload: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryEdge {
    pub id: String,
    pub from_node_id: String,
    pub to_node_id: String,
    pub relation: MemoryEdgeRelation,
    pub strength: f64,
    pub confidence: f64,
    pub evidence_event_ids_json: Value,
    pub last_activated_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub tombstoned_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewMemoryEdge {
    pub id: String,
    pub from_node_id: String,
    pub to_node_id: String,
    pub relation: MemoryEdgeRelation,
    pub strength: f64,
    pub confidence: f64,
    pub evidence_event_ids_json: Value,
    pub last_activated_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryActivationRun {
    pub id: String,
    pub query_kind: MemoryQueryKind,
    pub query_ref: String,
    pub query_json: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryActivationItem {
    pub run_id: String,
    pub node_id: String,
    pub source_channel: MemorySourceChannel,
    pub direct_score: f64,
    pub ripple_score: f64,
    pub salience_score: f64,
    pub strength_score: f64,
    pub recency_score: f64,
    pub resource_penalty: f64,
    pub hub_penalty: f64,
    pub role_trust_penalty: f64,
    pub hop_penalty: f64,
    pub stale_penalty: f64,
    pub final_score: f64,
    pub rank: i64,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryWriteback {
    pub id: String,
    pub activation_run_id: String,
    pub node_id: String,
    pub action: MemoryWritebackAction,
    pub before_json: Value,
    pub after_json: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryDreamRun {
    pub id: String,
    pub trigger: MemoryDreamTrigger,
    pub scope: MemoryDreamScope,
    pub input_activation_run_ids_json: Value,
    pub model_status: MemoryModelStatus,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryDream {
    pub id: String,
    pub dream_run_id: String,
    pub dream_type: MemoryDreamType,
    pub summary: String,
    pub evidence_node_ids_json: Value,
    pub evidence_event_ids_json: Value,
    pub evidence_hint_ids_json: Value,
    pub status: MemoryDreamStatus,
    pub confidence: f64,
    pub version: i64,
    pub created_at: String,
    pub reviewed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewMemoryDream {
    pub id: String,
    pub dream_run_id: String,
    pub dream_type: MemoryDreamType,
    pub summary: String,
    pub evidence_node_ids_json: Value,
    pub evidence_event_ids_json: Value,
    pub evidence_hint_ids_json: Value,
    pub status: MemoryDreamStatus,
    pub confidence: f64,
    pub version: i64,
    pub created_at: String,
    pub reviewed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryHint {
    pub id: String,
    pub dream_id: String,
    pub hint_type: MemoryHintType,
    pub scope_type: MemoryHintScopeType,
    pub scope_ref: String,
    pub summary: String,
    pub policy_json: Value,
    pub weight: f64,
    pub status: MemoryHintStatus,
    pub created_at: String,
    pub approved_at: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewMemoryHint {
    pub id: String,
    pub dream_id: String,
    pub hint_type: MemoryHintType,
    pub scope_type: MemoryHintScopeType,
    pub scope_ref: String,
    pub summary: String,
    pub policy_json: Value,
    pub weight: f64,
    pub status: MemoryHintStatus,
    pub created_at: String,
    pub approved_at: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryHintStatusChange {
    pub id: i64,
    pub hint_id: String,
    pub from_status: MemoryHintStatus,
    pub to_status: MemoryHintStatus,
    pub changed_at: String,
    pub reason: Option<String>,
}
