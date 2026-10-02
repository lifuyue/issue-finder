//! Shared frozen evidence construction for offline regression and manual benchmarks.
use issue_finder::competition::assess_competition;
use issue_finder::config::ProfileConfig;
use issue_finder::decision::evidence::{
    CommentEvidence, CommentsEvidence, EvidenceSnapshot, EvidenceText,
};
use issue_finder::github::GitHubIssue;
use issue_finder::github_enrichment::{EnrichedComment, EnrichedIssue};
use serde_json::Value;

const SAMPLES: &str = include_str!("../../tests/fixtures/decision_eval/samples_v2.json");

pub fn dataset() -> Value {
    serde_json::from_str(SAMPLES).unwrap()
}
pub fn profile() -> ProfileConfig {
    serde_json::from_value(dataset()["evaluation_context"]["profile"].clone()).unwrap()
}
pub fn string(value: &Value) -> String {
    value.as_str().unwrap().to_owned()
}
pub fn source_issue(sample: &Value) -> GitHubIssue {
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
pub fn inputs(sample: &Value) -> (EnrichedIssue, EvidenceSnapshot) {
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
