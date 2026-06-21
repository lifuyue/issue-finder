use issue_finder::memory::{
    project_outcome, ranking_adjustment_for_candidate, MemoryHintType, OutcomeFeedbackInput,
    OutcomePriorKind,
};
use issue_finder::recommendation::IssueKey;

#[test]
fn runtime_failures_do_not_become_issue_quality_priors() {
    let environment = outcome("env", true, "failed", Some("workspace_unavailable"), None);
    let projections = project_outcome(&environment);

    assert!(projections
        .iter()
        .all(|projection| projection.prior_kind != OutcomePriorKind::IssueQuality));
    assert!(projections
        .iter()
        .any(|projection| projection.prior_kind == OutcomePriorKind::ExecutionFriction));
    assert_eq!(
        ranking_adjustment_for_candidate(&[environment], "owner/repo", "rust_cli_panic"),
        -26
    );

    let agent_runtime = outcome(
        "agent-runtime",
        true,
        "failed",
        Some("agent_runtime_error"),
        Some("codex"),
    );
    let projections = project_outcome(&agent_runtime);
    assert!(projections
        .iter()
        .all(|projection| projection.hint_type != MemoryHintType::Ranking));
    assert!(projections
        .iter()
        .any(|projection| projection.prior_kind == OutcomePriorKind::AgentSuitability));
    assert_eq!(
        ranking_adjustment_for_candidate(&[agent_runtime], "owner/repo", "rust_cli_panic"),
        0
    );
}

#[test]
fn contribution_outcomes_project_quality_and_friction_separately() {
    let reproduction = outcome(
        "reproduction",
        true,
        "failed",
        Some("reproduction_failed"),
        Some("codex"),
    );
    assert!(project_outcome(&reproduction)
        .iter()
        .any(|projection| projection.prior_kind == OutcomePriorKind::IssueQuality));
    assert_eq!(
        ranking_adjustment_for_candidate(&[reproduction], "owner/repo", "rust_cli_panic"),
        -36
    );

    let success = outcome("success", true, "fix_ready", None, Some("codex"));
    let validation = outcome(
        "validation",
        true,
        "failed",
        Some("validation_failed"),
        Some("codex"),
    );
    assert_eq!(
        ranking_adjustment_for_candidate(&[success, validation], "owner/repo", "rust_cli_panic"),
        -10
    );
}

#[test]
fn candidate_outcome_projections_are_inert_for_eval_adjustment() {
    let candidate = outcome("candidate", false, "fix_ready", None, Some("codex"));
    assert!(project_outcome(&candidate)
        .iter()
        .any(|projection| projection.prior_kind == OutcomePriorKind::IssueQuality));
    assert_eq!(
        ranking_adjustment_for_candidate(&[candidate], "owner/repo", "rust_cli_panic"),
        0
    );
}

fn outcome(
    id: &str,
    approved: bool,
    outcome_kind: &str,
    failure_class: Option<&str>,
    agent_id: Option<&str>,
) -> OutcomeFeedbackInput {
    OutcomeFeedbackInput {
        id: id.to_string(),
        approved,
        issue_key: IssueKey::new("owner/repo", 42),
        repo_scope: Some("owner/repo".to_string()),
        agent_id: agent_id.map(str::to_string),
        outcome_kind: outcome_kind.to_string(),
        failure_class: failure_class.map(str::to_string),
        task_class: Some("rust_cli_panic".to_string()),
        validation_outcome: None,
    }
}
