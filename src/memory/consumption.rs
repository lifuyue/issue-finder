use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use chrono::Utc;

use crate::github::GitHubIssue;
use crate::handoff::{HandoffMemoryContext, HandoffMemoryHint};
use crate::memory::controls::{
    MemoryControlPlane, MemoryDecisionHint, MemoryDecisionHintRequest, MemoryHintScope,
    MemoryRuntimeMode,
};
use crate::memory::model::{MemoryHintScopeType, MemoryHintType};
use crate::memory::store::MemoryStore;
use crate::paths::IssueFinderPaths;
use crate::value_scoring::RankedValueIssue;

const MEMORY_HINT_REFS_PREFIX: &str = "Memory hint refs:";

pub fn apply_ranking_hints_to_ranked(
    paths: &IssueFinderPaths,
    ranked: &mut [RankedValueIssue],
) -> Result<()> {
    let store = MemoryStore::open(paths)?;
    let now = Utc::now().to_rfc3339();
    for item in ranked {
        item.explanation
            .retain(|reason| !reason.starts_with(MEMORY_HINT_REFS_PREFIX));
        item.recommendation
            .reasons
            .retain(|reason| !reason.starts_with(MEMORY_HINT_REFS_PREFIX));

        let hints = decision_hints_for_scope(
            &store,
            MemoryHintType::Ranking,
            MemoryHintScopeType::Repo,
            item.issue.repo_full_name.as_str(),
            &now,
        )?;
        if hints.is_empty() {
            continue;
        }
        let refs = hints
            .iter()
            .map(|hint| hint.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let reason = format!("{MEMORY_HINT_REFS_PREFIX} {refs}");
        if !item.explanation.contains(&reason) {
            item.explanation.push(reason.clone());
        }
        if !item.recommendation.reasons.contains(&reason) {
            item.recommendation.reasons.push(reason);
        }
    }
    Ok(())
}

pub fn handoff_memory_context_for_issue(
    paths: &IssueFinderPaths,
    issue: &GitHubIssue,
) -> Result<HandoffMemoryContext> {
    let store = MemoryStore::open(paths)?;
    let now = Utc::now().to_rfc3339();
    let ranking_hints = decision_hints_for_scope(
        &store,
        MemoryHintType::Ranking,
        MemoryHintScopeType::Repo,
        issue.repo_full_name.as_str(),
        &now,
    )?;
    let repo_dispatch_hints = decision_hints_for_scope(
        &store,
        MemoryHintType::Dispatch,
        MemoryHintScopeType::Repo,
        issue.repo_full_name.as_str(),
        &now,
    )?;
    let agent_dispatch_hints = agent_dispatch_hints(&store, &now)?;

    let mut hint_by_id = BTreeMap::new();
    for hint in ranking_hints
        .into_iter()
        .chain(repo_dispatch_hints)
        .chain(agent_dispatch_hints)
    {
        hint_by_id.entry(hint.id.clone()).or_insert(hint);
    }

    let approved_hints = hint_by_id
        .into_values()
        .map(|hint| HandoffMemoryHint {
            id: hint.id,
            hint_type: hint.hint_type,
            scope_type: hint.scope_type,
            scope_ref: hint.scope_ref,
            summary: hint.summary,
            effective_weight: hint.effective_weight,
        })
        .collect::<Vec<_>>();
    let evidence_refs = approved_hints
        .iter()
        .map(|hint| format!("memory_hint:{}", hint.id))
        .collect::<Vec<_>>();
    let agent_selection_notes = approved_hints
        .iter()
        .filter(|hint| hint.hint_type == MemoryHintType::Dispatch.as_str())
        .map(|hint| hint.summary.clone())
        .collect::<Vec<_>>();
    Ok(HandoffMemoryContext {
        approved_hints,
        activation_run_id: None,
        evidence_refs,
        risk_notes: Vec::new(),
        agent_selection_notes,
    })
}

#[derive(Debug, Clone, PartialEq)]
struct ConsumedMemoryHint {
    id: String,
    hint_type: String,
    scope_type: String,
    scope_ref: String,
    summary: String,
    effective_weight: f64,
}

fn decision_hints_for_scope(
    store: &MemoryStore,
    hint_type: MemoryHintType,
    scope_type: MemoryHintScopeType,
    scope_ref: &str,
    now: &str,
) -> Result<Vec<ConsumedMemoryHint>> {
    decision_hints_for_scope_with_limit(store, hint_type, scope_type, scope_ref, now, 5)
}

fn decision_hints_for_scope_with_limit(
    store: &MemoryStore,
    hint_type: MemoryHintType,
    scope_type: MemoryHintScopeType,
    scope_ref: &str,
    now: &str,
    limit: usize,
) -> Result<Vec<ConsumedMemoryHint>> {
    let hints = MemoryControlPlane::decision_eligible_hints(
        store,
        &MemoryDecisionHintRequest {
            mode: MemoryRuntimeMode::Enabled,
            hint_type: Some(hint_type),
            scope: Some(MemoryHintScope {
                scope_type,
                scope_ref: scope_ref.to_string(),
            }),
            now: Some(now.to_string()),
            limit,
        },
    )?;
    Ok(hints.into_iter().map(consumed_hint).collect())
}

fn agent_dispatch_hints(store: &MemoryStore, now: &str) -> Result<Vec<ConsumedMemoryHint>> {
    let agent_scope_refs = store
        .list_hints()?
        .into_iter()
        .filter(|hint| hint.hint_type == MemoryHintType::Dispatch)
        .filter(|hint| hint.scope_type == MemoryHintScopeType::Agent)
        .map(|hint| hint.scope_ref)
        .collect::<BTreeSet<_>>();

    let mut hints = Vec::new();
    for scope_ref in agent_scope_refs {
        hints.extend(decision_hints_for_scope_with_limit(
            store,
            MemoryHintType::Dispatch,
            MemoryHintScopeType::Agent,
            scope_ref.as_str(),
            now,
            5,
        )?);
    }
    Ok(hints)
}

fn consumed_hint(decision: MemoryDecisionHint) -> ConsumedMemoryHint {
    ConsumedMemoryHint {
        id: decision.hint.id,
        hint_type: decision.hint.hint_type.as_str().to_string(),
        scope_type: decision.hint.scope_type.as_str().to_string(),
        scope_ref: decision.hint.scope_ref,
        summary: decision.hint.summary,
        effective_weight: decision.effective_weight,
    }
}
