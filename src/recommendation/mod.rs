pub mod candidate_ranker;
pub mod competition_completion;
pub mod engine;
pub mod eval;
pub mod events;
pub mod feed_ranker;
pub mod feedback;
pub mod freshness;
pub mod model;
pub mod quality_policy;
pub mod state;

pub use crate::discovery::{DiscoveryScope, RepositoryScope};
pub use engine::{RecommendationEngine, ScoutOptions, ScoutResult};
pub use events::{
    append_event, load_events, record_event_for_issue, record_event_for_key, IssueKey,
    RecommendationEvent, RecommendationEventSource, RecommendationEventType,
};
pub use model::{RecommendationAssessment, RecommendationVisibility};
pub use state::{load_state_map, recent_events_for_issue, RecommendationIssueState};
