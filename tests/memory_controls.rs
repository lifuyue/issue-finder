use std::path::Path;

use issue_finder::memory::{
    DispatchMemoryOutcome, MemoryActivationItem, MemoryActivationRequest, MemoryActivationRun,
    MemoryControlPlane, MemoryDecisionHintRequest, MemoryDreamRequest, MemoryDreamRun,
    MemoryDreamScope, MemoryDreamStatus, MemoryDreamTrigger, MemoryDreamType, MemoryHintScope,
    MemoryHintScopeType, MemoryHintStatus, MemoryHintType, MemoryIndexBuilder, MemoryIngestor,
    MemoryModelStatus, MemoryQueryKind, MemoryRuntimeMode, MemorySourceChannel, MemoryStore,
    NewMemoryDream, NewMemoryHint,
};
use issue_finder::paths::IssueFinderPaths;
use issue_finder::recommendation::IssueKey;
use serde_json::json;
use tempfile::tempdir;

const NOW: &str = "2026-06-18T00:00:00Z";

#[test]
fn decision_query_only_returns_approved_pinned_and_deprioritized_hints() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    seed_dream(&store);
    seed_hint(&store, "candidate", MemoryHintStatus::Candidate, 5.0, None);
    seed_hint(&store, "approved", MemoryHintStatus::Approved, 0.8, None);
    seed_hint(&store, "pinned", MemoryHintStatus::Pinned, 0.1, None);
    seed_hint(
        &store,
        "deprioritized",
        MemoryHintStatus::Deprioritized,
        1.0,
        None,
    );
    seed_hint(&store, "rejected", MemoryHintStatus::Rejected, 5.0, None);
    seed_hint(&store, "stale", MemoryHintStatus::Stale, 5.0, None);
    seed_hint(
        &store,
        "tombstoned",
        MemoryHintStatus::Tombstoned,
        5.0,
        None,
    );

    let hints = MemoryControlPlane::decision_eligible_hints(
        &store,
        &MemoryDecisionHintRequest {
            hint_type: Some(MemoryHintType::Ranking),
            scope: Some(repo_scope("owner/repo")),
            now: Some(NOW.to_string()),
            ..MemoryDecisionHintRequest::default()
        },
    )
    .unwrap();
    let ids = hints
        .iter()
        .map(|hint| hint.hint.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["pinned", "approved", "deprioritized"]);
    assert_eq!(
        hints
            .iter()
            .find(|hint| hint.hint.id == "deprioritized")
            .unwrap()
            .effective_weight,
        0.5
    );

    let pinned_survives_limit = MemoryControlPlane::decision_eligible_hints(
        &store,
        &MemoryDecisionHintRequest {
            hint_type: Some(MemoryHintType::Ranking),
            scope: Some(repo_scope("owner/repo")),
            now: Some(NOW.to_string()),
            limit: 1,
            ..MemoryDecisionHintRequest::default()
        },
    )
    .unwrap();
    assert_eq!(pinned_survives_limit[0].hint.id, "pinned");
}

#[test]
fn suppressed_scope_hides_decision_hints_for_that_scope() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    seed_dream(&store);
    seed_hint(
        &store,
        "repo-approved",
        MemoryHintStatus::Approved,
        1.0,
        None,
    );
    seed_hint(
        &store,
        "repo-suppressed",
        MemoryHintStatus::Suppressed,
        0.0,
        None,
    );
    seed_global_hint(&store, "global-approved", MemoryHintStatus::Approved);

    let suppressed = MemoryControlPlane::decision_eligible_hints(
        &store,
        &MemoryDecisionHintRequest {
            scope: Some(repo_scope("owner/repo")),
            ..MemoryDecisionHintRequest::default()
        },
    )
    .unwrap();
    assert!(suppressed.is_empty());

    let other_repo = MemoryControlPlane::decision_eligible_hints(
        &store,
        &MemoryDecisionHintRequest {
            scope: Some(repo_scope("owner/other")),
            ..MemoryDecisionHintRequest::default()
        },
    )
    .unwrap();
    assert_eq!(other_repo.len(), 1);
    assert_eq!(other_repo[0].hint.id, "global-approved");
}

#[test]
fn memory_off_returns_no_hints_and_no_write_mode_recalls_without_persistence_or_writeback() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    seed_dream(&store);
    seed_hint(&store, "approved", MemoryHintStatus::Approved, 1.0, None);
    let seeded = seed_dispatch(&store);
    MemoryIndexBuilder::rebuild(&store, NOW).unwrap();

    let no_hints = MemoryControlPlane::decision_eligible_hints(
        &store,
        &MemoryDecisionHintRequest {
            mode: MemoryRuntimeMode::MemoryOff,
            scope: Some(repo_scope("owner/repo")),
            ..MemoryDecisionHintRequest::default()
        },
    )
    .unwrap();
    assert!(no_hints.is_empty());

    let memory_off_recall = MemoryControlPlane::activate(
        &store,
        &activation_request("memory-off-run"),
        MemoryRuntimeMode::MemoryOff,
    )
    .unwrap();
    assert!(memory_off_recall.items.is_empty());

    let no_write_recall = MemoryControlPlane::activate(
        &store,
        &activation_request("no-write-run"),
        MemoryRuntimeMode::NoWrite,
    )
    .unwrap();
    assert!(!no_write_recall.items.is_empty());
    assert!(store.get_activation_run("no-write-run").unwrap().is_none());

    insert_activation(&store, "persisted-run", &seeded.raw_node_id);
    let report = MemoryControlPlane::apply_writeback(
        &store,
        "persisted-run",
        NOW,
        MemoryRuntimeMode::NoWrite,
    )
    .unwrap();
    assert_eq!(report.recalled, 0);
    assert!(store
        .list_writebacks_for_run("persisted-run")
        .unwrap()
        .is_empty());

    let skipped_dream = MemoryControlPlane::dream(
        &store,
        &MemoryDreamRequest {
            run_id: "no-write-dream".to_string(),
            trigger: MemoryDreamTrigger::Manual,
            scope: MemoryDreamScope::Global,
            scope_ref: None,
            input_activation_run_ids: Vec::new(),
            created_at: NOW.to_string(),
        },
        None,
        MemoryRuntimeMode::NoWrite,
    )
    .unwrap();
    assert!(skipped_dream.is_none());
    assert!(store.get_dream_run("no-write-dream").unwrap().is_none());
}

fn seed_dream(store: &MemoryStore) {
    if store.get_dream_run("controls-dream-run").unwrap().is_some() {
        return;
    }
    store
        .insert_dream_run(&MemoryDreamRun {
            id: "controls-dream-run".to_string(),
            trigger: MemoryDreamTrigger::Manual,
            scope: MemoryDreamScope::Global,
            input_activation_run_ids_json: json!([]),
            model_status: MemoryModelStatus::Disabled,
            created_at: NOW.to_string(),
        })
        .unwrap();
    store
        .insert_dream(&NewMemoryDream {
            id: "controls-dream".to_string(),
            dream_run_id: "controls-dream-run".to_string(),
            dream_type: MemoryDreamType::DiscoveryPolicy,
            summary: "controls seed".to_string(),
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
}

fn seed_hint(
    store: &MemoryStore,
    id: &str,
    status: MemoryHintStatus,
    weight: f64,
    expires_at: Option<&str>,
) {
    store
        .insert_hint(&NewMemoryHint {
            id: id.to_string(),
            dream_id: "controls-dream".to_string(),
            hint_type: MemoryHintType::Ranking,
            scope_type: MemoryHintScopeType::Repo,
            scope_ref: "owner/repo".to_string(),
            summary: format!("{id} hint"),
            policy_json: json!({"kind": "ranking_test"}),
            weight,
            status,
            created_at: NOW.to_string(),
            approved_at: status.is_active_decision_status().then(|| NOW.to_string()),
            expires_at: expires_at.map(ToString::to_string),
        })
        .unwrap();
}

fn seed_global_hint(store: &MemoryStore, id: &str, status: MemoryHintStatus) {
    store
        .insert_hint(&NewMemoryHint {
            id: id.to_string(),
            dream_id: "controls-dream".to_string(),
            hint_type: MemoryHintType::Ranking,
            scope_type: MemoryHintScopeType::Global,
            scope_ref: "global".to_string(),
            summary: format!("{id} hint"),
            policy_json: json!({"kind": "ranking_test"}),
            weight: 1.0,
            status,
            created_at: NOW.to_string(),
            approved_at: status.is_active_decision_status().then(|| NOW.to_string()),
            expires_at: None,
        })
        .unwrap();
}

struct SeededDispatch {
    raw_node_id: String,
}

fn seed_dispatch(store: &MemoryStore) -> SeededDispatch {
    let ingested = MemoryIngestor::new(store)
        .ingest_dispatch_outcome(&DispatchMemoryOutcome {
            id: "controls-dispatch".to_string(),
            issue_key: IssueKey::new("owner/repo", 42),
            agent_id: "codex".to_string(),
            task_type: "rust_cli_panic".to_string(),
            succeeded: true,
            failure_reason: None,
            validation_paths: vec!["cargo test".to_string()],
            artifact_refs: Vec::new(),
            occurred_at: NOW.to_string(),
            metadata: json!({}),
        })
        .unwrap();
    SeededDispatch {
        raw_node_id: ingested.node_ids[0].clone(),
    }
}

fn activation_request(run_id: &str) -> MemoryActivationRequest {
    MemoryActivationRequest {
        run_id: run_id.to_string(),
        query_kind: MemoryQueryKind::DispatchPlanning,
        query_ref: "owner/repo#42".to_string(),
        query_text: "rust_cli_panic codex".to_string(),
        entities: Vec::new(),
        created_at: NOW.to_string(),
        limit: 5,
        persist_trace: true,
    }
}

fn insert_activation(store: &MemoryStore, run_id: &str, node_id: &str) {
    store
        .insert_activation_run(&MemoryActivationRun {
            id: run_id.to_string(),
            query_kind: MemoryQueryKind::DispatchPlanning,
            query_ref: "owner/repo#42".to_string(),
            query_json: json!({"query": "rust cli panic"}),
            created_at: NOW.to_string(),
        })
        .unwrap();
    store
        .insert_activation_item(&MemoryActivationItem {
            run_id: run_id.to_string(),
            node_id: node_id.to_string(),
            source_channel: MemorySourceChannel::Fts,
            direct_score: 1.0,
            ripple_score: 0.0,
            salience_score: 0.0,
            strength_score: 0.0,
            recency_score: 0.0,
            resource_penalty: 0.0,
            hub_penalty: 0.0,
            role_trust_penalty: 0.0,
            hop_penalty: 0.0,
            stale_penalty: 0.0,
            final_score: 1.0,
            rank: 1,
            explanation: "test activation".to_string(),
        })
        .unwrap();
}

fn repo_scope(scope_ref: &str) -> MemoryHintScope {
    MemoryHintScope {
        scope_type: MemoryHintScopeType::Repo,
        scope_ref: scope_ref.to_string(),
    }
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
