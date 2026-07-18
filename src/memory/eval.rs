use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    MemoryControlPlane, MemoryDecisionHintRequest, MemoryDreamRun, MemoryDreamScope,
    MemoryDreamStatus, MemoryDreamTrigger, MemoryDreamType, MemoryEdgeRelation, MemoryHintStatus,
    MemoryHintType, MemoryIndex, MemoryIndexType, MemoryModelStatus, MemoryNodeType, MemoryRole,
    MemorySourceType, MemoryStore, MemorySubjectType, MemoryTrustLevel, NewMemoryDream,
    NewMemoryEdge, NewMemoryHint, NewMemoryNode, NewMemoryRawEvent, NewMemorySource,
};
use crate::paths::atomic_write;

const MEMORY_EVAL_FIXTURES: &str = include_str!("../../tests/fixtures/memory_eval/samples.json");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEvalReport {
    pub kind: String,
    pub metrics: MemoryEvalMetrics,
    pub samples: Vec<MemoryEvalSampleResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEvalMetrics {
    pub total_samples: usize,
    pub passed_samples: usize,
    pub failed_samples: usize,
    pub dimensions: BTreeMap<String, MemoryEvalDimensionMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEvalDimensionMetrics {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEvalSampleResult {
    pub id: String,
    pub dimension: String,
    pub passed: bool,
    pub expected_behavior: String,
    pub observed_behavior: String,
}

#[derive(Debug, Deserialize)]
struct MemoryEvalFixtures {
    samples: Vec<MemoryEvalFixtureSample>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemoryEvalFixtureSample {
    id: String,
    dimension: String,
    expected_behavior: String,
}

const NOW: &str = "2026-06-18T00:00:00Z";
const LATER: &str = "2026-06-19T00:00:00Z";
static EVAL_STORE_COUNTER: AtomicU64 = AtomicU64::new(0);

struct EvalStoreDir(PathBuf);

impl EvalStoreDir {
    fn new(label: &str) -> Result<Self> {
        let sequence = EVAL_STORE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "issue-finder-memory-eval-{label}-{}-{sequence}",
            process::id()
        ));
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn database(&self) -> PathBuf {
        self.0.join("memory.sqlite3")
    }
}

impl Drop for EvalStoreDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn run_offline_eval(output_dir: &Path) -> Result<MemoryEvalReport> {
    let fixtures = serde_json::from_str::<MemoryEvalFixtures>(MEMORY_EVAL_FIXTURES)
        .context("memory eval fixture JSON is invalid")?;
    let samples = fixtures
        .samples
        .into_iter()
        .map(evaluate_fixture_sample)
        .collect::<Result<Vec<_>>>()?;
    let metrics = metrics_for(&samples);
    let report = MemoryEvalReport {
        kind: "memory_eval_report".to_string(),
        metrics,
        samples,
    };

    fs::create_dir_all(output_dir)
        .with_context(|| format!("unable to create {}", output_dir.display()))?;
    atomic_write(
        &output_dir.join("metrics.json"),
        &serde_json::to_string_pretty(&report.metrics)?,
    )?;
    atomic_write(
        &output_dir.join("report.md"),
        render_markdown_report(&report),
    )?;
    Ok(report)
}

fn evaluate_fixture_sample(sample: MemoryEvalFixtureSample) -> Result<MemoryEvalSampleResult> {
    let (passed, observed_behavior) = match sample.id.as_str() {
        "over_recall_candidate_hidden" => evaluate_candidate_hint_is_ineligible()?,
        "deletion_tombstone_cascade" => evaluate_tombstone_cascade()?,
        other => (false, format!("unsupported memory eval scenario `{other}`")),
    };
    Ok(MemoryEvalSampleResult {
        id: sample.id,
        dimension: sample.dimension,
        passed,
        expected_behavior: sample.expected_behavior,
        observed_behavior,
    })
}

fn evaluate_candidate_hint_is_ineligible() -> Result<(bool, String)> {
    let directory = EvalStoreDir::new("candidate-hint")?;
    let store = MemoryStore::open_at(directory.database())?;
    seed_dream_and_candidate_hint(&store, false)?;
    let eligible =
        MemoryControlPlane::decision_eligible_hints(&store, &MemoryDecisionHintRequest::default())?;
    let candidate_present = store.get_hint("hint-1")?.is_some();
    let candidate_selected = eligible.iter().any(|item| item.hint.id == "hint-1");
    Ok((
        candidate_present && !candidate_selected,
        format!(
            "candidate_present={candidate_present}; decision_selected={candidate_selected}; eligible_count={}",
            eligible.len()
        ),
    ))
}

fn evaluate_tombstone_cascade() -> Result<(bool, String)> {
    let directory = EvalStoreDir::new("tombstone")?;
    let store = MemoryStore::open_at(directory.database())?;
    seed_source_event_node_dream_and_hint(&store)?;
    store.insert_node(&NewMemoryNode {
        id: "node-2".to_string(),
        node_type: MemoryNodeType::Entity,
        raw_event_id: None,
        entity_type: Some("repo".to_string()),
        entity_value: Some("owner/repo".to_string()),
        normalized_value: Some("owner/repo".to_string()),
        text_ref: None,
        metadata_json: json!({}),
        created_at: NOW.to_string(),
    })?;
    store.insert_index(&MemoryIndex {
        node_id: "node-1".to_string(),
        index_type: MemoryIndexType::Fts,
        index_ref_or_payload: "broad".to_string(),
        created_at: NOW.to_string(),
    })?;
    store.insert_edge(&NewMemoryEdge {
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
    })?;
    store.tombstone_raw_event("event-1", LATER)?;
    let event_tombstoned = store
        .get_raw_event("event-1")?
        .and_then(|value| value.tombstoned_at)
        .as_deref()
        == Some(LATER);
    let node_tombstoned = store
        .get_node("node-1")?
        .and_then(|value| value.tombstoned_at)
        .as_deref()
        == Some(LATER);
    let indexes_removed = store.list_indexes_for_node("node-1")?.is_empty();
    let edge_tombstoned = store
        .get_edge("edge-1")?
        .and_then(|value| value.tombstoned_at)
        .as_deref()
        == Some(LATER);
    let dream_tombstoned = store
        .get_dream("dream-1")?
        .is_some_and(|value| value.status == MemoryDreamStatus::Tombstoned);
    let hint_tombstoned = store
        .get_hint("hint-1")?
        .is_some_and(|value| value.status == MemoryHintStatus::Tombstoned);
    let passed = event_tombstoned
        && node_tombstoned
        && indexes_removed
        && edge_tombstoned
        && dream_tombstoned
        && hint_tombstoned;
    Ok((
        passed,
        format!(
            "event={event_tombstoned}; node={node_tombstoned}; indexes_removed={indexes_removed}; edge={edge_tombstoned}; dream={dream_tombstoned}; hint={hint_tombstoned}"
        ),
    ))
}

fn seed_dream_and_candidate_hint(store: &MemoryStore, bind_event: bool) -> Result<()> {
    store.insert_dream_run(&MemoryDreamRun {
        id: "dream-run-1".to_string(),
        trigger: MemoryDreamTrigger::Manual,
        scope: MemoryDreamScope::Repo,
        input_activation_run_ids_json: json!([]),
        model_status: MemoryModelStatus::Disabled,
        created_at: NOW.to_string(),
    })?;
    store.insert_dream(&NewMemoryDream {
        id: "dream-1".to_string(),
        dream_run_id: "dream-run-1".to_string(),
        dream_type: MemoryDreamType::DiscoveryPolicy,
        summary: "Avoid broad issues".to_string(),
        evidence_node_ids_json: if bind_event {
            json!(["node-1"])
        } else {
            json!([])
        },
        evidence_event_ids_json: if bind_event {
            json!(["event-1"])
        } else {
            json!([])
        },
        evidence_hint_ids_json: json!([]),
        status: MemoryDreamStatus::Candidate,
        confidence: 0.9,
        version: 1,
        created_at: NOW.to_string(),
        reviewed_at: None,
    })?;
    store.insert_hint(&NewMemoryHint {
        id: "hint-1".to_string(),
        dream_id: "dream-1".to_string(),
        hint_type: MemoryHintType::Ranking,
        scope_type: super::MemoryHintScopeType::Global,
        scope_ref: "global".to_string(),
        summary: "Candidate must not affect decisions".to_string(),
        policy_json: json!({"avoid": "unclear_validation"}),
        weight: 5.0,
        status: MemoryHintStatus::Candidate,
        created_at: NOW.to_string(),
        approved_at: None,
        expires_at: None,
    })?;
    Ok(())
}

fn seed_source_event_node_dream_and_hint(store: &MemoryStore) -> Result<()> {
    store.insert_source(&NewMemorySource {
        id: "source-1".to_string(),
        source_type: MemorySourceType::RecommendationEvent,
        source_ref: "recommendation-event-1".to_string(),
        trust_level: MemoryTrustLevel::UserExplicit,
        created_at: NOW.to_string(),
    })?;
    store.insert_raw_event(&NewMemoryRawEvent {
        id: "event-1".to_string(),
        source_id: "source-1".to_string(),
        event_type: super::MemoryRawEventType::Reject,
        role: MemoryRole::User,
        trust_level: MemoryTrustLevel::UserExplicit,
        subject_type: MemorySubjectType::Issue,
        subject_ref: "owner/repo#123".to_string(),
        payload_json: json!({"reason": "too broad"}),
        confidence: 1.0,
        occurred_at: NOW.to_string(),
        created_at: NOW.to_string(),
    })?;
    store.insert_node(&NewMemoryNode {
        id: "node-1".to_string(),
        node_type: MemoryNodeType::RawEvent,
        raw_event_id: Some("event-1".to_string()),
        entity_type: None,
        entity_value: None,
        normalized_value: None,
        text_ref: Some("memory_raw_events:event-1".to_string()),
        metadata_json: json!({"summary": "user rejected broad issue"}),
        created_at: NOW.to_string(),
    })?;
    seed_dream_and_candidate_hint(store, true)
}

fn metrics_for(samples: &[MemoryEvalSampleResult]) -> MemoryEvalMetrics {
    let mut dimensions = BTreeMap::<String, MemoryEvalDimensionMetrics>::new();
    for sample in samples {
        let entry =
            dimensions
                .entry(sample.dimension.clone())
                .or_insert(MemoryEvalDimensionMetrics {
                    total: 0,
                    passed: 0,
                    failed: 0,
                });
        entry.total += 1;
        if sample.passed {
            entry.passed += 1;
        } else {
            entry.failed += 1;
        }
    }
    MemoryEvalMetrics {
        total_samples: samples.len(),
        passed_samples: samples.iter().filter(|sample| sample.passed).count(),
        failed_samples: samples.iter().filter(|sample| !sample.passed).count(),
        dimensions,
    }
}

fn render_markdown_report(report: &MemoryEvalReport) -> String {
    let mut lines = vec![
        "# Memory Eval".to_string(),
        String::new(),
        format!("- Total samples: {}", report.metrics.total_samples),
        format!("- Passed: {}", report.metrics.passed_samples),
        format!("- Failed: {}", report.metrics.failed_samples),
        String::new(),
        "## Dimensions".to_string(),
        String::new(),
    ];
    for (dimension, metrics) in &report.metrics.dimensions {
        lines.push(format!(
            "- {dimension}: {}/{} passed",
            metrics.passed, metrics.total
        ));
    }
    lines.extend([String::new(), "## Samples".to_string(), String::new()]);
    for sample in &report.samples {
        let status = if sample.passed { "pass" } else { "fail" };
        lines.push(format!(
            "- `{}` [{}]: {} Expected: {} Observed: {}",
            sample.id, status, sample.dimension, sample.expected_behavior, sample.observed_behavior
        ));
    }
    lines.push(String::new());
    lines.join("\n")
}
