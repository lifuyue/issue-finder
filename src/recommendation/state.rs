use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::paths::IssueFinderPaths;

use super::events::{load_events, IssueKey, RecommendationEvent, RecommendationEventType};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecommendationIssueState {
    pub issue_key: IssueKey,
    pub shown_count: u32,
    pub read_count: u32,
    pub prepared_count: u32,
    pub dismissed: bool,
    pub done: bool,
    pub restored_at: Option<String>,
    pub last_shown_at: Option<String>,
    pub last_read_at: Option<String>,
    pub last_prepared_at: Option<String>,
    pub last_feedback_at: Option<String>,
    pub last_seen_issue_updated_at: Option<String>,
    pub last_seen_comments_count: Option<u64>,
}

pub fn load_state_map(
    paths: &IssueFinderPaths,
) -> Result<HashMap<IssueKey, RecommendationIssueState>> {
    Ok(derive_state_map(&load_events(paths)?))
}

pub fn derive_state_map(
    events: &[RecommendationEvent],
) -> HashMap<IssueKey, RecommendationIssueState> {
    let mut states = HashMap::<IssueKey, RecommendationIssueState>::new();
    let mut sorted = events.to_vec();
    sorted.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.event_id.cmp(&right.event_id))
    });

    for event in sorted {
        let state =
            states
                .entry(event.issue_key.clone())
                .or_insert_with(|| RecommendationIssueState {
                    issue_key: event.issue_key.clone(),
                    ..RecommendationIssueState::default()
                });
        apply_event(state, &event);
    }

    states
}

pub fn recent_events_for_issue(
    paths: &IssueFinderPaths,
    issue_key: &IssueKey,
    limit: usize,
) -> Result<Vec<RecommendationEvent>> {
    let mut events = load_events(paths)?
        .into_iter()
        .filter(|event| &event.issue_key == issue_key)
        .collect::<Vec<_>>();
    events.sort_by(|left, right| {
        right
            .timestamp
            .cmp(&left.timestamp)
            .then_with(|| right.event_id.cmp(&left.event_id))
    });
    events.truncate(limit);
    Ok(events)
}

fn apply_event(state: &mut RecommendationIssueState, event: &RecommendationEvent) {
    state.last_feedback_at = Some(event.timestamp.clone());
    if let Some(updated_at) = &event.issue_updated_at {
        state.last_seen_issue_updated_at = Some(updated_at.clone());
    }
    if let Some(comments_count) = event.issue_comments_count {
        state.last_seen_comments_count = Some(comments_count);
    }

    match event.event_type {
        RecommendationEventType::Shown => {
            state.shown_count = state.shown_count.saturating_add(1);
            state.last_shown_at = Some(event.timestamp.clone());
        }
        RecommendationEventType::Read => {
            state.read_count = state.read_count.saturating_add(1);
            state.last_read_at = Some(event.timestamp.clone());
        }
        RecommendationEventType::Prepared => {
            state.prepared_count = state.prepared_count.saturating_add(1);
            state.last_prepared_at = Some(event.timestamp.clone());
        }
        RecommendationEventType::Done => {
            state.done = true;
            state.dismissed = false;
        }
        RecommendationEventType::Dismissed => {
            state.dismissed = true;
            state.done = false;
        }
        RecommendationEventType::Restored => {
            state.dismissed = false;
            state.done = false;
            state.restored_at = Some(event.timestamp.clone());
        }
    }
}
