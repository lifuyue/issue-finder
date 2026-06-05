pub mod candidate_ranker;
pub mod engine;
pub mod events;
pub mod feed_ranker;
pub mod feedback;
pub mod freshness;
pub mod model;
pub mod state;

pub use engine::{RecommendationEngine, ScoutOptions, ScoutResult};
pub use events::{
    append_event, load_events, record_event_for_issue, record_event_for_key, IssueKey,
    RecommendationEvent, RecommendationEventSource, RecommendationEventType,
};
pub use model::{RecommendationAssessment, RecommendationVisibility};
pub use state::{load_state_map, recent_events_for_issue, RecommendationIssueState};
