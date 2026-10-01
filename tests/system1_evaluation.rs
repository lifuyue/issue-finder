//! Frozen, hand-labelled evidence. Offline tests consume recorded semantic answers;
//! they measure policy regressions, not a mock model's classification accuracy.
use std::collections::BTreeMap;
use std::time::Duration;

use issue_finder::competition::assess_competition;
use issue_finder::config::ProfileConfig;
use issue_finder::github::GitHubIssue;
use issue_finder::github_enrichment::{EnrichedComment, EnrichedIssue};
use issue_finder::recommendation::model::category_anchor;
use issue_finder::system1::contract::Provider;
use issue_finder::system1::evidence::{
    CommentEvidence, CommentsEvidence, EvidenceSnapshot, EvidenceText,
};
use issue_finder::system1::questions::SemanticAnswers;
use issue_finder::system1::{policy, request_for};
use issue_finder::value_scoring::RecommendationCategory;
use serde_json::{json, Value};

const SAMPLES: &str = include_str!("fixtures/system1_eval/samples_v2.json");
const HISTORICAL_SAMPLES: &str = include_str!("fixtures/system1_eval/samples.json");
const BASELINE: &str = include_str!("fixtures/system1_eval/legacy_baseline.json");

fn dataset() -> Value {
    serde_json::from_str(SAMPLES).unwrap()
}
fn profile() -> ProfileConfig {
    serde_json::from_value(dataset()["evaluation_context"]["profile"].clone()).unwrap()
}
fn string(value: &Value) -> String {
    value.as_str().unwrap().to_owned()
}
fn source_issue(sample: &Value) -> GitHubIssue {
    let source = &sample["issue"];
    GitHubIssue {
        id: source["number"].as_u64().unwrap(),
        number: source["number"].as_u64().unwrap(),
        title: string(&source["title"]),
        body: string(&source["body"]),
        labels: serde_json::from_value(source["labels"].clone()).unwrap(),
        url: string(&source["url"]),
        repo_full_name: string(&source["repo_full_name"]),
        repo_name: "semantic-regression".into(),
        repo_description: "Rust and TypeScript CLI developer tools".into(),
        repo_stars: 10_000,
        created_at: string(&source["created_at"]),
        updated_at: string(&source["updated_at"]),
    }
}
fn inputs(sample: &Value) -> (EnrichedIssue, EvidenceSnapshot) {
    let issue = source_issue(sample);
    let mut enriched = EnrichedIssue::from_issue(&issue);
    enriched.repository.forks = 1_500;
    enriched.repository.language = Some("Rust".into());
    enriched.repository.open_issues = Some(40);
    enriched.activity.recent_repo_activity = true;
    enriched.activity.recent_issue_activity = true;
    enriched.repository.pushed_at = Some("2026-09-30T00:00:00Z".into());
    enriched.source_fetched_at = "2026-10-01T00:00:00Z".into();
    enriched.comments = sample["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|comment| EnrichedComment {
            source_ref: string(&comment["url"]),
            author: Some(string(&comment["author"])),
            author_association: string(&comment["author_association"]),
            created_at: string(&comment["created_at"]),
            body_excerpt: string(&comment["body"]),
        })
        .collect();
    enriched.issue.comments_count = sample["evidence"]["comments_available"].as_u64().unwrap();
    // Keep stale legacy counters to prove the new consumer cannot double-charge them.
    enriched.competition = assess_competition(
        &[],
        &enriched
            .comments
            .iter()
            .map(|comment| comment.body_excerpt.clone())
            .collect::<Vec<_>>(),
        vec![],
    );
    enriched.competition.open_pr_refs = sample["facts"]["open_pr_refs"].as_u64().unwrap() as usize;
    enriched.competition.closed_pr_refs =
        sample["facts"]["closed_pr_refs"].as_u64().unwrap() as usize;
    let mut evidence = EvidenceSnapshot::from_issue(&issue, &enriched);
    evidence.source_fetched_at = enriched.source_fetched_at.clone();
    evidence.body.truncated = sample["evidence"]["body_truncated"].as_bool().unwrap();
    if evidence.body.truncated {
        evidence.body.original_chars += 1;
    }
    let comments = sample["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|comment| CommentEvidence {
            id: comment["id"].as_u64(),
            url: Some(string(&comment["url"])),
            author: Some(string(&comment["author"])),
            author_association: string(&comment["author_association"]),
            created_at: Some(string(&comment["created_at"])),
            updated_at: None,
            body: EvidenceText::bounded(comment["body"].as_str().unwrap(), 4_000),
        })
        .collect();
    evidence.comments = if enriched.comments.is_empty()
        && !sample["evidence"]["comments_complete"].as_bool().unwrap()
    {
        CommentsEvidence::unavailable(Some(enriched.issue.comments_count))
    } else {
        CommentsEvidence::from_comments(Some(enriched.issue.comments_count), comments)
    };
    (enriched, evidence)
}
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
        // otherwise hide an issue after a correct System 1 interpretation.
        let (mut enriched, evidence) = inputs(sample);
        let mut snapshot = issue_finder::system1::JudgmentSnapshot::pending("frozen replay");
        snapshot.status = issue_finder::system1::JudgmentStatus::Completed;
        snapshot.evaluated_at = "2026-10-01T00:00:00Z".into();
        snapshot.answers = Some(judgement.clone());
        snapshot.evidence = Some(evidence);
        enriched.system1 = Some(snapshot);
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

#[tokio::test]
#[ignore = "explicit live Luna acceptance; never run in routine offline tests"]
async fn live_luna_classifies_frozen_github_material_and_reports_quality() {
    assert_eq!(
        std::env::var("ISSUE_FINDER_SYSTEM1_LIVE_EVAL").as_deref(),
        Ok("1"),
        "set ISSUE_FINDER_SYSTEM1_LIVE_EVAL=1 explicitly"
    );
    let provider =
        issue_finder::system1::codex::CodexProvider::new(None, Duration::from_secs(120)).unwrap();
    let fixture = dataset();
    let selected = std::env::var("ISSUE_FINDER_SYSTEM1_EVAL_IDS").ok();
    let mut results = Vec::new();
    let mut core_correct = 0;
    let mut core_total = 0;
    let mut false_hidden = 0;
    for sample in fixture["samples"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|sample| {
            selected
                .as_ref()
                .map(|ids| ids.split(',').any(|id| sample["id"] == id))
                .unwrap_or(sample["provenance"]["kind"] == "real_github_snapshot")
        })
    {
        let (_, evidence) = inputs(sample);
        let request = request_for(&evidence, &profile());
        let response = provider.decide(&request).await.unwrap();
        response.validate(&request).unwrap();
        let answers = SemanticAnswers::from_response(&request, &response).unwrap();
        let actual_answers = serde_json::to_value(&answers).unwrap();
        let mismatches: Vec<_> = sample["expected"]["answers"].as_object().unwrap().iter()
            .filter(|(key, value)| actual_answers[*key] != **value)
            .map(|(key, value)| json!({"question":key,"expected":value,"actual":actual_answers[key]})).collect();
        for key in ["task_type", "description_quality", "contribution_signal"] {
            core_total += 1;
            core_correct += usize::from(actual_answers[key] == sample["expected"]["answers"][key]);
        }
        let actual = result(sample, Some(&answers));
        false_hidden += usize::from(
            sample["expected"]["quality"] == "good"
                && sample["expected"]["behavior"] != "hidden"
                && actual["hidden"] == true,
        );
        eprintln!("live {}: {mismatches:?}", sample["id"]);
        results.push(json!({"id":sample["id"],"answers":actual_answers,"mismatches":mismatches,"policy":actual,"response":response}));
    }
    provider.close().await;
    let report = json!({"question_set_version":fixture["question_set_version"],"fixture_labels_frozen_at":fixture["labelled_at"],"provider":provider.fingerprint(),"core_correct":core_correct,"core_total":core_total,"false_hidden_good":false_hidden,"samples":results});
    if let Ok(path) = std::env::var("ISSUE_FINDER_SYSTEM1_EVAL_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    assert!(core_total > 0, "selection matched no samples");
    assert_eq!(
        core_correct, core_total,
        "live model core semantic regression: {report}"
    );
    assert_eq!(
        false_hidden, 0,
        "live model hid a hand-labelled good candidate"
    );
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
        let old =
            issue_finder::system1::request_for_version("scout-semantics-v1", &evidence, &profile())
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

/// Small transport acceptance only; this does not score model quality or compare prompts.
#[tokio::test]
#[ignore = "explicit five-request live concurrency acceptance"]
async fn live_luna_five_issues_complete_in_four_slots() {
    use futures::{stream, StreamExt};
    use std::time::Instant;

    assert_eq!(
        std::env::var("ISSUE_FINDER_SYSTEM1_LIVE_CONCURRENCY").as_deref(),
        Ok("1"),
        "set ISSUE_FINDER_SYSTEM1_LIVE_CONCURRENCY=1 explicitly"
    );
    let provider =
        issue_finder::system1::codex::CodexProvider::new(None, Duration::from_secs(120)).unwrap();
    let fixture = dataset();
    let requests: Vec<_> = fixture["samples"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|sample| sample["provenance"]["kind"] == "real_github_snapshot")
        .take(5)
        .map(|sample| request_for(&inputs(sample).1, &profile()))
        .collect();
    assert_eq!(requests.len(), 5);
    assert!(requests.iter().all(|request| request.questions.len() == 7));
    let started = Instant::now();
    let results: Vec<_> = stream::iter(requests.iter().map(|request| {
        let provider = &provider;
        async move {
            let admitted_ms = started.elapsed().as_millis();
            let response = provider.decide(request).await;
            let completed_ms = started.elapsed().as_millis();
            let outcome = response.and_then(|response| {
                response.validate(request)?;
                SemanticAnswers::from_response(request, &response).map_err(|error| {
                    issue_finder::system1::contract::ProviderError::new(
                        "invalid_semantics",
                        error.to_string(),
                    )
                })?;
                Ok(response)
            });
            json!({"candidate":request.candidate_id,"queue_wait_ms":admitted_ms,
                "completed_ms":completed_ms,"execution_ms":completed_ms-admitted_ms,
                "success":outcome.is_ok(),"outcome":outcome})
        }
    }))
    .buffer_unordered(4)
    .collect()
    .await;
    provider.close().await;
    let report = json!({"purpose":"five-request transport acceptance, not a quality or speedup benchmark",
        "question_set_version":issue_finder::system1::questions::QUESTION_SET_VERSION,
        "concurrency":4,"provider":provider.fingerprint(),"wall_ms":started.elapsed().as_millis(),
        "requests":results});
    if let Ok(path) = std::env::var("ISSUE_FINDER_SYSTEM1_CONCURRENCY_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    assert!(
        results.iter().all(|result| result["success"] == true),
        "{report}"
    );
}
