use anyhow::Result;
use chrono::Utc;

use crate::github::GitHubIssue;
use crate::handoff::{HandoffMemoryContext, HandoffMemoryHint};
use crate::memory::controls::{
    MemoryControlPlane, MemoryDecisionHintRequest, MemoryHintScope, MemoryRuntimeMode,
};
use crate::memory::model::{MemoryHintScopeType, MemoryHintType};
use crate::memory::store::MemoryStore;
use crate::paths::IssueFinderPaths;
use crate::value_scoring::RankedValueIssue;

pub fn apply_ranking_hints_to_ranked(
    paths: &IssueFinderPaths,
    ranked: &mut [RankedValueIssue],
) -> Result<()> {
    let store = MemoryStore::open(paths)?;
    let now = Utc::now().to_rfc3339();
    for item in ranked {
        let hints = decision_hints_for_repo(
            &store,
            MemoryHintType::Ranking,
            &item.issue.repo_full_name,
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
        let reason = format!("Memory hint refs: {refs}");
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
    let ranking_hints =
        decision_hints_for_repo(&store, MemoryHintType::Ranking, &issue.repo_full_name, &now)?;
    let dispatch_hints = decision_hints_for_repo(
        &store,
        MemoryHintType::Dispatch,
        &issue.repo_full_name,
        &now,
    )?;
    let mut approved_hints = ranking_hints
        .into_iter()
        .chain(dispatch_hints)
        .map(|hint| HandoffMemoryHint {
            id: hint.id,
            hint_type: hint.hint_type,
            scope_type: hint.scope_type,
            scope_ref: hint.scope_ref,
            summary: hint.summary,
            effective_weight: hint.effective_weight.unwrap_or(hint.weight),
        })
        .collect::<Vec<_>>();
    approved_hints.sort_by(|left, right| left.id.cmp(&right.id));
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

fn decision_hints_for_repo(
    store: &MemoryStore,
    hint_type: MemoryHintType,
    repo_full_name: &str,
    now: &str,
) -> Result<Vec<crate::memory::commands::MemoryHintOutput>> {
    let hints = MemoryControlPlane::decision_eligible_hints(
        store,
        &MemoryDecisionHintRequest {
            mode: MemoryRuntimeMode::Enabled,
            hint_type: Some(hint_type),
            scope: Some(MemoryHintScope {
                scope_type: MemoryHintScopeType::Repo,
                scope_ref: repo_full_name.to_string(),
            }),
            now: Some(now.to_string()),
            limit: 5,
        },
    )?;
    Ok(hints
        .into_iter()
        .map(|decision| crate::memory::commands::MemoryHintOutput {
            id: decision.hint.id,
            dream_id: decision.hint.dream_id,
            hint_type: decision.hint.hint_type.as_str().to_string(),
            scope_type: decision.hint.scope_type.as_str().to_string(),
            scope_ref: decision.hint.scope_ref,
            summary: decision.hint.summary,
            weight: decision.hint.weight,
            effective_weight: Some(decision.effective_weight),
            status: decision.hint.status.as_str().to_string(),
            created_at: decision.hint.created_at,
            approved_at: decision.hint.approved_at,
            expires_at: decision.hint.expires_at,
            policy: decision.hint.policy_json,
        })
        .collect())
}
