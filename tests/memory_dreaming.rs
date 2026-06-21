use std::path::Path;

use anyhow::{bail, Result};
use issue_finder::memory::{
    DispatchMemoryOutcome, MemoryActivationItem, MemoryActivationRun, MemoryDreamEngine,
    MemoryDreamProposal, MemoryDreamRequest, MemoryDreamRun, MemoryDreamScope, MemoryDreamStatus,
    MemoryDreamSynthesizer, MemoryDreamTrigger, MemoryDreamType, MemoryHintProposal,
    MemoryHintScopeType, MemoryHintStatus, MemoryHintType, MemoryIngestor, MemoryModelStatus,
    MemoryQueryKind, MemorySourceChannel, MemoryStore, NewMemoryDream, NewMemoryHint,
};
use issue_finder::paths::IssueFinderPaths;
use issue_finder::recommendation::IssueKey;
use serde_json::{json, Value};
use tempfile::tempdir;

const OLD: &str = "2026-05-01T00:00:00Z";
const NOW: &str = "2026-06-18T00:00:00Z";

#[test]
fn deterministic_dreaming_consumes_activation_and_raw_evidence_as_candidates() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    let seeded = seed_dispatch(&store, "dispatch-success", true);
    insert_activation(&store, "activation-1", &seeded.raw_node_id);

    let result = MemoryDreamEngine::dream(
        &store,
        &MemoryDreamRequest {
            run_id: "dream-run-disabled".to_string(),
            trigger: MemoryDreamTrigger::Manual,
            scope: MemoryDreamScope::Global,
            scope_ref: None,
            input_activation_run_ids: vec!["activation-1".to_string()],
            created_at: NOW.to_string(),
        },
        None,
    )
    .unwrap();

    assert_eq!(result.run.model_status, MemoryModelStatus::Disabled);
    assert!(json_array_contains(
        &result.run.input_activation_run_ids_json,
        "activation-1"
    ));

    let agent_dream = result
        .dreams
        .iter()
        .find(|dream| dream.dream_type == MemoryDreamType::AgentPerformance)
        .unwrap();
    assert_eq!(agent_dream.status, MemoryDreamStatus::Candidate);
    assert!(json_array_contains(
        &agent_dream.evidence_event_ids_json,
        &seeded.raw_event_id
    ));
    assert!(json_array_contains(
        &agent_dream.evidence_node_ids_json,
        &seeded.raw_node_id
    ));

    let hint = result
        .hints
        .iter()
        .find(|hint| hint.dream_id == agent_dream.id)
        .unwrap();
    assert_eq!(hint.status, MemoryHintStatus::Candidate);
    assert_eq!(hint.hint_type, MemoryHintType::Dispatch);
    assert_eq!(hint.approved_at, None);
    assert_eq!(hint.policy_json["recommendation"], "prefer");
}

#[test]
fn dreaming_generates_stale_and_conflict_candidates_without_mutating_old_hint() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    seed_approved_success_hint(&store);
    let seeded = seed_dispatch(&store, "dispatch-failure", false);

    let result = MemoryDreamEngine::dream(
        &store,
        &MemoryDreamRequest {
            run_id: "dream-run-conflict".to_string(),
            trigger: MemoryDreamTrigger::AfterDispatch,
            scope: MemoryDreamScope::Global,
            scope_ref: None,
            input_activation_run_ids: Vec::new(),
            created_at: NOW.to_string(),
        },
        None,
    )
    .unwrap();

    let conflict = result
        .dreams
        .iter()
        .find(|dream| dream.dream_type == MemoryDreamType::Conflict)
        .unwrap();
    assert_eq!(conflict.status, MemoryDreamStatus::Candidate);
    assert!(json_array_contains(
        &conflict.evidence_event_ids_json,
        &seeded.raw_event_id
    ));
    assert!(json_array_contains(
        &conflict.evidence_hint_ids_json,
        "old-success-hint"
    ));

    let stale = result
        .dreams
        .iter()
        .find(|dream| dream.dream_type == MemoryDreamType::StaleMemory)
        .unwrap();
    assert_eq!(stale.status, MemoryDreamStatus::Candidate);
    assert!(json_array_contains(
        &stale.evidence_hint_ids_json,
        "old-success-hint"
    ));

    let conflict_hint = result
        .hints
        .iter()
        .find(|hint| hint.policy_json["kind"] == "conflict_review")
        .unwrap();
    assert_eq!(conflict_hint.status, MemoryHintStatus::Candidate);
    assert_eq!(
        store.get_hint("old-success-hint").unwrap().unwrap().status,
        MemoryHintStatus::Approved
    );
}

#[test]
fn dreaming_separates_dispatch_agent_hints_from_ranking_trend_hints() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    seed_dispatch(&store, "dispatch-success", true);
    seed_dispatch(&store, "dispatch-failure", false);
    seed_policy_blocked_dispatch(&store);

    let result = MemoryDreamEngine::dream(
        &store,
        &MemoryDreamRequest {
            run_id: "dream-run-ranking-trend".to_string(),
            trigger: MemoryDreamTrigger::AfterDispatch,
            scope: MemoryDreamScope::Global,
            scope_ref: None,
            input_activation_run_ids: Vec::new(),
            created_at: NOW.to_string(),
        },
        None,
    )
    .unwrap();

    assert!(result
        .hints
        .iter()
        .any(|hint| hint.hint_type == MemoryHintType::Dispatch
            && hint.policy_json["kind"] == "agent_performance"));
    let ranking_hints = result
        .hints
        .iter()
        .filter(|hint| {
            hint.hint_type == MemoryHintType::Ranking
                && hint.policy_json["kind"] == "dispatch_outcome_trend"
        })
        .collect::<Vec<_>>();
    assert!(
        ranking_hints
            .iter()
            .any(|hint| hint.scope_type == MemoryHintScopeType::Repo
                && hint.scope_ref == "owner/repo")
    );
    assert!(ranking_hints.iter().any(|hint| {
        hint.scope_type == MemoryHintScopeType::IssueType && hint.scope_ref == "rust_cli_panic"
    }));
    assert!(ranking_hints
        .iter()
        .all(|hint| hint.policy_json["failures"].as_u64().unwrap_or(0) <= 1));
}

#[test]
fn optional_llm_synthesis_stores_candidates_and_failure_does_not_block_deterministic_dreams() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    let seeded = seed_dispatch(&store, "dispatch-success", true);
    insert_activation(&store, "activation-llm", &seeded.raw_node_id);

    let successful = SyntheticSynthesizer;
    let success = MemoryDreamEngine::dream(
        &store,
        &MemoryDreamRequest {
            run_id: "dream-run-llm-success".to_string(),
            trigger: MemoryDreamTrigger::Manual,
            scope: MemoryDreamScope::Global,
            scope_ref: None,
            input_activation_run_ids: vec!["activation-llm".to_string()],
            created_at: NOW.to_string(),
        },
        Some(&successful),
    )
    .unwrap();
    assert_eq!(success.run.model_status, MemoryModelStatus::Success);
    let synthesized = success
        .dreams
        .iter()
        .find(|dream| dream.summary == "LLM synthesized profile candidate")
        .unwrap();
    assert_eq!(synthesized.status, MemoryDreamStatus::Candidate);
    assert!(success
        .hints
        .iter()
        .any(|hint| hint.dream_id == synthesized.id && hint.status == MemoryHintStatus::Candidate));

    let failing = FailingSynthesizer;
    let failed = MemoryDreamEngine::dream(
        &store,
        &MemoryDreamRequest {
            run_id: "dream-run-llm-failed".to_string(),
            trigger: MemoryDreamTrigger::Manual,
            scope: MemoryDreamScope::Global,
            scope_ref: None,
            input_activation_run_ids: vec!["activation-llm".to_string()],
            created_at: NOW.to_string(),
        },
        Some(&failing),
    )
    .unwrap();
    assert_eq!(failed.run.model_status, MemoryModelStatus::Failed);
    assert!(failed
        .dreams
        .iter()
        .any(|dream| dream.dream_type == MemoryDreamType::AgentPerformance));
}

struct SyntheticSynthesizer;

impl MemoryDreamSynthesizer for SyntheticSynthesizer {
    fn synthesize(
        &self,
        context: &issue_finder::memory::MemoryDreamContext,
    ) -> Result<Vec<MemoryDreamProposal>> {
        Ok(vec![MemoryDreamProposal {
            dream_type: MemoryDreamType::ProfileAdjustment,
            summary: "LLM synthesized profile candidate".to_string(),
            evidence_node_ids: context.evidence_node_ids.clone(),
            evidence_event_ids: context.evidence_event_ids.clone(),
            evidence_hint_ids: Vec::new(),
            confidence: 0.9,
            hint: Some(MemoryHintProposal {
                hint_type: MemoryHintType::ProfileCandidate,
                scope_type: MemoryHintScopeType::Global,
                scope_ref: "global".to_string(),
                summary: "Review synthesized profile preference.".to_string(),
                policy_json: json!({"kind": "profile_adjustment_candidate"}),
                weight: 0.0,
                expires_at: None,
            }),
        }])
    }
}

struct FailingSynthesizer;

impl MemoryDreamSynthesizer for FailingSynthesizer {
    fn synthesize(
        &self,
        _context: &issue_finder::memory::MemoryDreamContext,
    ) -> Result<Vec<MemoryDreamProposal>> {
        bail!("llm unavailable")
    }
}

struct SeededDispatch {
    raw_event_id: String,
    raw_node_id: String,
}

fn seed_dispatch(store: &MemoryStore, id: &str, succeeded: bool) -> SeededDispatch {
    let ingested = MemoryIngestor::new(store)
        .ingest_dispatch_outcome(&DispatchMemoryOutcome {
            id: id.to_string(),
            issue_key: IssueKey::new("owner/repo", 42),
            agent_id: "codex".to_string(),
            outcome_kind: Some(if succeeded { "fix_ready" } else { "failed" }.to_string()),
            task_type: "rust_cli_panic".to_string(),
            succeeded,
            failure_class: (!succeeded).then(|| "validation_failed".to_string()),
            failure_reason: (!succeeded).then(|| "panic still reproduces".to_string()),
            validation_outcome: Some(if succeeded { "passed" } else { "failed" }.to_string()),
            validation_paths: vec!["cargo test".to_string()],
            artifact_refs: Vec::new(),
            occurred_at: NOW.to_string(),
            metadata: json!({}),
        })
        .unwrap();
    SeededDispatch {
        raw_event_id: ingested.raw_event_ids[0].clone(),
        raw_node_id: ingested.node_ids[0].clone(),
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

fn seed_approved_success_hint(store: &MemoryStore) {
    store
        .insert_dream_run(&MemoryDreamRun {
            id: "old-dream-run".to_string(),
            trigger: MemoryDreamTrigger::Manual,
            scope: MemoryDreamScope::Global,
            input_activation_run_ids_json: json!([]),
            model_status: MemoryModelStatus::Disabled,
            created_at: OLD.to_string(),
        })
        .unwrap();
    store
        .insert_dream(&NewMemoryDream {
            id: "old-dream".to_string(),
            dream_run_id: "old-dream-run".to_string(),
            dream_type: MemoryDreamType::AgentPerformance,
            summary: "Older success pattern".to_string(),
            evidence_node_ids_json: json!([]),
            evidence_event_ids_json: json!([]),
            evidence_hint_ids_json: json!([]),
            status: MemoryDreamStatus::Approved,
            confidence: 0.8,
            version: 1,
            created_at: OLD.to_string(),
            reviewed_at: Some(OLD.to_string()),
        })
        .unwrap();
    store
        .insert_hint(&NewMemoryHint {
            id: "old-success-hint".to_string(),
            dream_id: "old-dream".to_string(),
            hint_type: MemoryHintType::Dispatch,
            scope_type: MemoryHintScopeType::Agent,
            scope_ref: "codex".to_string(),
            summary: "Codex usually succeeds on rust cli panic work.".to_string(),
            policy_json: json!({
                "kind": "agent_performance",
                "agentId": "codex",
                "taskType": "rust_cli_panic",
                "recommendation": "prefer",
            }),
            weight: 0.5,
            status: MemoryHintStatus::Approved,
            created_at: OLD.to_string(),
            approved_at: Some(OLD.to_string()),
            expires_at: Some(OLD.to_string()),
        })
        .unwrap();
}

fn seed_policy_blocked_dispatch(store: &MemoryStore) {
    MemoryIngestor::new(store)
        .ingest_dispatch_outcome(&DispatchMemoryOutcome {
            id: "dispatch-policy-blocked".to_string(),
            issue_key: IssueKey::new("owner/repo", 77),
            agent_id: "codex".to_string(),
            outcome_kind: Some("blocked".to_string()),
            task_type: "rust_cli_panic".to_string(),
            succeeded: false,
            failure_class: Some("policy_blocked".to_string()),
            failure_reason: Some("prepare gate rejected".to_string()),
            validation_outcome: None,
            validation_paths: Vec::new(),
            artifact_refs: Vec::new(),
            occurred_at: NOW.to_string(),
            metadata: json!({}),
        })
        .unwrap();
}

fn json_array_contains(value: &Value, needle: &str) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(needle)))
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
