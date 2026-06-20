use std::path::Path;

use issue_finder::memory::{
    MemoryActivationItem, MemoryActivationRun, MemoryDreamRun, MemoryDreamScope, MemoryDreamStatus,
    MemoryDreamTrigger, MemoryDreamType, MemoryEdgeRelation, MemoryHintScopeType, MemoryHintStatus,
    MemoryHintType, MemoryIndex, MemoryIndexType, MemoryModelStatus, MemoryNodeState,
    MemoryNodeType, MemoryQueryKind, MemoryRawEventType, MemoryRole, MemorySourceChannel,
    MemorySourceType, MemoryStore, MemorySubjectType, MemoryTrustLevel, MemoryWriteback,
    MemoryWritebackAction, NewMemoryDream, NewMemoryEdge, NewMemoryHint, NewMemoryNode,
    NewMemoryRawEvent, NewMemorySource,
};
use issue_finder::paths::IssueFinderPaths;
use serde_json::json;
use tempfile::tempdir;

const NOW: &str = "2026-06-18T00:00:00Z";
const LATER: &str = "2026-06-18T00:10:00Z";

#[test]
fn memory_schema_initialization_is_idempotent() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());

    let store = MemoryStore::open(&paths).unwrap();
    store.init_schema().unwrap();
    store.init_schema().unwrap();

    assert!(paths.state_db_path().exists());

    let reopened = MemoryStore::open(&paths).unwrap();
    assert!(reopened.get_source("missing").unwrap().is_none());
}

#[test]
fn memory_store_round_trips_milestone_one_tables() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    seed_source_event_and_node(&store);

    let state = store
        .insert_node_state(&MemoryNodeState {
            node_id: "node-1".to_string(),
            salience: 0.7,
            strength: 2.5,
            resource: 1.0,
            recall_count: 1,
            reinforce_count: 0,
            fan_in: 2,
            fan_out: 3,
            last_recalled_at: Some(NOW.to_string()),
            last_reinforced_at: None,
            updated_at: NOW.to_string(),
        })
        .unwrap();
    assert_eq!(store.get_node_state("node-1").unwrap(), Some(state));

    let index = store
        .insert_index(&MemoryIndex {
            node_id: "node-1".to_string(),
            index_type: MemoryIndexType::RareToken,
            index_ref_or_payload: "panic".to_string(),
            created_at: NOW.to_string(),
        })
        .unwrap();
    assert_eq!(store.list_indexes_for_node("node-1").unwrap(), vec![index]);

    store
        .insert_node(&NewMemoryNode {
            id: "node-2".to_string(),
            node_type: MemoryNodeType::Entity,
            raw_event_id: None,
            entity_type: Some("repo".to_string()),
            entity_value: Some("owner/repo".to_string()),
            normalized_value: Some("owner/repo".to_string()),
            text_ref: None,
            metadata_json: json!({}),
            created_at: NOW.to_string(),
        })
        .unwrap();
    let edge = store
        .insert_edge(&NewMemoryEdge {
            id: "edge-1".to_string(),
            from_node_id: "node-1".to_string(),
            to_node_id: "node-2".to_string(),
            relation: MemoryEdgeRelation::CoActivated,
            strength: 0.5,
            confidence: 0.6,
            evidence_event_ids_json: json!(["event-1"]),
            last_activated_at: None,
            created_at: NOW.to_string(),
            updated_at: NOW.to_string(),
        })
        .unwrap();
    assert_eq!(store.get_edge("edge-1").unwrap(), Some(edge));
    let updated_edge = store
        .update_edge_state("edge-1", 0.9, 0.8, Some(LATER), LATER)
        .unwrap()
        .unwrap();
    assert_eq!(updated_edge.strength, 0.9);
    assert_eq!(updated_edge.last_activated_at.as_deref(), Some(LATER));

    let run = store
        .insert_activation_run(&MemoryActivationRun {
            id: "activation-1".to_string(),
            query_kind: MemoryQueryKind::ScoutRanking,
            query_ref: "owner/repo#123".to_string(),
            query_json: json!({"issue": "owner/repo#123"}),
            created_at: NOW.to_string(),
        })
        .unwrap();
    assert_eq!(store.get_activation_run("activation-1").unwrap(), Some(run));

    let item = store
        .insert_activation_item(&MemoryActivationItem {
            run_id: "activation-1".to_string(),
            node_id: "node-1".to_string(),
            source_channel: MemorySourceChannel::Fts,
            direct_score: 0.9,
            ripple_score: 0.0,
            salience_score: 0.7,
            strength_score: 0.5,
            recency_score: 0.2,
            resource_penalty: 0.1,
            hub_penalty: 0.0,
            role_trust_penalty: 0.0,
            hop_penalty: 0.0,
            stale_penalty: 0.0,
            final_score: 2.2,
            rank: 1,
            explanation: "rare token matched".to_string(),
        })
        .unwrap();
    assert_eq!(
        store.list_activation_items("activation-1").unwrap(),
        vec![item]
    );

    let writeback = store
        .insert_writeback(&MemoryWriteback {
            id: "writeback-1".to_string(),
            activation_run_id: "activation-1".to_string(),
            node_id: "node-1".to_string(),
            action: MemoryWritebackAction::Recalled,
            before_json: json!({"recall_count": 0}),
            after_json: json!({"recall_count": 1}),
            created_at: NOW.to_string(),
        })
        .unwrap();
    assert_eq!(store.get_writeback("writeback-1").unwrap(), Some(writeback));

    let dream_run = store
        .insert_dream_run(&MemoryDreamRun {
            id: "dream-run-1".to_string(),
            trigger: MemoryDreamTrigger::Manual,
            scope: MemoryDreamScope::Repo,
            input_activation_run_ids_json: json!(["activation-1"]),
            model_status: MemoryModelStatus::Disabled,
            created_at: NOW.to_string(),
        })
        .unwrap();
    assert_eq!(store.get_dream_run("dream-run-1").unwrap(), Some(dream_run));

    let dream = store
        .insert_dream(&NewMemoryDream {
            id: "dream-1".to_string(),
            dream_run_id: "dream-run-1".to_string(),
            dream_type: MemoryDreamType::RepoSummary,
            summary: "This repo has successful Rust CLI fixes.".to_string(),
            evidence_node_ids_json: json!(["node-1"]),
            evidence_event_ids_json: json!(["event-1"]),
            evidence_hint_ids_json: json!([]),
            status: MemoryDreamStatus::Candidate,
            confidence: 0.8,
            version: 1,
            created_at: NOW.to_string(),
            reviewed_at: None,
        })
        .unwrap();
    assert_eq!(store.get_dream("dream-1").unwrap(), Some(dream));

    let hint = store
        .insert_hint(&NewMemoryHint {
            id: "hint-1".to_string(),
            dream_id: "dream-1".to_string(),
            hint_type: MemoryHintType::Ranking,
            scope_type: MemoryHintScopeType::Repo,
            scope_ref: "owner/repo".to_string(),
            summary: "Prefer focused Rust CLI bug fixes in this repo.".to_string(),
            policy_json: json!({"adjustment": "prefer"}),
            weight: 0.4,
            status: MemoryHintStatus::Candidate,
            created_at: NOW.to_string(),
            approved_at: None,
            expires_at: None,
        })
        .unwrap();
    assert_eq!(store.get_hint("hint-1").unwrap(), Some(hint));
}

#[test]
fn hint_status_transitions_are_centralized_and_audited() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    seed_source_event_node_dream_and_hint(&store);

    let error = store
        .transition_hint_status("hint-1", MemoryHintStatus::Pinned, LATER, None)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("invalid memory hint transition from candidate to pinned"),
        "{error:?}"
    );

    let approved = store
        .transition_hint_status(
            "hint-1",
            MemoryHintStatus::Approved,
            LATER,
            Some("reviewed"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(approved.status, MemoryHintStatus::Approved);
    assert_eq!(approved.approved_at.as_deref(), Some(LATER));

    let pinned = store
        .transition_hint_status("hint-1", MemoryHintStatus::Pinned, LATER, Some("important"))
        .unwrap()
        .unwrap();
    assert_eq!(pinned.status, MemoryHintStatus::Pinned);

    let restored = store
        .restore_hint_prior_status("hint-1", LATER, Some("undo pin"))
        .unwrap()
        .unwrap();
    assert_eq!(restored.status, MemoryHintStatus::Approved);

    let changes = store.list_hint_status_changes("hint-1").unwrap();
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0].from_status, MemoryHintStatus::Candidate);
    assert_eq!(changes[0].to_status, MemoryHintStatus::Approved);
    assert_eq!(changes[2].from_status, MemoryHintStatus::Pinned);
    assert_eq!(changes[2].to_status, MemoryHintStatus::Approved);
}

#[test]
fn tombstoning_raw_event_invalidates_derived_memory() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    seed_source_event_node_dream_and_hint(&store);

    store
        .insert_node(&NewMemoryNode {
            id: "node-2".to_string(),
            node_type: MemoryNodeType::Entity,
            raw_event_id: None,
            entity_type: Some("repo".to_string()),
            entity_value: Some("owner/repo".to_string()),
            normalized_value: Some("owner/repo".to_string()),
            text_ref: None,
            metadata_json: json!({}),
            created_at: NOW.to_string(),
        })
        .unwrap();
    store
        .insert_index(&MemoryIndex {
            node_id: "node-1".to_string(),
            index_type: MemoryIndexType::Fts,
            index_ref_or_payload: "broad".to_string(),
            created_at: NOW.to_string(),
        })
        .unwrap();
    store
        .insert_edge(&NewMemoryEdge {
            id: "edge-1".to_string(),
            from_node_id: "node-1".to_string(),
            to_node_id: "node-2".to_string(),
            relation: MemoryEdgeRelation::Avoids,
            strength: 0.7,
            confidence: 0.9,
            evidence_event_ids_json: json!(["event-1"]),
            last_activated_at: Some(NOW.to_string()),
            created_at: NOW.to_string(),
            updated_at: NOW.to_string(),
        })
        .unwrap();

    store.tombstone_raw_event("event-1", LATER).unwrap();

    assert_eq!(
        store
            .get_raw_event("event-1")
            .unwrap()
            .unwrap()
            .tombstoned_at
            .as_deref(),
        Some(LATER)
    );
    assert_eq!(
        store
            .get_node("node-1")
            .unwrap()
            .unwrap()
            .tombstoned_at
            .as_deref(),
        Some(LATER)
    );
    assert!(store.list_indexes_for_node("node-1").unwrap().is_empty());
    assert_eq!(
        store
            .get_edge("edge-1")
            .unwrap()
            .unwrap()
            .tombstoned_at
            .as_deref(),
        Some(LATER)
    );
    assert_eq!(
        store.get_dream("dream-1").unwrap().unwrap().status,
        MemoryDreamStatus::Tombstoned
    );
    assert_eq!(
        store.get_hint("hint-1").unwrap().unwrap().status,
        MemoryHintStatus::Tombstoned
    );
}

fn seed_source_event_and_node(store: &MemoryStore) {
    let source = store
        .insert_source(&NewMemorySource {
            id: "source-1".to_string(),
            source_type: MemorySourceType::RecommendationEvent,
            source_ref: "recommendation-event-1".to_string(),
            trust_level: MemoryTrustLevel::UserExplicit,
            created_at: NOW.to_string(),
        })
        .unwrap();
    assert_eq!(store.get_source("source-1").unwrap(), Some(source));

    let event = store
        .insert_raw_event(&NewMemoryRawEvent {
            id: "event-1".to_string(),
            source_id: "source-1".to_string(),
            event_type: MemoryRawEventType::Reject,
            role: MemoryRole::User,
            trust_level: MemoryTrustLevel::UserExplicit,
            subject_type: MemorySubjectType::Issue,
            subject_ref: "owner/repo#123".to_string(),
            payload_json: json!({"reason": "too broad"}),
            confidence: 1.0,
            occurred_at: NOW.to_string(),
            created_at: NOW.to_string(),
        })
        .unwrap();
    assert_eq!(store.get_raw_event("event-1").unwrap(), Some(event.clone()));
    assert_eq!(store.list_raw_events().unwrap(), vec![event]);

    let node = store
        .insert_node(&NewMemoryNode {
            id: "node-1".to_string(),
            node_type: MemoryNodeType::RawEvent,
            raw_event_id: Some("event-1".to_string()),
            entity_type: None,
            entity_value: None,
            normalized_value: None,
            text_ref: Some("memory_raw_events:event-1".to_string()),
            metadata_json: json!({"summary": "user rejected broad issue"}),
            created_at: NOW.to_string(),
        })
        .unwrap();
    assert_eq!(store.get_node("node-1").unwrap(), Some(node));
}

fn seed_source_event_node_dream_and_hint(store: &MemoryStore) {
    seed_source_event_and_node(store);
    store
        .insert_dream_run(&MemoryDreamRun {
            id: "dream-run-1".to_string(),
            trigger: MemoryDreamTrigger::Manual,
            scope: MemoryDreamScope::Repo,
            input_activation_run_ids_json: json!([]),
            model_status: MemoryModelStatus::Disabled,
            created_at: NOW.to_string(),
        })
        .unwrap();
    store
        .insert_dream(&NewMemoryDream {
            id: "dream-1".to_string(),
            dream_run_id: "dream-run-1".to_string(),
            dream_type: MemoryDreamType::DiscoveryPolicy,
            summary: "Avoid broad issues with unclear validation.".to_string(),
            evidence_node_ids_json: json!(["node-1"]),
            evidence_event_ids_json: json!(["event-1"]),
            evidence_hint_ids_json: json!([]),
            status: MemoryDreamStatus::Candidate,
            confidence: 0.9,
            version: 1,
            created_at: NOW.to_string(),
            reviewed_at: None,
        })
        .unwrap();
    store
        .insert_hint(&NewMemoryHint {
            id: "hint-1".to_string(),
            dream_id: "dream-1".to_string(),
            hint_type: MemoryHintType::Ranking,
            scope_type: MemoryHintScopeType::Global,
            scope_ref: "global".to_string(),
            summary: "Avoid broad issues with unclear validation.".to_string(),
            policy_json: json!({"avoid": "unclear_validation"}),
            weight: 0.5,
            status: MemoryHintStatus::Candidate,
            created_at: NOW.to_string(),
            approved_at: None,
            expires_at: None,
        })
        .unwrap();
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
