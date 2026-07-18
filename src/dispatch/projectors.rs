use anyhow::Result;
use serde::Serialize;

use crate::memory::sync_dispatch_outcome_feedback;

use super::github_projection;
use super::model::DispatchRunOutcome;
use super::store::DispatchStore;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionReport {
    pub outcome_id: String,
    pub memory_projected: usize,
    pub github_decision: String,
}

pub fn project_terminal_outcome(
    store: &DispatchStore,
    outcome: &DispatchRunOutcome,
) -> Result<ProjectionReport> {
    let memory = sync_dispatch_outcome_feedback(&store.paths())?;
    let github_decision = if let Some(existing) = store
        .list_github_interaction_decisions_for_run(&outcome.run_id)?
        .into_iter()
        .next()
    {
        existing.decision_kind.to_string()
    } else {
        github_projection::draft_final_comment(store, &outcome.run_id, None)?
            .decision
            .decision_kind
            .to_string()
    };
    Ok(ProjectionReport {
        outcome_id: outcome.id.clone(),
        memory_projected: memory.projected_outcomes,
        github_decision,
    })
}
