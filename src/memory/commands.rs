use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde::Serialize;
use serde_json::{json, Value};

use crate::memory::activation::{MemoryActivationRequest, MemoryActivationResult};
use crate::memory::controls::{MemoryControlPlane, MemoryDecisionHintRequest, MemoryRuntimeMode};
use crate::memory::dreaming::MemoryDreamRequest;
use crate::memory::model::{
    MemoryDream, MemoryDreamScope, MemoryDreamStatus, MemoryDreamTrigger, MemoryHint,
    MemoryHintScopeType, MemoryHintStatus, MemoryHintType, MemoryModelStatus, MemoryQueryKind,
    MemoryRawEvent, NewMemoryDream, NewMemoryHint,
};
use crate::memory::store::MemoryStore;
use crate::paths::IssueFinderPaths;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStatusOutput {
    pub kind: String,
    pub state_db_path: String,
    pub counts: MemoryCountsOutput,
    pub decision_eligible_hint_count: usize,
    pub embedding_status: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCountsOutput {
    pub raw_events: usize,
    pub nodes: usize,
    pub edges: usize,
    pub dreams: usize,
    pub hints: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEventsOutput {
    pub kind: String,
    pub issue: Option<String>,
    pub events: Vec<MemoryEventOutput>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEventOutput {
    pub id: String,
    pub event_type: String,
    pub role: String,
    pub trust_level: String,
    pub subject_type: String,
    pub subject_ref: String,
    pub confidence: f64,
    pub occurred_at: String,
    pub tombstoned: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecallOutput {
    pub kind: String,
    pub activation_run_id: String,
    pub query_kind: String,
    pub items: Vec<MemoryRecallItemOutput>,
    pub decision_eligible_hints: Vec<MemoryHintOutput>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecallItemOutput {
    pub rank: i64,
    pub node_id: String,
    pub event_id: Option<String>,
    pub source_channel: String,
    pub final_score: f64,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDreamsOutput {
    pub kind: String,
    pub dreams: Vec<MemoryDreamOutput>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDreamDetailOutput {
    pub kind: String,
    pub dream: MemoryDreamOutput,
    pub hints: Vec<MemoryHintOutput>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDreamRunOutput {
    pub kind: String,
    pub run_id: String,
    pub model_status: String,
    pub dreams: Vec<MemoryDreamOutput>,
    pub hints: Vec<MemoryHintOutput>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDreamOutput {
    pub id: String,
    pub dream_run_id: String,
    pub dream_type: String,
    pub summary: String,
    pub evidence_node_ids: Value,
    pub evidence_event_ids: Value,
    pub evidence_hint_ids: Value,
    pub status: String,
    pub confidence: f64,
    pub created_at: String,
    pub reviewed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryHintsOutput {
    pub kind: String,
    pub hints: Vec<MemoryHintOutput>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryHintOutput {
    pub id: String,
    pub dream_id: String,
    pub hint_type: String,
    pub scope_type: String,
    pub scope_ref: String,
    pub summary: String,
    pub weight: f64,
    pub effective_weight: Option<f64>,
    pub status: String,
    pub created_at: String,
    pub approved_at: Option<String>,
    pub expires_at: Option<String>,
    pub policy: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryTombstoneOutput {
    pub kind: String,
    pub target_id: String,
    pub target_type: String,
    pub tombstoned_at: String,
}

pub fn memory_status(paths: &IssueFinderPaths) -> Result<MemoryStatusOutput> {
    let store = MemoryStore::open(paths)?;
    let decision_eligible_hint_count =
        MemoryControlPlane::decision_eligible_hints(&store, &MemoryDecisionHintRequest::default())?
            .len();
    Ok(MemoryStatusOutput {
        kind: "memory_status".to_string(),
        state_db_path: paths.state_db_path().to_string_lossy().to_string(),
        counts: MemoryCountsOutput {
            raw_events: store.list_raw_events()?.len(),
            nodes: store.list_nodes()?.len(),
            edges: store.list_edges()?.len(),
            dreams: store.list_dreams()?.len(),
            hints: store.list_hints()?.len(),
        },
        decision_eligible_hint_count,
        embedding_status: "disabled".to_string(),
    })
}

pub fn memory_events(
    paths: &IssueFinderPaths,
    issue: Option<String>,
) -> Result<MemoryEventsOutput> {
    let store = MemoryStore::open(paths)?;
    let events = store
        .list_raw_events()?
        .into_iter()
        .filter(|event| {
            issue
                .as_ref()
                .is_none_or(|issue| event.subject_ref == *issue)
        })
        .map(event_output)
        .collect();
    Ok(MemoryEventsOutput {
        kind: "memory_events".to_string(),
        issue,
        events,
    })
}

pub fn memory_recall(
    paths: &IssueFinderPaths,
    issue: &str,
    query_kind: MemoryQueryKind,
    limit: usize,
) -> Result<MemoryRecallOutput> {
    let store = MemoryStore::open(paths)?;
    let now = Utc::now().to_rfc3339();
    let run_id = stable_id(
        "memory-activation",
        &format!("{issue}:{query_kind:?}:{now}"),
    );
    let result = MemoryControlPlane::activate(
        &store,
        &MemoryActivationRequest {
            run_id: run_id.clone(),
            query_kind,
            query_ref: issue.to_string(),
            query_text: issue.to_string(),
            entities: Vec::new(),
            created_at: now.clone(),
            limit: limit.max(1),
            persist_trace: true,
        },
        MemoryRuntimeMode::Enabled,
    )?;
    recall_output(&store, result, query_kind, &now)
}

pub fn memory_dreams_list(paths: &IssueFinderPaths) -> Result<MemoryDreamsOutput> {
    let store = MemoryStore::open(paths)?;
    Ok(MemoryDreamsOutput {
        kind: "memory_dreams".to_string(),
        dreams: store.list_dreams()?.into_iter().map(dream_output).collect(),
    })
}

pub fn memory_dream_show(
    paths: &IssueFinderPaths,
    dream_id: &str,
) -> Result<MemoryDreamDetailOutput> {
    let store = MemoryStore::open(paths)?;
    let dream = store
        .get_dream(dream_id)?
        .with_context(|| format!("memory dream `{dream_id}` not found"))?;
    let hints = store
        .list_hints_for_dream(dream_id)?
        .into_iter()
        .map(|hint| hint_output(hint, None))
        .collect();
    Ok(MemoryDreamDetailOutput {
        kind: "memory_dream".to_string(),
        dream: dream_output(dream),
        hints,
    })
}

pub fn memory_hints_list(paths: &IssueFinderPaths) -> Result<MemoryHintsOutput> {
    let store = MemoryStore::open(paths)?;
    let decision_hints =
        MemoryControlPlane::decision_eligible_hints(&store, &MemoryDecisionHintRequest::default())?;
    let hints = store
        .list_hints()?
        .into_iter()
        .map(|hint| {
            let effective = decision_hints
                .iter()
                .find(|decision| decision.hint.id == hint.id)
                .map(|decision| decision.effective_weight);
            hint_output(hint, effective)
        })
        .collect();
    Ok(MemoryHintsOutput {
        kind: "memory_hints".to_string(),
        hints,
    })
}

pub fn memory_hint_update(
    paths: &IssueFinderPaths,
    hint_id: &str,
    status: MemoryHintStatus,
    reason: Option<&str>,
) -> Result<MemoryHintOutput> {
    let store = MemoryStore::open(paths)?;
    let now = Utc::now().to_rfc3339();
    let hint = store
        .transition_hint_status(hint_id, status, &now, reason)?
        .with_context(|| format!("memory hint `{hint_id}` not found"))?;
    Ok(hint_output(hint, None))
}

pub fn memory_suppress_scope(paths: &IssueFinderPaths, scope: &str) -> Result<MemoryHintOutput> {
    let store = MemoryStore::open(paths)?;
    let (scope_type, scope_ref) = parse_scope(scope)?;
    let now = Utc::now().to_rfc3339();
    let dream_id = ensure_control_dream(&store, &now)?;
    let hint_id = stable_id("memory-suppression", scope);
    if let Some(existing) = store.get_hint(&hint_id)? {
        return Ok(hint_output(existing, None));
    }
    let hint = store.insert_hint(&NewMemoryHint {
        id: hint_id,
        dream_id,
        hint_type: MemoryHintType::Ranking,
        scope_type,
        scope_ref,
        summary: format!("Suppress memory hints for scope `{scope}`."),
        policy_json: json!({"kind": "scope_suppression", "scope": scope}),
        weight: 0.0,
        status: MemoryHintStatus::Suppressed,
        created_at: now,
        approved_at: None,
        expires_at: None,
    })?;
    Ok(hint_output(hint, None))
}

pub fn memory_tombstone(
    paths: &IssueFinderPaths,
    target_id: &str,
) -> Result<MemoryTombstoneOutput> {
    let store = MemoryStore::open(paths)?;
    let now = Utc::now().to_rfc3339();
    let target_type = if store.get_raw_event(target_id)?.is_some() {
        store.tombstone_raw_event(target_id, &now)?;
        "raw_event"
    } else if store.get_node(target_id)?.is_some() {
        store.tombstone_node(target_id, &now)?;
        "node"
    } else if store.get_hint(target_id)?.is_some() {
        store.tombstone_hint(target_id, &now)?;
        "hint"
    } else {
        return Err(anyhow!("memory target `{target_id}` not found"));
    };
    Ok(MemoryTombstoneOutput {
        kind: "memory_tombstone".to_string(),
        target_id: target_id.to_string(),
        target_type: target_type.to_string(),
        tombstoned_at: now,
    })
}

pub fn memory_dream(paths: &IssueFinderPaths, scope: &str) -> Result<MemoryDreamRunOutput> {
    let store = MemoryStore::open(paths)?;
    let now = Utc::now().to_rfc3339();
    let (scope, scope_ref) = parse_dream_scope(scope)?;
    let run_id = stable_id(
        "memory-dream-run",
        &format!("{}:{scope:?}:{scope_ref:?}", now),
    );
    let result = MemoryControlPlane::dream(
        &store,
        &MemoryDreamRequest {
            run_id: run_id.clone(),
            trigger: MemoryDreamTrigger::Manual,
            scope,
            scope_ref,
            input_activation_run_ids: Vec::new(),
            created_at: now,
        },
        None,
        MemoryRuntimeMode::Enabled,
    )?
    .context("memory dreaming is disabled")?;
    Ok(MemoryDreamRunOutput {
        kind: "memory_dream_run".to_string(),
        run_id: result.run.id,
        model_status: result.run.model_status.as_str().to_string(),
        dreams: result.dreams.into_iter().map(dream_output).collect(),
        hints: result
            .hints
            .into_iter()
            .map(|hint| hint_output(hint, None))
            .collect(),
    })
}

pub fn render_memory_status(output: &MemoryStatusOutput) -> String {
    format!(
        "Memory: {} events, {} nodes, {} dreams, {} hints ({} decision-eligible). Embedding: {}.",
        output.counts.raw_events,
        output.counts.nodes,
        output.counts.dreams,
        output.counts.hints,
        output.decision_eligible_hint_count,
        output.embedding_status
    )
}

pub fn render_memory_events(output: &MemoryEventsOutput) -> String {
    if output.events.is_empty() {
        return "No memory events.".to_string();
    }
    output
        .events
        .iter()
        .map(|event| {
            format!(
                "{} {} {} {}",
                event.id, event.event_type, event.subject_type, event.subject_ref
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_memory_recall(output: &MemoryRecallOutput) -> String {
    if output.items.is_empty() {
        return format!("No recalled memory for {}.", output.activation_run_id);
    }
    output
        .items
        .iter()
        .map(|item| format!("#{} {} {:.3}", item.rank, item.node_id, item.final_score))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_memory_dreams(output: &MemoryDreamsOutput) -> String {
    if output.dreams.is_empty() {
        return "No memory dreams.".to_string();
    }
    output
        .dreams
        .iter()
        .map(|dream| format!("{} {} {}", dream.id, dream.dream_type, dream.status))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_memory_dream_detail(output: &MemoryDreamDetailOutput) -> String {
    format!(
        "{} {}\n{}\n{} hints",
        output.dream.id,
        output.dream.status,
        output.dream.summary,
        output.hints.len()
    )
}

pub fn render_memory_hints(output: &MemoryHintsOutput) -> String {
    if output.hints.is_empty() {
        return "No memory hints.".to_string();
    }
    output
        .hints
        .iter()
        .map(|hint| {
            format!(
                "{} {} {} {}",
                hint.id, hint.hint_type, hint.scope_ref, hint.status
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_memory_hint(output: &MemoryHintOutput) -> String {
    format!("{} {} {}", output.id, output.scope_ref, output.status)
}

pub fn render_memory_tombstone(output: &MemoryTombstoneOutput) -> String {
    format!(
        "Tombstoned {} {} at {}.",
        output.target_type, output.target_id, output.tombstoned_at
    )
}

pub fn parse_query_kind(value: &str) -> Result<MemoryQueryKind> {
    match value {
        "scout-ranking" | "scout_ranking" => Ok(MemoryQueryKind::ScoutRanking),
        "dispatch-planning" | "dispatch_planning" => Ok(MemoryQueryKind::DispatchPlanning),
        "github-draft" | "github_draft" => Ok(MemoryQueryKind::GithubDraft),
        "profile-review" | "profile_review" => Ok(MemoryQueryKind::ProfileReview),
        other => Err(anyhow!("unknown memory recall kind `{other}`")),
    }
}

fn recall_output(
    store: &MemoryStore,
    result: MemoryActivationResult,
    query_kind: MemoryQueryKind,
    now: &str,
) -> Result<MemoryRecallOutput> {
    let items = result
        .items
        .into_iter()
        .map(|item| {
            let event_id = store
                .get_node(&item.node_id)?
                .and_then(|node| node.raw_event_id);
            Ok(MemoryRecallItemOutput {
                rank: item.rank,
                node_id: item.node_id,
                event_id,
                source_channel: item.source_channel.as_str().to_string(),
                final_score: item.final_score,
                explanation: item.explanation,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let decision_eligible_hints = MemoryControlPlane::decision_eligible_hints(
        store,
        &MemoryDecisionHintRequest {
            now: Some(now.to_string()),
            ..MemoryDecisionHintRequest::default()
        },
    )?
    .into_iter()
    .map(|decision| hint_output(decision.hint, Some(decision.effective_weight)))
    .collect();
    Ok(MemoryRecallOutput {
        kind: "memory_recall".to_string(),
        activation_run_id: result.run_id,
        query_kind: query_kind.as_str().to_string(),
        items,
        decision_eligible_hints,
    })
}

fn event_output(event: MemoryRawEvent) -> MemoryEventOutput {
    MemoryEventOutput {
        id: event.id,
        event_type: event.event_type.as_str().to_string(),
        role: event.role.as_str().to_string(),
        trust_level: event.trust_level.as_str().to_string(),
        subject_type: event.subject_type.as_str().to_string(),
        subject_ref: event.subject_ref,
        confidence: event.confidence,
        occurred_at: event.occurred_at,
        tombstoned: event.tombstoned_at.is_some(),
    }
}

fn dream_output(dream: MemoryDream) -> MemoryDreamOutput {
    MemoryDreamOutput {
        id: dream.id,
        dream_run_id: dream.dream_run_id,
        dream_type: dream.dream_type.as_str().to_string(),
        summary: dream.summary,
        evidence_node_ids: dream.evidence_node_ids_json,
        evidence_event_ids: dream.evidence_event_ids_json,
        evidence_hint_ids: dream.evidence_hint_ids_json,
        status: dream.status.as_str().to_string(),
        confidence: dream.confidence,
        created_at: dream.created_at,
        reviewed_at: dream.reviewed_at,
    }
}

fn hint_output(hint: MemoryHint, effective_weight: Option<f64>) -> MemoryHintOutput {
    MemoryHintOutput {
        id: hint.id,
        dream_id: hint.dream_id,
        hint_type: hint.hint_type.as_str().to_string(),
        scope_type: hint.scope_type.as_str().to_string(),
        scope_ref: hint.scope_ref,
        summary: hint.summary,
        weight: hint.weight,
        effective_weight,
        status: hint.status.as_str().to_string(),
        created_at: hint.created_at,
        approved_at: hint.approved_at,
        expires_at: hint.expires_at,
        policy: hint.policy_json,
    }
}

fn ensure_control_dream(store: &MemoryStore, now: &str) -> Result<String> {
    let run_id = "memory-control-run".to_string();
    if store.get_dream_run(&run_id)?.is_none() {
        store.insert_dream_run(&crate::memory::model::MemoryDreamRun {
            id: run_id.clone(),
            trigger: MemoryDreamTrigger::Manual,
            scope: MemoryDreamScope::Global,
            input_activation_run_ids_json: json!([]),
            model_status: MemoryModelStatus::Disabled,
            created_at: now.to_string(),
        })?;
    }
    let dream_id = "memory-control-dream".to_string();
    if store.get_dream(&dream_id)?.is_none() {
        store.insert_dream(&NewMemoryDream {
            id: dream_id.clone(),
            dream_run_id: run_id,
            dream_type: crate::memory::model::MemoryDreamType::DiscoveryPolicy,
            summary: "Manual memory control.".to_string(),
            evidence_node_ids_json: json!([]),
            evidence_event_ids_json: json!([]),
            evidence_hint_ids_json: json!([]),
            status: MemoryDreamStatus::Candidate,
            confidence: 1.0,
            version: 1,
            created_at: now.to_string(),
            reviewed_at: None,
        })?;
    }
    Ok(dream_id)
}

fn parse_scope(scope: &str) -> Result<(MemoryHintScopeType, String)> {
    if scope == "global" {
        return Ok((MemoryHintScopeType::Global, "global".to_string()));
    }
    let Some((kind, value)) = scope.split_once(':') else {
        return Err(anyhow!("scope must be global or kind:value"));
    };
    let scope_type = match kind {
        "repo" => MemoryHintScopeType::Repo,
        "agent" => MemoryHintScopeType::Agent,
        "issue_type" | "issue-type" => MemoryHintScopeType::IssueType,
        "maintainer" => MemoryHintScopeType::Maintainer,
        other => return Err(anyhow!("unknown memory scope `{other}`")),
    };
    if value.trim().is_empty() {
        return Err(anyhow!("memory scope value cannot be empty"));
    }
    Ok((scope_type, value.to_string()))
}

fn parse_dream_scope(scope: &str) -> Result<(MemoryDreamScope, Option<String>)> {
    if scope == "global" {
        return Ok((MemoryDreamScope::Global, None));
    }
    let Some((kind, value)) = scope.split_once(':') else {
        return Err(anyhow!("dream scope must be global or kind:value"));
    };
    let scope = match kind {
        "repo" => MemoryDreamScope::Repo,
        "agent" => MemoryDreamScope::Agent,
        "profile" => MemoryDreamScope::Profile,
        "issue_type" | "issue-type" => MemoryDreamScope::IssueType,
        other => return Err(anyhow!("unknown dream scope `{other}`")),
    };
    Ok((scope, Some(value.to_string())))
}

fn stable_id(prefix: &str, value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{prefix}-{hash:016x}")
}
