use anyhow::Result;
use issue_finder::dispatch::adapters::{
    AdapterSession, AdapterStartSessionRequest, AdapterTurn, NativeExecutionAdapter,
};
use issue_finder::dispatch::session_approvals::{
    approve_session_mutation_with_adapter, reject_session_mutation, request_session_archive,
    request_session_fork, request_session_rename,
};
use issue_finder::dispatch::session_ops::{
    archive_session, fork_session, read_session_transcript, rename_session, sync_sessions,
    SessionsSyncRequest,
};
use issue_finder::dispatch::{
    AgentSessionStatus, ApprovalStatus, ApprovalType, DispatchEventKind, DispatchRuntime,
    IssueTaskStatus, NewAgentSessionLink, NewIssueTask, TranscriptPayloadStorage,
};
use issue_finder::paths::IssueFinderPaths;
use serde_json::{json, Value};
use tempfile::tempdir;

#[test]
fn session_ops_read_rename_and_archive_native_session_links() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
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
    let session = runtime
        .store()
        .create_session_link(NewAgentSessionLink {
            agent_id: "codex".to_string(),
            native_session_id: "native_session_1".to_string(),
            issue_task_id: Some(task.id.clone()),
            display_name: "old name".to_string(),
            goal: None,
            status: AgentSessionStatus::Active,
            metadata_json: json!({}),
        })
        .unwrap();
    let mut adapter = FakeSessionAdapter::default();

    let transcript = read_session_transcript(runtime.store(), &mut adapter, &session.id).unwrap();
    assert_eq!(transcript.session.status, AgentSessionStatus::Idle);
    assert_eq!(transcript.transcript_artifact.kind, "session_transcript");
    let payload = String::from_utf8(
        runtime
            .store()
            .read_artifact_bytes(&transcript.transcript_artifact.id)
            .unwrap(),
    )
    .unwrap();
    assert!(payload.contains("native_session_1"));
    assert_eq!(transcript.replay_items.len(), 1);
    assert_eq!(
        transcript.replay_items[0].payload_storage,
        TranscriptPayloadStorage::Inline
    );
    assert_eq!(
        runtime
            .store()
            .list_session_transcript_items(&session.id)
            .unwrap()
            .len(),
        1
    );

    let renamed = rename_session(
        runtime.store(),
        &mut adapter,
        &session.id,
        "issue-finder: renamed",
    )
    .unwrap();
    assert_eq!(renamed.session.display_name, "issue-finder: renamed");

    let archived = archive_session(runtime.store(), &mut adapter, &session.id).unwrap();
    assert_eq!(archived.session.status, AgentSessionStatus::Archived);
    assert_eq!(
        adapter.calls,
        vec![
            "read:native_session_1",
            "rename:native_session_1",
            "archive:native_session_1"
        ]
    );
}

#[test]
fn session_ops_forks_native_session_into_new_local_link() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
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
    let session = runtime
        .store()
        .create_session_link(NewAgentSessionLink {
            agent_id: "codex".to_string(),
            native_session_id: "native_source".to_string(),
            issue_task_id: Some(task.id.clone()),
            display_name: "issue-finder: owner/repo#123".to_string(),
            goal: Some("Fix owner/repo#123".to_string()),
            status: AgentSessionStatus::Idle,
            metadata_json: json!({ "issueKey": "owner/repo#123" }),
        })
        .unwrap();
    let mut adapter = FakeSessionAdapter::default();

    let forked = fork_session(runtime.store(), &mut adapter, &session.id).unwrap();

    assert_ne!(forked.session.id, session.id);
    assert_eq!(forked.session.native_session_id, "native_source_fork");
    assert_eq!(
        forked.session.issue_task_id.as_deref(),
        Some(task.id.as_str())
    );
    assert_eq!(forked.session.status, AgentSessionStatus::Idle);
    assert_eq!(forked.session.goal.as_deref(), Some("Forked goal"));
    assert_eq!(forked.event.event_kind, DispatchEventKind::SessionForked);
    assert_eq!(
        forked.event.payload_json["sourceSessionLinkId"].as_str(),
        Some(session.id.as_str())
    );
    assert_eq!(
        runtime
            .store()
            .list_session_links_for_issue_task(&task.id)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(adapter.calls, vec!["fork:native_source"]);
}

#[test]
fn session_ops_syncs_native_sessions_into_local_links() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
    let mut adapter = FakeSessionAdapter::default();

    let first = sync_sessions(
        runtime.store(),
        &mut adapter,
        SessionsSyncRequest {
            agent_id: "codex".to_string(),
            search: None,
            limit: Some(2),
        },
    )
    .unwrap();
    assert_eq!(first.synced.len(), 2);
    assert_eq!(
        runtime
            .store()
            .list_session_links(Some("codex"))
            .unwrap()
            .len(),
        2
    );

    let second = sync_sessions(
        runtime.store(),
        &mut adapter,
        SessionsSyncRequest {
            agent_id: "codex".to_string(),
            search: Some("parser".to_string()),
            limit: Some(1),
        },
    )
    .unwrap();
    assert_eq!(second.synced.len(), 1);
    assert_eq!(
        runtime
            .store()
            .list_session_links(Some("codex"))
            .unwrap()
            .len(),
        2
    );
    assert_eq!(second.synced[0].display_name, "parser search result");
}

#[test]
fn session_transcript_replay_spills_large_items_to_artifacts() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
    let task = runtime
        .store()
        .upsert_issue_task(NewIssueTask {
            repo_full_name: "owner/repo".to_string(),
            issue_number: 124,
            title: "Fix parser panic".to_string(),
            url: "https://github.com/owner/repo/issues/124".to_string(),
            status: IssueTaskStatus::UserApproved,
            priority: Some(10),
            category: Some("high_value_ready".to_string()),
        })
        .unwrap();
    let session = runtime
        .store()
        .create_session_link(NewAgentSessionLink {
            agent_id: "codex".to_string(),
            native_session_id: "native_large".to_string(),
            issue_task_id: Some(task.id),
            display_name: "large transcript".to_string(),
            goal: None,
            status: AgentSessionStatus::Active,
            metadata_json: json!({}),
        })
        .unwrap();
    let mut adapter = FakeSessionAdapter {
        transcript_text: Some("x".repeat((16 * 1024) + 1)),
        ..FakeSessionAdapter::default()
    };

    let transcript = read_session_transcript(runtime.store(), &mut adapter, &session.id).unwrap();

    assert_eq!(transcript.replay_items.len(), 1);
    assert_eq!(
        transcript.replay_items[0].payload_storage,
        TranscriptPayloadStorage::Artifact
    );
    let artifact_id = transcript.replay_items[0]
        .payload_artifact_id
        .as_deref()
        .expect("spilled artifact id");
    assert_eq!(
        runtime
            .store()
            .read_artifact_bytes(artifact_id)
            .unwrap()
            .len(),
        (16 * 1024) + 1
    );
}

#[test]
fn session_mutations_are_approval_gated_before_native_calls() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
    let session = runtime
        .store()
        .create_session_link(NewAgentSessionLink {
            agent_id: "codex".to_string(),
            native_session_id: "native_session_approval".to_string(),
            issue_task_id: None,
            display_name: "old name".to_string(),
            goal: None,
            status: AgentSessionStatus::Idle,
            metadata_json: json!({}),
        })
        .unwrap();
    let mut adapter = FakeSessionAdapter::default();

    let proposal = request_session_rename(
        runtime.store(),
        &session.id,
        "issue-finder: approved rename",
    )
    .unwrap();
    assert_eq!(proposal.status, "pending_approval");
    assert_eq!(
        proposal.approval_request.approval_type,
        ApprovalType::SessionMutation
    );
    assert_eq!(proposal.approval_request.status, ApprovalStatus::Pending);
    assert!(adapter.calls.is_empty());

    let approved = approve_session_mutation_with_adapter(
        runtime.store(),
        &mut adapter,
        &proposal.approval_request.id,
    )
    .unwrap();
    assert_eq!(approved.approval_request.status, ApprovalStatus::Approved);
    assert_eq!(
        approved.mutation.unwrap().session.display_name,
        "issue-finder: approved rename"
    );
    assert_eq!(adapter.calls, vec!["rename:native_session_approval"]);

    let fork = request_session_fork(runtime.store(), &session.id).unwrap();
    assert_eq!(fork.status, "pending_approval");
    assert_eq!(adapter.calls, vec!["rename:native_session_approval"]);

    let forked = approve_session_mutation_with_adapter(
        runtime.store(),
        &mut adapter,
        &fork.approval_request.id,
    )
    .unwrap();
    assert_eq!(forked.approval_request.status, ApprovalStatus::Approved);
    assert_eq!(
        forked.mutation.unwrap().session.native_session_id,
        "native_session_approval_fork"
    );
    assert_eq!(
        adapter.calls,
        vec![
            "rename:native_session_approval",
            "fork:native_session_approval"
        ]
    );

    let archive = request_session_archive(runtime.store(), &session.id).unwrap();
    let rejected = reject_session_mutation(runtime.store(), &archive.approval_request.id).unwrap();
    assert_eq!(rejected.approval_request.status, ApprovalStatus::Rejected);
    assert!(rejected.mutation.is_none());
    assert_eq!(
        adapter.calls,
        vec![
            "rename:native_session_approval",
            "fork:native_session_approval"
        ]
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

#[derive(Default)]
struct FakeSessionAdapter {
    calls: Vec<String>,
    transcript_text: Option<String>,
}

impl NativeExecutionAdapter for FakeSessionAdapter {
    fn adapter_start_session(
        &mut self,
        request: AdapterStartSessionRequest,
    ) -> Result<AdapterSession> {
        Ok(AdapterSession {
            native_session_id: "unused".to_string(),
            display_name: Some(request.display_name),
            goal: request.goal,
            metadata_json: request.metadata_json,
        })
    }

    fn adapter_resume_session(&mut self, native_session_id: &str) -> Result<AdapterSession> {
        Ok(existing_session(native_session_id))
    }

    fn adapter_fork_session(&mut self, native_session_id: &str) -> Result<AdapterSession> {
        self.calls.push(format!("fork:{native_session_id}"));
        Ok(AdapterSession {
            native_session_id: format!("{native_session_id}_fork"),
            display_name: Some(format!("fork of {native_session_id}")),
            goal: Some("Forked goal".to_string()),
            metadata_json: json!({ "source": "fork" }),
        })
    }

    fn adapter_rename_session(
        &mut self,
        native_session_id: &str,
        display_name: &str,
    ) -> Result<AdapterSession> {
        self.calls.push(format!("rename:{native_session_id}"));
        Ok(AdapterSession {
            display_name: Some(display_name.to_string()),
            ..existing_session(native_session_id)
        })
    }

    fn adapter_set_goal(&mut self, native_session_id: &str, goal: &str) -> Result<AdapterSession> {
        Ok(AdapterSession {
            goal: Some(goal.to_string()),
            ..existing_session(native_session_id)
        })
    }

    fn adapter_set_metadata(
        &mut self,
        native_session_id: &str,
        metadata_json: Value,
    ) -> Result<AdapterSession> {
        Ok(AdapterSession {
            metadata_json,
            ..existing_session(native_session_id)
        })
    }

    fn adapter_start_turn(
        &mut self,
        _native_session_id: &str,
        _prompt: &str,
    ) -> Result<AdapterTurn> {
        Ok(AdapterTurn {
            native_turn_id: "unused".to_string(),
            status: None,
        })
    }

    fn adapter_read_transcript(&mut self, native_session_id: &str) -> Result<Value> {
        self.calls.push(format!("read:{native_session_id}"));
        let text = self
            .transcript_text
            .clone()
            .unwrap_or_else(|| "done".to_string());
        Ok(json!({
            "thread": { "id": native_session_id },
            "turns": [{ "id": "turn_1" }],
            "items": [{ "turnId": "turn_1", "type": "assistant_message", "text": text }]
        }))
    }

    fn adapter_archive_session(&mut self, native_session_id: &str) -> Result<AdapterSession> {
        self.calls.push(format!("archive:{native_session_id}"));
        Ok(existing_session(native_session_id))
    }

    fn adapter_list_sessions(&mut self, limit: Option<usize>) -> Result<Vec<AdapterSession>> {
        self.calls.push(format!("list:{limit:?}"));
        Ok(vec![
            AdapterSession {
                native_session_id: "native_list_1".to_string(),
                display_name: Some("first native".to_string()),
                goal: None,
                metadata_json: json!({ "source": "list" }),
            },
            AdapterSession {
                native_session_id: "native_list_2".to_string(),
                display_name: Some("second native".to_string()),
                goal: None,
                metadata_json: json!({ "source": "list" }),
            },
        ])
    }

    fn adapter_search_sessions(
        &mut self,
        search_term: &str,
        limit: Option<usize>,
    ) -> Result<Vec<AdapterSession>> {
        self.calls.push(format!("search:{search_term}:{limit:?}"));
        Ok(vec![AdapterSession {
            native_session_id: "native_list_1".to_string(),
            display_name: Some(format!("{search_term} search result")),
            goal: None,
            metadata_json: json!({ "source": "search" }),
        }])
    }
}

fn existing_session(native_session_id: &str) -> AdapterSession {
    AdapterSession {
        native_session_id: native_session_id.to_string(),
        display_name: None,
        goal: None,
        metadata_json: Value::Null,
    }
}
