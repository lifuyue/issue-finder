use std::path::Path;

use issue_finder::dispatch::{
    ApprovalStatus, DispatchOutcomeFailureClass, DispatchOutcomeKind, DispatchOutcomeRecordRequest,
    DispatchProposalRequest, DispatchRuntime, DispatchTaskClass, DispatchValidationOutcome,
    IssueTaskPackage, IssueTaskPackageIssue, IssueTaskStatus, MemoryEventType, NewIssueTask,
};
use issue_finder::github::GitHubIssue;
use issue_finder::handoff::{write_handoff, Handoff};
use issue_finder::inbox::upsert_ready;
use issue_finder::memory::{sync_dispatch_outcome_feedback, MemoryRawEventType, MemoryStore};
use issue_finder::paths::IssueFinderPaths;
use issue_finder::repo_scan::{CandidateFile, RepoScan, ValidationCommand};
use issue_finder::workspace::{PreparedWorkspace, WorkspaceInfo};
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
fn issue_review_rejection_records_memory_and_blocks_dispatch() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let issue = github_issue("owner/repo", 456);
    let handoff = Handoff::build(&issue, &prepared_workspace(dir.path()));
    let written = write_handoff(&paths, &handoff, &issue).unwrap();
    upsert_ready(&paths, &issue, 90, &written).unwrap();
    let runtime = DispatchRuntime::open(paths).unwrap();

    let imported = runtime.import_handoff_from_inbox(&written.id).unwrap();
    assert!(imported.package_artifact.is_none());
    let rejected = runtime
        .reject_issue_review(
            &imported.approval_request.id,
            Some("not worth dispatching".to_string()),
        )
        .unwrap();

    assert_eq!(rejected.approval_request.status, ApprovalStatus::Rejected);
    assert!(rejected.package_artifact.is_none());
    let memory = runtime
        .store()
        .list_memory_events_for_issue_task(&imported.issue_task.id)
        .unwrap();
    assert_eq!(memory.len(), 1);
    assert_eq!(memory[0].event_type, MemoryEventType::NegativeSignal);
    assert_eq!(memory[0].source, "issue_review");
    assert_eq!(memory[0].payload_json["signal"], "issue_review_rejected");
    assert_eq!(memory[0].payload_json["reason"], "not worth dispatching");

    let error = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#456".to_string(),
            agent_id: "codex".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: None,
            new_session: true,
        })
        .unwrap_err();
    assert!(error.to_string().contains("was rejected"));
    assert!(runtime
        .store()
        .get_issue_task(&imported.issue_task.id)
        .unwrap()
        .current_package_artifact_id
        .is_none());
}

#[test]
fn dispatch_outcome_record_leaves_hybrid_memory_to_memory_projector() {
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
            failure_class: Some(DispatchOutcomeFailureClass::ValidationFailed),
            failure_detail: Some("cargo test still fails".to_string()),
            task_class: Some(DispatchTaskClass::RustCliPanic),
            validation_outcome: Some(DispatchValidationOutcome::Failed),
            result_artifact_id: None,
            metadata_json: serde_json::json!({ "source": "test" }),
        })
        .unwrap();

    assert_eq!(result.run.status.to_string(), "failed");
    assert_eq!(result.outcome.outcome_kind, DispatchOutcomeKind::Failed);
    let memory = runtime
        .store()
        .list_memory_events_for_issue_task(&task.id)
        .unwrap();
    assert_eq!(memory.len(), 1);
    assert_eq!(memory[0].source, "dispatch_approval");

    let raw_events = MemoryStore::open(&paths)
        .unwrap()
        .list_raw_events()
        .unwrap();
    assert!(raw_events.is_empty());

    let sync = sync_dispatch_outcome_feedback(&paths).unwrap();
    assert_eq!(sync.projected_outcomes, 1);

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

fn github_issue(repo_full_name: &str, number: u64) -> GitHubIssue {
    GitHubIssue {
        id: number,
        number,
        title: "Fix parser panic".to_string(),
        body: "Expected graceful behavior in src/lib.rs".to_string(),
        labels: vec!["good first issue".to_string()],
        url: format!("https://github.com/{repo_full_name}/issues/{number}"),
        repo_full_name: repo_full_name.to_string(),
        repo_name: repo_full_name.split('/').nth(1).unwrap().to_string(),
        repo_description: "Rust CLI".to_string(),
        repo_stars: 10,
        created_at: "2026-06-20T00:00:00Z".to_string(),
        updated_at: "2026-06-20T00:00:00Z".to_string(),
    }
}

fn prepared_workspace(root: &Path) -> PreparedWorkspace {
    PreparedWorkspace {
        info: WorkspaceInfo {
            path: root.join("repo").to_string_lossy().to_string(),
            default_branch: "main".to_string(),
            branch: "issue-finder/456-fix-parser-panic".to_string(),
            dirty: false,
        },
        scan: RepoScan {
            discovered_files: vec!["src/lib.rs".to_string()],
            candidate_files: vec![CandidateFile {
                path: "src/lib.rs".to_string(),
                reason: "Mentioned by issue title".to_string(),
            }],
            validation_commands: vec![ValidationCommand {
                command: "cargo test".to_string(),
                reason: "Rust project".to_string(),
            }],
            warnings: Vec::new(),
        },
        warnings: Vec::new(),
    }
}
