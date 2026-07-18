use anyhow::Result;
use serde::Serialize;

use crate::dispatch::{
    ApprovalStatus, DispatchProposalRequest, DispatchRunStatus, DispatchRuntime, IssueTaskStatus,
    NewIssueTask, TaskIdentity, TaskPackage,
};
use crate::paths::IssueFinderPaths;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryEvalScenario {
    pub kind: &'static str,
    pub version: u32,
    pub scenario: String,
    pub marker: String,
    pub issue_ref: String,
    pub run_id: String,
    pub workspace: String,
    pub database: String,
}

pub fn prepare_recovery_eval(
    paths: IssueFinderPaths,
    scenario: &str,
    workspace: &str,
    marker: String,
) -> Result<RecoveryEvalScenario> {
    if !matches!(scenario, "E01" | "E02" | "E03") {
        anyhow::bail!("unsupported recovery evaluation scenario {scenario}");
    }
    std::fs::create_dir_all(workspace)?;
    let runtime = DispatchRuntime::open(paths.clone())?;
    let number = 10_000
        + marker.bytes().fold(0_u64, |value, byte| {
            value.wrapping_mul(31) + u64::from(byte)
        }) % 80_000;
    let repo = format!("recovery-{}/runtime", marker.to_ascii_lowercase());
    let issue_ref = format!("{repo}#{number}");
    let title = format!("Recovery boundary {scenario} {marker}");
    let url = format!("https://github.com/{repo}/issues/{number}");
    let task = runtime.store().upsert_issue_task(NewIssueTask {
        repo_full_name: repo.clone(),
        issue_number: number,
        title: title.clone(),
        url: url.clone(),
        status: IssueTaskStatus::UserApproved,
        priority: Some(100),
        category: Some("high_value_ready".to_string()),
    })?;
    let mut package = TaskPackage::new(TaskIdentity {
        repo_full_name: repo,
        issue_number: number,
        title,
        url,
    });
    package.workspace.path = workspace.to_string();
    package.context_snapshot.snapshot_id = "recovery-eval-snapshot".to_string();
    package.context_snapshot.artifact_id = "recovery-eval-snapshot-artifact".to_string();
    package.context_snapshot.entry_artifact_id = "recovery-eval-entry-artifact".to_string();
    runtime
        .store()
        .write_task_package_artifact(&task.id, &package)?;
    let proposal = runtime.propose_dispatch(DispatchProposalRequest {
        issue: issue_ref.clone(),
        agent_id: "codex".to_string(),
        requested_by: "recovery-eval".to_string(),
        selected_thread_id: None,
        new_session: true,
    })?;
    let approved = runtime.resolve_dispatch_approval(&proposal.run.id, ApprovalStatus::Approved)?;
    if scenario == "E03" {
        runtime.store().update_dispatch_run_status(
            &approved.run.id,
            DispatchRunStatus::Running,
            None,
        )?;
    }
    Ok(RecoveryEvalScenario {
        kind: "issue_finder_recovery_eval_scenario",
        version: 1,
        scenario: scenario.to_string(),
        marker,
        issue_ref,
        run_id: approved.run.id,
        workspace: workspace.to_string(),
        database: paths.dispatch_db_path().to_string_lossy().to_string(),
    })
}
