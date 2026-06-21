use std::fs;
use std::path::Path;

use issue_finder::dispatch::{
    AdapterProbeStatus, AgentCapabilityName, AgentSessionStatus, ApprovalStatus, ApprovalType,
    CapabilityStatus, DispatchEventKind, DispatchEventSeverity, DispatchEventSource,
    DispatchFailureClass, DispatchOutcomeFailureClass, DispatchOutcomeKind, DispatchRunStatus,
    DispatchStore, DispatchSubjectType, DispatchTaskClass, DispatchValidationOutcome,
    GitHubInteractionStatus, GitHubInteractionType, IssueTaskPackage, IssueTaskPackageIssue,
    IssueTaskStatus, MemoryEventType, NewAdapterProbeResult, NewAgentCapability, NewAgentProfile,
    NewAgentSessionLink, NewApprovalRequest, NewArtifact, NewDispatchEvent, NewDispatchFailure,
    NewDispatchRun, NewDispatchRunOutcome, NewGitHubInteraction, NewIssueTask, NewMemoryEvent,
    NewSessionTranscriptItem, TranscriptPayloadStorage,
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
        .append_dispatch_event(NewDispatchEvent {
            run_id: Some(run.id.clone()),
            session_link_id: Some(session.id.clone()),
            issue_task_id: Some(task.id.clone()),
            event_kind: DispatchEventKind::SessionStarted,
            subject_type: DispatchSubjectType::Session,
            subject_id: Some(session.id.clone()),
            source: DispatchEventSource::Adapter,
            severity: DispatchEventSeverity::Info,
            correlation_id: Some(run.id.clone()),
            causation_id: None,
            native_event_id: Some("native_event_1".to_string()),
            payload_json: json!({ "status": "ok" }),
        })
        .unwrap();
    assert_eq!(event.payload_json["status"], "ok");
    assert_eq!(event.event_kind, DispatchEventKind::SessionStarted);
    assert_eq!(event.sequence, 1);
    assert_eq!(
        store.list_dispatch_events_for_run(&run.id).unwrap().len(),
        1
    );

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

    let failure = store
        .record_dispatch_failure(NewDispatchFailure {
            run_id: run.id.clone(),
            phase: "execute".to_string(),
            failure_class: DispatchFailureClass::Adapter,
            code: "adapter_error".to_string(),
            retryable: true,
            message: "codex app-server unavailable".to_string(),
            details_json: json!({ "method": "turn/start" }),
        })
        .unwrap();
    assert_eq!(failure.failure_class, DispatchFailureClass::Adapter);
    assert_eq!(
        store.list_dispatch_failures_for_run(&run.id).unwrap()[0].code,
        "adapter_error"
    );

    let probe = store
        .record_adapter_probe(NewAdapterProbeResult {
            agent_id: agent.id.clone(),
            adapter: "codex_app_server".to_string(),
            capability: AgentCapabilityName::StartSession,
            method: Some("thread/start".to_string()),
            status: AdapterProbeStatus::Supported,
            protocol_version: Some("codex_app_server_json_rpc".to_string()),
            expires_at: Some("2999-01-01T00:00:00Z".to_string()),
            error_code: None,
            details_json: json!({ "source": "test" }),
        })
        .unwrap();
    assert_eq!(probe.status, AdapterProbeStatus::Supported);
    assert_eq!(
        store
            .latest_adapter_probe(&agent.id, AgentCapabilityName::StartSession)
            .unwrap()
            .unwrap()
            .id,
        probe.id
    );

    let replay_item = store
        .append_session_transcript_item(NewSessionTranscriptItem {
            session_link_id: session.id.clone(),
            turn_id: Some("turn_1".to_string()),
            item_index: 0,
            item_type: "message".to_string(),
            text: Some("hello".to_string()),
            payload_artifact_id: None,
            payload_storage: TranscriptPayloadStorage::Inline,
            metadata_json: json!({}),
        })
        .unwrap();
    assert_eq!(
        replay_item.payload_storage,
        TranscriptPayloadStorage::Inline
    );
    assert_eq!(
        store
            .list_session_transcript_items(&session.id)
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
fn dispatch_store_migrates_v1_agent_events_to_typed_dispatch_events() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    std::fs::create_dir_all(paths.dispatch_db_path().parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(paths.dispatch_db_path()).unwrap();
    conn.execute_batch(
        r#"
        PRAGMA foreign_keys = ON;
        CREATE TABLE agent_profiles (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            display_name TEXT NOT NULL,
            adapter TEXT NOT NULL,
            config_json TEXT NOT NULL,
            enabled INTEGER NOT NULL CHECK (enabled IN (0, 1))
        );
        CREATE TABLE issue_tasks (
            id TEXT PRIMARY KEY,
            issue_key TEXT NOT NULL UNIQUE,
            repo_full_name TEXT NOT NULL,
            issue_number INTEGER NOT NULL,
            title TEXT NOT NULL,
            url TEXT NOT NULL,
            status TEXT NOT NULL,
            priority INTEGER,
            category TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            current_package_artifact_id TEXT,
            profile_snapshot_artifact_id TEXT
        );
        CREATE TABLE dispatch_runs (
            id TEXT PRIMARY KEY,
            issue_task_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            status TEXT NOT NULL,
            requested_by TEXT NOT NULL,
            approval_state TEXT NOT NULL,
            created_at TEXT NOT NULL,
            started_at TEXT,
            completed_at TEXT,
            selected_session_link_id TEXT,
            result_artifact_id TEXT,
            failure_reason TEXT
        );
        CREATE TABLE agent_session_links (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            native_session_id TEXT NOT NULL,
            issue_task_id TEXT,
            display_name TEXT NOT NULL,
            goal TEXT,
            status TEXT NOT NULL,
            metadata_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL,
            archived_at TEXT
        );
        CREATE TABLE agent_events (
            id TEXT PRIMARY KEY,
            run_id TEXT,
            session_link_id TEXT,
            event_type TEXT NOT NULL,
            native_event_id TEXT,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        INSERT INTO agent_profiles VALUES ('codex','codex','Codex','codex_app_server','{}',1);
        INSERT INTO issue_tasks VALUES ('task-1','owner/repo#1','owner/repo',1,'Fix bug','https://github.com/owner/repo/issues/1','discovered',NULL,NULL,'2026-01-01T00:00:00Z','2026-01-01T00:00:00Z',NULL,NULL);
        INSERT INTO dispatch_runs VALUES ('run-1','task-1','codex','running','test','approved','2026-01-01T00:00:00Z',NULL,NULL,NULL,NULL,NULL);
        INSERT INTO agent_events VALUES ('event-1','run-1',NULL,'dispatch_starting',NULL,'{"status":"ok"}','2026-01-01T00:00:01Z');
        PRAGMA user_version = 1;
        "#,
    )
    .unwrap();
    drop(conn);

    let store = DispatchStore::open(paths.clone()).unwrap();
    let events = store.list_dispatch_events_for_run("run-1").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_kind, DispatchEventKind::DispatchStarting);
    assert_eq!(events[0].source, DispatchEventSource::Migration);
    let conn = rusqlite::Connection::open(paths.dispatch_db_path()).unwrap();
    let legacy_table: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'agent_events'",
            [],
            |row| row.get(0),
        )
        .ok();
    assert_eq!(legacy_table, None);
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

#[test]
fn dispatch_store_persists_outcomes_idempotently_and_rejects_conflicts() {
    let dir = tempdir().unwrap();
    let store = DispatchStore::open(test_paths(dir.path())).unwrap();
    seed_agent(&store);
    let task = store
        .upsert_issue_task(NewIssueTask {
            repo_full_name: "owner/repo".to_string(),
            issue_number: 99,
            title: "Fix parser panic".to_string(),
            url: "https://github.com/owner/repo/issues/99".to_string(),
            status: IssueTaskStatus::UserApproved,
            priority: Some(10),
            category: Some("high_value_ready".to_string()),
        })
        .unwrap();
    let run = store
        .create_dispatch_run(NewDispatchRun {
            issue_task_id: task.id,
            agent_id: "codex".to_string(),
            status: DispatchRunStatus::Running,
            requested_by: "test".to_string(),
            approval_state: ApprovalStatus::Approved,
            selected_session_link_id: None,
        })
        .unwrap();
    let input = NewDispatchRunOutcome {
        run_id: run.id.clone(),
        idempotency_key: "outcome-key-1".to_string(),
        outcome_kind: DispatchOutcomeKind::Failed,
        failure_class: Some(DispatchOutcomeFailureClass::ValidationFailed),
        failure_detail: Some("cargo test still fails".to_string()),
        task_class: Some(DispatchTaskClass::RustCliPanic),
        validation_outcome: Some(DispatchValidationOutcome::Failed),
        result_artifact_id: None,
        metadata_json: json!({ "source": "test" }),
    };

    let first = store.record_dispatch_run_outcome(input.clone()).unwrap();
    let second = store.record_dispatch_run_outcome(input).unwrap();

    assert_eq!(first, second);
    assert_eq!(
        store
            .find_dispatch_run_outcome_by_run(&run.id)
            .unwrap()
            .unwrap()
            .outcome_kind,
        DispatchOutcomeKind::Failed
    );
    let conflict = store
        .record_dispatch_run_outcome(NewDispatchRunOutcome {
            run_id: run.id,
            idempotency_key: "outcome-key-1".to_string(),
            outcome_kind: DispatchOutcomeKind::FixReady,
            failure_class: None,
            failure_detail: None,
            task_class: Some(DispatchTaskClass::RustCliPanic),
            validation_outcome: Some(DispatchValidationOutcome::Passed),
            result_artifact_id: None,
            metadata_json: json!({ "source": "test" }),
        })
        .unwrap_err();
    assert!(conflict.to_string().contains("already has outcome"));
}

fn seed_agent(store: &DispatchStore) {
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
