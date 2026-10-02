//! Frozen, hand-labelled evidence. Offline tests consume recorded semantic answers;
//! they measure policy regressions, not a mock model's classification accuracy.
use std::collections::BTreeMap;

use issue_finder::decision::evidence::EvidenceSnapshot;
use issue_finder::decision::questions::SemanticAnswers;
use issue_finder::decision::{policy, request_for};
use issue_finder::github_enrichment::EnrichedIssue;
use issue_finder::recommendation::model::category_anchor;
use issue_finder::value_scoring::RecommendationCategory;
use serde_json::{json, Value};

#[path = "../examples/support/decision_fixtures.rs"]
mod fixtures;
use fixtures::{dataset, inputs, profile, source_issue, string};
const HISTORICAL_SAMPLES: &str = include_str!("fixtures/decision_eval/samples.json");
const BASELINE: &str = include_str!("fixtures/decision_eval/legacy_baseline.json");

fn answers(sample: &Value) -> SemanticAnswers {
    serde_json::from_value(sample["expected"]["answers"].clone()).unwrap()
}
fn result(sample: &Value, answers: Option<&SemanticAnswers>) -> Value {
    let (enriched, _) = inputs(sample);
    let assessment = policy::assess(&enriched, answers, &profile());
    let quality = policy::quality(&enriched, answers);
    let hidden = quality.visibility.is_some()
        || assessment.recommendation_category == RecommendationCategory::FilteredLowDepth;
    json!({"id":sample["id"], "hidden":hidden, "category":assessment.recommendation_category,
        "score":category_anchor(assessment.recommendation_category)+assessment.final_rank_score,
        "risk_tags":assessment.risk_tags, "missing_evidence":assessment.missing_evidence,
        "execution_score":assessment.execution_score, "penalty":quality.penalty})
}

#[test]
fn frozen_semantic_policy_recovers_good_candidates_without_leaking_rejects() {
    let fixture = dataset();
    let baseline: Value = serde_json::from_str(BASELINE).unwrap();
    assert_eq!(baseline["revision"], fixture["baseline_revision"]);
    let old: BTreeMap<_, _> = baseline["samples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|sample| (string(&sample["id"]), sample))
        .collect();
    let mut old_false_hidden = 0;
    let mut new_false_hidden = 0;
    let mut ranked = Vec::new();
    for sample in fixture["samples"].as_array().unwrap() {
        let judgement = answers(sample);
        let actual = result(sample, Some(&judgement));
        // Exercise the public downstream consumer, where old quality rules could
        // otherwise hide an issue after a correct decision model interpretation.
        let (mut enriched, evidence) = inputs(sample);
        let mut snapshot = issue_finder::decision::JudgmentSnapshot::pending("frozen replay");
        snapshot.status = issue_finder::decision::JudgmentStatus::Completed;
        snapshot.evaluated_at = "2026-10-01T00:00:00Z".into();
        snapshot.answers = Some(judgement.clone());
        snapshot.evidence = Some(evidence);
        enriched.decision = Some(snapshot);
        let value = issue_finder::value_scoring::assess_issue(&enriched, &profile());
        let ranked_item = issue_finder::value_scoring::RankedValueIssue {
            issue: source_issue(sample),
            score: value.final_rank_score,
            value_assessment: value,
            enriched_issue: enriched,
            explanation: Vec::new(),
            recommendation: Default::default(),
        };
        let feed = issue_finder::recommendation::feed_ranker::recommendation_assessment(
            &ranked_item,
            None,
        );
        assert_eq!(
            feed.visibility != issue_finder::recommendation::RecommendationVisibility::Visible,
            actual["hidden"] == true,
            "downstream consumer overrode semantic snapshot: {}",
            sample["id"]
        );
        assert_eq!(feed.quality_penalty, 0);
        let good =
            sample["expected"]["quality"] == "good" && sample["expected"]["behavior"] != "hidden";
        old_false_hidden += usize::from(
            good && old
                .get(&string(&sample["id"]))
                .is_some_and(|baseline| baseline["hidden"] == true),
        );
        new_false_hidden += usize::from(good && actual["hidden"] == true);
        assert_eq!(
            actual["hidden"] == true,
            sample["expected"]["behavior"] == "hidden",
            "{}: {}",
            sample["id"],
            sample["expected"]["reasons"]
        );
        if sample["expected"]["behavior"] == "visible_lower" {
            assert_ne!(
                actual["category"], "high_value_ready",
                "{} must retain uncertainty or competition",
                sample["id"]
            );
        }
        assert_eq!(
            actual["penalty"], 0,
            "{} must not charge legacy semantic counters again",
            sample["id"]
        );
        ranked.push((actual, sample));
    }
    assert!(
        old_false_hidden >= 8,
        "baseline must actually expose semantic false hiding"
    );
    assert_eq!(new_false_hidden, 0);
    ranked.sort_by_key(|(actual, _)| {
        (
            actual["hidden"] == true,
            std::cmp::Reverse(actual["score"].as_i64().unwrap()),
        )
    });
    let top = &ranked[..5];
    assert!(
        top.iter().all(|(actual, sample)| actual["hidden"] == false
            && sample["expected"]["quality"] == "good"
            && sample["expected"]["behavior"] == "visible"),
        "first five must be good, available candidates"
    );
    eprintln!("frozen policy eval: old false hidden={old_false_hidden}, new={new_false_hidden}; top-five appropriate=5/5");
}

#[test]
fn frozen_judgments_replay_identically_without_network_and_failures_are_neutral() {
    for sample in dataset()["samples"].as_array().unwrap() {
        let (enriched, evidence) = inputs(sample);
        let judgement = answers(sample);
        let saved = serde_json::to_vec(&(enriched, evidence, judgement)).unwrap();
        let (replayed, evidence, answers): (EnrichedIssue, EvidenceSnapshot, SemanticAnswers) =
            serde_json::from_slice(&saved).unwrap();
        let actual = policy::assess(&replayed, Some(&answers), &profile());
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(policy::assess(
                &inputs(sample).0,
                Some(&answers),
                &profile()
            ))
            .unwrap()
        );
        assert_eq!(
            request_for(&evidence, &profile()),
            request_for(&inputs(sample).1, &profile())
        );
        if sample["facts"]["open_pr_refs"] == 0 {
            let failed = result(sample, None);
            assert_eq!(
                failed["hidden"], false,
                "provider failure must not restore old text hiding: {}",
                sample["id"]
            );
            assert_eq!(failed["execution_score"], 50);
            assert!(!failed["missing_evidence"].as_array().unwrap().is_empty());
        }
    }
}

#[test]
fn fixture_provenance_and_labels_are_explicit_and_bounded() {
    let fixture = dataset();
    let mut real = 0;
    for sample in fixture["samples"].as_array().unwrap() {
        assert!(!sample["expected"]["reasons"].as_array().unwrap().is_empty());
        if sample["provenance"]["kind"] == "real_github_snapshot" {
            real += 1;
            assert!(sample["issue"]["url"]
                .as_str()
                .unwrap()
                .starts_with("https://github.com/"));
            assert_eq!(sample["evidence"]["comments_complete"], true);
            for comment in sample["comments"].as_array().unwrap() {
                assert!(comment["id"].as_u64().is_some());
                assert!(comment["url"].as_str().unwrap().contains("#issuecomment-"));
            }
        } else {
            assert_eq!(sample["provenance"]["kind"], "synthetic");
        }
        assert!(sample["issue"]["body"].as_str().unwrap().chars().count() <= 12_000);
    }
    assert_eq!(real, 5);
}

#[test]
fn v2_questions_and_answers_drop_task_form_without_rewriting_historical_labels() {
    let fixture = dataset();
    assert_eq!(fixture["question_set_version"], "scout-semantics-v2");
    let historical: Value = serde_json::from_str(HISTORICAL_SAMPLES).unwrap();
    assert_eq!(historical["version"], 1);
    let historical_empty = historical["samples"]
        .as_array()
        .unwrap()
        .iter()
        .find(|sample| sample["id"] == "synthetic_empty_template")
        .unwrap();
    assert_eq!(historical_empty["expected"]["behavior"], "hidden");
    for sample in fixture["samples"].as_array().unwrap() {
        let (_, evidence) = inputs(sample);
        let current = request_for(&evidence, &profile());
        let old = issue_finder::decision::request_for_version(
            "scout-semantics-v1",
            &evidence,
            &profile(),
        )
        .unwrap();
        assert_eq!(current.questions.len(), 7);
        assert_eq!(old.questions.len(), 8);
        assert_ne!(current.input_id, old.input_id);
        assert!(current
            .questions
            .iter()
            .all(|question| question.id != "task_shape"));
        assert!(sample["expected"]["answers"].get("task_shape").is_none());
        if sample["historical_v1_policy"]["hidden"] == true {
            let current_result = result(sample, Some(&answers(sample)));
            assert_eq!(current_result["hidden"], false, "{}", sample["id"]);
            assert_eq!(current_result["category"], "high_value_ready");
            assert!(!current_result["risk_tags"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tag| ["content_fill", "template_like", "low_trust_repo"]
                    .contains(&tag.as_str().unwrap())));
        }
    }
}
