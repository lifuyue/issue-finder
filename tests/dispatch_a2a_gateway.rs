use std::path::Path;

use issue_finder::dispatch::{
    ApprovalStatus, ApprovalType, DispatchOutcomeKind, DispatchRunStatus, DispatchRuntime,
    IssueTaskPackage, IssueTaskPackageIssue, IssueTaskStatus, MemoryEventType, NewDispatchRun,
    NewIssueTask,
};
use issue_finder::paths::IssueFinderPaths;
use tempfile::tempdir;

#[test]
fn a2a_export_creates_local_task_artifact_and_pending_send_approval() {
    let dir = tempdir().unwrap();
    let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
    create_packaged_task(&runtime);

    let export = runtime.export_a2a_task("owner/repo#123").unwrap();

    assert_eq!(export.status, "pending_approval");
    assert_eq!(export.task.task.task_type, "fix_github_issue");
    assert_eq!(export.task.task.issue_key, "owner/repo#123");
    assert_eq!(export.task.callback.import_mode, "local_artifact_only");
    assert_eq!(export.export_artifact.kind, "a2a_task_export");
    assert_eq!(export.approval_request.approval_type, ApprovalType::A2aSend);
    assert_eq!(export.approval_request.status, ApprovalStatus::Pending);
    assert_eq!(export.approval_request.run_id, None);
    assert_eq!(
        export.approval_request.details_json["a2aTaskArtifactId"],
        export.export_artifact.id
    );

    let artifact_bytes = runtime
        .store()
        .read_artifact_bytes(&export.export_artifact.id)
        .unwrap();
    let task: issue_finder::dispatch::A2aTaskExport =
        serde_json::from_slice(&artifact_bytes).unwrap();
    assert_eq!(task, export.task);

    let approved = runtime
        .approve_a2a_send(&export.approval_request.id)
        .unwrap();
    assert_eq!(approved.approval_request.status, ApprovalStatus::Approved);
    assert_eq!(approved.export_artifact.id, export.export_artifact.id);
    assert_eq!(approved.task.task.issue_key, "owner/repo#123");
}

#[test]
fn a2a_send_approval_can_be_rejected_without_external_send() {
    let dir = tempdir().unwrap();
    let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
    create_packaged_task(&runtime);
    let export = runtime.export_a2a_task("owner/repo#123").unwrap();

    let rejected = runtime
        .reject_a2a_send(&export.approval_request.id)
        .unwrap();

    assert_eq!(rejected.approval_request.status, ApprovalStatus::Rejected);
    assert_eq!(rejected.export_artifact.id, export.export_artifact.id);
}

#[test]
fn completed_a2a_fix_result_marks_issue_task_fix_ready() {
    let dir = tempdir().unwrap();
    let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
    let task = create_packaged_task(&runtime);
    let run = runtime
        .store()
        .create_dispatch_run(NewDispatchRun {
            issue_task_id: task.id.clone(),
            agent_id: "codex".to_string(),
            status: DispatchRunStatus::Running,
            requested_by: "test".to_string(),
            approval_state: ApprovalStatus::Approved,
            selected_session_link_id: None,
        })
        .unwrap();
    let result_path = dir.path().join("fix_result.json");
    std::fs::write(&result_path, r#"{"summary":"fixed"}"#).unwrap();

    let imported = runtime
        .import_a2a_result(
            &run.id,
            &result_path,
            "fix_result",
            "application/json",
            Some(DispatchRunStatus::Completed),
            None,
        )
        .unwrap();

    assert_eq!(imported.run.status, DispatchRunStatus::Completed);
    assert_eq!(
        imported
            .outcome
            .as_ref()
            .map(|outcome| outcome.outcome_kind),
        Some(DispatchOutcomeKind::FixReady)
    );
    assert_eq!(
        imported.run.result_artifact_id.as_deref(),
        Some(imported.artifact.id.as_str())
    );
    assert_eq!(
        runtime.store().get_issue_task(&task.id).unwrap().status,
        IssueTaskStatus::FixReady
    );
    let events = runtime.store().list_agent_events_for_run(&run.id).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_type, "a2a_result_imported");
    assert_eq!(events[1].event_type, "dispatch_outcome_recorded");
    assert_eq!(
        events[0].payload_json["artifactId"].as_str(),
        Some(imported.artifact.id.as_str())
    );
    let memory = runtime
        .store()
        .list_memory_events_for_issue_task(&task.id)
        .unwrap();
    assert_eq!(memory.len(), 1);
    assert_eq!(memory[0].event_type, MemoryEventType::PositiveSignal);
    assert_eq!(memory[0].source, "dispatch_outcome");
}

fn create_packaged_task(runtime: &DispatchRuntime) -> issue_finder::dispatch::IssueTask {
    let task = runtime
        .store()
        .upsert_issue_task(NewIssueTask {
            repo_full_name: "owner/repo".to_string(),
            issue_number: 123,
            title: "Fix parser panic".to_string(),
            url: "https://github.com/owner/repo/issues/123".to_string(),
            status: IssueTaskStatus::UserApproved,
            priority: Some(10),
            category: Some("high_value_ready".to_string()),
        })
        .unwrap();
    let package = IssueTaskPackage::new(IssueTaskPackageIssue {
        repo_full_name: "owner/repo".to_string(),
        number: 123,
        title: "Fix parser panic".to_string(),
        url: "https://github.com/owner/repo/issues/123".to_string(),
    });
    runtime
        .store()
        .write_task_package_artifact(&task.id, &package)
        .unwrap();
    runtime.store().get_issue_task(&task.id).unwrap()
}

fn test_paths(root: &Path) -> IssueFinderPaths {
    IssueFinderPaths {
        home: root.to_path_buf(),
        config: root.join("config.toml"),
        cache_dir: root.join("cache"),
        workspaces_dir: root.join("workspaces"),
        inbox_dir: root.join("inbox"),
        reports_dir: root.join("reports"),
    }
}
