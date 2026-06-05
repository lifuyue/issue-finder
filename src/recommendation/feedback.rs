use chrono::{DateTime, Utc};

use crate::github_enrichment::EnrichedIssue;

use super::model::RecommendationVisibility;
use super::state::RecommendationIssueState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackAssessment {
    pub penalty: i32,
    pub reactivation_boost: i32,
    pub visibility: RecommendationVisibility,
    pub reasons: Vec<String>,
}

pub fn assess_feedback(
    state: Option<&RecommendationIssueState>,
    enriched: &EnrichedIssue,
) -> FeedbackAssessment {
    let Some(state) = state else {
        return FeedbackAssessment {
            penalty: 0,
            reactivation_boost: 0,
            visibility: RecommendationVisibility::Visible,
            reasons: Vec::new(),
        };
    };

    if state.done {
        return FeedbackAssessment {
            penalty: 0,
            reactivation_boost: 0,
            visibility: RecommendationVisibility::HiddenDone,
            reasons: vec!["Issue is marked done in recommendation feedback".to_string()],
        };
    }
    if state.dismissed {
        return FeedbackAssessment {
            penalty: 0,
            reactivation_boost: 0,
            visibility: RecommendationVisibility::HiddenDismissed,
            reasons: vec!["Issue is dismissed in recommendation feedback".to_string()],
        };
    }

    let mut penalty = shown_penalty(state) + read_penalty(state) + prepared_penalty(state);
    let mut reactivation_boost = 0;
    let mut reasons = Vec::new();

    if state.shown_count > 0 {
        reasons.push(format!(
            "Shown {} time(s), reducing feed priority",
            state.shown_count
        ));
    }
    if state.read_count > 0 {
        reasons.push(format!(
            "Read {} time(s), reducing feed priority",
            state.read_count
        ));
    }
    if state.prepared_count > 0 {
        reasons.push(format!(
            "Prepared {} time(s), strongly reducing repeat priority",
            state.prepared_count
        ));
    }

    let updated_after_feedback = timestamp_after(
        Some(enriched.issue.updated_at.as_str()),
        state.last_feedback_at.as_deref(),
    );
    if updated_after_feedback {
        reactivation_boost += 15;
        penalty *= 0.70;
        reasons.push(
            "Issue changed after the last recommendation feedback (+15 recovery)".to_string(),
        );
    }

    if state
        .last_seen_comments_count
        .is_some_and(|count| enriched.issue.comments_count > count)
    {
        reactivation_boost += 25;
        penalty *= 0.50;
        reasons.push("Issue has new comments after prior feedback (+25 recovery)".to_string());
    }

    if updated_after_feedback && enriched.activity.maintainer_recent_response {
        reactivation_boost += 35;
        penalty *= 0.35;
        reasons.push("Maintainer activity appears after prior feedback (+35 recovery)".to_string());
    }

    FeedbackAssessment {
        penalty: penalty.round() as i32,
        reactivation_boost,
        visibility: RecommendationVisibility::Visible,
        reasons,
    }
}

fn shown_penalty(state: &RecommendationIssueState) -> f64 {
    8.0 * decay(state.last_shown_at.as_deref()) * state.shown_count.min(5) as f64
}

fn read_penalty(state: &RecommendationIssueState) -> f64 {
    35.0 * decay(state.last_read_at.as_deref()) * state.read_count.min(3) as f64
}

fn prepared_penalty(state: &RecommendationIssueState) -> f64 {
    80.0 * decay(state.last_prepared_at.as_deref()) * state.prepared_count.min(2) as f64
}

fn decay(timestamp: Option<&str>) -> f64 {
    let Some(timestamp) = timestamp else {
        return 0.0;
    };
    let Ok(timestamp) = DateTime::parse_from_rfc3339(timestamp) else {
        return 0.0;
    };
    let age_days = (Utc::now() - timestamp.with_timezone(&Utc)).num_days();
    match age_days {
        value if value <= 1 => 1.0,
        value if value <= 3 => 0.75,
        value if value <= 7 => 0.50,
        value if value <= 14 => 0.25,
        _ => 0.10,
    }
}

fn timestamp_after(left: Option<&str>, right: Option<&str>) -> bool {
    let (Some(left), Some(right)) = (left, right) else {
        return false;
    };
    let Ok(left) = DateTime::parse_from_rfc3339(left) else {
        return false;
    };
    let Ok(right) = DateTime::parse_from_rfc3339(right) else {
        return false;
    };
    left > right
}
