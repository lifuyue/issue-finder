use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::memory::hints::validate_hint_transition;
use crate::memory::model::*;
use crate::memory::row::{
    collect_rows, json_array_contains_string, json_to_string, parse_hint_status_column,
    parse_json_column, row_to_activation_item, row_to_activation_run, row_to_dream,
    row_to_dream_run, row_to_edge, row_to_hint, row_to_hint_status_change, row_to_index,
    row_to_node, row_to_node_state, row_to_raw_event, row_to_source, row_to_writeback,
};
use crate::memory::schema::SCHEMA;
use crate::paths::IssueFinderPaths;

pub struct MemoryStore {
    conn: Connection,
}

impl MemoryStore {
    pub fn open(paths: &IssueFinderPaths) -> Result<Self> {
        Self::open_at(paths.state_db_path())
    }

    pub fn open_at(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("unable to create {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("unable to open state database {}", path.display()))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    pub fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA)?;
        Ok(())
    }

    pub fn insert_source(&self, source: &NewMemorySource) -> Result<MemorySource> {
        self.conn.execute(
            "INSERT INTO memory_sources (id, source_type, source_ref, trust_level, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                source.id,
                source.source_type.as_str(),
                source.source_ref,
                source.trust_level.as_str(),
                source.created_at,
            ],
        )?;
        Ok(MemorySource {
            id: source.id.clone(),
            source_type: source.source_type,
            source_ref: source.source_ref.clone(),
            trust_level: source.trust_level,
            created_at: source.created_at.clone(),
        })
    }

    pub fn get_source(&self, id: &str) -> Result<Option<MemorySource>> {
        self.conn
            .query_row(
                "SELECT id, source_type, source_ref, trust_level, created_at
                 FROM memory_sources WHERE id = ?1",
                params![id],
                row_to_source,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn insert_raw_event(&self, event: &NewMemoryRawEvent) -> Result<MemoryRawEvent> {
        self.conn.execute(
            "INSERT INTO memory_raw_events (
                id, source_id, event_type, role, trust_level, subject_type, subject_ref,
                payload_json, confidence, occurred_at, created_at, tombstoned_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL)",
            params![
                event.id,
                event.source_id,
                event.event_type.as_str(),
                event.role.as_str(),
                event.trust_level.as_str(),
                event.subject_type.as_str(),
                event.subject_ref,
                json_to_string(&event.payload_json)?,
                event.confidence,
                event.occurred_at,
                event.created_at,
            ],
        )?;
        Ok(MemoryRawEvent {
            id: event.id.clone(),
            source_id: event.source_id.clone(),
            event_type: event.event_type,
            role: event.role,
            trust_level: event.trust_level,
            subject_type: event.subject_type,
            subject_ref: event.subject_ref.clone(),
            payload_json: event.payload_json.clone(),
            confidence: event.confidence,
            occurred_at: event.occurred_at.clone(),
            created_at: event.created_at.clone(),
            tombstoned_at: None,
        })
    }

    pub fn get_raw_event(&self, id: &str) -> Result<Option<MemoryRawEvent>> {
        self.conn
            .query_row(
                "SELECT id, source_id, event_type, role, trust_level, subject_type, subject_ref,
                        payload_json, confidence, occurred_at, created_at, tombstoned_at
                 FROM memory_raw_events WHERE id = ?1",
                params![id],
                row_to_raw_event,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_raw_events(&self) -> Result<Vec<MemoryRawEvent>> {
        let mut statement = self.conn.prepare(
            "SELECT id, source_id, event_type, role, trust_level, subject_type, subject_ref,
                    payload_json, confidence, occurred_at, created_at, tombstoned_at
             FROM memory_raw_events ORDER BY created_at, id",
        )?;
        let rows = statement.query_map([], row_to_raw_event)?;
        let events = collect_rows(rows)?;
        Ok(events)
    }

    pub fn tombstone_raw_event(&self, id: &str, tombstoned_at: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE memory_raw_events
             SET tombstoned_at = COALESCE(tombstoned_at, ?2)
             WHERE id = ?1",
            params![id, tombstoned_at],
        )?;

        let node_ids = self.node_ids_for_raw_event(id)?;
        self.tombstone_nodes_by_id(&node_ids, tombstoned_at)?;
        self.tombstone_edges_with_event_evidence(id, tombstoned_at)?;
        self.tombstone_dreams_with_event_evidence(id, tombstoned_at)?;
        Ok(())
    }

    pub fn insert_node(&self, node: &NewMemoryNode) -> Result<MemoryNode> {
        self.conn.execute(
            "INSERT INTO memory_nodes (
                id, node_type, raw_event_id, entity_type, entity_value, normalized_value,
                text_ref, metadata_json, created_at, tombstoned_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
            params![
                node.id,
                node.node_type.as_str(),
                node.raw_event_id,
                node.entity_type,
                node.entity_value,
                node.normalized_value,
                node.text_ref,
                json_to_string(&node.metadata_json)?,
                node.created_at,
            ],
        )?;
        Ok(MemoryNode {
            id: node.id.clone(),
            node_type: node.node_type,
            raw_event_id: node.raw_event_id.clone(),
            entity_type: node.entity_type.clone(),
            entity_value: node.entity_value.clone(),
            normalized_value: node.normalized_value.clone(),
            text_ref: node.text_ref.clone(),
            metadata_json: node.metadata_json.clone(),
            created_at: node.created_at.clone(),
            tombstoned_at: None,
        })
    }

    pub fn get_node(&self, id: &str) -> Result<Option<MemoryNode>> {
        self.conn
            .query_row(
                "SELECT id, node_type, raw_event_id, entity_type, entity_value, normalized_value,
                        text_ref, metadata_json, created_at, tombstoned_at
                 FROM memory_nodes WHERE id = ?1",
                params![id],
                row_to_node,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_nodes_for_raw_event(&self, raw_event_id: &str) -> Result<Vec<MemoryNode>> {
        let mut statement = self.conn.prepare(
            "SELECT id, node_type, raw_event_id, entity_type, entity_value, normalized_value,
                    text_ref, metadata_json, created_at, tombstoned_at
             FROM memory_nodes WHERE raw_event_id = ?1 ORDER BY node_type, id",
        )?;
        let rows = statement.query_map(params![raw_event_id], row_to_node)?;
        let nodes = collect_rows(rows)?;
        Ok(nodes)
    }

    pub fn list_nodes(&self) -> Result<Vec<MemoryNode>> {
        let mut statement = self.conn.prepare(
            "SELECT id, node_type, raw_event_id, entity_type, entity_value, normalized_value,
                    text_ref, metadata_json, created_at, tombstoned_at
             FROM memory_nodes ORDER BY created_at, id",
        )?;
        let rows = statement.query_map([], row_to_node)?;
        let nodes = collect_rows(rows)?;
        Ok(nodes)
    }

    pub fn tombstone_node(&self, id: &str, tombstoned_at: &str) -> Result<()> {
        self.tombstone_nodes_by_id(&[id.to_string()], tombstoned_at)
    }

    pub fn insert_node_state(&self, state: &MemoryNodeState) -> Result<MemoryNodeState> {
        self.conn.execute(
            "INSERT INTO memory_node_state (
                node_id, salience, strength, resource, recall_count, reinforce_count,
                fan_in, fan_out, last_recalled_at, last_reinforced_at, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                state.node_id,
                state.salience,
                state.strength,
                state.resource,
                state.recall_count,
                state.reinforce_count,
                state.fan_in,
                state.fan_out,
                state.last_recalled_at,
                state.last_reinforced_at,
                state.updated_at,
            ],
        )?;
        Ok(state.clone())
    }

    pub fn upsert_node_state(&self, state: &MemoryNodeState) -> Result<MemoryNodeState> {
        self.conn.execute(
            "INSERT INTO memory_node_state (
                node_id, salience, strength, resource, recall_count, reinforce_count,
                fan_in, fan_out, last_recalled_at, last_reinforced_at, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(node_id) DO UPDATE SET
                salience = excluded.salience,
                strength = excluded.strength,
                resource = excluded.resource,
                recall_count = excluded.recall_count,
                reinforce_count = excluded.reinforce_count,
                fan_in = excluded.fan_in,
                fan_out = excluded.fan_out,
                last_recalled_at = excluded.last_recalled_at,
                last_reinforced_at = excluded.last_reinforced_at,
                updated_at = excluded.updated_at",
            params![
                state.node_id,
                state.salience,
                state.strength,
                state.resource,
                state.recall_count,
                state.reinforce_count,
                state.fan_in,
                state.fan_out,
                state.last_recalled_at,
                state.last_reinforced_at,
                state.updated_at,
            ],
        )?;
        Ok(state.clone())
    }

    pub fn get_node_state(&self, node_id: &str) -> Result<Option<MemoryNodeState>> {
        self.conn
            .query_row(
                "SELECT node_id, salience, strength, resource, recall_count, reinforce_count,
                        fan_in, fan_out, last_recalled_at, last_reinforced_at, updated_at
                 FROM memory_node_state WHERE node_id = ?1",
                params![node_id],
                row_to_node_state,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn insert_index(&self, index: &MemoryIndex) -> Result<MemoryIndex> {
        self.conn.execute(
            "INSERT INTO memory_indexes (node_id, index_type, index_ref_or_payload, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                index.node_id,
                index.index_type.as_str(),
                index.index_ref_or_payload,
                index.created_at,
            ],
        )?;
        Ok(index.clone())
    }

    pub fn list_indexes_for_node(&self, node_id: &str) -> Result<Vec<MemoryIndex>> {
        let mut statement = self.conn.prepare(
            "SELECT node_id, index_type, index_ref_or_payload, created_at
             FROM memory_indexes WHERE node_id = ?1 ORDER BY index_type, index_ref_or_payload",
        )?;
        let rows = statement.query_map(params![node_id], row_to_index)?;
        let indexes = collect_rows(rows)?;
        Ok(indexes)
    }

    pub fn clear_indexes(&self) -> Result<()> {
        self.conn.execute("DELETE FROM memory_indexes", [])?;
        Ok(())
    }

    pub fn find_indexes(
        &self,
        index_type: MemoryIndexType,
        index_ref_or_payload: &str,
    ) -> Result<Vec<MemoryIndex>> {
        let mut statement = self.conn.prepare(
            "SELECT node_id, index_type, index_ref_or_payload, created_at
             FROM memory_indexes
             WHERE index_type = ?1 AND index_ref_or_payload = ?2
             ORDER BY node_id",
        )?;
        let rows = statement.query_map(
            params![index_type.as_str(), index_ref_or_payload],
            row_to_index,
        )?;
        let indexes = collect_rows(rows)?;
        Ok(indexes)
    }

    pub fn insert_edge(&self, edge: &NewMemoryEdge) -> Result<MemoryEdge> {
        self.conn.execute(
            "INSERT INTO memory_edges (
                id, from_node_id, to_node_id, relation, strength, confidence,
                evidence_event_ids_json, last_activated_at, created_at, updated_at, tombstoned_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL)",
            params![
                edge.id,
                edge.from_node_id,
                edge.to_node_id,
                edge.relation.as_str(),
                edge.strength,
                edge.confidence,
                json_to_string(&edge.evidence_event_ids_json)?,
                edge.last_activated_at,
                edge.created_at,
                edge.updated_at,
            ],
        )?;
        Ok(MemoryEdge {
            id: edge.id.clone(),
            from_node_id: edge.from_node_id.clone(),
            to_node_id: edge.to_node_id.clone(),
            relation: edge.relation,
            strength: edge.strength,
            confidence: edge.confidence,
            evidence_event_ids_json: edge.evidence_event_ids_json.clone(),
            last_activated_at: edge.last_activated_at.clone(),
            created_at: edge.created_at.clone(),
            updated_at: edge.updated_at.clone(),
            tombstoned_at: None,
        })
    }

    pub fn get_edge(&self, id: &str) -> Result<Option<MemoryEdge>> {
        self.conn
            .query_row(
                "SELECT id, from_node_id, to_node_id, relation, strength, confidence,
                        evidence_event_ids_json, last_activated_at, created_at, updated_at,
                        tombstoned_at
                 FROM memory_edges WHERE id = ?1",
                params![id],
                row_to_edge,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_edges(&self) -> Result<Vec<MemoryEdge>> {
        let mut statement = self.conn.prepare(
            "SELECT id, from_node_id, to_node_id, relation, strength, confidence,
                    evidence_event_ids_json, last_activated_at, created_at, updated_at,
                    tombstoned_at
             FROM memory_edges ORDER BY created_at, id",
        )?;
        let rows = statement.query_map([], row_to_edge)?;
        let edges = collect_rows(rows)?;
        Ok(edges)
    }

    pub fn delete_edges_by_relation(&self, relation: MemoryEdgeRelation) -> Result<()> {
        self.conn.execute(
            "DELETE FROM memory_edges WHERE relation = ?1",
            params![relation.as_str()],
        )?;
        Ok(())
    }

    pub fn update_edge_state(
        &self,
        id: &str,
        strength: f64,
        confidence: f64,
        last_activated_at: Option<&str>,
        updated_at: &str,
    ) -> Result<Option<MemoryEdge>> {
        self.conn.execute(
            "UPDATE memory_edges
             SET strength = ?2, confidence = ?3, last_activated_at = ?4, updated_at = ?5
             WHERE id = ?1",
            params![id, strength, confidence, last_activated_at, updated_at],
        )?;
        self.get_edge(id)
    }

    pub fn insert_activation_run(&self, run: &MemoryActivationRun) -> Result<MemoryActivationRun> {
        self.conn.execute(
            "INSERT INTO memory_activation_runs (id, query_kind, query_ref, query_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                run.id,
                run.query_kind.as_str(),
                run.query_ref,
                json_to_string(&run.query_json)?,
                run.created_at,
            ],
        )?;
        Ok(run.clone())
    }

    pub fn get_activation_run(&self, id: &str) -> Result<Option<MemoryActivationRun>> {
        self.conn
            .query_row(
                "SELECT id, query_kind, query_ref, query_json, created_at
                 FROM memory_activation_runs WHERE id = ?1",
                params![id],
                row_to_activation_run,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn insert_activation_item(
        &self,
        item: &MemoryActivationItem,
    ) -> Result<MemoryActivationItem> {
        self.conn.execute(
            "INSERT INTO memory_activation_items (
                run_id, node_id, source_channel, direct_score, ripple_score, salience_score,
                strength_score, recency_score, resource_penalty, hub_penalty,
                role_trust_penalty, hop_penalty, stale_penalty, final_score, rank, explanation
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                item.run_id,
                item.node_id,
                item.source_channel.as_str(),
                item.direct_score,
                item.ripple_score,
                item.salience_score,
                item.strength_score,
                item.recency_score,
                item.resource_penalty,
                item.hub_penalty,
                item.role_trust_penalty,
                item.hop_penalty,
                item.stale_penalty,
                item.final_score,
                item.rank,
                item.explanation,
            ],
        )?;
        Ok(item.clone())
    }

    pub fn list_activation_items(&self, run_id: &str) -> Result<Vec<MemoryActivationItem>> {
        let mut statement = self.conn.prepare(
            "SELECT run_id, node_id, source_channel, direct_score, ripple_score, salience_score,
                    strength_score, recency_score, resource_penalty, hub_penalty,
                    role_trust_penalty, hop_penalty, stale_penalty, final_score, rank, explanation
             FROM memory_activation_items WHERE run_id = ?1 ORDER BY rank, node_id",
        )?;
        let rows = statement.query_map(params![run_id], row_to_activation_item)?;
        let items = collect_rows(rows)?;
        Ok(items)
    }

    pub fn insert_writeback(&self, writeback: &MemoryWriteback) -> Result<MemoryWriteback> {
        self.conn.execute(
            "INSERT INTO memory_writebacks (
                id, activation_run_id, node_id, action, before_json, after_json, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                writeback.id,
                writeback.activation_run_id,
                writeback.node_id,
                writeback.action.as_str(),
                json_to_string(&writeback.before_json)?,
                json_to_string(&writeback.after_json)?,
                writeback.created_at,
            ],
        )?;
        Ok(writeback.clone())
    }

    pub fn get_writeback(&self, id: &str) -> Result<Option<MemoryWriteback>> {
        self.conn
            .query_row(
                "SELECT id, activation_run_id, node_id, action, before_json, after_json, created_at
                 FROM memory_writebacks WHERE id = ?1",
                params![id],
                row_to_writeback,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_writebacks_for_run(&self, activation_run_id: &str) -> Result<Vec<MemoryWriteback>> {
        let mut statement = self.conn.prepare(
            "SELECT id, activation_run_id, node_id, action, before_json, after_json, created_at
             FROM memory_writebacks
             WHERE activation_run_id = ?1
             ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![activation_run_id], row_to_writeback)?;
        let writebacks = collect_rows(rows)?;
        Ok(writebacks)
    }

    pub fn insert_dream_run(&self, run: &MemoryDreamRun) -> Result<MemoryDreamRun> {
        self.conn.execute(
            "INSERT INTO memory_dream_runs (
                id, trigger, scope, input_activation_run_ids_json, model_status, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                run.id,
                run.trigger.as_str(),
                run.scope.as_str(),
                json_to_string(&run.input_activation_run_ids_json)?,
                run.model_status.as_str(),
                run.created_at,
            ],
        )?;
        Ok(run.clone())
    }

    pub fn get_dream_run(&self, id: &str) -> Result<Option<MemoryDreamRun>> {
        self.conn
            .query_row(
                "SELECT id, trigger, scope, input_activation_run_ids_json, model_status, created_at
                 FROM memory_dream_runs WHERE id = ?1",
                params![id],
                row_to_dream_run,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn insert_dream(&self, dream: &NewMemoryDream) -> Result<MemoryDream> {
        self.conn.execute(
            "INSERT INTO memory_dreams (
                id, dream_run_id, dream_type, summary, evidence_node_ids_json,
                evidence_event_ids_json, evidence_hint_ids_json, status, confidence, version,
                created_at, reviewed_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                dream.id,
                dream.dream_run_id,
                dream.dream_type.as_str(),
                dream.summary,
                json_to_string(&dream.evidence_node_ids_json)?,
                json_to_string(&dream.evidence_event_ids_json)?,
                json_to_string(&dream.evidence_hint_ids_json)?,
                dream.status.as_str(),
                dream.confidence,
                dream.version,
                dream.created_at,
                dream.reviewed_at,
            ],
        )?;
        Ok(MemoryDream {
            id: dream.id.clone(),
            dream_run_id: dream.dream_run_id.clone(),
            dream_type: dream.dream_type,
            summary: dream.summary.clone(),
            evidence_node_ids_json: dream.evidence_node_ids_json.clone(),
            evidence_event_ids_json: dream.evidence_event_ids_json.clone(),
            evidence_hint_ids_json: dream.evidence_hint_ids_json.clone(),
            status: dream.status,
            confidence: dream.confidence,
            version: dream.version,
            created_at: dream.created_at.clone(),
            reviewed_at: dream.reviewed_at.clone(),
        })
    }

    pub fn get_dream(&self, id: &str) -> Result<Option<MemoryDream>> {
        self.conn
            .query_row(
                "SELECT id, dream_run_id, dream_type, summary, evidence_node_ids_json,
                        evidence_event_ids_json, evidence_hint_ids_json, status, confidence,
                        version, created_at, reviewed_at
                 FROM memory_dreams WHERE id = ?1",
                params![id],
                row_to_dream,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_dreams_for_run(&self, dream_run_id: &str) -> Result<Vec<MemoryDream>> {
        let mut statement = self.conn.prepare(
            "SELECT id, dream_run_id, dream_type, summary, evidence_node_ids_json,
                    evidence_event_ids_json, evidence_hint_ids_json, status, confidence,
                    version, created_at, reviewed_at
             FROM memory_dreams WHERE dream_run_id = ?1 ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![dream_run_id], row_to_dream)?;
        let dreams = collect_rows(rows)?;
        Ok(dreams)
    }

    pub fn list_dreams(&self) -> Result<Vec<MemoryDream>> {
        let mut statement = self.conn.prepare(
            "SELECT id, dream_run_id, dream_type, summary, evidence_node_ids_json,
                    evidence_event_ids_json, evidence_hint_ids_json, status, confidence,
                    version, created_at, reviewed_at
             FROM memory_dreams ORDER BY created_at, id",
        )?;
        let rows = statement.query_map([], row_to_dream)?;
        let dreams = collect_rows(rows)?;
        Ok(dreams)
    }

    pub fn insert_hint(&self, hint: &NewMemoryHint) -> Result<MemoryHint> {
        self.conn.execute(
            "INSERT INTO memory_hints (
                id, dream_id, hint_type, scope_type, scope_ref, summary, policy_json, weight,
                status, created_at, approved_at, expires_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                hint.id,
                hint.dream_id,
                hint.hint_type.as_str(),
                hint.scope_type.as_str(),
                hint.scope_ref,
                hint.summary,
                json_to_string(&hint.policy_json)?,
                hint.weight,
                hint.status.as_str(),
                hint.created_at,
                hint.approved_at,
                hint.expires_at,
            ],
        )?;
        Ok(MemoryHint {
            id: hint.id.clone(),
            dream_id: hint.dream_id.clone(),
            hint_type: hint.hint_type,
            scope_type: hint.scope_type,
            scope_ref: hint.scope_ref.clone(),
            summary: hint.summary.clone(),
            policy_json: hint.policy_json.clone(),
            weight: hint.weight,
            status: hint.status,
            created_at: hint.created_at.clone(),
            approved_at: hint.approved_at.clone(),
            expires_at: hint.expires_at.clone(),
        })
    }

    pub fn get_hint(&self, id: &str) -> Result<Option<MemoryHint>> {
        self.conn
            .query_row(
                "SELECT id, dream_id, hint_type, scope_type, scope_ref, summary, policy_json,
                        weight, status, created_at, approved_at, expires_at
                 FROM memory_hints WHERE id = ?1",
                params![id],
                row_to_hint,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_hints(&self) -> Result<Vec<MemoryHint>> {
        let mut statement = self.conn.prepare(
            "SELECT id, dream_id, hint_type, scope_type, scope_ref, summary, policy_json,
                    weight, status, created_at, approved_at, expires_at
             FROM memory_hints ORDER BY created_at, id",
        )?;
        let rows = statement.query_map([], row_to_hint)?;
        let hints = collect_rows(rows)?;
        Ok(hints)
    }

    pub fn list_hints_for_dream(&self, dream_id: &str) -> Result<Vec<MemoryHint>> {
        let mut statement = self.conn.prepare(
            "SELECT id, dream_id, hint_type, scope_type, scope_ref, summary, policy_json,
                    weight, status, created_at, approved_at, expires_at
             FROM memory_hints WHERE dream_id = ?1 ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![dream_id], row_to_hint)?;
        let hints = collect_rows(rows)?;
        Ok(hints)
    }

    pub fn transition_hint_status(
        &self,
        id: &str,
        target: MemoryHintStatus,
        changed_at: &str,
        reason: Option<&str>,
    ) -> Result<Option<MemoryHint>> {
        let Some(hint) = self.get_hint(id)? else {
            return Ok(None);
        };
        validate_hint_transition(hint.status, target)?;
        if hint.status == target {
            return Ok(Some(hint));
        }

        let approved_at = if target.is_active_decision_status() && hint.approved_at.is_none() {
            Some(changed_at)
        } else {
            hint.approved_at.as_deref()
        };

        self.conn.execute(
            "UPDATE memory_hints SET status = ?2, approved_at = ?3 WHERE id = ?1",
            params![id, target.as_str(), approved_at],
        )?;
        self.insert_hint_status_change(id, hint.status, target, changed_at, reason)?;
        self.get_hint(id)
    }

    pub fn restore_hint_prior_status(
        &self,
        id: &str,
        changed_at: &str,
        reason: Option<&str>,
    ) -> Result<Option<MemoryHint>> {
        let Some(hint) = self.get_hint(id)? else {
            return Ok(None);
        };
        if hint.status == MemoryHintStatus::Tombstoned {
            validate_hint_transition(hint.status, MemoryHintStatus::Approved)?;
        }

        let prior = self
            .conn
            .query_row(
                "SELECT from_status FROM memory_hint_status_changes
                 WHERE hint_id = ?1 ORDER BY id DESC LIMIT 1",
                params![id],
                |row| parse_hint_status_column(row, 0),
            )
            .optional()?;

        let Some(target) = prior else {
            return Ok(Some(hint));
        };
        let approved_at = if target.is_active_decision_status() && hint.approved_at.is_none() {
            Some(changed_at)
        } else {
            hint.approved_at.as_deref()
        };
        self.conn.execute(
            "UPDATE memory_hints SET status = ?2, approved_at = ?3 WHERE id = ?1",
            params![id, target.as_str(), approved_at],
        )?;
        self.insert_hint_status_change(id, hint.status, target, changed_at, reason)?;
        self.get_hint(id)
    }

    pub fn tombstone_hint(&self, id: &str, tombstoned_at: &str) -> Result<Option<MemoryHint>> {
        self.transition_hint_status(id, MemoryHintStatus::Tombstoned, tombstoned_at, None)
    }

    pub fn list_hint_status_changes(&self, hint_id: &str) -> Result<Vec<MemoryHintStatusChange>> {
        let mut statement = self.conn.prepare(
            "SELECT id, hint_id, from_status, to_status, changed_at, reason
             FROM memory_hint_status_changes WHERE hint_id = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map(params![hint_id], row_to_hint_status_change)?;
        let changes = collect_rows(rows)?;
        Ok(changes)
    }

    fn insert_hint_status_change(
        &self,
        hint_id: &str,
        from_status: MemoryHintStatus,
        to_status: MemoryHintStatus,
        changed_at: &str,
        reason: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO memory_hint_status_changes (
                hint_id, from_status, to_status, changed_at, reason
             )
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                hint_id,
                from_status.as_str(),
                to_status.as_str(),
                changed_at,
                reason,
            ],
        )?;
        Ok(())
    }

    fn node_ids_for_raw_event(&self, raw_event_id: &str) -> Result<Vec<String>> {
        let mut statement = self
            .conn
            .prepare("SELECT id FROM memory_nodes WHERE raw_event_id = ?1")?;
        let rows = statement.query_map(params![raw_event_id], |row| row.get::<_, String>(0))?;
        let node_ids = collect_rows(rows)?;
        Ok(node_ids)
    }

    fn tombstone_nodes_by_id(&self, node_ids: &[String], tombstoned_at: &str) -> Result<()> {
        for node_id in node_ids {
            self.conn.execute(
                "UPDATE memory_nodes
                 SET tombstoned_at = COALESCE(tombstoned_at, ?2)
                 WHERE id = ?1",
                params![node_id, tombstoned_at],
            )?;
            self.conn.execute(
                "DELETE FROM memory_indexes WHERE node_id = ?1",
                params![node_id],
            )?;
            self.conn.execute(
                "UPDATE memory_edges
                 SET tombstoned_at = COALESCE(tombstoned_at, ?2)
                 WHERE from_node_id = ?1 OR to_node_id = ?1",
                params![node_id, tombstoned_at],
            )?;
            self.tombstone_dreams_with_node_evidence(node_id, tombstoned_at)?;
        }
        Ok(())
    }

    fn tombstone_edges_with_event_evidence(
        &self,
        event_id: &str,
        tombstoned_at: &str,
    ) -> Result<()> {
        let mut statement = self
            .conn
            .prepare("SELECT id, evidence_event_ids_json FROM memory_edges")?;
        let rows = collect_rows(statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, parse_json_column(row, 1)?))
        })?)?;
        for (edge_id, evidence) in rows {
            if json_array_contains_string(&evidence, event_id) {
                self.conn.execute(
                    "UPDATE memory_edges
                     SET tombstoned_at = COALESCE(tombstoned_at, ?2)
                     WHERE id = ?1",
                    params![edge_id, tombstoned_at],
                )?;
            }
        }
        Ok(())
    }

    fn tombstone_dreams_with_event_evidence(
        &self,
        event_id: &str,
        tombstoned_at: &str,
    ) -> Result<()> {
        self.tombstone_dreams_by_evidence(
            "SELECT id, evidence_event_ids_json FROM memory_dreams",
            event_id,
            tombstoned_at,
        )
    }

    fn tombstone_dreams_with_node_evidence(
        &self,
        node_id: &str,
        tombstoned_at: &str,
    ) -> Result<()> {
        self.tombstone_dreams_by_evidence(
            "SELECT id, evidence_node_ids_json FROM memory_dreams",
            node_id,
            tombstoned_at,
        )
    }

    fn tombstone_dreams_by_evidence(
        &self,
        sql: &str,
        evidence_id: &str,
        tombstoned_at: &str,
    ) -> Result<()> {
        let mut statement = self.conn.prepare(sql)?;
        let dreams = collect_rows(statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, parse_json_column(row, 1)?))
        })?)?;
        for (dream_id, evidence) in dreams {
            if json_array_contains_string(&evidence, evidence_id) {
                self.conn.execute(
                    "UPDATE memory_dreams
                     SET status = ?2, reviewed_at = COALESCE(reviewed_at, ?3)
                     WHERE id = ?1",
                    params![
                        dream_id,
                        MemoryDreamStatus::Tombstoned.as_str(),
                        tombstoned_at
                    ],
                )?;
                self.tombstone_hints_for_dream(&dream_id, tombstoned_at)?;
            }
        }
        Ok(())
    }

    fn tombstone_hints_for_dream(&self, dream_id: &str, tombstoned_at: &str) -> Result<()> {
        let mut statement = self
            .conn
            .prepare("SELECT id FROM memory_hints WHERE dream_id = ?1")?;
        let hint_ids =
            collect_rows(statement.query_map(params![dream_id], |row| row.get::<_, String>(0))?)?;
        for hint_id in hint_ids {
            let _ = self.tombstone_hint(&hint_id, tombstoned_at)?;
        }
        Ok(())
    }
}
