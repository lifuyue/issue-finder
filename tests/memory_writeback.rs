use std::path::Path;

use issue_finder::memory::{
    DispatchMemoryOutcome, MemoryActivationItem, MemoryActivationRun, MemoryEdgeRelation,
    MemoryGraphBuilder, MemoryIngestor, MemoryNodeState, MemoryNodeType, MemoryQueryKind,
    MemorySourceChannel, MemoryStore, MemoryWritebackAction, MemoryWritebackGuard, NewMemoryNode,
};
use issue_finder::paths::IssueFinderPaths;
use issue_finder::recommendation::IssueKey;
use serde_json::json;
use tempfile::tempdir;

const NOW: &str = "2026-06-18T00:00:00Z";
const LATER: &str = "2026-06-18T00:10:00Z";

#[test]
fn good_graph_recall_reinforces_state_edges_and_audit_rows() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    let seeded = seed_dispatch_graph(&store);
    store
        .insert_node_state(&MemoryNodeState {
            node_id: seeded.raw_node_id.clone(),
            salience: 0.5,
            strength: 1.0,
            resource: 1.0,
            recall_count: 0,
            reinforce_count: 0,
            fan_in: 1,
            fan_out: 1,
            last_recalled_at: None,
            last_reinforced_at: None,
            updated_at: NOW.to_string(),
        })
        .unwrap();
    insert_activation(
        &store,
        "activation-good",
        activation_item("activation-good", &seeded.raw_node_id, 1, 1.0, 0.8),
    );

    let report = MemoryWritebackGuard::apply(&store, "activation-good", LATER).unwrap();
    let state = store.get_node_state(&seeded.raw_node_id).unwrap().unwrap();
    let edge = store.get_edge(&seeded.edge_id).unwrap().unwrap();
    let writebacks = store.list_writebacks_for_run("activation-good").unwrap();

    assert_eq!(report.recalled, 1);
    assert_eq!(report.resource_decremented, 1);
    assert_eq!(report.reinforced, 1);
    assert_eq!(report.edge_reinforced, seeded.reinforceable_edge_count);
    assert_eq!(state.recall_count, 1);
    assert_eq!(state.reinforce_count, 1);
    assert_eq!(state.resource, 0.8);
    assert_eq!(state.strength, 1.25);
    assert_eq!(edge.strength, 1.15);
    assert_eq!(edge.last_activated_at.as_deref(), Some(LATER));
    assert!(writebacks.iter().any(|writeback| {
        writeback.action == MemoryWritebackAction::Recalled
            && writeback.before_json["recallCount"] == 0
            && writeback.after_json["recallCount"] == 1
    }));
    assert!(writebacks
        .iter()
        .any(|writeback| writeback.action == MemoryWritebackAction::EdgeReinforced));

    let second = MemoryWritebackGuard::apply(&store, "activation-good", LATER).unwrap();
    assert_eq!(second.recalled, 0);
    assert_eq!(second.reinforced, 0);
    assert_eq!(
        store
            .get_node_state(&seeded.raw_node_id)
            .unwrap()
            .unwrap()
            .recall_count,
        1
    );
}

#[test]
fn pure_ripple_noise_is_recalled_but_not_reinforced() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    let seeded = seed_dispatch_graph(&store);
    store
        .insert_node_state(&MemoryNodeState {
            node_id: seeded.raw_node_id.clone(),
            salience: 0.0,
            strength: 0.5,
            resource: 1.0,
            recall_count: 0,
            reinforce_count: 0,
            fan_in: 1,
            fan_out: 1,
            last_recalled_at: None,
            last_reinforced_at: None,
            updated_at: NOW.to_string(),
        })
        .unwrap();
    insert_activation(
        &store,
        "activation-ripple-noise",
        MemoryActivationItem {
            source_channel: MemorySourceChannel::NearRipple,
            direct_score: 0.0,
            ripple_score: 0.8,
            ..activation_item("activation-ripple-noise", &seeded.raw_node_id, 1, 0.0, 0.8)
        },
    );

    let report = MemoryWritebackGuard::apply(&store, "activation-ripple-noise", LATER).unwrap();
    let state = store.get_node_state(&seeded.raw_node_id).unwrap().unwrap();
    let edge = store.get_edge(&seeded.edge_id).unwrap().unwrap();

    assert_eq!(report.recalled, 1);
    assert_eq!(report.resource_decremented, 1);
    assert_eq!(report.reinforced, 0);
    assert_eq!(report.edge_reinforced, 0);
    assert_eq!(state.recall_count, 1);
    assert_eq!(state.reinforce_count, 0);
    assert_eq!(state.resource, 0.8);
    assert_eq!(edge.strength, 1.0);
}

#[test]
fn tombstoned_and_suppressed_nodes_do_not_write_back() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    let seeded = seed_dispatch_graph(&store);
    store
        .tombstone_raw_event(&seeded.raw_event_id, LATER)
        .unwrap();
    insert_activation(
        &store,
        "activation-tombstoned",
        activation_item("activation-tombstoned", &seeded.raw_node_id, 1, 1.0, 0.0),
    );

    let tombstoned_report =
        MemoryWritebackGuard::apply(&store, "activation-tombstoned", LATER).unwrap();
    assert_eq!(tombstoned_report.recalled, 0);
    assert_eq!(tombstoned_report.reinforced, 0);
    assert!(store
        .list_writebacks_for_run("activation-tombstoned")
        .unwrap()
        .is_empty());

    let suppressed_node_id = "suppressed-hint-node".to_string();
    store
        .insert_node(&NewMemoryNode {
            id: suppressed_node_id.clone(),
            node_type: MemoryNodeType::Hint,
            raw_event_id: None,
            entity_type: None,
            entity_value: None,
            normalized_value: None,
            text_ref: None,
            metadata_json: json!({"status": "suppressed"}),
            created_at: NOW.to_string(),
        })
        .unwrap();
    insert_activation(
        &store,
        "activation-suppressed",
        activation_item("activation-suppressed", &suppressed_node_id, 1, 1.0, 0.0),
    );
    let suppressed_report =
        MemoryWritebackGuard::apply(&store, "activation-suppressed", LATER).unwrap();

    assert_eq!(suppressed_report.recalled, 0);
    assert_eq!(suppressed_report.reinforced, 0);
    assert!(store
        .list_writebacks_for_run("activation-suppressed")
        .unwrap()
        .is_empty());
}

fn insert_activation(store: &MemoryStore, run_id: &str, item: MemoryActivationItem) {
    store
        .insert_activation_run(&MemoryActivationRun {
            id: run_id.to_string(),
            query_kind: MemoryQueryKind::ScoutRanking,
            query_ref: "owner/repo#42".to_string(),
            query_json: json!({"query": run_id}),
            created_at: NOW.to_string(),
        })
        .unwrap();
    store.insert_activation_item(&item).unwrap();
}

fn activation_item(
    run_id: &str,
    node_id: &str,
    rank: i64,
    direct_score: f64,
    ripple_score: f64,
) -> MemoryActivationItem {
    MemoryActivationItem {
        run_id: run_id.to_string(),
        node_id: node_id.to_string(),
        source_channel: MemorySourceChannel::Fts,
        direct_score,
        ripple_score,
        salience_score: 0.0,
        strength_score: 0.0,
        recency_score: 0.0,
        resource_penalty: 0.0,
        hub_penalty: 0.0,
        role_trust_penalty: 0.0,
        hop_penalty: 0.0,
        stale_penalty: 0.0,
        final_score: direct_score + ripple_score,
        rank,
        explanation: "test activation".to_string(),
    }
}

struct SeededGraph {
    raw_event_id: String,
    raw_node_id: String,
    edge_id: String,
    reinforceable_edge_count: usize,
}

fn seed_dispatch_graph(store: &MemoryStore) -> SeededGraph {
    let ingestor = MemoryIngestor::new(store);
    let ingested = ingestor
        .ingest_dispatch_outcome(&DispatchMemoryOutcome {
            id: "dispatch-writeback".to_string(),
            issue_key: IssueKey::new("owner/repo", 42),
            agent_id: "codex".to_string(),
            outcome_kind: Some("fix_ready".to_string()),
            task_type: "rust_cli_panic".to_string(),
            succeeded: true,
            failure_class: None,
            failure_reason: None,
            validation_outcome: Some("passed".to_string()),
            validation_paths: Vec::new(),
            artifact_refs: Vec::new(),
            occurred_at: NOW.to_string(),
            metadata: json!({}),
        })
        .unwrap();
    MemoryGraphBuilder::rebuild_coactivation_edges(store, NOW).unwrap();
    let raw_node_id = ingested.node_ids[0].clone();
    let edges = store.list_edges().unwrap();
    let reinforceable_edge_count = edges
        .iter()
        .filter(|edge| edge.from_node_id == raw_node_id || edge.to_node_id == raw_node_id)
        .count();
    let edge = edges
        .into_iter()
        .find(|edge| {
            edge.relation == MemoryEdgeRelation::CoActivated && edge.from_node_id == raw_node_id
        })
        .unwrap();

    SeededGraph {
        raw_event_id: ingested.raw_event_ids[0].clone(),
        raw_node_id,
        edge_id: edge.id,
        reinforceable_edge_count,
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
