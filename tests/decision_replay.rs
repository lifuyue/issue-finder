use std::collections::HashMap;

use issue_finder::config::Config;
use issue_finder::decision::contract::*;
use issue_finder::decision::evidence::EvidenceSnapshot;
use issue_finder::decision::questions::{
    SemanticAnswers, HISTORICAL_QUESTION_SET_VERSION, QUESTION_SET_VERSION,
};
use issue_finder::decision::replay::ScoutReplay;
use issue_finder::decision::{request_for_version, JudgmentSnapshot, JudgmentStatus};
use issue_finder::github::GitHubIssue;
use issue_finder::github_enrichment::EnrichedIssue;
use issue_finder::paths::IssueFinderPaths;
use issue_finder::recommendation::events::{
    IssueKey, RecommendationEvent, RecommendationEventSource, RecommendationEventType,
};
use issue_finder::recommendation::feed_ranker::{apply_recommendation_assessments, sort_by_feed};
use issue_finder::recommendation::state::{
    derive_state_map_with_policy, FeedbackPolicy, RecommendationIssueState,
};
use issue_finder::recommendation::RecommendationVisibility;
use issue_finder::value_scoring::{assess_issue, RankedValueIssue};
use serde_json::json;

fn candidate(number: u64, stars: u64, support: bool) -> RankedValueIssue {
    candidate_for_version(number, stars, support, QUESTION_SET_VERSION)
}

fn candidate_for_version(
    number: u64,
    stars: u64,
    support: bool,
    version: &str,
) -> RankedValueIssue {
    let issue = GitHubIssue {
        id: number,
        number,
        title: format!("Correct CLI documentation {number}"),
        body: "Change the --output example from adress to address in docs/cli.md.".into(),
        labels: vec!["good first issue".into()],
        url: format!("https://github.com/o/r/issues/{number}"),
        repo_full_name: "o/r".into(),
        repo_name: "r".into(),
        repo_description: "Rust CLI tools".into(),
        repo_stars: stars,
        created_at: "2025-01-01T00:00:00Z".into(),
        updated_at: "2025-01-02T00:00:00Z".into(),
    };
    let mut enriched = EnrichedIssue::from_issue(&issue);
    enriched.repository.stars = stars;
    enriched.repository.forks = 120;
    enriched.repository.subscribers = Some(42);
    enriched.repository.created_at = Some("2020-01-01T00:00:00Z".into());
    enriched.repository.default_branch = Some("main".into());
    enriched.repository.language = Some("Rust".into());
    enriched.activity.recent_repo_activity = true;
    enriched.activity.recent_issue_activity = false;
    enriched.activity.maintainer_recent_response = number == 2;
    let mut evidence = EvidenceSnapshot::from_issue(&issue, &enriched);
    evidence.github_status.issue_state = Some("open".into());
    evidence.github_status.assignees = Some(Vec::new());
    let profile = Config::default().profile;
    let request = request_for_version(version, &evidence, &profile).unwrap();
    let response = DecisionResponse {
        candidate_id: request.candidate_id.clone(),
        input_id: request.input_id.clone(),
        status: ResponseStatus::Complete,
        answers: request
            .questions
            .iter()
            .map(|question| {
                let choice = match question.id.as_str() {
                    "task_type" if support => "support_question",
                    "task_type" => "concrete_change",
                    "description_quality" => "clear",
                    "contribution_signal" => "interest_only",
                    "scope" => "bounded",
                    "preference_match" => "matches",
                    "task_shape" => "documentation",
                    "verification_clues" => "present",
                    "maintainer_signal" => "not_observed",
                    _ => panic!("unknown question"),
                };
                QuestionResponse {
                    question_id: question.id.clone(),
                    status: AnswerStatus::Answered,
                    answer: Some(Answer::Choice(choice.into())),
                    probabilities: None,
                }
            })
            .collect(),
        metadata: ProviderMetadata {
            provider: "offline".into(),
            model: "frozen-fixture".into(),
            reasoning_effort: "none".into(),
            ..ProviderMetadata::default()
        },
    };
    let answers = SemanticAnswers::from_response(&request, &response).unwrap();
    enriched.decision = Some(JudgmentSnapshot {
        status: JudgmentStatus::Completed,
        question_set_version: Some(version.into()),
        input_id: Some(request.input_id),
        provider_fingerprint: Some("offline-frozen-v1".into()),
        evaluated_at: "2025-01-02T01:00:00Z".into(),
        answers: Some(answers),
        response: Some(response),
        evidence: Some(evidence),
        error: None,
        snapshot_path: None,
        cache_hit: false,
    });
    let value_assessment = assess_issue(&enriched, &profile);
    RankedValueIssue {
        issue,
        score: value_assessment.final_rank_score,
        value_assessment,
        enriched_issue: enriched,
        explanation: Vec::new(),
        recommendation: Default::default(),
    }
}

fn feedback_event(kind: RecommendationEventType) -> RecommendationEvent {
    RecommendationEvent {
        event_id: format!("event-{kind:?}"),
        timestamp: "2025-01-02T00:30:00Z".into(),
        issue_key: IssueKey::new("o/r", 1),
        event_type: kind,
        source: RecommendationEventSource::ToolScout,
        issue_updated_at: Some("2025-01-02T00:00:00Z".into()),
        issue_comments_count: Some(0),
        metadata: json!({}),
    }
}

fn facts_and_ranked() -> (
    Vec<RankedValueIssue>,
    HashMap<IssueKey, RecommendationIssueState>,
) {
    let states = derive_state_map_with_policy(
        &[
            feedback_event(RecommendationEventType::Shown),
            feedback_event(RecommendationEventType::Read),
            feedback_event(RecommendationEventType::Done),
        ],
        FeedbackPolicy::CodexExposure,
    );
    let mut ranked = vec![
        candidate(1, 20_000, false),
        candidate(2, 2_000, false),
        candidate(3, 5_000, true),
    ];
    apply_recommendation_assessments(&mut ranked, &states);
    sort_by_feed(&mut ranked);
    (ranked, states)
}

#[test]
fn complete_snapshot_replays_scores_order_visibility_and_feedback_without_external_state() {
    let (ranked, states) = facts_and_ranked();
    assert_eq!(
        ranked[0].issue.number, 2,
        "feedback must move the numerically stronger issue below a fresh candidate"
    );
    let shown = ranked.iter().find(|item| item.issue.number == 1).unwrap();
    assert!(shown.recommendation.feedback_penalty > 200);
    assert_eq!(
        shown.recommendation.visibility,
        RecommendationVisibility::Visible,
        "ignored Done lifecycle event must not enter Codex replay"
    );
    assert_eq!(
        shown.recommendation.freshness_boost, 53,
        "age calculation must use 2025 judgment time, not today's time"
    );
    assert_eq!(
        ranked[2].recommendation.visibility,
        RecommendationVisibility::HiddenFiltered
    );
    let replay = ScoutReplay::capture(
        &Config::default().profile,
        &ranked,
        &states,
        FeedbackPolicy::CodexExposure,
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let paths = IssueFinderPaths {
        home: directory.path().into(),
        config: directory.path().join("config.toml"),
        cache_dir: directory.path().join("cache"),
        workspaces_dir: directory.path().join("workspaces"),
        inbox_dir: directory.path().join("inbox"),
        reports_dir: directory.path().join("reports"),
    };
    let path = replay.save(&paths).unwrap();
    assert_eq!(
        path.parent().unwrap(),
        paths.cache_dir.join("system1/replay")
    );
    // Stored enriched issue keys from before the rename remain readable.
    let mut legacy = serde_json::to_value(&replay).unwrap();
    for item in legacy["ranked"].as_array_mut().unwrap() {
        let enriched = item["enriched_issue"].as_object_mut().unwrap();
        let judgment = enriched.remove("decision").unwrap();
        assert!(!enriched.contains_key("system1"));
        enriched.insert("system1".into(), judgment);
    }
    let legacy: ScoutReplay = serde_json::from_value(legacy).unwrap();
    assert_eq!(legacy, replay);
    assert_eq!(legacy.replay().unwrap(), ranked);
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(!saved.contains("api_key") && !saved.contains("github_token"));
    let loaded = ScoutReplay::load(&path).unwrap();
    assert_eq!(loaded, replay);
    std::fs::remove_dir_all(&paths.cache_dir).unwrap();
    let recomputed = loaded.replay().unwrap();
    assert_eq!(recomputed,ranked,"replay must reconstruct intrinsic score, freshness, feedback, visibility and sorting from frozen inputs");
    assert_eq!(recomputed[0].enriched_issue.repository.stars, 2_000);
    assert_eq!(
        recomputed[0].enriched_issue.repository.subscribers,
        Some(42)
    );
    assert_eq!(
        recomputed[0]
            .enriched_issue
            .decision
            .as_ref()
            .unwrap()
            .evaluated_at,
        "2025-01-02T01:00:00Z"
    );

    let mut changed_facts = loaded.clone();
    let candidate = changed_facts
        .ranked
        .iter_mut()
        .find(|item| item.issue.number == 2)
        .unwrap();
    candidate.enriched_issue.repository.stars = 200;
    candidate.enriched_issue.repository.forks = 10;
    let changed = changed_facts.replay().unwrap();
    assert_ne!(
        changed
            .iter()
            .find(|item| item.issue.number == 2)
            .unwrap()
            .value_assessment
            .final_rank_score,
        ranked[0].value_assessment.final_rank_score,
        "saved numeric facts must actively participate in recomputation"
    );
}

#[test]
fn replay_rejects_unfiltered_feedback_mismatched_answers_and_unfrozen_time() {
    let (ranked, states) = facts_and_ranked();
    assert!(ScoutReplay::capture(
        &Config::default().profile,
        &ranked,
        &states,
        FeedbackPolicy::LegacyLifecycle
    )
    .is_err());
    let replay = ScoutReplay::capture(
        &Config::default().profile,
        &ranked,
        &states,
        FeedbackPolicy::CodexExposure,
    )
    .unwrap();
    let mut legacy_feedback = replay.clone();
    legacy_feedback.states[0].done = true;
    assert!(legacy_feedback.replay().is_err());
    let mut mismatched = replay.clone();
    mismatched.ranked[0]
        .enriched_issue
        .decision
        .as_mut()
        .unwrap()
        .answers
        .as_mut()
        .unwrap()
        .description_quality = None;
    assert!(mismatched.replay().is_err());
    let mut no_clock = replay;
    no_clock.ranked[0]
        .enriched_issue
        .decision
        .as_mut()
        .unwrap()
        .evaluated_at
        .clear();
    assert!(no_clock.replay().is_err());
}

#[test]
fn legacy_eight_question_capture_is_explicitly_historical_and_preserves_recorded_outcome() {
    let mut old = candidate_for_version(7, 2_000, false, HISTORICAL_QUESTION_SET_VERSION);
    // Preserve an outcome generated by the former form-based policy.
    let snapshot = old.enriched_issue.decision.as_mut().unwrap();
    snapshot.answers.as_mut().unwrap().task_shape =
        Some(issue_finder::decision::questions::TaskShape::GeneratedTemplate);
    snapshot
        .response
        .as_mut()
        .unwrap()
        .answers
        .iter_mut()
        .find(|answer| answer.question_id == "task_shape")
        .unwrap()
        .answer = Some(Answer::Choice("generated_template".into()));
    old.value_assessment.category =
        issue_finder::value_scoring::RecommendationCategory::FilteredLowDepth;
    old.value_assessment.recommendation_category =
        issue_finder::value_scoring::RecommendationCategory::FilteredLowDepth;
    old.value_assessment.gates.low_depth.status = issue_finder::value_scoring::GateStatus::HardFail;
    let mut ranked = vec![old];
    apply_recommendation_assessments(&mut ranked, &HashMap::new());
    assert_eq!(
        ranked[0].recommendation.visibility,
        RecommendationVisibility::HiddenFiltered
    );
    let replay = ScoutReplay {
        schema_version: 1,
        question_set_version: HISTORICAL_QUESTION_SET_VERSION.into(),
        captured_at: "2025-01-02T01:00:00Z".into(),
        profile: Config::default().profile,
        feedback_policy: FeedbackPolicy::CodexExposure,
        ranked,
        states: Vec::new(),
    };
    // Actual schema 1 JSON omitted both version fields.
    let mut payload = serde_json::to_value(&replay).unwrap();
    payload
        .as_object_mut()
        .unwrap()
        .remove("question_set_version");
    let enriched = payload["ranked"][0]["enriched_issue"]
        .as_object_mut()
        .unwrap();
    let judgment = enriched.remove("decision").unwrap();
    enriched.insert("system1".into(), judgment);
    payload["ranked"][0]["enriched_issue"]["system1"]
        .as_object_mut()
        .unwrap()
        .remove("question_set_version");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("historical.json");
    std::fs::write(&path, serde_json::to_vec(&payload).unwrap()).unwrap();
    let historical = ScoutReplay::load(&path).unwrap();
    assert!(historical.is_historical());
    assert_eq!(historical.replay().unwrap(), historical.ranked);
    assert_eq!(
        historical.replay().unwrap()[0].recommendation.visibility,
        RecommendationVisibility::HiddenFiltered
    );
    assert_eq!(
        historical.ranked[0]
            .enriched_issue
            .decision
            .as_ref()
            .unwrap()
            .response
            .as_ref()
            .unwrap()
            .answers
            .len(),
        8
    );
    assert!(
        ScoutReplay::capture(
            &historical.profile,
            &historical.ranked,
            &HashMap::new(),
            FeedbackPolicy::CodexExposure
        )
        .is_err(),
        "old judgments must not silently enter a current replay"
    );
    let mut mislabelled = historical.clone();
    mislabelled.question_set_version = QUESTION_SET_VERSION.into();
    assert!(mislabelled.replay().is_err());
    let mut malformed = historical;
    malformed.ranked[0]
        .enriched_issue
        .decision
        .as_mut()
        .unwrap()
        .response
        .as_mut()
        .unwrap()
        .answers
        .pop();
    assert!(
        malformed.replay().is_err(),
        "historical replay still validates its original provider contract"
    );
}
