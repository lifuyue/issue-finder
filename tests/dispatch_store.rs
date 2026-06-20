use std::fs;
use std::path::Path;

use issue_finder::dispatch::{
    AgentCapabilityName, AgentSessionStatus, ApprovalStatus, ApprovalType, CapabilityStatus,
    DispatchRunStatus, DispatchStore, GitHubInteractionStatus, GitHubInteractionType,
    IssueTaskPackage, IssueTaskPackageIssue, IssueTaskStatus, MemoryEventType, NewAgentCapability,
    NewAgentEvent, NewAgentProfile, NewAgentSessionLink, NewApprovalRequest, NewArtifact,
    NewDispatchRun, NewGitHubInteraction, NewIssueTask, NewMemoryEvent,
};
use issue_finder::paths::IssueFinderPaths;
use serde_json::json;
use tempfile::tempdir;

#[test]
fn dispatch_store_creates_schema_and_persists_core_state() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = DispatchStore::open(paths.clone()).unwrap();
    assert!(paths.dispatch_db_path().exists());

    let agent = store
        .create_agent_profile(NewAgentProfile {
            id: Some("codex".to_string()),
            kind: "codex".to_string(),
            display_name: "Codex".to_string(),
            adapter: "codex_app_server".to_string(),
            config_json: json!({ "mode": "local" }),
            enabled: true,
        })
        .unwrap();
    assert_eq!(agent.id, "codex");
    assert_eq!(agent.config_json["mode"], "local");
    assert_eq!(store.list_agent_profiles().unwrap().len(), 1);

    let start_session = store
        .upsert_agent_capability(NewAgentCapability {
            agent_id: agent.id.clone(),
            capability: AgentCapabilityName::StartSession,
            status: CapabilityStatus::Experimental,
            details_json: json!({ "method": "thread/start" }),
        })
        .unwrap();
    assert_eq!(start_session.status, CapabilityStatus::Experimental);
    let open_pr = store
        .upsert_agent_capability(NewAgentCapability {
            agent_id: agent.id.clone(),
            capability: AgentCapabilityName::OpenPr,
            status: CapabilityStatus::Unsupported,
            details_json: json!({ "reason": "not implemented in milestone 1" }),
        })
        .unwrap();
    assert_eq!(open_pr.status, CapabilityStatus::Unsupported);
    assert_eq!(store.list_agent_capabilities(&agent.id).unwrap().len(), 2);

    let task = store
        .upsert_issue_task(NewIssueTask {
            repo_full_name: "owner/repo".to_string(),
            issue_number: 123,
            title: "Fix parser panic".to_string(),
            url: "https://github.com/owner/repo/issues/123".to_string(),
            status: IssueTaskStatus::Discovered,
            priority: Some(10),
            category: Some("high_value_ready".to_string()),
        })
        .unwrap();
    assert_eq!(task.issue_key, "owner/repo#123");
    assert_eq!(task.status, IssueTaskStatus::Discovered);

    let updated = store
        .upsert_issue_task(NewIssueTask {
            repo_full_name: "owner/repo".to_string(),
            issue_number: 123,
            title: "Fix parser panic in tokenizer".to_string(),
            url: "https://github.com/owner/repo/issues/123".to_string(),
            status: IssueTaskStatus::LlmConfirmed,
            priority: Some(20),
            category: Some("high_value_ready".to_string()),
        })
        .unwrap();
    assert_eq!(updated.id, task.id);
    assert_eq!(updated.title, "Fix parser panic in tokenizer");
    assert_eq!(updated.status, IssueTaskStatus::LlmConfirmed);

    let session = store
        .create_session_link(NewAgentSessionLink {
            agent_id: agent.id.clone(),
            native_session_id: "thread_123".to_string(),
            issue_task_id: Some(task.id.clone()),
            display_name: "issue-finder: owner/repo#123 - Fix parser panic".to_string(),
            goal: Some("Fix owner/repo#123".to_string()),
            status: AgentSessionStatus::Linked,
            metadata_json: json!({ "threadId": "thread_123" }),
        })
        .unwrap();
    assert_eq!(session.native_session_id, "thread_123");

    let run = store
        .create_dispatch_run(NewDispatchRun {
            issue_task_id: task.id.clone(),
            agent_id: agent.id.clone(),
            status: DispatchRunStatus::Proposed,
            requested_by: "test".to_string(),
            approval_state: ApprovalStatus::Pending,
            selected_session_link_id: Some(session.id.clone()),
        })
        .unwrap();
    assert_eq!(run.approval_state, ApprovalStatus::Pending);

    let run = store
        .update_dispatch_run_status(&run.id, DispatchRunStatus::Running, None)
        .unwrap();
    assert_eq!(run.status, DispatchRunStatus::Running);
    assert!(run.started_at.is_some());

    let event = store
        .append_agent_event(NewAgentEvent {
            run_id: Some(run.id.clone()),
            session_link_id: Some(session.id.clone()),
            event_type: "thread_started".to_string(),
            native_event_id: Some("native_event_1".to_string()),
            payload_json: json!({ "status": "ok" }),
        })
        .unwrap();
    assert_eq!(event.payload_json["status"], "ok");
    assert_eq!(store.list_agent_events_for_run(&run.id).unwrap().len(), 1);

    let artifact = store
        .write_artifact(
            NewArtifact {
                issue_task_id: Some(task.id.clone()),
                run_id: Some(run.id.clone()),
                kind: "test_payload".to_string(),
                content_type: "text/plain".to_string(),
                metadata_json: json!({ "purpose": "sha check" }),
            },
            b"hello",
        )
        .unwrap();
    assert_eq!(
        artifact.sha256,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    assert_eq!(fs::read_to_string(&artifact.path).unwrap(), "hello");
    assert_eq!(store.read_artifact_bytes(&artifact.id).unwrap(), b"hello");
    assert!(artifact.path.contains("/dispatch/artifacts/"));
    assert_eq!(
        store.list_artifacts_for_issue_task(&task.id).unwrap().len(),
        1
    );

    let package = IssueTaskPackage::new(IssueTaskPackageIssue {
        repo_full_name: "owner/repo".to_string(),
        number: 123,
        title: "Fix parser panic".to_string(),
        url: "https://github.com/owner/repo/issues/123".to_string(),
    });
    let package_artifact = store
        .write_task_package_artifact(&task.id, &package)
        .unwrap();
    let packaged_task = store.get_issue_task(&task.id).unwrap();
    assert_eq!(
        packaged_task.current_package_artifact_id,
        Some(package_artifact.id.clone())
    );
    assert_eq!(package_artifact.kind, "issue_task_package");
    let snapshot = json!({
        "source": "test",
        "profile": {
            "techStack": ["Rust"],
            "keywords": ["cli"]
        }
    });
    let snapshot_artifact = store
        .write_profile_snapshot_artifact(&task.id, &snapshot)
        .unwrap();
    let profiled_task = store.get_issue_task(&task.id).unwrap();
    assert_eq!(
        profiled_task.profile_snapshot_artifact_id,
        Some(snapshot_artifact.id.clone())
    );
    assert_eq!(snapshot_artifact.kind, "user_profile_snapshot");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &store.read_artifact_bytes(&snapshot_artifact.id).unwrap()
        )
        .unwrap(),
        snapshot
    );

    let run = store
        .set_dispatch_run_result_artifact(&run.id, &package_artifact.id)
        .unwrap();
    assert_eq!(run.result_artifact_id, Some(package_artifact.id.clone()));
    let run = store
        .update_dispatch_run_status(&run.id, DispatchRunStatus::Completed, None)
        .unwrap();
    assert_eq!(run.status, DispatchRunStatus::Completed);
    assert!(run.completed_at.is_some());

    let approval = store
        .create_approval_request(NewApprovalRequest {
            run_id: Some(run.id.clone()),
            approval_type: ApprovalType::Dispatch,
            status: ApprovalStatus::Pending,
            prompt: "Dispatch to Codex?".to_string(),
            details_json: json!({ "agent": "codex" }),
        })
        .unwrap();
    assert_eq!(approval.approval_type, ApprovalType::Dispatch);
    assert_eq!(
        store.list_approval_requests_for_run(&run.id).unwrap().len(),
        1
    );
    let approval = store
        .resolve_approval_request(&approval.id, ApprovalStatus::Approved)
        .unwrap();
    assert_eq!(approval.status, ApprovalStatus::Approved);
    assert!(approval.resolved_at.is_some());

    let github = store
        .create_github_interaction(NewGitHubInteraction {
            issue_task_id: task.id.clone(),
            interaction_type: GitHubInteractionType::TrackingComment,
            body_artifact_id: Some(artifact.id.clone()),
            status: GitHubInteractionStatus::Draft,
        })
        .unwrap();
    assert_eq!(github.status, GitHubInteractionStatus::Draft);
    let github = store
        .mark_github_interaction_failed(&github.id, "rate limited")
        .unwrap();
    assert_eq!(github.status, GitHubInteractionStatus::Failed);
    assert_eq!(github.error.as_deref(), Some("rate limited"));
    let github = store
        .mark_github_interaction_posted(&github.id, "123456")
        .unwrap();
    assert_eq!(github.status, GitHubInteractionStatus::Posted);
    assert_eq!(github.github_comment_id.as_deref(), Some("123456"));
    assert_eq!(github.error, None);
    assert_eq!(
        store
            .list_github_interactions_for_issue_task(&task.id)
            .unwrap()
            .len(),
        1
    );

    let memory = store
        .append_memory_event(NewMemoryEvent {
            issue_task_id: Some(task.id.clone()),
            event_type: MemoryEventType::PositiveSignal,
            source: "test".to_string(),
            payload_json: json!({ "reason": "approved dispatch" }),
        })
        .unwrap();
    assert_eq!(memory.event_type, MemoryEventType::PositiveSignal);
    assert_eq!(
        store
            .list_memory_events_for_issue_task(&task.id)
            .unwrap()
            .len(),
        1
    );

    let active_session = store
        .update_session_link_status(&session.id, AgentSessionStatus::Active)
        .unwrap();
    assert_eq!(active_session.status, AgentSessionStatus::Active);
    assert_eq!(
        store
            .list_session_links_for_issue_task(&task.id)
            .unwrap()
            .len(),
        1
    );

    let done_task = store
        .update_issue_task_status(&task.id, IssueTaskStatus::Done)
        .unwrap();
    assert_eq!(done_task.status, IssueTaskStatus::Done);
}

#[test]
fn dispatch_store_reopens_existing_state() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());

    let task_id = {
        let store = DispatchStore::open(paths.clone()).unwrap();
        store
            .create_agent_profile(NewAgentProfile {
                id: Some("codex".to_string()),
                kind: "codex".to_string(),
                display_name: "Codex".to_string(),
                adapter: "codex_app_server".to_string(),
                config_json: json!({}),
                enabled: true,
            })
            .unwrap();
        store
            .upsert_issue_task(NewIssueTask {
                repo_full_name: "owner/repo".to_string(),
                issue_number: 7,
                title: "Fix crash".to_string(),
                url: "https://github.com/owner/repo/issues/7".to_string(),
                status: IssueTaskStatus::Discovered,
                priority: None,
                category: None,
            })
            .unwrap()
            .id
    };

    let reopened = DispatchStore::open(paths).unwrap();
    assert_eq!(reopened.list_agent_profiles().unwrap().len(), 1);
    let task = reopened.get_issue_task(&task_id).unwrap();
    assert_eq!(task.issue_key, "owner/repo#7");
    assert_eq!(task.status, IssueTaskStatus::Discovered);
}

#[test]
fn artifact_write_cleans_file_when_database_insert_fails() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = DispatchStore::open(paths.clone()).unwrap();

    let error = store
        .write_artifact(
            NewArtifact {
                issue_task_id: Some("missing-task".to_string()),
                run_id: None,
                kind: "orphan_check".to_string(),
                content_type: "text/plain".to_string(),
                metadata_json: json!({}),
            },
            b"should not survive",
        )
        .unwrap_err();
    assert!(error.to_string().contains("FOREIGN KEY"));
    assert_eq!(count_files(&paths.dispatch_artifacts_dir()), 0);
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

fn count_files(path: &Path) -> usize {
    if !path.exists() {
        return 0;
    }

    let mut count = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                count += 1;
            }
        }
    }
    count
}
