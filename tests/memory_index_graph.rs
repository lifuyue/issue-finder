use std::path::Path;

use issue_finder::memory::{
    DispatchMemoryOutcome, EmbeddingIndexStatus, ManualMemoryEvent, MemoryEdgeRelation,
    MemoryGraphBuilder, MemoryIndexBuilder, MemoryIndexSearch, MemoryIndexType, MemoryIngestor,
    MemoryRawEventType, MemoryRole, MemoryStore, MemorySubjectType, MemoryTrustLevel,
};
use issue_finder::paths::IssueFinderPaths;
use issue_finder::recommendation::IssueKey;
use serde_json::json;
use tempfile::tempdir;

const NOW: &str = "2026-06-18T00:00:00Z";
const LATER: &str = "2026-06-18T00:10:00Z";

#[test]
fn index_rebuild_supports_fts_rare_tokens_and_entity_recall_without_embeddings() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    let ingestor = MemoryIngestor::new(&store);

    let manual = ingestor
        .ingest_manual_event(&ManualMemoryEvent {
            id: "manual-error-1".to_string(),
            event_type: MemoryRawEventType::Reject,
            role: MemoryRole::User,
            trust_level: MemoryTrustLevel::UserExplicit,
            subject_type: MemorySubjectType::Issue,
            subject_ref: "owner/repo#123".to_string(),
            payload_json: json!({
                "labels": ["good first issue", "parser-core"],
                "body": "panic E0425 appears in src/parser.rs",
            }),
            occurred_at: NOW.to_string(),
        })
        .unwrap();
    let dispatch = ingestor
        .ingest_dispatch_outcome(&DispatchMemoryOutcome {
            id: "dispatch-run-1".to_string(),
            issue_key: IssueKey::new("owner/repo", 123),
            agent_id: "codex".to_string(),
            outcome_kind: Some("failed".to_string()),
            task_type: "rust_cli_panic".to_string(),
            succeeded: false,
            failure_class: Some("validation_failed".to_string()),
            failure_reason: Some("unclear_validation".to_string()),
            validation_outcome: Some("failed".to_string()),
            validation_paths: vec!["cargo test -p cli".to_string()],
            artifact_refs: Vec::new(),
            occurred_at: NOW.to_string(),
            metadata: json!({}),
        })
        .unwrap();

    let report = MemoryIndexBuilder::rebuild(&store, LATER).unwrap();

    assert_eq!(report.embedding_status, EmbeddingIndexStatus::Disabled);
    assert!(report.nodes_indexed >= 5);
    assert!(report.indexes_written > report.nodes_indexed);

    let fts = MemoryIndexSearch::search_fts(&store, "owner/repo E0425").unwrap();
    assert!(fts.iter().any(|item| {
        item.node_id == manual.node_ids[0]
            && item.index_type == MemoryIndexType::Fts
            && item.matched_payload == "owner/repo"
    }));
    assert!(fts.iter().any(|item| {
        item.node_id == manual.node_ids[0]
            && item.index_type == MemoryIndexType::Fts
            && item.matched_payload == "e0425"
    }));

    let rare = MemoryIndexSearch::search_rare_tokens(&store, "parser-core").unwrap();
    assert!(rare.iter().any(|item| {
        item.node_id == manual.node_ids[0]
            && item.index_type == MemoryIndexType::RareToken
            && item.matched_payload == "parser-core"
    }));

    let agent = MemoryIndexSearch::search_entity(&store, "agent", "Codex").unwrap();
    assert!(agent
        .iter()
        .any(|item| dispatch.node_ids.contains(&item.node_id)));
    let failure =
        MemoryIndexSearch::search_entity(&store, "failure_reason", "unclear_validation").unwrap();
    assert!(failure
        .iter()
        .any(|item| dispatch.node_ids.contains(&item.node_id)));
}

#[test]
fn graph_rebuild_creates_coactivation_edges_and_excludes_tombstoned_events() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    let ingestor = MemoryIngestor::new(&store);
    let outcome = DispatchMemoryOutcome {
        id: "dispatch-run-1".to_string(),
        issue_key: IssueKey::new("owner/repo", 42),
        agent_id: "codex".to_string(),
        outcome_kind: Some("failed".to_string()),
        task_type: "rust_cli_panic".to_string(),
        succeeded: false,
        failure_class: Some("validation_failed".to_string()),
        failure_reason: Some("unclear_validation".to_string()),
        validation_outcome: Some("failed".to_string()),
        validation_paths: vec!["cargo test -p cli".to_string()],
        artifact_refs: Vec::new(),
        occurred_at: NOW.to_string(),
        metadata: json!({}),
    };
    let result = ingestor.ingest_dispatch_outcome(&outcome).unwrap();

    let report = MemoryGraphBuilder::rebuild_coactivation_edges(&store, LATER).unwrap();
    assert_eq!(report.raw_events_seen, 1);
    assert_eq!(report.raw_events_linked, 1);
    assert_eq!(report.edges_written, 7);

    let edges = store.list_edges().unwrap();
    assert_eq!(edges.len(), 7);
    assert!(edges
        .iter()
        .all(|edge| edge.relation == MemoryEdgeRelation::CoActivated));
    assert!(edges.iter().all(|edge| {
        edge.evidence_event_ids_json
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some(result.raw_event_ids[0].as_str()))
    }));

    store
        .tombstone_raw_event(&result.raw_event_ids[0], LATER)
        .unwrap();
    MemoryIndexBuilder::rebuild(&store, LATER).unwrap();
    let graph_report = MemoryGraphBuilder::rebuild_coactivation_edges(&store, LATER).unwrap();

    assert_eq!(graph_report.raw_events_seen, 1);
    assert_eq!(graph_report.raw_events_linked, 0);
    assert_eq!(graph_report.edges_written, 0);
    assert!(store.list_edges().unwrap().is_empty());
    assert!(MemoryIndexSearch::search_entity(&store, "agent", "codex")
        .unwrap()
        .is_empty());
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
