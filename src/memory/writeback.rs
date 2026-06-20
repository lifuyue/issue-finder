use anyhow::Result;
use serde_json::{json, Value};

use crate::memory::model::{
    MemoryActivationItem, MemoryEdge, MemoryHintStatus, MemoryNode, MemoryNodeState,
    MemoryNodeType, MemoryRawEvent, MemorySourceChannel, MemoryTrustLevel, MemoryWriteback,
    MemoryWritebackAction,
};
use crate::memory::store::MemoryStore;

const RESOURCE_SPEND: f64 = 0.2;
const STRENGTH_GAIN: f64 = 0.25;
const EDGE_STRENGTH_GAIN: f64 = 0.15;
const EDGE_CONFIDENCE_GAIN: f64 = 0.05;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryWritebackReport {
    pub activation_run_id: String,
    pub recalled: usize,
    pub resource_decremented: usize,
    pub reinforced: usize,
    pub edge_reinforced: usize,
    pub skipped: usize,
}

pub struct MemoryWritebackGuard;

impl MemoryWritebackGuard {
    pub fn apply(
        store: &MemoryStore,
        activation_run_id: &str,
        occurred_at: &str,
    ) -> Result<MemoryWritebackReport> {
        let items = store.list_activation_items(activation_run_id)?;
        let mut report = MemoryWritebackReport {
            activation_run_id: activation_run_id.to_string(),
            recalled: 0,
            resource_decremented: 0,
            reinforced: 0,
            edge_reinforced: 0,
            skipped: 0,
        };

        for item in items {
            let Some(node) = store.get_node(&item.node_id)? else {
                report.skipped += 1;
                continue;
            };
            if !node_can_write_back(store, &node)? {
                report.skipped += 1;
                continue;
            }

            let mut state = state_or_default(store, &node.id, occurred_at)?;
            if apply_recalled(store, activation_run_id, &item, &mut state, occurred_at)? {
                report.recalled += 1;
            }
            if apply_resource_decrement(store, activation_run_id, &item, &mut state, occurred_at)? {
                report.resource_decremented += 1;
            }

            if can_reinforce(store, &node, &item)? {
                if apply_reinforced(store, activation_run_id, &item, &mut state, occurred_at)? {
                    report.reinforced += 1;
                }
                report.edge_reinforced +=
                    reinforce_related_edges(store, activation_run_id, &item, occurred_at)?;
            } else {
                report.skipped += 1;
            }
        }

        Ok(report)
    }
}

fn apply_recalled(
    store: &MemoryStore,
    activation_run_id: &str,
    item: &MemoryActivationItem,
    state: &mut MemoryNodeState,
    occurred_at: &str,
) -> Result<bool> {
    let writeback_id = writeback_id(
        activation_run_id,
        &item.node_id,
        MemoryWritebackAction::Recalled,
    );
    if store.get_writeback(&writeback_id)?.is_some() {
        return Ok(false);
    }

    let before = state_json(state);
    state.recall_count += 1;
    state.last_recalled_at = Some(occurred_at.to_string());
    state.updated_at = occurred_at.to_string();
    store.upsert_node_state(state)?;
    let after = state_json(state);
    insert_writeback(
        store,
        WritebackRecord {
            id: &writeback_id,
            activation_run_id,
            node_id: &item.node_id,
            action: MemoryWritebackAction::Recalled,
            before_json: before,
            after_json: after,
            created_at: occurred_at,
        },
    )?;
    Ok(true)
}

fn apply_resource_decrement(
    store: &MemoryStore,
    activation_run_id: &str,
    item: &MemoryActivationItem,
    state: &mut MemoryNodeState,
    occurred_at: &str,
) -> Result<bool> {
    let writeback_id = writeback_id(
        activation_run_id,
        &item.node_id,
        MemoryWritebackAction::ResourceDecremented,
    );
    if store.get_writeback(&writeback_id)?.is_some() {
        return Ok(false);
    }

    let before = state_json(state);
    state.resource = (state.resource - RESOURCE_SPEND).max(0.0);
    state.updated_at = occurred_at.to_string();
    store.upsert_node_state(state)?;
    let after = state_json(state);
    insert_writeback(
        store,
        WritebackRecord {
            id: &writeback_id,
            activation_run_id,
            node_id: &item.node_id,
            action: MemoryWritebackAction::ResourceDecremented,
            before_json: before,
            after_json: after,
            created_at: occurred_at,
        },
    )?;
    Ok(true)
}

fn apply_reinforced(
    store: &MemoryStore,
    activation_run_id: &str,
    item: &MemoryActivationItem,
    state: &mut MemoryNodeState,
    occurred_at: &str,
) -> Result<bool> {
    let writeback_id = writeback_id(
        activation_run_id,
        &item.node_id,
        MemoryWritebackAction::Reinforced,
    );
    if store.get_writeback(&writeback_id)?.is_some() {
        return Ok(false);
    }

    let before = state_json(state);
    state.strength += STRENGTH_GAIN;
    state.reinforce_count += 1;
    state.last_reinforced_at = Some(occurred_at.to_string());
    state.updated_at = occurred_at.to_string();
    store.upsert_node_state(state)?;
    let after = state_json(state);
    insert_writeback(
        store,
        WritebackRecord {
            id: &writeback_id,
            activation_run_id,
            node_id: &item.node_id,
            action: MemoryWritebackAction::Reinforced,
            before_json: before,
            after_json: after,
            created_at: occurred_at,
        },
    )?;
    Ok(true)
}

fn reinforce_related_edges(
    store: &MemoryStore,
    activation_run_id: &str,
    item: &MemoryActivationItem,
    occurred_at: &str,
) -> Result<usize> {
    if item.ripple_score <= 0.0 {
        return Ok(0);
    }

    let mut count = 0;
    for edge in store
        .list_edges()?
        .into_iter()
        .filter(|edge| edge.tombstoned_at.is_none())
        .filter(|edge| edge.from_node_id == item.node_id || edge.to_node_id == item.node_id)
    {
        let writeback_id = edge_writeback_id(activation_run_id, &item.node_id, &edge.id);
        if store.get_writeback(&writeback_id)?.is_some() {
            continue;
        }
        let before = edge_json(&edge);
        let updated = update_edge(store, &edge, occurred_at)?;
        let after = edge_json(&updated);
        insert_writeback(
            store,
            WritebackRecord {
                id: &writeback_id,
                activation_run_id,
                node_id: &item.node_id,
                action: MemoryWritebackAction::EdgeReinforced,
                before_json: before,
                after_json: after,
                created_at: occurred_at,
            },
        )?;
        count += 1;
    }
    Ok(count)
}

fn update_edge(store: &MemoryStore, edge: &MemoryEdge, occurred_at: &str) -> Result<MemoryEdge> {
    Ok(store
        .update_edge_state(
            &edge.id,
            edge.strength + EDGE_STRENGTH_GAIN,
            (edge.confidence + EDGE_CONFIDENCE_GAIN).min(1.0),
            Some(occurred_at),
            occurred_at,
        )?
        .expect("edge exists before update"))
}

fn can_reinforce(
    store: &MemoryStore,
    node: &MemoryNode,
    item: &MemoryActivationItem,
) -> Result<bool> {
    if item.rank != 1 || item.direct_score <= 0.0 {
        return Ok(false);
    }
    if matches!(
        item.source_channel,
        MemorySourceChannel::NearRipple | MemorySourceChannel::FarRipple
    ) {
        return Ok(false);
    }
    if item.hub_penalty > 0.0 || item.stale_penalty > 0.0 || item.role_trust_penalty >= 1.0 {
        return Ok(false);
    }
    if matches!(node.node_type, MemoryNodeType::Hint) && hint_node_is_suppressed(node) {
        return Ok(false);
    }
    if let Some(raw_event) = raw_event_for_node(store, node)? {
        if raw_event.tombstoned_at.is_some()
            || raw_event.trust_level == MemoryTrustLevel::LlmInferred
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn node_can_write_back(store: &MemoryStore, node: &MemoryNode) -> Result<bool> {
    if node.tombstoned_at.is_some() || hint_node_is_suppressed(node) {
        return Ok(false);
    }
    if let Some(raw_event) = raw_event_for_node(store, node)? {
        return Ok(raw_event.tombstoned_at.is_none());
    }
    Ok(true)
}

fn hint_node_is_suppressed(node: &MemoryNode) -> bool {
    node.metadata_json
        .get("status")
        .and_then(|value| value.as_str())
        .and_then(|value| MemoryHintStatus::parse(value).ok())
        == Some(MemoryHintStatus::Suppressed)
}

fn raw_event_for_node(store: &MemoryStore, node: &MemoryNode) -> Result<Option<MemoryRawEvent>> {
    match node.raw_event_id.as_deref() {
        Some(raw_event_id) => store.get_raw_event(raw_event_id),
        None => Ok(None),
    }
}

fn state_or_default(
    store: &MemoryStore,
    node_id: &str,
    updated_at: &str,
) -> Result<MemoryNodeState> {
    Ok(store
        .get_node_state(node_id)?
        .unwrap_or_else(|| MemoryNodeState {
            node_id: node_id.to_string(),
            salience: 0.0,
            strength: 0.0,
            resource: 1.0,
            recall_count: 0,
            reinforce_count: 0,
            fan_in: 0,
            fan_out: 0,
            last_recalled_at: None,
            last_reinforced_at: None,
            updated_at: updated_at.to_string(),
        }))
}

struct WritebackRecord<'a> {
    id: &'a str,
    activation_run_id: &'a str,
    node_id: &'a str,
    action: MemoryWritebackAction,
    before_json: Value,
    after_json: Value,
    created_at: &'a str,
}

fn insert_writeback(store: &MemoryStore, record: WritebackRecord<'_>) -> Result<()> {
    store.insert_writeback(&MemoryWriteback {
        id: record.id.to_string(),
        activation_run_id: record.activation_run_id.to_string(),
        node_id: record.node_id.to_string(),
        action: record.action,
        before_json: record.before_json,
        after_json: record.after_json,
        created_at: record.created_at.to_string(),
    })?;
    Ok(())
}

fn state_json(state: &MemoryNodeState) -> Value {
    json!({
        "nodeId": state.node_id,
        "salience": state.salience,
        "strength": state.strength,
        "resource": state.resource,
        "recallCount": state.recall_count,
        "reinforceCount": state.reinforce_count,
        "fanIn": state.fan_in,
        "fanOut": state.fan_out,
        "lastRecalledAt": state.last_recalled_at,
        "lastReinforcedAt": state.last_reinforced_at,
        "updatedAt": state.updated_at,
    })
}

fn edge_json(edge: &MemoryEdge) -> Value {
    json!({
        "id": edge.id,
        "fromNodeId": edge.from_node_id,
        "toNodeId": edge.to_node_id,
        "relation": edge.relation.as_str(),
        "strength": edge.strength,
        "confidence": edge.confidence,
        "lastActivatedAt": edge.last_activated_at,
        "updatedAt": edge.updated_at,
    })
}

fn writeback_id(activation_run_id: &str, node_id: &str, action: MemoryWritebackAction) -> String {
    format!(
        "memory-writeback-{:016x}",
        stable_hash(format!("{activation_run_id}:{node_id}:{}", action.as_str()).as_bytes())
    )
}

fn edge_writeback_id(activation_run_id: &str, node_id: &str, edge_id: &str) -> String {
    format!(
        "memory-writeback-{:016x}",
        stable_hash(format!("{activation_run_id}:{node_id}:edge:{edge_id}").as_bytes())
    )
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
