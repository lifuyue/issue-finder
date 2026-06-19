use anyhow::Result;
use serde_json::json;

use crate::memory::model::{MemoryEdgeRelation, MemoryNode, MemoryNodeType, NewMemoryEdge};
use crate::memory::store::MemoryStore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryGraphBuildReport {
    pub raw_events_seen: usize,
    pub raw_events_linked: usize,
    pub edges_written: usize,
}

pub struct MemoryGraphBuilder;

impl MemoryGraphBuilder {
    pub fn rebuild_coactivation_edges(
        store: &MemoryStore,
        created_at: &str,
    ) -> Result<MemoryGraphBuildReport> {
        store.delete_edges_by_relation(MemoryEdgeRelation::CoActivated)?;
        let raw_events = store.list_raw_events()?;
        let mut raw_events_linked = 0;
        let mut edges_written = 0;

        for raw_event in raw_events
            .iter()
            .filter(|raw_event| raw_event.tombstoned_at.is_none())
        {
            let nodes = store
                .list_nodes_for_raw_event(&raw_event.id)?
                .into_iter()
                .filter(|node| node.tombstoned_at.is_none())
                .collect::<Vec<_>>();
            let raw_nodes = nodes
                .iter()
                .filter(|node| node.node_type == MemoryNodeType::RawEvent)
                .collect::<Vec<_>>();
            let entity_nodes = nodes
                .iter()
                .filter(|node| node.node_type == MemoryNodeType::Entity)
                .collect::<Vec<_>>();
            if raw_nodes.is_empty() || entity_nodes.is_empty() {
                continue;
            }

            raw_events_linked += 1;
            for raw_node in raw_nodes {
                for entity_node in &entity_nodes {
                    store.insert_edge(&coactivation_edge(
                        raw_node,
                        entity_node,
                        &raw_event.id,
                        created_at,
                    ))?;
                    edges_written += 1;
                }
            }
        }

        Ok(MemoryGraphBuildReport {
            raw_events_seen: raw_events.len(),
            raw_events_linked,
            edges_written,
        })
    }
}

fn coactivation_edge(
    from_node: &MemoryNode,
    to_node: &MemoryNode,
    raw_event_id: &str,
    created_at: &str,
) -> NewMemoryEdge {
    NewMemoryEdge {
        id: stable_edge_id(from_node.id.as_str(), to_node.id.as_str(), raw_event_id),
        from_node_id: from_node.id.clone(),
        to_node_id: to_node.id.clone(),
        relation: MemoryEdgeRelation::CoActivated,
        strength: 1.0,
        confidence: 1.0,
        evidence_event_ids_json: json!([raw_event_id]),
        last_activated_at: None,
        created_at: created_at.to_string(),
        updated_at: created_at.to_string(),
    }
}

fn stable_edge_id(from_node_id: &str, to_node_id: &str, raw_event_id: &str) -> String {
    format!(
        "memory-edge-coactivated-{:016x}",
        stable_hash(format!("{from_node_id}:{to_node_id}:{raw_event_id}").as_bytes())
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
