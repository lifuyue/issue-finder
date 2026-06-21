use anyhow::Result;
use issue_finder::dispatch::adapters::{
    AdapterSession, AdapterStartSessionRequest, AdapterTurn, NativeExecutionAdapter,
};
use issue_finder::dispatch::execution::execute_approved_dispatch;
use issue_finder::dispatch::{
    AgentCapabilityName, AgentSessionStatus, ApprovalStatus, CapabilityStatus, DispatchEventKind,
    DispatchFailureClass, DispatchProposalRequest, DispatchRunStatus, DispatchRuntime,
    IssueTaskPackage, IssueTaskPackageIssue, IssueTaskStatus, NewAgentCapability, NewAgentProfile,
    NewAgentSessionLink, NewIssueTask,
};
use issue_finder::github::GitHubIssue;
use issue_finder::handoff::{write_handoff, Handoff};
use issue_finder::inbox::upsert_ready;
use issue_finder::paths::IssueFinderPaths;
use issue_finder::repo_scan::{CandidateFile, RepoScan, ValidationCommand};
use issue_finder::workspace::{PreparedWorkspace, WorkspaceInfo};
use serde_json::{json, Value};
use tempfile::tempdir;

#[test]
fn execution_starts_new_native_session_after_approval() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
    let task = create_packaged_task(&runtime, 123);
    let proposal = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#123".to_string(),
            agent_id: "codex".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: None,
            new_session: true,
        })
        .unwrap();
    runtime
        .resolve_dispatch_approval(&proposal.run.id, ApprovalStatus::Approved)
        .unwrap();

    let mut adapter = FakeNativeAdapter::default();
    let result =
        execute_approved_dispatch(runtime.store(), &mut adapter, &proposal.run.id).unwrap();

    assert_eq!(result.run.status, DispatchRunStatus::Running);
    assert_eq!(result.run.approval_state, ApprovalStatus::Approved);
    assert_eq!(result.session.native_session_id, "native_started_1");
    assert_eq!(
        result.session.display_name,
        "issue-finder: owner/repo#123 - Fix parser panic"
    );
    assert_eq!(result.session.status, AgentSessionStatus::Active);
    assert_eq!(result.turn.native_turn_id, "turn_1");
    assert_eq!(result.prompt_artifact.kind, "dispatch_prompt");
    let prompt = String::from_utf8(
        runtime
            .store()
            .read_artifact_bytes(&result.prompt_artifact.id)
            .unwrap(),
    )
    .unwrap();
    assert!(prompt.contains("owner/repo#123"));
    assert!(prompt.contains("Task package path:"));

    let event_types = runtime
        .dispatch_events(&proposal.run.id)
        .unwrap()
        .into_iter()
        .map(|event| event.event_kind)
        .collect::<Vec<_>>();
    assert_eq!(
        event_types,
        vec![
            DispatchEventKind::DispatchApprovalResolved,
            DispatchEventKind::DispatchStarting,
            DispatchEventKind::SessionStarted,
            DispatchEventKind::TurnStarted
        ]
    );
    assert_eq!(
        runtime.store().get_issue_task(&task.id).unwrap().status,
        IssueTaskStatus::InProgress
    );
    assert_eq!(
        adapter.calls,
        vec!["start_session", "start_turn:native_started_1"]
    );
}

#[test]
fn execution_resumes_selected_session_after_approval() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
    let task = create_packaged_task(&runtime, 456);
    let session = runtime
        .store()
        .create_session_link(NewAgentSessionLink {
            agent_id: "codex".to_string(),
            native_session_id: "native_existing".to_string(),
            issue_task_id: Some(task.id.clone()),
            display_name: "old".to_string(),
            goal: None,
            status: AgentSessionStatus::Idle,
            metadata_json: json!({}),
        })
        .unwrap();
    let proposal = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#456".to_string(),
            agent_id: "codex".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: Some(session.id.clone()),
            new_session: false,
        })
        .unwrap();
    runtime
        .resolve_dispatch_approval(&proposal.run.id, ApprovalStatus::Approved)
        .unwrap();

    let mut adapter = FakeNativeAdapter::default();
    let result =
        execute_approved_dispatch(runtime.store(), &mut adapter, &proposal.run.id).unwrap();

    assert_eq!(result.run.status, DispatchRunStatus::Running);
    assert_eq!(result.session.id, session.id);
    assert_eq!(result.session.status, AgentSessionStatus::Active);
    assert_eq!(
        adapter.calls,
        vec![
            "resume_session:native_existing",
            "rename_session:native_existing",
            "set_goal:native_existing",
            "set_metadata:native_existing",
            "start_turn:native_existing"
        ]
    );
}

#[test]
fn dispatch_proposal_accepts_native_session_id_selector() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
    let task = create_packaged_task(&runtime, 457);
    let session = runtime
        .store()
        .create_session_link(NewAgentSessionLink {
            agent_id: "codex".to_string(),
            native_session_id: "native_existing_457".to_string(),
            issue_task_id: Some(task.id.clone()),
            display_name: "old".to_string(),
            goal: None,
            status: AgentSessionStatus::Idle,
            metadata_json: json!({}),
        })
        .unwrap();

    let proposal = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#457".to_string(),
            agent_id: "codex".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: Some("native_existing_457".to_string()),
            new_session: false,
        })
        .unwrap();

    assert_eq!(
        proposal.run.selected_session_link_id.as_deref(),
        Some(session.id.as_str())
    );
    assert_eq!(
        proposal.approval_request.details_json["executionMode"],
        "resume_session"
    );
    assert_eq!(proposal.approval_request.details_json["newSession"], false);
    assert_eq!(
        proposal.approval_request.details_json["requestedNewSession"],
        false
    );
    assert_eq!(
        proposal.approval_request.details_json["selectedNativeSessionId"],
        "native_existing_457"
    );
    assert!(proposal
        .approval_request
        .prompt
        .contains("resuming native session native_existing_457"));
}

#[test]
fn dispatch_proposal_records_actual_new_session_mode_when_flag_is_omitted() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
    create_packaged_task(&runtime, 458);

    let proposal = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#458".to_string(),
            agent_id: "codex".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: None,
            new_session: false,
        })
        .unwrap();

    assert_eq!(
        proposal.approval_request.details_json["executionMode"],
        "start_session"
    );
    assert_eq!(proposal.approval_request.details_json["newSession"], true);
    assert_eq!(
        proposal.approval_request.details_json["requestedNewSession"],
        false
    );
    assert_eq!(
        proposal.approval_request.details_json["selectedNativeSessionId"],
        serde_json::Value::Null
    );
    assert!(proposal
        .approval_request
        .prompt
        .contains("starting a new native session"));
}

#[test]
fn dispatch_proposal_auto_imports_ready_handoff_from_inbox() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let issue = github_issue("owner/repo", 458);
    let handoff = Handoff::build(&issue, &prepared_workspace(dir.path()));
    let written = write_handoff(&paths, &handoff, &issue).unwrap();
    upsert_ready(&paths, &issue, 91, &written).unwrap();

    let runtime = DispatchRuntime::open(paths).unwrap();
    let error = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#458".to_string(),
            agent_id: "codex".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: None,
            new_session: true,
        })
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("is pending issue review approval"));
    let issue_task = runtime
        .store()
        .get_issue_task_by_key("owner/repo#458")
        .unwrap();
    assert_eq!(issue_task.issue_key, "owner/repo#458");
    assert!(issue_task.current_package_artifact_id.is_none());
    let artifact_kinds = runtime
        .store()
        .list_artifacts_for_issue_task(&issue_task.id)
        .unwrap()
        .into_iter()
        .map(|artifact| artifact.kind)
        .collect::<Vec<_>>();
    assert!(artifact_kinds.contains(&"handoff_json".to_string()));
    assert!(!artifact_kinds.contains(&"issue_task_package".to_string()));
    assert!(artifact_kinds.contains(&"user_profile_snapshot".to_string()));
}

#[test]
fn handoff_import_is_idempotent_for_same_inbox_item() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let issue = github_issue("owner/repo", 460);
    let handoff = Handoff::build(&issue, &prepared_workspace(dir.path()));
    let written = write_handoff(&paths, &handoff, &issue).unwrap();
    upsert_ready(&paths, &issue, 91, &written).unwrap();

    let runtime = DispatchRuntime::open(paths).unwrap();
    let first = runtime.import_handoff_from_inbox(&written.id).unwrap();
    let artifacts_after_first = runtime
        .store()
        .list_artifacts_for_issue_task(&first.issue_task.id)
        .unwrap();
    let second = runtime.import_handoff_from_inbox(&written.id).unwrap();
    let artifacts_after_second = runtime
        .store()
        .list_artifacts_for_issue_task(&second.issue_task.id)
        .unwrap();

    assert_eq!(second.issue_task.id, first.issue_task.id);
    assert_eq!(second.handoff_artifact.id, first.handoff_artifact.id);
    assert_eq!(second.approval_request.id, first.approval_request.id);
    assert_eq!(second.package_artifact, None);
    assert_eq!(second.package, None);
    assert_eq!(artifacts_after_second.len(), artifacts_after_first.len());
}

#[test]
fn dispatch_auto_import_rejects_mismatched_inbox_handoff_payload() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let inbox_issue = github_issue("owner/repo", 459);
    let handoff_issue = github_issue("owner/other", 999);
    let handoff = Handoff::build(&handoff_issue, &prepared_workspace(dir.path()));
    let written = write_handoff(&paths, &handoff, &handoff_issue).unwrap();
    upsert_ready(&paths, &inbox_issue, 91, &written).unwrap();

    let runtime = DispatchRuntime::open(paths).unwrap();
    let error = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#459".to_string(),
            agent_id: "codex".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: None,
            new_session: true,
        })
        .unwrap_err();

    assert!(error.to_string().contains("but inbox item"));
    assert!(runtime
        .store()
        .find_issue_task_by_key("owner/repo#459")
        .unwrap()
        .is_none());
}

#[test]
fn execution_records_needs_user_when_native_turn_waits_for_approval() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
    let task = create_packaged_task(&runtime, 555);
    let proposal = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#555".to_string(),
            agent_id: "codex".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: None,
            new_session: true,
        })
        .unwrap();
    runtime
        .resolve_dispatch_approval(&proposal.run.id, ApprovalStatus::Approved)
        .unwrap();

    let mut adapter = FakeNativeAdapter {
        turn_status: Some("needs_user".to_string()),
        ..FakeNativeAdapter::default()
    };
    let result =
        execute_approved_dispatch(runtime.store(), &mut adapter, &proposal.run.id).unwrap();

    assert_eq!(result.run.status, DispatchRunStatus::NeedsUser);
    assert_eq!(
        runtime.store().get_issue_task(&task.id).unwrap().status,
        IssueTaskStatus::InProgress
    );
    assert_eq!(result.turn.status.as_deref(), Some("needs_user"));
}

#[test]
fn execution_requires_approved_dispatch() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
    create_packaged_task(&runtime, 789);
    let proposal = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#789".to_string(),
            agent_id: "codex".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: None,
            new_session: true,
        })
        .unwrap();

    let mut adapter = FakeNativeAdapter::default();
    let error =
        execute_approved_dispatch(runtime.store(), &mut adapter, &proposal.run.id).unwrap_err();

    assert!(error.to_string().contains("is not approved"));
    assert_eq!(
        runtime
            .store()
            .get_dispatch_run(&proposal.run.id)
            .unwrap()
            .status,
        DispatchRunStatus::Proposed
    );
    assert!(adapter.calls.is_empty());
}

#[test]
fn execution_records_structured_failure_and_trace_timeline() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
    create_packaged_task(&runtime, 790);
    let proposal = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#790".to_string(),
            agent_id: "codex".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: None,
            new_session: true,
        })
        .unwrap();
    runtime
        .resolve_dispatch_approval(&proposal.run.id, ApprovalStatus::Approved)
        .unwrap();

    let mut adapter = FakeNativeAdapter {
        fail_start_turn: true,
        ..FakeNativeAdapter::default()
    };
    let error =
        execute_approved_dispatch(runtime.store(), &mut adapter, &proposal.run.id).unwrap_err();

    assert!(error
        .to_string()
        .contains("codex app-server adapter unavailable"));
    let run = runtime.store().get_dispatch_run(&proposal.run.id).unwrap();
    assert_eq!(run.status, DispatchRunStatus::Failed);
    let failures = runtime
        .store()
        .list_dispatch_failures_for_run(&proposal.run.id)
        .unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].failure_class, DispatchFailureClass::Adapter);
    let trace = runtime.dispatch_trace(&proposal.run.id).unwrap();
    assert_eq!(trace.failures.len(), 1);
    assert!(trace
        .timeline
        .iter()
        .any(|item| item.kind == "failure" && item.summary.contains("adapter_error")));
    assert!(runtime
        .dispatch_timeline(&proposal.run.id)
        .unwrap()
        .items
        .iter()
        .any(|item| item.kind == "event" && item.summary == "dispatch_failed"));
}

#[test]
fn dispatch_approval_marks_issue_task_dispatched() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
    let task = create_packaged_task(&runtime, 246);
    let proposal = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#246".to_string(),
            agent_id: "codex".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: None,
            new_session: true,
        })
        .unwrap();

    runtime
        .resolve_dispatch_approval(&proposal.run.id, ApprovalStatus::Approved)
        .unwrap();

    assert_eq!(
        runtime.store().get_issue_task(&task.id).unwrap().status,
        IssueTaskStatus::Dispatched
    );
    let status = runtime.dispatch_status(&proposal.run.id).unwrap();
    assert_eq!(status.approval_latencies.len(), 1);
    assert!(status.approval_latencies[0].latency_ms.is_some());
}

#[test]
fn dispatch_proposal_rejects_session_for_different_agent() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
    let task = create_packaged_task(&runtime, 321);
    runtime
        .store()
        .create_agent_profile(NewAgentProfile {
            id: Some("other-agent".to_string()),
            kind: "other".to_string(),
            display_name: "Other Agent".to_string(),
            adapter: "fake_adapter".to_string(),
            config_json: json!({}),
            enabled: true,
        })
        .unwrap();
    let session = runtime
        .store()
        .create_session_link(NewAgentSessionLink {
            agent_id: "other-agent".to_string(),
            native_session_id: "native_other".to_string(),
            issue_task_id: Some(task.id),
            display_name: "other session".to_string(),
            goal: None,
            status: AgentSessionStatus::Idle,
            metadata_json: json!({}),
        })
        .unwrap();

    let error = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#321".to_string(),
            agent_id: "codex".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: Some(session.id.clone()),
            new_session: false,
        })
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("belongs to agent other-agent, not codex"));
}

#[test]
fn dispatch_proposal_rejects_new_session_with_selected_session() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
    let task = create_packaged_task(&runtime, 654);
    let session = runtime
        .store()
        .create_session_link(NewAgentSessionLink {
            agent_id: "codex".to_string(),
            native_session_id: "native_existing".to_string(),
            issue_task_id: Some(task.id),
            display_name: "old".to_string(),
            goal: None,
            status: AgentSessionStatus::Idle,
            metadata_json: json!({}),
        })
        .unwrap();

    let error = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#654".to_string(),
            agent_id: "codex".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: Some(session.id),
            new_session: true,
        })
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("new_session cannot be combined with selected_session_link_id"));
}

#[test]
fn dispatch_proposal_rejects_agent_without_required_capability() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths).unwrap();
    create_packaged_task(&runtime, 987);
    runtime
        .store()
        .create_agent_profile(NewAgentProfile {
            id: Some("limited-agent".to_string()),
            kind: "limited".to_string(),
            display_name: "Limited Agent".to_string(),
            adapter: "fake_adapter".to_string(),
            config_json: json!({}),
            enabled: true,
        })
        .unwrap();
    runtime
        .store()
        .upsert_agent_capability(NewAgentCapability {
            agent_id: "limited-agent".to_string(),
            capability: AgentCapabilityName::StartSession,
            status: CapabilityStatus::Unsupported,
            details_json: json!({ "reason": "test" }),
        })
        .unwrap();

    let error = runtime
        .propose_dispatch(DispatchProposalRequest {
            issue: "owner/repo#987".to_string(),
            agent_id: "limited-agent".to_string(),
            requested_by: "test".to_string(),
            selected_session_link_id: None,
            new_session: true,
        })
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("does not support capability start_session"));
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

fn github_issue(repo_full_name: &str, number: u64) -> GitHubIssue {
    let repo_name = repo_full_name
        .split('/')
        .next_back()
        .unwrap_or(repo_full_name)
        .to_string();
    GitHubIssue {
        id: number,
        number,
        title: "Fix parser panic".to_string(),
        body: "Parser panics when input is empty.".to_string(),
        labels: vec!["bug".to_string()],
        url: format!("https://github.com/{repo_full_name}/issues/{number}"),
        repo_full_name: repo_full_name.to_string(),
        repo_name,
        repo_description: "Test repository".to_string(),
        repo_stars: 42,
        created_at: "2026-06-18T00:00:00Z".to_string(),
        updated_at: "2026-06-18T00:00:00Z".to_string(),
    }
}

fn prepared_workspace(root: &std::path::Path) -> PreparedWorkspace {
    PreparedWorkspace {
        info: WorkspaceInfo {
            path: root.join("repo").to_string_lossy().to_string(),
            default_branch: "main".to_string(),
            branch: "issue-finder/458-fix-parser-panic".to_string(),
            dirty: false,
        },
        scan: RepoScan {
            discovered_files: vec!["src/main.rs".to_string()],
            candidate_files: vec![CandidateFile {
                path: "src/main.rs".to_string(),
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
struct FakeNativeAdapter {
    calls: Vec<String>,
    turn_status: Option<String>,
    fail_start_turn: bool,
}

impl NativeExecutionAdapter for FakeNativeAdapter {
    fn adapter_start_session(
        &mut self,
        request: AdapterStartSessionRequest,
    ) -> Result<AdapterSession> {
        self.calls.push("start_session".to_string());
        assert!(request.display_name.contains("owner/repo#"));
        assert_eq!(
            request.metadata_json["source"],
            "issue_finder_dispatch_runtime"
        );
        Ok(AdapterSession {
            native_session_id: "native_started_1".to_string(),
            display_name: Some(request.display_name),
            goal: request.goal,
            metadata_json: request.metadata_json,
        })
    }

    fn adapter_resume_session(&mut self, native_session_id: &str) -> Result<AdapterSession> {
        self.calls
            .push(format!("resume_session:{native_session_id}"));
        Ok(existing_session(native_session_id))
    }

    fn adapter_fork_session(&mut self, native_session_id: &str) -> Result<AdapterSession> {
        self.calls.push(format!("fork_session:{native_session_id}"));
        Ok(AdapterSession {
            native_session_id: format!("{native_session_id}_fork"),
            ..existing_session(native_session_id)
        })
    }

    fn adapter_rename_session(
        &mut self,
        native_session_id: &str,
        display_name: &str,
    ) -> Result<AdapterSession> {
        self.calls
            .push(format!("rename_session:{native_session_id}"));
        Ok(AdapterSession {
            display_name: Some(display_name.to_string()),
            ..existing_session(native_session_id)
        })
    }

    fn adapter_set_goal(&mut self, native_session_id: &str, goal: &str) -> Result<AdapterSession> {
        self.calls.push(format!("set_goal:{native_session_id}"));
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
        self.calls.push(format!("set_metadata:{native_session_id}"));
        assert_eq!(metadata_json["source"], "issue_finder_dispatch_runtime");
        Ok(AdapterSession {
            metadata_json,
            ..existing_session(native_session_id)
        })
    }

    fn adapter_start_turn(&mut self, native_session_id: &str, prompt: &str) -> Result<AdapterTurn> {
        self.calls.push(format!("start_turn:{native_session_id}"));
        assert!(prompt.contains("Task package artifact id:"));
        if self.fail_start_turn {
            anyhow::bail!("codex app-server adapter unavailable");
        }
        Ok(AdapterTurn {
            native_turn_id: "turn_1".to_string(),
            status: Some(
                self.turn_status
                    .clone()
                    .unwrap_or_else(|| "running".to_string()),
            ),
        })
    }

    fn adapter_read_transcript(&mut self, native_session_id: &str) -> Result<Value> {
        self.calls
            .push(format!("read_transcript:{native_session_id}"));
        Ok(json!({ "thread": { "id": native_session_id }, "turns": [], "items": [] }))
    }

    fn adapter_archive_session(&mut self, native_session_id: &str) -> Result<AdapterSession> {
        self.calls
            .push(format!("archive_session:{native_session_id}"));
        Ok(existing_session(native_session_id))
    }

    fn adapter_list_sessions(&mut self, _limit: Option<usize>) -> Result<Vec<AdapterSession>> {
        Ok(Vec::new())
    }

    fn adapter_search_sessions(
        &mut self,
        _search_term: &str,
        _limit: Option<usize>,
    ) -> Result<Vec<AdapterSession>> {
        Ok(Vec::new())
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
