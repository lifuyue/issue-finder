use issue_finder::dispatch::{
    ApprovalStatus, DispatchOutcomeKind, DispatchRunStatus, DispatchRuntime, IssueTaskStatus,
    NewAgentProfile, NewDispatchRun, NewDispatchRunOutcome, NewIssueTask,
};
use issue_finder::paths::IssueFinderPaths;
use serde_json::json;
use tempfile::tempdir;

#[test]
fn terminal_outcome_and_run_task_state_commit_atomically() {
    let dir = tempdir().unwrap();
    let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
    runtime
        .store()
        .create_agent_profile(NewAgentProfile {
            id: Some("test-agent".to_string()),
            kind: "test".to_string(),
            display_name: "Test".to_string(),
            adapter: "fake".to_string(),
            config_json: json!({}),
            enabled: true,
        })
        .unwrap();
    let task = runtime
        .store()
        .upsert_issue_task(NewIssueTask {
            repo_full_name: "owner/repo".to_string(),
            issue_number: 2,
            title: "Recover outcome projection".to_string(),
            url: "https://github.com/owner/repo/issues/2".to_string(),
            status: IssueTaskStatus::InProgress,
            priority: None,
            category: None,
        })
        .unwrap();
    let run = runtime
        .store()
        .create_dispatch_run(NewDispatchRun {
            issue_task_id: task.id.clone(),
            agent_id: "test-agent".to_string(),
            status: DispatchRunStatus::Running,
            requested_by: "test".to_string(),
            approval_state: ApprovalStatus::Approved,
            selected_thread_id: Some("thread-2".to_string()),
        })
        .unwrap();
    let idempotency_key = format!("terminal:{}", run.id);
    runtime
        .store()
        .record_dispatch_run_outcome(NewDispatchRunOutcome {
            run_id: run.id.clone(),
            idempotency_key: idempotency_key.clone(),
            outcome_kind: DispatchOutcomeKind::Success,
            failure_class: None,
            failure_detail: None,
            task_class: None,
            validation_outcome: None,
            result_artifact_id: None,
            metadata_json: json!({"source":"recovery-test"}),
        })
        .unwrap();
    assert_eq!(
        runtime.store().get_dispatch_run(&run.id).unwrap().status,
        DispatchRunStatus::Succeeded
    );
    assert_eq!(
        runtime.store().get_issue_task(&task.id).unwrap().status,
        IssueTaskStatus::Succeeded
    );
    runtime
        .store()
        .record_dispatch_run_outcome(NewDispatchRunOutcome {
            run_id: run.id.clone(),
            idempotency_key,
            outcome_kind: DispatchOutcomeKind::Success,
            failure_class: None,
            failure_detail: None,
            task_class: None,
            validation_outcome: None,
            result_artifact_id: None,
            metadata_json: json!({"source":"recovery-test"}),
        })
        .unwrap();

    assert_eq!(
        runtime
            .store()
            .list_dispatch_run_outcomes()
            .unwrap()
            .into_iter()
            .filter(|outcome| outcome.run_id == run.id)
            .count(),
        1
    );
}

fn test_paths(root: &std::path::Path) -> IssueFinderPaths {
    IssueFinderPaths {
        home: root.to_path_buf(),
        config: root.join("config.toml"),
        cache_dir: root.join("cache"),
        workspaces_dir: root.join("workspaces"),
        inbox_dir: root.join("inbox"),
        reports_dir: root.join("reports"),
    }
}
