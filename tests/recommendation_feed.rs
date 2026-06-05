use chrono::{Duration, Utc};
use issue_finder::github::GitHubIssue;
use issue_finder::github_enrichment::EnrichedIssue;
use issue_finder::recommendation::events::{
    IssueKey, RecommendationEvent, RecommendationEventSource, RecommendationEventType,
};
use issue_finder::recommendation::feed_ranker::{apply_recommendation_assessments, sort_by_feed};
use issue_finder::recommendation::state::{derive_state_map, RecommendationIssueState};
use issue_finder::recommendation::{RecommendationAssessment, RecommendationVisibility};
use issue_finder::value_scoring::{
    RankedValueIssue, RecommendationCategory, ScoreBand, ValueAssessment,
};
use serde_json::json;
use std::collections::HashMap;

#[test]
fn repeated_shown_feedback_lowers_next_feed_rank() {
    let mut ranked = vec![
        ranked_issue("owner/seen", RecommendationCategory::HighValueReady, 70, 10),
        ranked_issue(
            "owner/fresh",
            RecommendationCategory::HighValueReady,
            70,
            10,
        ),
    ];
    let mut states = HashMap::new();
    states.insert(
        IssueKey::new("owner/seen", 1),
        RecommendationIssueState {
            issue_key: IssueKey::new("owner/seen", 1),
            shown_count: 3,
            last_shown_at: Some(Utc::now().to_rfc3339()),
            last_feedback_at: Some(Utc::now().to_rfc3339()),
            ..RecommendationIssueState::default()
        },
    );

    apply_recommendation_assessments(&mut ranked, &states);
    sort_by_feed(&mut ranked);

    assert_eq!(ranked[0].issue.repo_full_name, "owner/fresh");
    assert!(ranked[1].recommendation.feedback_penalty > 0);
}

#[test]
fn read_feedback_penalizes_more_than_shown_feedback() {
    let mut shown = ranked_issue(
        "owner/shown",
        RecommendationCategory::HighValueReady,
        70,
        10,
    );
    let mut read = ranked_issue("owner/read", RecommendationCategory::HighValueReady, 70, 10);
    let now = Utc::now().to_rfc3339();
    let mut states = HashMap::new();
    states.insert(
        IssueKey::new("owner/shown", 1),
        RecommendationIssueState {
            issue_key: IssueKey::new("owner/shown", 1),
            shown_count: 1,
            last_shown_at: Some(now.clone()),
            last_feedback_at: Some(now.clone()),
            ..RecommendationIssueState::default()
        },
    );
    states.insert(
        IssueKey::new("owner/read", 1),
        RecommendationIssueState {
            issue_key: IssueKey::new("owner/read", 1),
            read_count: 1,
            last_read_at: Some(now.clone()),
            last_feedback_at: Some(now),
            ..RecommendationIssueState::default()
        },
    );

    apply_recommendation_assessments(std::slice::from_mut(&mut shown), &states);
    apply_recommendation_assessments(std::slice::from_mut(&mut read), &states);

    assert!(read.recommendation.feedback_penalty > shown.recommendation.feedback_penalty);
}

#[test]
fn dismissed_done_and_restored_visibility_follow_event_order() {
    let now = Utc::now();
    let key = IssueKey::new("owner/restored", 1);
    let events = vec![
        event(
            &key,
            RecommendationEventType::Dismissed,
            now - Duration::hours(2),
        ),
        event(
            &key,
            RecommendationEventType::Restored,
            now - Duration::hours(1),
        ),
    ];
    let states = derive_state_map(&events);
    let mut ranked = ranked_issue(
        "owner/restored",
        RecommendationCategory::HighValueReady,
        70,
        10,
    );

    apply_recommendation_assessments(std::slice::from_mut(&mut ranked), &states);

    assert_eq!(
        ranked.recommendation.visibility,
        RecommendationVisibility::Visible
    );

    let done_states = derive_state_map(&[event(
        &IssueKey::new("owner/done", 1),
        RecommendationEventType::Done,
        now,
    )]);
    let mut done = ranked_issue("owner/done", RecommendationCategory::HighValueReady, 70, 10);
    apply_recommendation_assessments(std::slice::from_mut(&mut done), &done_states);
    assert_eq!(
        done.recommendation.visibility,
        RecommendationVisibility::HiddenDone
    );
}

#[test]
fn freshness_can_cross_adjacent_high_value_categories_only() {
    let mut ranked = vec![
        ranked_issue(
            "owner/ready-old",
            RecommendationCategory::HighValueReady,
            70,
            60,
        ),
        ranked_issue(
            "owner/scoping-new",
            RecommendationCategory::HighValueNeedsScoping,
            70,
            0,
        ),
        ranked_issue(
            "owner/filtered-new",
            RecommendationCategory::FilteredLowDepth,
            100,
            0,
        ),
    ];

    apply_recommendation_assessments(&mut ranked, &HashMap::new());
    sort_by_feed(&mut ranked);

    assert_eq!(ranked[0].issue.repo_full_name, "owner/scoping-new");
    assert_eq!(ranked[1].issue.repo_full_name, "owner/ready-old");
    assert!(ranked[2].recommendation.final_feed_score < ranked[1].recommendation.final_feed_score);
    assert_eq!(
        ranked[2].recommendation.visibility,
        RecommendationVisibility::HiddenFiltered
    );
}

#[test]
fn reactivation_recovers_part_of_prior_feedback_penalty() {
    let mut ranked = ranked_issue(
        "owner/reactivated",
        RecommendationCategory::HighValueReady,
        70,
        0,
    );
    ranked.enriched_issue.issue.comments_count = 2;
    ranked.enriched_issue.activity.maintainer_recent_response = true;
    let last_feedback = (Utc::now() - Duration::days(2)).to_rfc3339();
    let states = HashMap::from([(
        IssueKey::new("owner/reactivated", 1),
        RecommendationIssueState {
            issue_key: IssueKey::new("owner/reactivated", 1),
            read_count: 2,
            last_read_at: Some(last_feedback.clone()),
            last_feedback_at: Some(last_feedback),
            last_seen_comments_count: Some(1),
            ..RecommendationIssueState::default()
        },
    )]);

    apply_recommendation_assessments(std::slice::from_mut(&mut ranked), &states);

    assert!(ranked.recommendation.reactivation_boost >= 25);
    assert!(ranked.recommendation.feedback_penalty < 70);
}

fn ranked_issue(
    repo_full_name: &str,
    category: RecommendationCategory,
    base_score: i32,
    age_days: i64,
) -> RankedValueIssue {
    let updated_at = (Utc::now() - Duration::days(age_days)).to_rfc3339();
    let issue = GitHubIssue {
        id: 1,
        number: 1,
        title: format!("Issue in {repo_full_name}"),
        body: "Expected behavior in src/lib.rs".to_string(),
        labels: vec!["good first issue".to_string()],
        url: format!("https://github.com/{repo_full_name}/issues/1"),
        repo_full_name: repo_full_name.to_string(),
        repo_name: repo_full_name.split('/').nth(1).unwrap().to_string(),
        repo_description: "Rust CLI".to_string(),
        repo_stars: 1_000,
        created_at: updated_at.clone(),
        updated_at,
    };
    let mut enriched_issue = EnrichedIssue::from_issue(&issue);
    enriched_issue.activity.recent_repo_activity = age_days <= 7;
    enriched_issue.activity.recent_issue_activity = age_days <= 7;
    let value_assessment = ValueAssessment {
        final_rank_score: base_score,
        category,
        recommendation_category: category,
        attention_score: base_score,
        execution_score: 70,
        profile_fit_score: 70,
        attention_band: ScoreBand::High,
        execution_band: ScoreBand::High,
        explanation: vec!["test value assessment".to_string()],
        ..ValueAssessment::default()
    };
    RankedValueIssue {
        issue,
        score: base_score,
        value_assessment: value_assessment.clone(),
        enriched_issue,
        explanation: value_assessment.explanation.clone(),
        recommendation: RecommendationAssessment::from_value_assessment(&value_assessment),
    }
}

fn event(
    issue_key: &IssueKey,
    event_type: RecommendationEventType,
    timestamp: chrono::DateTime<Utc>,
) -> RecommendationEvent {
    RecommendationEvent {
        event_id: format!("event-{}-{event_type:?}", timestamp.timestamp()),
        timestamp: timestamp.to_rfc3339(),
        issue_key: issue_key.clone(),
        event_type,
        source: RecommendationEventSource::FeedbackCommand,
        issue_updated_at: None,
        issue_comments_count: None,
        metadata: json!({}),
    }
}
