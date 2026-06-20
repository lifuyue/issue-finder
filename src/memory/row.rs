use anyhow::Result;
use rusqlite::types::Type;
use rusqlite::Row;
use serde_json::Value;

use crate::memory::model::*;

pub(super) fn row_to_source(row: &Row<'_>) -> rusqlite::Result<MemorySource> {
    Ok(MemorySource {
        id: row.get(0)?,
        source_type: parse_source_type_column(row, 1)?,
        source_ref: row.get(2)?,
        trust_level: parse_trust_level_column(row, 3)?,
        created_at: row.get(4)?,
    })
}

pub(super) fn row_to_raw_event(row: &Row<'_>) -> rusqlite::Result<MemoryRawEvent> {
    Ok(MemoryRawEvent {
        id: row.get(0)?,
        source_id: row.get(1)?,
        event_type: parse_raw_event_type_column(row, 2)?,
        role: parse_role_column(row, 3)?,
        trust_level: parse_trust_level_column(row, 4)?,
        subject_type: parse_subject_type_column(row, 5)?,
        subject_ref: row.get(6)?,
        payload_json: parse_json_column(row, 7)?,
        confidence: row.get(8)?,
        occurred_at: row.get(9)?,
        created_at: row.get(10)?,
        tombstoned_at: row.get(11)?,
    })
}

pub(super) fn row_to_node(row: &Row<'_>) -> rusqlite::Result<MemoryNode> {
    Ok(MemoryNode {
        id: row.get(0)?,
        node_type: parse_node_type_column(row, 1)?,
        raw_event_id: row.get(2)?,
        entity_type: row.get(3)?,
        entity_value: row.get(4)?,
        normalized_value: row.get(5)?,
        text_ref: row.get(6)?,
        metadata_json: parse_json_column(row, 7)?,
        created_at: row.get(8)?,
        tombstoned_at: row.get(9)?,
    })
}

pub(super) fn row_to_node_state(row: &Row<'_>) -> rusqlite::Result<MemoryNodeState> {
    Ok(MemoryNodeState {
        node_id: row.get(0)?,
        salience: row.get(1)?,
        strength: row.get(2)?,
        resource: row.get(3)?,
        recall_count: row.get(4)?,
        reinforce_count: row.get(5)?,
        fan_in: row.get(6)?,
        fan_out: row.get(7)?,
        last_recalled_at: row.get(8)?,
        last_reinforced_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

pub(super) fn row_to_index(row: &Row<'_>) -> rusqlite::Result<MemoryIndex> {
    Ok(MemoryIndex {
        node_id: row.get(0)?,
        index_type: parse_index_type_column(row, 1)?,
        index_ref_or_payload: row.get(2)?,
        created_at: row.get(3)?,
    })
}

pub(super) fn row_to_edge(row: &Row<'_>) -> rusqlite::Result<MemoryEdge> {
    Ok(MemoryEdge {
        id: row.get(0)?,
        from_node_id: row.get(1)?,
        to_node_id: row.get(2)?,
        relation: parse_edge_relation_column(row, 3)?,
        strength: row.get(4)?,
        confidence: row.get(5)?,
        evidence_event_ids_json: parse_json_column(row, 6)?,
        last_activated_at: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        tombstoned_at: row.get(10)?,
    })
}

pub(super) fn row_to_activation_run(row: &Row<'_>) -> rusqlite::Result<MemoryActivationRun> {
    Ok(MemoryActivationRun {
        id: row.get(0)?,
        query_kind: parse_query_kind_column(row, 1)?,
        query_ref: row.get(2)?,
        query_json: parse_json_column(row, 3)?,
        created_at: row.get(4)?,
    })
}

pub(super) fn row_to_activation_item(row: &Row<'_>) -> rusqlite::Result<MemoryActivationItem> {
    Ok(MemoryActivationItem {
        run_id: row.get(0)?,
        node_id: row.get(1)?,
        source_channel: parse_source_channel_column(row, 2)?,
        direct_score: row.get(3)?,
        ripple_score: row.get(4)?,
        salience_score: row.get(5)?,
        strength_score: row.get(6)?,
        recency_score: row.get(7)?,
        resource_penalty: row.get(8)?,
        hub_penalty: row.get(9)?,
        role_trust_penalty: row.get(10)?,
        hop_penalty: row.get(11)?,
        stale_penalty: row.get(12)?,
        final_score: row.get(13)?,
        rank: row.get(14)?,
        explanation: row.get(15)?,
    })
}

pub(super) fn row_to_writeback(row: &Row<'_>) -> rusqlite::Result<MemoryWriteback> {
    Ok(MemoryWriteback {
        id: row.get(0)?,
        activation_run_id: row.get(1)?,
        node_id: row.get(2)?,
        action: parse_writeback_action_column(row, 3)?,
        before_json: parse_json_column(row, 4)?,
        after_json: parse_json_column(row, 5)?,
        created_at: row.get(6)?,
    })
}

pub(super) fn row_to_dream_run(row: &Row<'_>) -> rusqlite::Result<MemoryDreamRun> {
    Ok(MemoryDreamRun {
        id: row.get(0)?,
        trigger: parse_dream_trigger_column(row, 1)?,
        scope: parse_dream_scope_column(row, 2)?,
        input_activation_run_ids_json: parse_json_column(row, 3)?,
        model_status: parse_model_status_column(row, 4)?,
        created_at: row.get(5)?,
    })
}

pub(super) fn row_to_dream(row: &Row<'_>) -> rusqlite::Result<MemoryDream> {
    Ok(MemoryDream {
        id: row.get(0)?,
        dream_run_id: row.get(1)?,
        dream_type: parse_dream_type_column(row, 2)?,
        summary: row.get(3)?,
        evidence_node_ids_json: parse_json_column(row, 4)?,
        evidence_event_ids_json: parse_json_column(row, 5)?,
        evidence_hint_ids_json: parse_json_column(row, 6)?,
        status: parse_dream_status_column(row, 7)?,
        confidence: row.get(8)?,
        version: row.get(9)?,
        created_at: row.get(10)?,
        reviewed_at: row.get(11)?,
    })
}

pub(super) fn row_to_hint(row: &Row<'_>) -> rusqlite::Result<MemoryHint> {
    Ok(MemoryHint {
        id: row.get(0)?,
        dream_id: row.get(1)?,
        hint_type: parse_hint_type_column(row, 2)?,
        scope_type: parse_hint_scope_type_column(row, 3)?,
        scope_ref: row.get(4)?,
        summary: row.get(5)?,
        policy_json: parse_json_column(row, 6)?,
        weight: row.get(7)?,
        status: parse_hint_status_column(row, 8)?,
        created_at: row.get(9)?,
        approved_at: row.get(10)?,
        expires_at: row.get(11)?,
    })
}

pub(super) fn row_to_hint_status_change(row: &Row<'_>) -> rusqlite::Result<MemoryHintStatusChange> {
    Ok(MemoryHintStatusChange {
        id: row.get(0)?,
        hint_id: row.get(1)?,
        from_status: parse_hint_status_column(row, 2)?,
        to_status: parse_hint_status_column(row, 3)?,
        changed_at: row.get(4)?,
        reason: row.get(5)?,
    })
}

pub(super) fn collect_rows<T>(
    rows: impl Iterator<Item = rusqlite::Result<T>>,
) -> rusqlite::Result<Vec<T>> {
    rows.collect()
}

pub(super) fn json_to_string(value: &Value) -> Result<String> {
    serde_json::to_string(value).map_err(Into::into)
}

pub(super) fn parse_json_column(row: &Row<'_>, index: usize) -> rusqlite::Result<Value> {
    let raw: String = row.get(index)?;
    serde_json::from_str(&raw).map_err(|error| conversion_error(index, error))
}

pub(super) fn json_array_contains_string(value: &Value, needle: &str) -> bool {
    value
        .as_array()
        .map(|items| items.iter().any(|item| item.as_str() == Some(needle)))
        .unwrap_or(false)
}

fn conversion_error(
    index: usize,
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
}

macro_rules! parse_column {
    ($fn_name:ident, $type_name:ty) => {
        pub(super) fn $fn_name(row: &Row<'_>, index: usize) -> rusqlite::Result<$type_name> {
            let raw: String = row.get(index)?;
            <$type_name>::parse(&raw).map_err(|error| conversion_error(index, error))
        }
    };
}

parse_column!(parse_source_type_column, MemorySourceType);
parse_column!(parse_trust_level_column, MemoryTrustLevel);
parse_column!(parse_raw_event_type_column, MemoryRawEventType);
parse_column!(parse_role_column, MemoryRole);
parse_column!(parse_subject_type_column, MemorySubjectType);
parse_column!(parse_node_type_column, MemoryNodeType);
parse_column!(parse_index_type_column, MemoryIndexType);
parse_column!(parse_edge_relation_column, MemoryEdgeRelation);
parse_column!(parse_query_kind_column, MemoryQueryKind);
parse_column!(parse_source_channel_column, MemorySourceChannel);
parse_column!(parse_writeback_action_column, MemoryWritebackAction);
parse_column!(parse_dream_trigger_column, MemoryDreamTrigger);
parse_column!(parse_dream_scope_column, MemoryDreamScope);
parse_column!(parse_model_status_column, MemoryModelStatus);
parse_column!(parse_dream_type_column, MemoryDreamType);
parse_column!(parse_dream_status_column, MemoryDreamStatus);
parse_column!(parse_hint_type_column, MemoryHintType);
parse_column!(parse_hint_scope_type_column, MemoryHintScopeType);
parse_column!(parse_hint_status_column, MemoryHintStatus);
