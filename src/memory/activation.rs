use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::json;

use crate::memory::index::{MemoryIndexSearch, MemoryRecallMatch};
use crate::memory::model::{
    MemoryActivationItem, MemoryActivationRun, MemoryEdge, MemoryNode, MemoryQueryKind,
    MemoryRawEvent, MemorySourceChannel, MemoryTrustLevel,
};
use crate::memory::store::MemoryStore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryActivationEntity {
    pub entity_type: String,
    pub entity_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryActivationRequest {
    pub run_id: String,
    pub query_kind: MemoryQueryKind,
    pub query_ref: String,
    pub query_text: String,
    pub entities: Vec<MemoryActivationEntity>,
    pub created_at: String,
    pub limit: usize,
    pub persist_trace: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryActivationResult {
    pub run_id: String,
    pub items: Vec<MemoryActivationItem>,
}

#[derive(Debug, Clone)]
struct Candidate {
    node_id: String,
    source_channel: MemorySourceChannel,
    direct_score: f64,
    ripple_score: f64,
    salience_score: f64,
    strength_score: f64,
    recency_score: f64,
    resource_penalty: f64,
    hub_penalty: f64,
    role_trust_penalty: f64,
    hop_penalty: f64,
    stale_penalty: f64,
    final_score: f64,
    direct_support: bool,
    ripple_only: bool,
    explanations: BTreeSet<String>,
}

impl Candidate {
    fn direct(node_id: String, source_channel: MemorySourceChannel) -> Self {
        Self {
            node_id,
            source_channel,
            direct_score: 0.0,
            ripple_score: 0.0,
            salience_score: 0.0,
            strength_score: 0.0,
            recency_score: 0.0,
            resource_penalty: 0.0,
            hub_penalty: 0.0,
            role_trust_penalty: 0.0,
            hop_penalty: 0.0,
            stale_penalty: 0.0,
            final_score: 0.0,
            direct_support: true,
            ripple_only: false,
            explanations: BTreeSet::new(),
        }
    }

    fn ripple(node_id: String) -> Self {
        Self {
            node_id,
            source_channel: MemorySourceChannel::NearRipple,
            direct_score: 0.0,
            ripple_score: 0.0,
            salience_score: 0.0,
            strength_score: 0.0,
            recency_score: 0.0,
            resource_penalty: 0.0,
            hub_penalty: 0.0,
            role_trust_penalty: 0.0,
            hop_penalty: 0.0,
            stale_penalty: 0.0,
            final_score: 0.0,
            direct_support: false,
            ripple_only: true,
            explanations: BTreeSet::new(),
        }
    }
}

pub struct MemoryActivationEngine;

impl MemoryActivationEngine {
    pub fn activate(
        store: &MemoryStore,
        request: &MemoryActivationRequest,
    ) -> Result<MemoryActivationResult> {
        let mut candidates = BTreeMap::<String, Candidate>::new();

        add_text_seed_matches(
            store,
            &mut candidates,
            MemoryIndexSearch::search_fts(store, &request.query_text)?,
            0.45,
            "fts",
        )?;
        add_text_seed_matches(
            store,
            &mut candidates,
            MemoryIndexSearch::search_rare_tokens(store, &request.query_text)?,
            0.75,
            "rare_token",
        )?;
        for entity in &request.entities {
            add_entity_seed_matches(
                store,
                &mut candidates,
                MemoryIndexSearch::search_entity(store, &entity.entity_type, &entity.entity_value)?,
                &entity.entity_type,
            )?;
        }

        add_near_ripple(store, &mut candidates)?;
        settle_candidates(store, request, &mut candidates)?;

        let mut items = candidates
            .into_values()
            .filter(|candidate| candidate.final_score > 0.0)
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .final_score
                .partial_cmp(&left.final_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.node_id.cmp(&right.node_id))
        });
        items.truncate(request.limit);

        let activation_items = items
            .into_iter()
            .enumerate()
            .map(|(index, candidate)| candidate_to_item(request, candidate, (index + 1) as i64))
            .collect::<Vec<_>>();

        if request.persist_trace {
            store.insert_activation_run(&MemoryActivationRun {
                id: request.run_id.clone(),
                query_kind: request.query_kind,
                query_ref: request.query_ref.clone(),
                query_json: json!({
                    "queryText": request.query_text,
                    "entities": request.entities.iter().map(|entity| {
                        json!({
                            "entityType": entity.entity_type,
                            "entityValue": entity.entity_value,
                        })
                    }).collect::<Vec<_>>(),
                    "limit": request.limit,
                }),
                created_at: request.created_at.clone(),
            })?;
            for item in &activation_items {
                store.insert_activation_item(item)?;
            }
        }

        Ok(MemoryActivationResult {
            run_id: request.run_id.clone(),
            items: activation_items,
        })
    }
}

fn add_text_seed_matches(
    store: &MemoryStore,
    candidates: &mut BTreeMap<String, Candidate>,
    matches: Vec<MemoryRecallMatch>,
    weight: f64,
    label: &str,
) -> Result<()> {
    for matched in matches {
        if !node_is_active(store, &matched.node_id)? {
            continue;
        }
        let candidate = candidates
            .entry(matched.node_id.clone())
            .or_insert_with(|| {
                Candidate::direct(matched.node_id.clone(), MemorySourceChannel::Fts)
            });
        candidate.direct_score += weight;
        candidate.direct_support = true;
        candidate.ripple_only = false;
        candidate
            .explanations
            .insert(format!("{label}:{}", matched.matched_payload));
    }
    Ok(())
}

fn add_entity_seed_matches(
    store: &MemoryStore,
    candidates: &mut BTreeMap<String, Candidate>,
    matches: Vec<MemoryRecallMatch>,
    entity_type: &str,
) -> Result<()> {
    for matched in matches {
        if !node_is_active(store, &matched.node_id)? {
            continue;
        }
        let candidate = candidates
            .entry(matched.node_id.clone())
            .or_insert_with(|| {
                Candidate::direct(matched.node_id.clone(), MemorySourceChannel::Entity)
            });
        candidate.direct_score += 0.9;
        candidate.direct_support = true;
        candidate.ripple_only = false;
        candidate
            .explanations
            .insert(format!("entity:{entity_type}:{}", matched.matched_payload));
    }
    Ok(())
}

fn add_near_ripple(
    store: &MemoryStore,
    candidates: &mut BTreeMap<String, Candidate>,
) -> Result<()> {
    let direct_seeds = candidates
        .values()
        .filter(|candidate| candidate.direct_support)
        .map(|candidate| candidate.node_id.clone())
        .collect::<BTreeSet<_>>();
    if direct_seeds.is_empty() {
        return Ok(());
    }

    for edge in store
        .list_edges()?
        .into_iter()
        .filter(|edge| edge.tombstoned_at.is_none())
    {
        let Some((seed, related)) = related_node_from_edge(&edge, &direct_seeds) else {
            continue;
        };
        if !node_is_active(store, &related)? {
            continue;
        }
        let candidate = candidates
            .entry(related.clone())
            .or_insert_with(|| Candidate::ripple(related.clone()));
        let ripple = (edge.strength * edge.confidence * 0.8).max(0.0);
        candidate.ripple_score += ripple;
        if !candidate.direct_support {
            candidate.source_channel = MemorySourceChannel::NearRipple;
            candidate.ripple_only = true;
        }
        candidate
            .explanations
            .insert(format!("near_ripple:{seed}:{}", edge.id));
    }
    Ok(())
}

fn related_node_from_edge(
    edge: &MemoryEdge,
    direct_seeds: &BTreeSet<String>,
) -> Option<(String, String)> {
    if direct_seeds.contains(&edge.from_node_id) {
        Some((edge.from_node_id.clone(), edge.to_node_id.clone()))
    } else if direct_seeds.contains(&edge.to_node_id) {
        Some((edge.to_node_id.clone(), edge.from_node_id.clone()))
    } else {
        None
    }
}

fn settle_candidates(
    store: &MemoryStore,
    request: &MemoryActivationRequest,
    candidates: &mut BTreeMap<String, Candidate>,
) -> Result<()> {
    for candidate in candidates.values_mut() {
        let Some(node) = store.get_node(&candidate.node_id)? else {
            candidate.stale_penalty = 100.0;
            continue;
        };
        if node.tombstoned_at.is_some() || raw_event_for_node(store, &node)?.is_tombstoned() {
            candidate.stale_penalty = 100.0;
            continue;
        }

        if let Some(state) = store.get_node_state(&candidate.node_id)? {
            candidate.salience_score = (state.salience * 0.35).clamp(0.0, 0.35);
            candidate.strength_score = (state.strength / 3.0 * 0.45).clamp(0.0, 0.45);
            candidate.resource_penalty = ((1.0 - state.resource).max(0.0) * 0.65).min(0.65);
            let fan_total = state.fan_in + state.fan_out;
            if fan_total > 8 {
                candidate.hub_penalty = ((fan_total - 8) as f64 * 0.08).min(0.8);
            }
        }

        if let Some(raw_event) = raw_event_for_node(store, &node)? {
            candidate.recency_score = recency_score(&raw_event, &request.created_at);
            candidate.role_trust_penalty = role_trust_penalty(&raw_event);
        }
        if candidate.ripple_only {
            candidate.hop_penalty = 0.15;
        }

        candidate.final_score = candidate.direct_score
            + candidate.ripple_score
            + candidate.salience_score
            + candidate.strength_score
            + candidate.recency_score
            - candidate.resource_penalty
            - candidate.hub_penalty
            - candidate.role_trust_penalty
            - candidate.hop_penalty
            - candidate.stale_penalty;
    }
    Ok(())
}

fn candidate_to_item(
    request: &MemoryActivationRequest,
    candidate: Candidate,
    rank: i64,
) -> MemoryActivationItem {
    MemoryActivationItem {
        run_id: request.run_id.clone(),
        node_id: candidate.node_id,
        source_channel: candidate.source_channel,
        direct_score: candidate.direct_score,
        ripple_score: candidate.ripple_score,
        salience_score: candidate.salience_score,
        strength_score: candidate.strength_score,
        recency_score: candidate.recency_score,
        resource_penalty: candidate.resource_penalty,
        hub_penalty: candidate.hub_penalty,
        role_trust_penalty: candidate.role_trust_penalty,
        hop_penalty: candidate.hop_penalty,
        stale_penalty: candidate.stale_penalty,
        final_score: candidate.final_score,
        rank,
        explanation: format!(
            "matches=[{}]; direct={:.3}; ripple={:.3}; salience={:.3}; strength={:.3}; recency={:.3}; resource_penalty={:.3}; hub_penalty={:.3}; role_trust_penalty={:.3}; hop_penalty={:.3}; stale_penalty={:.3}; final={:.3}",
            candidate.explanations.into_iter().collect::<Vec<_>>().join(","),
            candidate.direct_score,
            candidate.ripple_score,
            candidate.salience_score,
            candidate.strength_score,
            candidate.recency_score,
            candidate.resource_penalty,
            candidate.hub_penalty,
            candidate.role_trust_penalty,
            candidate.hop_penalty,
            candidate.stale_penalty,
            candidate.final_score,
        ),
    }
}

fn node_is_active(store: &MemoryStore, node_id: &str) -> Result<bool> {
    let Some(node) = store.get_node(node_id)? else {
        return Ok(false);
    };
    if node.tombstoned_at.is_some() {
        return Ok(false);
    }
    Ok(!raw_event_for_node(store, &node)?.is_tombstoned())
}

fn raw_event_for_node(store: &MemoryStore, node: &MemoryNode) -> Result<Option<MemoryRawEvent>> {
    match node.raw_event_id.as_deref() {
        Some(raw_event_id) => store.get_raw_event(raw_event_id),
        None => Ok(None),
    }
}

trait RawEventState {
    fn is_tombstoned(&self) -> bool;
}

impl RawEventState for Option<MemoryRawEvent> {
    fn is_tombstoned(&self) -> bool {
        self.as_ref()
            .and_then(|raw_event| raw_event.tombstoned_at.as_ref())
            .is_some()
    }
}

fn role_trust_penalty(raw_event: &MemoryRawEvent) -> f64 {
    match raw_event.trust_level {
        MemoryTrustLevel::UserExplicit => 0.0,
        MemoryTrustLevel::ExternalGithub => 0.05,
        MemoryTrustLevel::SystemObserved => 0.15,
        MemoryTrustLevel::AgentObserved => 0.2,
        MemoryTrustLevel::LlmInferred => 1.1,
    }
}

fn recency_score(raw_event: &MemoryRawEvent, query_time: &str) -> f64 {
    let Ok(query_time) = DateTime::parse_from_rfc3339(query_time) else {
        return 0.0;
    };
    let Ok(occurred_at) = DateTime::parse_from_rfc3339(&raw_event.occurred_at) else {
        return 0.0;
    };
    let age = query_time.with_timezone(&Utc) - occurred_at.with_timezone(&Utc);
    if age.num_days() <= 7 {
        0.25
    } else if age.num_days() <= 30 {
        0.1
    } else {
        0.0
    }
}
