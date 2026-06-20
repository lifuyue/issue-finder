use std::path::Path;

use issue_finder::dispatch::{
    ApprovalStatus, DispatchFailureClass, DispatchOutcomeKind, DispatchOutcomeRecordRequest,
    DispatchProposalRequest, DispatchRuntime, DispatchTaskClass, DispatchValidationOutcome,
    IssueTaskPackage, IssueTaskPackageIssue, IssueTaskStatus, MemoryEventType, NewIssueTask,
};
use issue_finder::memory::{MemoryRawEventType, MemoryStore};
use issue_finder::paths::IssueFinderPaths;
use tempfile::tempdir;

#[test]
fn dispatch_approval_resolution_records_memory_signals() {
    let dir = tempdir().unwrap();
    let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
    let task = create_packaged_task(&runtime, 123);
    let approved = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#123".to_string(),
            agent_id: "codex".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: None,
            new_session: true,
        })
        .unwrap();
    runtime
        .resolve_dispatch_approval(&approved.run.id, ApprovalStatus::Approved)
        .unwrap();

    let memory = runtime
        .store()
        .list_memory_events_for_issue_task(&task.id)
        .unwrap();
    assert_eq!(memory.len(), 1);
    assert_eq!(memory[0].event_type, MemoryEventType::PositiveSignal);
    assert_eq!(memory[0].source, "dispatch_approval");
    assert_eq!(memory[0].payload_json["signal"], "dispatch_approved");
    assert_eq!(memory[0].payload_json["issueKey"], "owner/repo#123");
    assert_eq!(memory[0].payload_json["agentId"], "codex");

    let rejected = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#123".to_string(),
            agent_id: "codex".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: None,
            new_session: true,
        })
        .unwrap();
    runtime
        .resolve_dispatch_approval(&rejected.run.id, ApprovalStatus::Rejected)
        .unwrap();

    let memory = runtime
        .store()
        .list_memory_events_for_issue_task(&task.id)
        .unwrap();
    assert_eq!(memory.len(), 2);
    assert_eq!(memory[1].event_type, MemoryEventType::NegativeSignal);
    assert_eq!(memory[1].payload_json["signal"], "dispatch_rejected");
}

#[test]
fn dispatch_outcome_record_ingests_hybrid_memory_best_effort() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths.clone()).unwrap();
    let task = create_packaged_task(&runtime, 456);
    let proposal = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#456".to_string(),
            agent_id: "codex".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: None,
            new_session: true,
        })
        .unwrap();
    runtime
        .resolve_dispatch_approval(&proposal.run.id, ApprovalStatus::Approved)
        .unwrap();

    let result = runtime
        .record_dispatch_outcome(DispatchOutcomeRecordRequest {
            run_id: proposal.run.id,
            idempotency_key: Some("outcome-456".to_string()),
            outcome_kind: DispatchOutcomeKind::Failed,
            failure_class: Some(DispatchFailureClass::ValidationFailed),
            failure_detail: Some("cargo test still fails".to_string()),
            task_class: Some(DispatchTaskClass::RustCliPanic),
            validation_outcome: Some(DispatchValidationOutcome::Failed),
            result_artifact_id: None,
            metadata_json: serde_json::json!({ "source": "test" }),
        })
        .unwrap();

    assert_eq!(result.run.status.to_string(), "failed");
    assert_eq!(result.outcome.outcome_kind, DispatchOutcomeKind::Failed);
    assert!(result.memory_ingest.is_some());
    let memory = runtime
        .store()
        .list_memory_events_for_issue_task(&task.id)
        .unwrap();
    assert_eq!(memory.len(), 2);
    assert_eq!(
        memory[1].event_type,
        MemoryEventType::AgentPerformanceSignal
    );
    assert_eq!(memory[1].payload_json["failureClass"], "validation_failed");

    let raw_events = MemoryStore::open(&paths)
        .unwrap()
        .list_raw_events()
        .unwrap();
    assert_eq!(raw_events.len(), 1);
    assert_eq!(
        raw_events[0].event_type,
        MemoryRawEventType::DispatchFailure
    );
    assert_eq!(raw_events[0].payload_json["outcomeKind"], "failed");
    assert_eq!(
        raw_events[0].payload_json["failureClass"],
        "validation_failed"
    );
}

fn create_packaged_task(
    runtime: &DispatchRuntime,
    number: u64,
) -> issue_finder::dispatch::IssueTask {
    let task = runtime
        .store()
        .upsert_issue_task(NewIssueTask {
            repo_full_name: "owner/repo".to_string(),
            issue_number: number,
            title: "Fix parser panic".to_string(),
            url: format!("https://github.com/owner/repo/issues/{number}"),
            status: IssueTaskStatus::UserApproved,
            priority: Some(10),
            category: Some("high_value_ready".to_string()),
        })
        .unwrap();
    let package = IssueTaskPackage::new(IssueTaskPackageIssue {
        repo_full_name: "owner/repo".to_string(),
        number,
        title: "Fix parser panic".to_string(),
        url: format!("https://github.com/owner/repo/issues/{number}"),
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
