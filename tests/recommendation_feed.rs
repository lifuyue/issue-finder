use chrono::{Duration, Utc};
use issue_finder::github::GitHubIssue;
use issue_finder::github_enrichment::{EnrichedComment, EnrichedIssue};
use issue_finder::recommendation::engine::select_display_candidates;
use issue_finder::recommendation::events::{
    IssueKey, RecommendationEvent, RecommendationEventSource, RecommendationEventType,
};
use issue_finder::recommendation::feed_ranker::{apply_recommendation_assessments, sort_by_feed};
use issue_finder::recommendation::state::{derive_state_map, RecommendationIssueState};
use issue_finder::recommendation::{RecommendationAssessment, RecommendationVisibility};
use issue_finder::value_scoring::{
    GateBand, GateStatus, GateVerdict, RankedValueIssue, RecommendationCategory, RiskTag,
    ScoreBand, ValueAssessment, ValueGates,
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
fn first_shown_feedback_does_not_keep_rank_one() {
    let mut ranked = vec![
        ranked_issue(
            "owner/shown-once",
            RecommendationCategory::HighValueReady,
            85,
            1,
        ),
        ranked_issue(
            "owner/unseen",
            RecommendationCategory::HighValueReady,
            82,
            1,
        ),
    ];
    let now = Utc::now().to_rfc3339();
    let states = HashMap::from([(
        IssueKey::new("owner/shown-once", 1),
        RecommendationIssueState {
            issue_key: IssueKey::new("owner/shown-once", 1),
            shown_count: 1,
            last_shown_at: Some(now.clone()),
            last_feedback_at: Some(now),
            ..RecommendationIssueState::default()
        },
    )]);

    apply_recommendation_assessments(&mut ranked, &states);
    sort_by_feed(&mut ranked);

    assert_eq!(ranked[0].issue.repo_full_name, "owner/unseen");
    assert!(ranked[1].recommendation.feedback_penalty >= 50);
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
fn read_feedback_leaves_first_screen() {
    let mut ranked = vec![ranked_issue(
        "owner/read",
        RecommendationCategory::HighValueReady,
        100,
        1,
    )];
    for index in 0..11 {
        ranked.push(ranked_issue(
            &format!("owner/fresh-{index}"),
            RecommendationCategory::HighValueReady,
            72,
            1,
        ));
    }
    let now = Utc::now().to_rfc3339();
    let states = HashMap::from([(
        IssueKey::new("owner/read", 1),
        RecommendationIssueState {
            issue_key: IssueKey::new("owner/read", 1),
            read_count: 1,
            last_read_at: Some(now.clone()),
            last_feedback_at: Some(now),
            ..RecommendationIssueState::default()
        },
    )]);

    apply_recommendation_assessments(&mut ranked, &states);
    sort_by_feed(&mut ranked);

    let read_rank = ranked
        .iter()
        .position(|item| item.issue.repo_full_name == "owner/read")
        .expect("read issue should remain present")
        + 1;
    assert!(
        read_rank > 10,
        "read issue should leave the first screen, got rank {read_rank}"
    );
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

#[test]
fn submitted_pr_is_hidden_by_quality_policy() {
    let mut ranked = ranked_issue(
        "owner/pr-submitted",
        RecommendationCategory::NeedsTriage,
        75,
        0,
    );
    ranked.enriched_issue.competition.open_pr_refs = 1;

    apply_recommendation_assessments(std::slice::from_mut(&mut ranked), &HashMap::new());

    assert_eq!(
        ranked.recommendation.visibility,
        RecommendationVisibility::HiddenQuality
    );
    assert!(ranked.recommendation.quality_penalty >= 200);
    assert!(!ranked.recommendation.displayable(true));
}

#[test]
fn claimed_issue_is_hidden_by_quality_policy() {
    let mut ranked = ranked_issue("owner/claimed", RecommendationCategory::NeedsTriage, 75, 0);
    ranked.enriched_issue.competition.claim_comments = 1;
    add_comment(
        &mut ranked,
        "I would like to take a look. Please assign me.",
    );
    add_comment(
        &mut ranked,
        "Hi, I'm interested in contributing to this issue and happy to implement it.",
    );
    add_comment(
        &mut ranked,
        "I will look into it and was able to replicate.",
    );

    apply_recommendation_assessments(std::slice::from_mut(&mut ranked), &HashMap::new());

    assert_eq!(
        ranked.recommendation.visibility,
        RecommendationVisibility::HiddenQuality
    );
    assert!(!ranked.recommendation.displayable(false));
}

#[test]
fn fixed_or_no_longer_reproduced_issue_is_hidden_by_quality_policy() {
    let mut ranked = ranked_issue("owner/fixed", RecommendationCategory::NeedsTriage, 75, 0);
    add_comment(
        &mut ranked,
        "Confirmed fixed by #3457. I can no longer reproduce this on current main.",
    );

    apply_recommendation_assessments(std::slice::from_mut(&mut ranked), &HashMap::new());

    assert_eq!(
        ranked.recommendation.visibility,
        RecommendationVisibility::HiddenQuality
    );
}

#[test]
fn possibly_resolved_status_verification_issue_is_hidden_by_quality_policy() {
    let mut ranked = ranked_issue(
        "owner/status-verification",
        RecommendationCategory::HighValueNeedsScoping,
        88,
        0,
    );
    add_comment(
        &mut ranked,
        "The fix shipped in #5036 (glibc); verify on musl before treating this as available work.",
    );

    apply_recommendation_assessments(std::slice::from_mut(&mut ranked), &HashMap::new());

    assert_eq!(
        ranked.recommendation.visibility,
        RecommendationVisibility::HiddenQuality
    );
    assert!(ranked.recommendation.quality_penalty >= 200);
    assert!(ranked
        .recommendation
        .reasons
        .iter()
        .any(|reason| reason.contains("resolution-verification")));
}

#[test]
fn trivial_docs_polish_is_hidden_by_quality_policy() {
    let mut ranked = ranked_issue("owner/docs", RecommendationCategory::NeedsTriage, 82, 0);
    set_issue_text(
        &mut ranked,
        "Improve English wording in README and manual",
        "Please review the English wording, grammar, and natural wording in README.md and the user manual.",
        &["documentation", "good first issue"],
    );

    apply_recommendation_assessments(std::slice::from_mut(&mut ranked), &HashMap::new());

    assert_eq!(
        ranked.recommendation.visibility,
        RecommendationVisibility::HiddenQuality
    );
    assert!(ranked.recommendation.quality_penalty >= 160);
}

#[test]
fn stale_doc_comment_only_issue_is_hidden_by_quality_policy() {
    let mut ranked = ranked_issue(
        "owner/doc-comments",
        RecommendationCategory::NeedsTriage,
        82,
        0,
    );
    set_issue_text(
        &mut ranked,
        "Stale native eval callback wording in EvalEngine doc comments",
        "Two doc comments still describe the removed native callback model. Documentation only, no behavior change.",
        &["docs", "good first issue"],
    );

    apply_recommendation_assessments(std::slice::from_mut(&mut ranked), &HashMap::new());

    assert_eq!(
        ranked.recommendation.visibility,
        RecommendationVisibility::HiddenQuality
    );
}

#[test]
fn broad_audit_issue_is_hidden_by_quality_policy() {
    let mut ranked = ranked_issue("owner/audit", RecommendationCategory::NeedsTriage, 88, 0);
    ranked.value_assessment.risk_tags = vec![RiskTag::HighTriageLoad, RiskTag::WeakValidationPath];
    set_issue_text(
        &mut ranked,
        "per-crate README completeness audit (110 gaps)",
        "Audit 127 publishable crates, triage 110 gaps, then run phase 1 triage and phase 2 remediation. This is not a mechanical fix and requires judgment.",
        &["documentation"],
    );

    apply_recommendation_assessments(std::slice::from_mut(&mut ranked), &HashMap::new());

    assert_eq!(
        ranked.recommendation.visibility,
        RecommendationVisibility::HiddenQuality
    );
    assert!(ranked.recommendation.quality_penalty >= 180);
}

#[test]
fn profile_mismatch_caps_freshness_without_hiding_issue() {
    let mut ranked = ranked_issue(
        "owner/profile-mismatch",
        RecommendationCategory::HighValueReady,
        88,
        0,
    );
    ranked.value_assessment.risk_tags = vec![RiskTag::ProfileMismatch];
    ranked.value_assessment.gates.profile_fit.status = GateStatus::HardFail;

    apply_recommendation_assessments(std::slice::from_mut(&mut ranked), &HashMap::new());

    assert_eq!(
        ranked.recommendation.visibility,
        RecommendationVisibility::Visible
    );
    assert_eq!(ranked.recommendation.freshness_boost, 8);
    assert!(ranked.recommendation.quality_penalty >= 180);
}

#[test]
fn profile_mismatch_triage_issue_is_hidden_by_quality_policy() {
    let mut ranked = ranked_issue(
        "owner/profile-mismatch-triage",
        RecommendationCategory::NeedsTriage,
        88,
        0,
    );
    ranked.value_assessment.risk_tags = vec![RiskTag::ProfileMismatch];

    apply_recommendation_assessments(std::slice::from_mut(&mut ranked), &HashMap::new());

    assert_eq!(
        ranked.recommendation.visibility,
        RecommendationVisibility::HiddenQuality
    );
}

#[test]
fn display_selection_limits_primary_results_per_repo_without_backfill() {
    let ranked = vec![
        ranked_issue("owner/busy", RecommendationCategory::HighValueReady, 100, 1),
        ranked_issue("owner/busy", RecommendationCategory::HighValueReady, 99, 1),
        ranked_issue("owner/busy", RecommendationCategory::HighValueReady, 98, 1),
        ranked_issue(
            "owner/alt-one",
            RecommendationCategory::HighValueReady,
            80,
            1,
        ),
        ranked_issue(
            "owner/alt-two",
            RecommendationCategory::HighValueReady,
            70,
            1,
        ),
    ];

    let selected = select_display_candidates(ranked, 5, false);
    let repos = selected
        .iter()
        .map(|item| item.issue.repo_full_name.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        repos,
        vec!["owner/busy", "owner/busy", "owner/alt-one", "owner/alt-two"]
    );
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
        gates: passing_gates(),
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

fn set_issue_text(ranked: &mut RankedValueIssue, title: &str, body: &str, labels: &[&str]) {
    ranked.issue.title = title.to_string();
    ranked.issue.body = body.to_string();
    ranked.issue.labels = labels.iter().map(|label| (*label).to_string()).collect();
    ranked.enriched_issue.issue.title = title.to_string();
    ranked.enriched_issue.issue.body = body.to_string();
    ranked.enriched_issue.issue.labels = ranked.issue.labels.clone();
}

fn add_comment(ranked: &mut RankedValueIssue, body: &str) {
    ranked.enriched_issue.comments.push(EnrichedComment {
        source_ref: "issue:comments.0".to_string(),
        author: Some("contributor".to_string()),
        author_association: "NONE".to_string(),
        created_at: Utc::now().to_rfc3339(),
        body_excerpt: body.to_string(),
    });
}

fn passing_gates() -> ValueGates {
    ValueGates {
        low_depth: pass_gate(),
        repo_influence: pass_gate(),
        competition: pass_gate(),
        profile_fit: pass_gate(),
    }
}

fn pass_gate() -> GateVerdict {
    GateVerdict {
        status: GateStatus::Pass,
        band: GateBand::Strong,
        reasons: vec!["test gate pass".to_string()],
        evidence_refs: Vec::new(),
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
