use issue_finder::dispatch::{
    ApprovalStatus, CapabilityStatus, DispatchEventKind, DispatchOutcomeKind,
    DispatchOutcomeRecordRequest, DispatchRunStatus, DispatchRuntime, IssueTaskStatus,
    NewAgentCapability, NewAgentProfile, NewArtifact, NewDispatchRun, NewDispatchRunOutcome,
    NewIssueTask,
};
use issue_finder::paths::IssueFinderPaths;
use serde_json::json;
use tempfile::tempdir;

#[test]
fn fix_ready_requires_owned_validated_fix_result() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
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
    runtime
        .store()
        .upsert_agent_capability(NewAgentCapability {
            agent_id: "test-agent".to_string(),
            capability: issue_finder::dispatch::AgentCapabilityName::StartSession,
            status: CapabilityStatus::Supported,
            details_json: json!({}),
        })
        .unwrap();
    let task = runtime
        .store()
        .upsert_issue_task(NewIssueTask {
            repo_full_name: "owner/repo".to_string(),
            issue_number: 1,
            title: "Validate outcome".to_string(),
            url: "https://github.com/owner/repo/issues/1".to_string(),
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
            selected_thread_id: None,
        })
        .unwrap();
    let request = DispatchOutcomeRecordRequest {
        run_id: run.id.clone(),
        idempotency_key: None,
        outcome_kind: DispatchOutcomeKind::FixReady,
        failure_class: None,
        failure_detail: None,
        task_class: None,
        validation_outcome: None,
        result_artifact_id: None,
        metadata_json: json!({}),
    };
    assert!(runtime
        .record_dispatch_outcome(request)
        .unwrap_err()
        .to_string()
        .contains("requires a fix_result"));

    let artifact = runtime
        .store()
        .write_artifact(
            NewArtifact {
                issue_task_id: Some(task.id),
                run_id: Some(run.id.clone()),
                kind: "fix_result".to_string(),
                content_type: "application/json".to_string(),
                metadata_json: json!({}),
            },
            br#"{"status":"fix_ready","validationOutcome":"passed"}"#,
        )
        .unwrap();
    let result = runtime
        .record_dispatch_outcome(DispatchOutcomeRecordRequest {
            run_id: run.id,
            idempotency_key: None,
            outcome_kind: DispatchOutcomeKind::FixReady,
            failure_class: None,
            failure_detail: None,
            task_class: None,
            validation_outcome: Some(issue_finder::dispatch::DispatchValidationOutcome::Passed),
            result_artifact_id: Some(artifact.id),
            metadata_json: json!({}),
        })
        .unwrap();
    assert_eq!(result.run.status, DispatchRunStatus::Completed);
}

#[test]
fn outcome_recording_recovers_after_crash_between_outcome_insert_and_projection() {
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
            issue_task_id: task.id,
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
            outcome_kind: DispatchOutcomeKind::CompletedNoChange,
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
        DispatchRunStatus::Running
    );

    for _ in 0..2 {
        let result = runtime
            .record_dispatch_outcome(DispatchOutcomeRecordRequest {
                run_id: run.id.clone(),
                idempotency_key: Some(idempotency_key.clone()),
                outcome_kind: DispatchOutcomeKind::CompletedNoChange,
                failure_class: None,
                failure_detail: None,
                task_class: None,
                validation_outcome: None,
                result_artifact_id: None,
                metadata_json: json!({"source":"recovery-test"}),
            })
            .unwrap();
        assert_eq!(result.run.status, DispatchRunStatus::Completed);
    }

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
    assert_eq!(
        runtime
            .store()
            .list_dispatch_events_for_run(&run.id)
            .unwrap()
            .into_iter()
            .filter(|event| event.event_kind == DispatchEventKind::DispatchOutcomeRecorded)
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
