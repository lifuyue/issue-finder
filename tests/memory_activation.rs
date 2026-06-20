use std::path::Path;

use issue_finder::memory::{
    DispatchMemoryOutcome, ManualMemoryEvent, MemoryActivationEngine, MemoryActivationEntity,
    MemoryActivationRequest, MemoryGraphBuilder, MemoryIndexBuilder, MemoryIngestor,
    MemoryNodeState, MemoryNodeType, MemoryQueryKind, MemoryRawEventType, MemoryRole,
    MemorySourceChannel, MemoryStore, MemorySubjectType, MemoryTrustLevel,
};
use issue_finder::paths::IssueFinderPaths;
use issue_finder::recommendation::IssueKey;
use serde_json::json;
use tempfile::tempdir;

const NOW: &str = "2026-06-18T00:00:00Z";

#[test]
fn activation_persists_trace_for_direct_and_rare_token_matches() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    let ingestor = MemoryIngestor::new(&store);
    let ingested = ingestor
        .ingest_manual_event(&ManualMemoryEvent {
            id: "manual-panic".to_string(),
            event_type: MemoryRawEventType::Reject,
            role: MemoryRole::User,
            trust_level: MemoryTrustLevel::UserExplicit,
            subject_type: MemorySubjectType::Issue,
            subject_ref: "owner/repo#123".to_string(),
            payload_json: json!({"body": "panic E0425 in parser-core"}),
            occurred_at: NOW.to_string(),
        })
        .unwrap();
    store
        .insert_node_state(&MemoryNodeState {
            node_id: ingested.node_ids[0].clone(),
            salience: 0.8,
            strength: 2.4,
            resource: 1.0,
            recall_count: 0,
            reinforce_count: 0,
            fan_in: 0,
            fan_out: 0,
            last_recalled_at: None,
            last_reinforced_at: None,
            updated_at: NOW.to_string(),
        })
        .unwrap();
    MemoryIndexBuilder::rebuild(&store, NOW).unwrap();

    let result = MemoryActivationEngine::activate(
        &store,
        &request("activation-direct", "panic E0425 parser-core", Vec::new()),
    )
    .unwrap();

    let top = result.items.first().unwrap();
    assert_eq!(top.node_id, ingested.node_ids[0]);
    assert_eq!(top.source_channel, MemorySourceChannel::Fts);
    assert!(top.direct_score > 0.0);
    assert!(top.salience_score > 0.0);
    assert!(top.strength_score > 0.0);
    assert!(top.explanation.contains("fts:panic"));
    assert!(top.explanation.contains("rare_token:parser-core"));
    assert!(top.explanation.contains("direct="));
    assert_eq!(
        store
            .get_activation_run("activation-direct")
            .unwrap()
            .unwrap()
            .query_ref,
        "owner/repo#123"
    );
    assert_eq!(
        store.list_activation_items("activation-direct").unwrap(),
        result.items
    );
}

#[test]
fn activation_surfaces_graph_only_related_node_through_near_ripple() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    let ingestor = MemoryIngestor::new(&store);
    let ingested = ingestor
        .ingest_dispatch_outcome(&DispatchMemoryOutcome {
            id: "dispatch-ripple".to_string(),
            issue_key: IssueKey::new("owner/repo", 7),
            agent_id: "codex".to_string(),
            outcome_kind: Some("failed".to_string()),
            task_type: "rust_cli_panic".to_string(),
            succeeded: false,
            failure_class: Some("validation_failed".to_string()),
            failure_reason: Some("unclear_validation".to_string()),
            validation_outcome: Some("failed".to_string()),
            validation_paths: Vec::new(),
            artifact_refs: Vec::new(),
            occurred_at: NOW.to_string(),
            metadata: json!({}),
        })
        .unwrap();
    MemoryIndexBuilder::rebuild(&store, NOW).unwrap();
    MemoryGraphBuilder::rebuild_coactivation_edges(&store, NOW).unwrap();

    let result = MemoryActivationEngine::activate(
        &store,
        &request(
            "activation-ripple",
            "",
            vec![MemoryActivationEntity {
                entity_type: "agent".to_string(),
                entity_value: "codex".to_string(),
            }],
        ),
    )
    .unwrap();

    let raw_item = result
        .items
        .iter()
        .find(|item| item.node_id == ingested.node_ids[0])
        .expect("raw event should surface through graph ripple");
    assert_eq!(raw_item.source_channel, MemorySourceChannel::NearRipple);
    assert_eq!(raw_item.direct_score, 0.0);
    assert!(raw_item.ripple_score > 0.0);
    assert!(raw_item.hop_penalty > 0.0);
    assert!(raw_item.explanation.contains("near_ripple"));
}

#[test]
fn activation_settlement_penalizes_hubs_and_low_resource_nodes() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    let ingestor = MemoryIngestor::new(&store);
    let ingested = ingestor
        .ingest_dispatch_outcome(&DispatchMemoryOutcome {
            id: "dispatch-hub".to_string(),
            issue_key: IssueKey::new("owner/repo", 8),
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
    let agent_node = store
        .list_nodes_for_raw_event(&ingested.raw_event_ids[0])
        .unwrap()
        .into_iter()
        .find(|node| {
            node.node_type == MemoryNodeType::Entity && node.entity_type.as_deref() == Some("agent")
        })
        .unwrap();
    store
        .insert_node_state(&MemoryNodeState {
            node_id: agent_node.id.clone(),
            salience: 0.1,
            strength: 1.0,
            resource: 0.6,
            recall_count: 5,
            reinforce_count: 1,
            fan_in: 8,
            fan_out: 8,
            last_recalled_at: Some(NOW.to_string()),
            last_reinforced_at: None,
            updated_at: NOW.to_string(),
        })
        .unwrap();
    MemoryIndexBuilder::rebuild(&store, NOW).unwrap();

    let result = MemoryActivationEngine::activate(
        &store,
        &request(
            "activation-hub",
            "",
            vec![MemoryActivationEntity {
                entity_type: "agent".to_string(),
                entity_value: "codex".to_string(),
            }],
        ),
    )
    .unwrap();

    let agent_item = result
        .items
        .iter()
        .find(|item| item.node_id == agent_node.id)
        .unwrap();
    assert!(agent_item.resource_penalty > 0.0);
    assert!(agent_item.hub_penalty > 0.0);
    assert!(agent_item.explanation.contains("resource_penalty="));
    assert!(agent_item.explanation.contains("hub_penalty="));
}

#[test]
fn activation_penalizes_llm_only_memory_below_user_explicit_feedback() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    let ingestor = MemoryIngestor::new(&store);
    let user = ingestor
        .ingest_manual_event(&ManualMemoryEvent {
            id: "manual-user".to_string(),
            event_type: MemoryRawEventType::Approve,
            role: MemoryRole::User,
            trust_level: MemoryTrustLevel::UserExplicit,
            subject_type: MemorySubjectType::Profile,
            subject_ref: "ranking".to_string(),
            payload_json: json!({"preference": "stable preference api tasks"}),
            occurred_at: NOW.to_string(),
        })
        .unwrap();
    let llm = ingestor
        .ingest_manual_event(&ManualMemoryEvent {
            id: "manual-llm".to_string(),
            event_type: MemoryRawEventType::Approve,
            role: MemoryRole::Llm,
            trust_level: MemoryTrustLevel::LlmInferred,
            subject_type: MemorySubjectType::Profile,
            subject_ref: "ranking".to_string(),
            payload_json: json!({"preference": "stable preference api tasks"}),
            occurred_at: NOW.to_string(),
        })
        .unwrap();
    MemoryIndexBuilder::rebuild(&store, NOW).unwrap();

    let result = MemoryActivationEngine::activate(
        &store,
        &request(
            "activation-trust",
            "stable preference api tasks",
            Vec::new(),
        ),
    )
    .unwrap();

    let user_item = result
        .items
        .iter()
        .find(|item| item.node_id == user.node_ids[0])
        .unwrap();
    let llm_item = result
        .items
        .iter()
        .find(|item| item.node_id == llm.node_ids[0])
        .unwrap();
    assert_eq!(user_item.role_trust_penalty, 0.0);
    assert!(llm_item.role_trust_penalty > user_item.role_trust_penalty);
    assert!(user_item.final_score > llm_item.final_score);
    assert!(user_item.rank < llm_item.rank);
}

fn request(
    run_id: &str,
    query_text: &str,
    entities: Vec<MemoryActivationEntity>,
) -> MemoryActivationRequest {
    MemoryActivationRequest {
        run_id: run_id.to_string(),
        query_kind: MemoryQueryKind::ScoutRanking,
        query_ref: "owner/repo#123".to_string(),
        query_text: query_text.to_string(),
        entities,
        created_at: NOW.to_string(),
        limit: 10,
        persist_trace: true,
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
