use std::path::Path;

use issue_finder::github::GitHubIssue;
use issue_finder::github_enrichment::EnrichedIssue;
use issue_finder::handoff::HandoffMemoryContext;
use issue_finder::memory::{
    apply_ranking_hints_to_ranked, handoff_memory_context_for_issue, MemoryDreamRun,
    MemoryDreamScope, MemoryDreamStatus, MemoryDreamTrigger, MemoryDreamType, MemoryHintScopeType,
    MemoryHintStatus, MemoryHintType, MemoryModelStatus, MemoryStore, NewMemoryDream,
    NewMemoryHint,
};
use issue_finder::paths::IssueFinderPaths;
use issue_finder::recommendation::RecommendationAssessment;
use issue_finder::value_scoring::{RankedValueIssue, ValueAssessment};
use serde_json::json;
use tempfile::tempdir;

const NOW: &str = "2026-06-18T00:00:00Z";

#[test]
fn approved_ranking_hints_append_explanation_without_changing_score() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    seed_memory_hints(&paths);
    let mut ranked = vec![ranked_issue("owner/repo", 42)];
    let before_score = ranked[0].recommendation.final_feed_score;

    apply_ranking_hints_to_ranked(&paths, &mut ranked).unwrap();

    assert_eq!(ranked[0].recommendation.final_feed_score, before_score);
    assert!(ranked[0]
        .explanation
        .iter()
        .any(|reason| reason.contains("approved-ranking")));
    assert!(!ranked[0]
        .explanation
        .iter()
        .any(|reason| reason.contains("candidate-ranking")));
    assert!(ranked[0]
        .recommendation
        .reasons
        .iter()
        .any(|reason| reason.contains("Memory hint refs")));
}

#[test]
fn approved_ranking_hints_replace_stale_cached_memory_refs() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    seed_memory_hints(&paths);
    let mut ranked = vec![ranked_issue("owner/repo", 42)];
    ranked[0]
        .explanation
        .push("Memory hint refs: stale-ranking".to_string());
    ranked[0]
        .recommendation
        .reasons
        .push("Memory hint refs: stale-ranking".to_string());

    apply_ranking_hints_to_ranked(&paths, &mut ranked).unwrap();

    let explanation_refs = ranked[0]
        .explanation
        .iter()
        .map(String::as_str)
        .filter(|reason| reason.starts_with("Memory hint refs:"))
        .collect::<Vec<_>>();
    let recommendation_refs = ranked[0]
        .recommendation
        .reasons
        .iter()
        .map(String::as_str)
        .filter(|reason| reason.starts_with("Memory hint refs:"))
        .collect::<Vec<_>>();

    assert_eq!(explanation_refs, vec!["Memory hint refs: approved-ranking"]);
    assert_eq!(
        recommendation_refs,
        vec!["Memory hint refs: approved-ranking"]
    );
}

#[test]
fn handoff_memory_context_contains_approved_ranking_and_dispatch_hints_only() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    seed_memory_hints(&paths);

    let context = handoff_memory_context_for_issue(&paths, &issue("owner/repo", 42)).unwrap();
    let ids = hint_ids(&context);

    assert_eq!(
        ids,
        vec![
            "approved-agent-dispatch",
            "approved-dispatch",
            "approved-ranking"
        ]
    );
    assert_eq!(
        context.evidence_refs,
        vec![
            "memory_hint:approved-agent-dispatch".to_string(),
            "memory_hint:approved-dispatch".to_string(),
            "memory_hint:approved-ranking".to_string()
        ]
    );
    assert_eq!(context.agent_selection_notes.len(), 2);
    assert!(context
        .agent_selection_notes
        .iter()
        .any(|note| note.contains("Dispatch")));
    assert!(context
        .agent_selection_notes
        .iter()
        .any(|note| note.contains("Agent dispatch")));
}

fn seed_memory_hints(paths: &IssueFinderPaths) {
    let store = MemoryStore::open(paths).unwrap();
    store
        .insert_dream_run(&MemoryDreamRun {
            id: "consumption-dream-run".to_string(),
            trigger: MemoryDreamTrigger::Manual,
            scope: MemoryDreamScope::Global,
            input_activation_run_ids_json: json!([]),
            model_status: MemoryModelStatus::Disabled,
            created_at: NOW.to_string(),
        })
        .unwrap();
    store
        .insert_dream(&NewMemoryDream {
            id: "consumption-dream".to_string(),
            dream_run_id: "consumption-dream-run".to_string(),
            dream_type: MemoryDreamType::DiscoveryPolicy,
            summary: "Consumption test".to_string(),
            evidence_node_ids_json: json!([]),
            evidence_event_ids_json: json!([]),
            evidence_hint_ids_json: json!([]),
            status: MemoryDreamStatus::Candidate,
            confidence: 0.5,
            version: 1,
            created_at: NOW.to_string(),
            reviewed_at: None,
        })
        .unwrap();
    seed_hint(
        &store,
        "approved-ranking",
        MemoryHintType::Ranking,
        MemoryHintStatus::Approved,
        "Ranking approved hint",
    );
    seed_hint(
        &store,
        "candidate-ranking",
        MemoryHintType::Ranking,
        MemoryHintStatus::Candidate,
        "Ranking candidate hint",
    );
    seed_hint(
        &store,
        "approved-dispatch",
        MemoryHintType::Dispatch,
        MemoryHintStatus::Approved,
        "Dispatch approved hint",
    );
    seed_scoped_hint(
        &store,
        "approved-agent-dispatch",
        MemoryHintType::Dispatch,
        MemoryHintStatus::Approved,
        MemoryHintScopeType::Agent,
        "codex",
        "Agent dispatch approved hint",
    );
    seed_scoped_hint(
        &store,
        "candidate-agent-dispatch",
        MemoryHintType::Dispatch,
        MemoryHintStatus::Candidate,
        MemoryHintScopeType::Agent,
        "codex",
        "Agent dispatch candidate hint",
    );
}

fn seed_hint(
    store: &MemoryStore,
    id: &str,
    hint_type: MemoryHintType,
    status: MemoryHintStatus,
    summary: &str,
) {
    seed_scoped_hint(
        store,
        id,
        hint_type,
        status,
        MemoryHintScopeType::Repo,
        "owner/repo",
        summary,
    );
}

fn seed_scoped_hint(
    store: &MemoryStore,
    id: &str,
    hint_type: MemoryHintType,
    status: MemoryHintStatus,
    scope_type: MemoryHintScopeType,
    scope_ref: &str,
    summary: &str,
) {
    store
        .insert_hint(&NewMemoryHint {
            id: id.to_string(),
            dream_id: "consumption-dream".to_string(),
            hint_type,
            scope_type,
            scope_ref: scope_ref.to_string(),
            summary: summary.to_string(),
            policy_json: json!({"kind": "consumption_test"}),
            weight: 1.0,
            status,
            created_at: NOW.to_string(),
            approved_at: status.is_active_decision_status().then(|| NOW.to_string()),
            expires_at: None,
        })
        .unwrap();
}

fn ranked_issue(repo_full_name: &str, number: u64) -> RankedValueIssue {
    let issue = issue(repo_full_name, number);
    let value_assessment = ValueAssessment::default();
    RankedValueIssue {
        issue: issue.clone(),
        score: 0,
        value_assessment: value_assessment.clone(),
        enriched_issue: EnrichedIssue::from_issue(&issue),
        explanation: Vec::new(),
        recommendation: RecommendationAssessment::from_value_assessment(&value_assessment),
    }
}

fn issue(repo_full_name: &str, number: u64) -> GitHubIssue {
    GitHubIssue {
        id: number,
        number,
        title: "Fix Rust CLI panic".to_string(),
        body: "Reproduce with cargo test.".to_string(),
        labels: vec!["good first issue".to_string()],
        url: format!("https://github.com/{repo_full_name}/issues/{number}"),
        repo_full_name: repo_full_name.to_string(),
        repo_name: repo_full_name.split('/').nth(1).unwrap().to_string(),
        repo_description: "Rust CLI".to_string(),
        repo_stars: 100,
        created_at: NOW.to_string(),
        updated_at: NOW.to_string(),
    }
}

fn hint_ids(context: &HandoffMemoryContext) -> Vec<&str> {
    let mut ids = context
        .approved_hints
        .iter()
        .map(|hint| hint.id.as_str())
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

fn test_paths(root: &Path) -> IssueFinderPaths {
    IssueFinderPaths {
        home: root.to_path_buf(),
        config: root.join("config.toml"),
        cache_dir: root.join("cache"),
        workspaces_dir: root.join("workspaces"),
        inbox_dir: root.join("inbox"),
        reports_dir: root.join("reports"),
    }
}
