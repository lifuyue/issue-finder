use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::memory::model::{
    MemoryDream, MemoryDreamRun, MemoryDreamScope, MemoryDreamStatus, MemoryDreamTrigger,
    MemoryDreamType, MemoryHint, MemoryHintScopeType, MemoryHintStatus, MemoryHintType,
    MemoryModelStatus, MemoryNode, MemoryRawEvent, MemoryRawEventType, MemorySubjectType,
    NewMemoryDream, NewMemoryHint,
};
use crate::memory::store::MemoryStore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryDreamRequest {
    pub run_id: String,
    pub trigger: MemoryDreamTrigger,
    pub scope: MemoryDreamScope,
    pub scope_ref: Option<String>,
    pub input_activation_run_ids: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryDreamResult {
    pub run: MemoryDreamRun,
    pub dreams: Vec<MemoryDream>,
    pub hints: Vec<MemoryHint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryDreamContext {
    pub activation_run_ids: Vec<String>,
    pub evidence_node_ids: Vec<String>,
    pub evidence_event_ids: Vec<String>,
    pub evidence_hint_ids: Vec<String>,
    pub evidence_summaries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryDreamProposal {
    pub dream_type: MemoryDreamType,
    pub summary: String,
    pub evidence_node_ids: Vec<String>,
    pub evidence_event_ids: Vec<String>,
    pub evidence_hint_ids: Vec<String>,
    pub confidence: f64,
    pub hint: Option<MemoryHintProposal>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryHintProposal {
    pub hint_type: MemoryHintType,
    pub scope_type: MemoryHintScopeType,
    pub scope_ref: String,
    pub summary: String,
    pub policy_json: Value,
    pub weight: f64,
    pub expires_at: Option<String>,
}

pub trait MemoryDreamSynthesizer {
    fn synthesize(&self, context: &MemoryDreamContext) -> Result<Vec<MemoryDreamProposal>>;
}

pub struct MemoryDreamEngine;

impl MemoryDreamEngine {
    pub fn dream(
        store: &MemoryStore,
        request: &MemoryDreamRequest,
        synthesizer: Option<&dyn MemoryDreamSynthesizer>,
    ) -> Result<MemoryDreamResult> {
        if let Some(run) = store.get_dream_run(&request.run_id)? {
            return existing_result(store, run);
        }

        let evidence = DreamEvidence::collect(store, request)?;
        let context = evidence.context(request);
        let mut proposals = deterministic_proposals(request, &evidence);
        let model_status = match synthesizer {
            Some(synthesizer) => match synthesizer.synthesize(&context) {
                Ok(mut synthesized) => {
                    proposals.append(&mut synthesized);
                    MemoryModelStatus::Success
                }
                Err(_) => MemoryModelStatus::Failed,
            },
            None => MemoryModelStatus::Disabled,
        };

        let run = store.insert_dream_run(&MemoryDreamRun {
            id: request.run_id.clone(),
            trigger: request.trigger,
            scope: request.scope,
            input_activation_run_ids_json: json!(request.input_activation_run_ids),
            model_status,
            created_at: request.created_at.clone(),
        })?;

        let mut dreams = Vec::new();
        let mut hints = Vec::new();
        for proposal in normalize_proposals(&request.run_id, proposals) {
            let dream_id = stable_id("memory-dream", &proposal.identity(&request.run_id));
            let dream = store.insert_dream(&NewMemoryDream {
                id: dream_id.clone(),
                dream_run_id: request.run_id.clone(),
                dream_type: proposal.dream_type,
                summary: proposal.summary.clone(),
                evidence_node_ids_json: json!(proposal.evidence_node_ids),
                evidence_event_ids_json: json!(proposal.evidence_event_ids),
                evidence_hint_ids_json: json!(proposal.evidence_hint_ids),
                status: MemoryDreamStatus::Candidate,
                confidence: proposal.confidence.clamp(0.0, 1.0),
                version: 1,
                created_at: request.created_at.clone(),
                reviewed_at: None,
            })?;
            if let Some(hint) = proposal.hint {
                let hint_id = stable_id(
                    "memory-hint",
                    &format!(
                        "{}:{}:{}:{}",
                        dream_id,
                        hint.hint_type.as_str(),
                        hint.scope_ref,
                        hint.summary
                    ),
                );
                hints.push(store.insert_hint(&NewMemoryHint {
                    id: hint_id,
                    dream_id,
                    hint_type: hint.hint_type,
                    scope_type: hint.scope_type,
                    scope_ref: hint.scope_ref,
                    summary: hint.summary,
                    policy_json: hint.policy_json,
                    weight: hint.weight,
                    status: MemoryHintStatus::Candidate,
                    created_at: request.created_at.clone(),
                    approved_at: None,
                    expires_at: hint.expires_at,
                })?);
            }
            dreams.push(dream);
        }

        Ok(MemoryDreamResult { run, dreams, hints })
    }
}

#[derive(Debug, Clone)]
struct DreamEvidence {
    activation_node_ids: BTreeSet<String>,
    raw_events: BTreeMap<String, MemoryRawEvent>,
    nodes_by_event: BTreeMap<String, BTreeSet<String>>,
    active_hints: Vec<MemoryHint>,
}

impl DreamEvidence {
    fn collect(store: &MemoryStore, request: &MemoryDreamRequest) -> Result<Self> {
        let mut evidence = Self {
            activation_node_ids: BTreeSet::new(),
            raw_events: BTreeMap::new(),
            nodes_by_event: BTreeMap::new(),
            active_hints: Vec::new(),
        };

        for activation_run_id in &request.input_activation_run_ids {
            let _ = store
                .get_activation_run(activation_run_id)?
                .with_context(|| format!("activation run `{activation_run_id}` does not exist"))?;
            for item in store.list_activation_items(activation_run_id)? {
                let Some(node) = active_node(store, &item.node_id)? else {
                    continue;
                };
                evidence.activation_node_ids.insert(node.id.clone());
                evidence.add_node_raw_event(store, request, &node)?;
            }
        }

        for raw_event in store.list_raw_events()? {
            if raw_event.tombstoned_at.is_none() && event_matches_scope(&raw_event, request) {
                evidence.add_raw_event(store, raw_event)?;
            }
        }

        evidence.active_hints = store
            .list_hints()?
            .into_iter()
            .filter(|hint| hint_can_be_reviewed(hint) && hint_matches_scope(hint, request))
            .collect();

        Ok(evidence)
    }

    fn add_node_raw_event(
        &mut self,
        store: &MemoryStore,
        request: &MemoryDreamRequest,
        node: &MemoryNode,
    ) -> Result<()> {
        let Some(raw_event_id) = node.raw_event_id.as_deref() else {
            return Ok(());
        };
        let Some(raw_event) = store.get_raw_event(raw_event_id)? else {
            return Ok(());
        };
        if raw_event.tombstoned_at.is_none() && event_matches_scope(&raw_event, request) {
            self.add_raw_event(store, raw_event)?;
        }
        Ok(())
    }

    fn add_raw_event(&mut self, store: &MemoryStore, raw_event: MemoryRawEvent) -> Result<()> {
        let event_id = raw_event.id.clone();
        self.raw_events.entry(event_id.clone()).or_insert(raw_event);
        let nodes = self.nodes_by_event.entry(event_id.clone()).or_default();
        for node in store.list_nodes_for_raw_event(&event_id)? {
            if node.tombstoned_at.is_none() {
                nodes.insert(node.id);
            }
        }
        Ok(())
    }

    fn context(&self, request: &MemoryDreamRequest) -> MemoryDreamContext {
        let mut evidence_node_ids = self.activation_node_ids.clone();
        for nodes in self.nodes_by_event.values() {
            evidence_node_ids.extend(nodes.iter().cloned());
        }
        let evidence_event_ids = self.raw_events.keys().cloned().collect::<Vec<_>>();
        let evidence_hint_ids = self
            .active_hints
            .iter()
            .map(|hint| hint.id.clone())
            .collect::<Vec<_>>();
        let evidence_summaries = self
            .raw_events
            .values()
            .map(event_summary)
            .chain(self.active_hints.iter().map(|hint| hint.summary.clone()))
            .collect::<Vec<_>>();
        MemoryDreamContext {
            activation_run_ids: request.input_activation_run_ids.clone(),
            evidence_node_ids: evidence_node_ids.into_iter().collect(),
            evidence_event_ids,
            evidence_hint_ids,
            evidence_summaries,
        }
    }

    fn event_node_ids(&self, event_id: &str) -> Vec<String> {
        self.nodes_by_event
            .get(event_id)
            .into_iter()
            .flat_map(|nodes| nodes.iter().cloned())
            .collect()
    }
}

fn deterministic_proposals(
    request: &MemoryDreamRequest,
    evidence: &DreamEvidence,
) -> Vec<MemoryDreamProposal> {
    let mut proposals = Vec::new();
    proposals.extend(agent_performance_proposals(evidence));
    proposals.extend(dispatch_ranking_trend_proposals(evidence));
    proposals.extend(feedback_policy_proposals(request, evidence));
    proposals.extend(repo_summary_proposals(evidence));
    proposals.extend(stale_hint_proposals(request, evidence));
    proposals.extend(conflict_proposals(evidence));
    proposals
}

#[derive(Default)]
struct AgentPerformanceStats {
    successes: usize,
    failures: usize,
    event_ids: BTreeSet<String>,
    node_ids: BTreeSet<String>,
}

#[derive(Default)]
struct OutcomeTrendStats {
    successes: usize,
    failures: usize,
    event_ids: BTreeSet<String>,
    node_ids: BTreeSet<String>,
}

fn agent_performance_proposals(evidence: &DreamEvidence) -> Vec<MemoryDreamProposal> {
    let mut by_agent_task = BTreeMap::<(String, String), AgentPerformanceStats>::new();
    for event in evidence.raw_events.values() {
        let Some((agent_id, task_type, succeeded)) = dispatch_outcome(event) else {
            continue;
        };
        let stats = by_agent_task.entry((agent_id, task_type)).or_default();
        if succeeded {
            stats.successes += 1;
        } else {
            stats.failures += 1;
        }
        stats.event_ids.insert(event.id.clone());
        stats.node_ids.extend(evidence.event_node_ids(&event.id));
    }

    by_agent_task
        .into_iter()
        .map(|((agent_id, task_type), stats)| {
            let recommendation = if stats.successes >= stats.failures {
                "prefer"
            } else {
                "avoid"
            };
            MemoryDreamProposal {
                dream_type: MemoryDreamType::AgentPerformance,
                summary: format!(
                    "Agent `{agent_id}` has {} successful and {} failed dispatch events for `{task_type}`.",
                    stats.successes, stats.failures
                ),
                evidence_node_ids: stats.node_ids.into_iter().collect(),
                evidence_event_ids: stats.event_ids.into_iter().collect(),
                evidence_hint_ids: Vec::new(),
                confidence: 0.55 + ((stats.successes + stats.failures) as f64 * 0.05).min(0.25),
                hint: Some(MemoryHintProposal {
                    hint_type: MemoryHintType::Dispatch,
                    scope_type: MemoryHintScopeType::Agent,
                    scope_ref: agent_id.clone(),
                    summary: format!(
                        "Review `{agent_id}` for `{task_type}` dispatches; current evidence suggests `{recommendation}`."
                    ),
                    policy_json: json!({
                        "kind": "agent_performance",
                        "agentId": agent_id,
                        "taskType": task_type,
                        "successes": stats.successes,
                        "failures": stats.failures,
                        "recommendation": recommendation,
                    }),
                    weight: if recommendation == "prefer" { 0.25 } else { -0.25 },
                    expires_at: None,
                }),
            }
        })
        .collect()
}

fn dispatch_ranking_trend_proposals(evidence: &DreamEvidence) -> Vec<MemoryDreamProposal> {
    let mut by_repo = BTreeMap::<String, OutcomeTrendStats>::new();
    let mut by_task = BTreeMap::<String, OutcomeTrendStats>::new();
    for event in evidence.raw_events.values() {
        let Some(outcome) = ranking_outcome(event) else {
            continue;
        };
        if let Some(repo) = repo_from_event(event) {
            let stats = by_repo.entry(repo).or_default();
            add_outcome_trend(stats, &outcome, evidence, event);
        }
        if let Some(task_type) = json_string(&event.payload_json, "taskType") {
            let stats = by_task.entry(task_type).or_default();
            add_outcome_trend(stats, &outcome, evidence, event);
        }
    }

    let repo_proposals = by_repo
        .into_iter()
        .map(|(repo, stats)| ranking_trend_proposal(MemoryHintScopeType::Repo, repo, stats));
    let task_proposals = by_task.into_iter().map(|(task_type, stats)| {
        ranking_trend_proposal(MemoryHintScopeType::IssueType, task_type, stats)
    });
    repo_proposals.chain(task_proposals).collect()
}

fn add_outcome_trend(
    stats: &mut OutcomeTrendStats,
    outcome: &RankingOutcome,
    evidence: &DreamEvidence,
    event: &MemoryRawEvent,
) {
    if outcome.positive {
        stats.successes += 1;
    } else {
        stats.failures += 1;
    }
    stats.event_ids.insert(event.id.clone());
    stats.node_ids.extend(evidence.event_node_ids(&event.id));
}

fn ranking_trend_proposal(
    scope_type: MemoryHintScopeType,
    scope_ref: String,
    stats: OutcomeTrendStats,
) -> MemoryDreamProposal {
    let net = stats.successes as isize - stats.failures as isize;
    let weight = (net as f64 * 0.20).clamp(-0.60, 0.45);
    let recommendation = if net >= 0 { "boost" } else { "cooldown" };
    MemoryDreamProposal {
        dream_type: MemoryDreamType::DiscoveryPolicy,
        summary: format!(
            "Dispatch outcomes for `{scope_ref}` include {} success and {} failure events.",
            stats.successes, stats.failures
        ),
        evidence_node_ids: stats.node_ids.into_iter().collect(),
        evidence_event_ids: stats.event_ids.into_iter().collect(),
        evidence_hint_ids: Vec::new(),
        confidence: 0.55,
        hint: Some(MemoryHintProposal {
            hint_type: MemoryHintType::Ranking,
            scope_type,
            scope_ref: scope_ref.clone(),
            summary: format!(
                "Use dispatch outcome trend for `{scope_ref}` as a `{recommendation}` ranking signal."
            ),
            policy_json: json!({
                "kind": "dispatch_outcome_trend",
                "scope": scope_ref,
                "successes": stats.successes,
                "failures": stats.failures,
                "recommendation": recommendation,
            }),
            weight,
            expires_at: None,
        }),
    }
}

#[derive(Default)]
struct FeedbackStats {
    approvals: usize,
    rejections: usize,
    dismissals: usize,
    event_ids: BTreeSet<String>,
    node_ids: BTreeSet<String>,
}

fn feedback_policy_proposals(
    request: &MemoryDreamRequest,
    evidence: &DreamEvidence,
) -> Vec<MemoryDreamProposal> {
    let mut by_scope = BTreeMap::<String, FeedbackStats>::new();
    for event in evidence.raw_events.values() {
        let Some(feedback) = feedback_kind(event) else {
            continue;
        };
        let scope = repo_from_event(event)
            .or_else(|| request.scope_ref.clone())
            .unwrap_or_else(|| "global".to_string());
        let stats = by_scope.entry(scope).or_default();
        match feedback {
            "approve" => stats.approvals += 1,
            "reject" => stats.rejections += 1,
            "dismiss" => stats.dismissals += 1,
            _ => {}
        }
        stats.event_ids.insert(event.id.clone());
        stats.node_ids.extend(evidence.event_node_ids(&event.id));
    }

    by_scope
        .into_iter()
        .map(|(scope, stats)| {
            let weight = ((stats.approvals as f64) - (stats.rejections + stats.dismissals) as f64)
                .clamp(-3.0, 3.0)
                / 10.0;
            let scope_type = if scope == "global" {
                MemoryHintScopeType::Global
            } else {
                MemoryHintScopeType::Repo
            };
            MemoryDreamProposal {
                dream_type: MemoryDreamType::DiscoveryPolicy,
                summary: format!(
                    "Recommendation feedback for `{scope}` includes {} approvals, {} rejections, and {} dismissals.",
                    stats.approvals, stats.rejections, stats.dismissals
                ),
                evidence_node_ids: stats.node_ids.into_iter().collect(),
                evidence_event_ids: stats.event_ids.into_iter().collect(),
                evidence_hint_ids: Vec::new(),
                confidence: 0.5,
                hint: Some(MemoryHintProposal {
                    hint_type: MemoryHintType::Ranking,
                    scope_type,
                    scope_ref: scope.clone(),
                    summary: format!("Review recommendation feedback trend for `{scope}`."),
                    policy_json: json!({
                        "kind": "recommendation_feedback",
                        "scope": scope,
                        "approvals": stats.approvals,
                        "rejections": stats.rejections,
                        "dismissals": stats.dismissals,
                    }),
                    weight,
                    expires_at: None,
                }),
            }
        })
        .collect()
}

#[derive(Default)]
struct RepoStats {
    event_ids: BTreeSet<String>,
    node_ids: BTreeSet<String>,
    event_types: BTreeSet<String>,
}

fn repo_summary_proposals(evidence: &DreamEvidence) -> Vec<MemoryDreamProposal> {
    let mut by_repo = BTreeMap::<String, RepoStats>::new();
    for event in evidence.raw_events.values() {
        let Some(repo) = repo_from_event(event) else {
            continue;
        };
        let stats = by_repo.entry(repo).or_default();
        stats.event_ids.insert(event.id.clone());
        stats.node_ids.extend(evidence.event_node_ids(&event.id));
        stats
            .event_types
            .insert(event.event_type.as_str().to_string());
    }

    by_repo
        .into_iter()
        .map(|(repo, stats)| MemoryDreamProposal {
            dream_type: MemoryDreamType::RepoSummary,
            summary: format!(
                "Recent memory for `{repo}` contains {} events: {}.",
                stats.event_ids.len(),
                stats.event_types.into_iter().collect::<Vec<_>>().join(", ")
            ),
            evidence_node_ids: stats.node_ids.into_iter().collect(),
            evidence_event_ids: stats.event_ids.into_iter().collect(),
            evidence_hint_ids: Vec::new(),
            confidence: 0.5,
            hint: None,
        })
        .collect()
}

fn stale_hint_proposals(
    request: &MemoryDreamRequest,
    evidence: &DreamEvidence,
) -> Vec<MemoryDreamProposal> {
    evidence
        .active_hints
        .iter()
        .filter(|hint| hint_affects_decisions(hint))
        .filter(|hint| {
            hint.expires_at
                .as_deref()
                .is_some_and(|expires_at| time_at_or_before(expires_at, &request.created_at))
        })
        .map(|hint| MemoryDreamProposal {
            dream_type: MemoryDreamType::StaleMemory,
            summary: format!(
                "Hint `{}` expired at `{}` and should be reviewed before further use.",
                hint.id,
                hint.expires_at.as_deref().unwrap_or("unknown")
            ),
            evidence_node_ids: Vec::new(),
            evidence_event_ids: Vec::new(),
            evidence_hint_ids: vec![hint.id.clone()],
            confidence: 0.8,
            hint: None,
        })
        .collect()
}

fn conflict_proposals(evidence: &DreamEvidence) -> Vec<MemoryDreamProposal> {
    let mut proposals = Vec::new();
    for hint in evidence
        .active_hints
        .iter()
        .filter(|hint| hint_affects_decisions(hint))
    {
        let Some(prediction) = hint_prediction(hint) else {
            continue;
        };
        for event in evidence.raw_events.values() {
            let Some((agent_id, task_type, succeeded)) = dispatch_outcome(event) else {
                continue;
            };
            if prediction.matches(&agent_id, &task_type) && prediction.predicts_success != succeeded
            {
                let outcome = if succeeded { "success" } else { "failure" };
                let expected = if prediction.predicts_success {
                    "success"
                } else {
                    "failure"
                };
                proposals.push(MemoryDreamProposal {
                    dream_type: MemoryDreamType::Conflict,
                    summary: format!(
                        "Hint `{}` expected `{expected}` for `{agent_id}` on `{task_type}`, but recent evidence recorded `{outcome}`.",
                        hint.id
                    ),
                    evidence_node_ids: evidence.event_node_ids(&event.id),
                    evidence_event_ids: vec![event.id.clone()],
                    evidence_hint_ids: vec![hint.id.clone()],
                    confidence: 0.75,
                    hint: Some(MemoryHintProposal {
                        hint_type: hint.hint_type,
                        scope_type: hint.scope_type,
                        scope_ref: hint.scope_ref.clone(),
                        summary: format!(
                            "Review conflicting `{agent_id}` / `{task_type}` dispatch memory before applying it."
                        ),
                        policy_json: json!({
                            "kind": "conflict_review",
                            "conflictsWith": hint.id,
                            "agentId": agent_id,
                            "taskType": task_type,
                            "expectedOutcome": expected,
                            "recentOutcome": outcome,
                        }),
                        weight: 0.0,
                        expires_at: None,
                    }),
                });
            }
        }
    }
    proposals
}

fn existing_result(store: &MemoryStore, run: MemoryDreamRun) -> Result<MemoryDreamResult> {
    let dreams = store.list_dreams_for_run(&run.id)?;
    let mut hints = Vec::new();
    for dream in &dreams {
        hints.extend(store.list_hints_for_dream(&dream.id)?);
    }
    Ok(MemoryDreamResult { run, dreams, hints })
}

fn normalize_proposals(
    run_id: &str,
    proposals: Vec<MemoryDreamProposal>,
) -> Vec<MemoryDreamProposal> {
    let mut by_id = BTreeMap::new();
    for mut proposal in proposals {
        proposal.evidence_node_ids = sorted_unique(proposal.evidence_node_ids);
        proposal.evidence_event_ids = sorted_unique(proposal.evidence_event_ids);
        proposal.evidence_hint_ids = sorted_unique(proposal.evidence_hint_ids);
        if proposal.summary.trim().is_empty() {
            continue;
        }
        by_id
            .entry(stable_id(
                "memory-dream-proposal",
                &proposal.identity(run_id),
            ))
            .or_insert(proposal);
    }
    by_id.into_values().collect()
}

impl MemoryDreamProposal {
    fn identity(&self, run_id: &str) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            run_id,
            self.dream_type.as_str(),
            self.summary,
            self.evidence_node_ids.join(","),
            self.evidence_event_ids.join(",")
        )
    }
}

fn active_node(store: &MemoryStore, node_id: &str) -> Result<Option<MemoryNode>> {
    let Some(node) = store.get_node(node_id)? else {
        return Ok(None);
    };
    if node.tombstoned_at.is_some() {
        return Ok(None);
    }
    if let Some(raw_event_id) = node.raw_event_id.as_deref() {
        if store
            .get_raw_event(raw_event_id)?
            .and_then(|event| event.tombstoned_at)
            .is_some()
        {
            return Ok(None);
        }
    }
    Ok(Some(node))
}

fn event_matches_scope(raw_event: &MemoryRawEvent, request: &MemoryDreamRequest) -> bool {
    let Some(scope_ref) = request.scope_ref.as_deref() else {
        return true;
    };
    match request.scope {
        MemoryDreamScope::Global => true,
        MemoryDreamScope::Repo => repo_from_event(raw_event).as_deref() == Some(scope_ref),
        MemoryDreamScope::Agent => {
            json_string(&raw_event.payload_json, "agentId").as_deref() == Some(scope_ref)
                || (raw_event.subject_type == MemorySubjectType::Agent
                    && raw_event.subject_ref == scope_ref)
        }
        MemoryDreamScope::Profile => raw_event.subject_type == MemorySubjectType::Profile,
        MemoryDreamScope::IssueType => {
            json_string(&raw_event.payload_json, "taskType").as_deref() == Some(scope_ref)
                || raw_event.subject_ref == scope_ref
        }
    }
}

fn hint_matches_scope(hint: &MemoryHint, request: &MemoryDreamRequest) -> bool {
    let Some(scope_ref) = request.scope_ref.as_deref() else {
        return true;
    };
    match request.scope {
        MemoryDreamScope::Global => true,
        MemoryDreamScope::Repo => {
            hint.scope_type == MemoryHintScopeType::Repo && hint.scope_ref == scope_ref
        }
        MemoryDreamScope::Agent => {
            hint.scope_type == MemoryHintScopeType::Agent && hint.scope_ref == scope_ref
        }
        MemoryDreamScope::Profile => hint.hint_type == MemoryHintType::ProfileCandidate,
        MemoryDreamScope::IssueType => {
            hint.scope_type == MemoryHintScopeType::IssueType && hint.scope_ref == scope_ref
        }
    }
}

fn hint_can_be_reviewed(hint: &MemoryHint) -> bool {
    !matches!(
        hint.status,
        MemoryHintStatus::Rejected | MemoryHintStatus::Tombstoned
    )
}

fn hint_affects_decisions(hint: &MemoryHint) -> bool {
    matches!(
        hint.status,
        MemoryHintStatus::Approved | MemoryHintStatus::Pinned | MemoryHintStatus::Deprioritized
    )
}

fn feedback_kind(event: &MemoryRawEvent) -> Option<&'static str> {
    match event.event_type {
        MemoryRawEventType::Approve => Some("approve"),
        MemoryRawEventType::Reject => Some("reject"),
        MemoryRawEventType::Dismiss => Some("dismiss"),
        _ => None,
    }
}

fn dispatch_outcome(event: &MemoryRawEvent) -> Option<(String, String, bool)> {
    let succeeded = match event.event_type {
        MemoryRawEventType::DispatchSuccess | MemoryRawEventType::ValidationPass => true,
        MemoryRawEventType::DispatchFailure | MemoryRawEventType::ValidationFail => false,
        _ => return None,
    };
    let agent_id = json_string(&event.payload_json, "agentId")?;
    let task_type =
        json_string(&event.payload_json, "taskType").unwrap_or_else(|| "unknown_task".to_string());
    Some((agent_id, task_type, succeeded))
}

struct RankingOutcome {
    positive: bool,
}

fn ranking_outcome(event: &MemoryRawEvent) -> Option<RankingOutcome> {
    let failure_class = json_string(&event.payload_json, "failureClass");
    if failure_class
        .as_deref()
        .is_some_and(|class| matches!(class, "policy_blocked" | "user_canceled"))
    {
        return None;
    }

    if let Some(outcome_kind) = json_string(&event.payload_json, "outcomeKind") {
        let positive = match outcome_kind.as_str() {
            "fix_ready" | "completed_no_change" => true,
            "failed" | "blocked" => false,
            "needs_user" | "canceled" => return None,
            _ => return None,
        };
        return Some(RankingOutcome { positive });
    }

    dispatch_outcome(event).map(|(_, _, succeeded)| RankingOutcome {
        positive: succeeded,
    })
}

#[derive(Debug, Clone)]
struct HintPrediction {
    agent_id: Option<String>,
    task_type: Option<String>,
    predicts_success: bool,
}

impl HintPrediction {
    fn matches(&self, agent_id: &str, task_type: &str) -> bool {
        self.agent_id
            .as_deref()
            .is_none_or(|value| value == agent_id)
            && self
                .task_type
                .as_deref()
                .is_none_or(|value| value == task_type)
    }
}

fn hint_prediction(hint: &MemoryHint) -> Option<HintPrediction> {
    let outcome = first_json_string(
        &hint.policy_json,
        &[
            "prediction",
            "predicts",
            "outcome",
            "expectedOutcome",
            "recommendation",
        ],
    )?
    .to_ascii_lowercase();
    let predicts_success = match outcome.as_str() {
        "success" | "succeeded" | "succeeds" | "prefer" | "preferred" | "positive" => true,
        "failure" | "failed" | "fails" | "avoid" | "negative" => false,
        _ => return None,
    };
    Some(HintPrediction {
        agent_id: first_json_string(&hint.policy_json, &["agentId", "agent_id"]),
        task_type: first_json_string(&hint.policy_json, &["taskType", "task_type"]),
        predicts_success,
    })
}

fn repo_from_event(event: &MemoryRawEvent) -> Option<String> {
    if event.subject_type == MemorySubjectType::Repo {
        return Some(event.subject_ref.clone());
    }
    if event.subject_type == MemorySubjectType::Issue {
        return repo_from_issue_ref(&event.subject_ref);
    }
    json_string(&event.payload_json, "repoFullName")
        .or_else(|| json_string(&event.payload_json, "repo"))
        .or_else(|| {
            json_string(&event.payload_json, "issue").and_then(|issue| repo_from_issue_ref(&issue))
        })
}

fn repo_from_issue_ref(value: &str) -> Option<String> {
    value.split_once('#').map(|(repo, _)| repo.to_string())
}

fn event_summary(event: &MemoryRawEvent) -> String {
    format!(
        "{} {} {}",
        event.event_type.as_str(),
        event.subject_type.as_str(),
        event.subject_ref
    )
}

fn first_json_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| json_string(value, key))
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(ToString::to_string)
}

fn time_at_or_before(left: &str, right: &str) -> bool {
    let Ok(left) = DateTime::parse_from_rfc3339(left) else {
        return false;
    };
    let Ok(right) = DateTime::parse_from_rfc3339(right) else {
        return false;
    };
    left.with_timezone(&Utc) <= right.with_timezone(&Utc)
}

fn sorted_unique(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn stable_id(prefix: &str, value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{prefix}-{hash:016x}")
}
