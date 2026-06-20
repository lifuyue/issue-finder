use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use issue_finder::config::Config;
use issue_finder::dispatch::{
    AgentCapabilityName, AgentSessionStatus, ApprovalStatus, CapabilityStatus, DispatchRunStatus,
    DispatchRuntime, IssueTaskPackage, IssueTaskPackageIssue, IssueTaskStatus, NewAgentCapability,
    NewAgentEvent, NewAgentProfile, NewAgentSessionLink, NewArtifact, NewDispatchRun, NewIssueTask,
};
use issue_finder::github::GitHubIssue;
use issue_finder::handoff::{write_handoff, Handoff, WrittenHandoff};
use issue_finder::inbox::{load_index, upsert_ready};
use issue_finder::memory::{
    MemoryDreamRun, MemoryDreamScope, MemoryDreamStatus, MemoryDreamTrigger, MemoryDreamType,
    MemoryHintScopeType, MemoryHintStatus, MemoryHintType, MemoryModelStatus, MemoryStore,
    NewMemoryDream, NewMemoryHint,
};
use issue_finder::paths::IssueFinderPaths;
use issue_finder::prepare_gate::{
    default_prepare_allowed, prepare_gate_decision, PrepareGateDecision,
};
use issue_finder::recommendation::{
    load_events, RecommendationEventSource, RecommendationEventType,
};
use issue_finder::repo_scan::{CandidateFile, RepoScan, ValidationCommand};
use issue_finder::tool_runtime::{IssueFinderToolInvocation, IssueFinderToolRuntime};
use issue_finder::tool_specs::list_tool_specs;
use issue_finder::value_scoring::{
    is_daily_prepare_candidate, RecommendationCategory, ValueAssessment,
};
use issue_finder::workflow::{self, PrepareOutcome};
use issue_finder::workspace::{git_available, PreparedWorkspace, WorkspaceInfo};
use tempfile::tempdir;
use tokio::sync::Mutex;

#[path = "support/env_lock.rs"]
mod env_lock;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

#[test]
fn tools_list_outputs_stable_issue_finder_specs() {
    let specs = serde_json::to_value(list_tool_specs()).unwrap();
    assert_eq!(specs["kind"], "issue_finder_tool_specs");
    assert_eq!(specs["version"], 1);
    assert_eq!(
        specs["quickStart"]["firstCall"]["defaultTool"],
        "issue-finder.scout"
    );
    assert_eq!(
        specs["quickStart"]["firstCall"]["defaultArguments"]["repo"],
        "owner/repo"
    );
    assert_eq!(
        specs["quickStart"]["firstCall"]["defaultArguments"]["limit"],
        10
    );
    assert_eq!(
        specs["quickStart"]["firstCall"]["whenReadyUnknown"],
        "issue-finder.status"
    );
    assert_eq!(
        specs["quickStart"]["firstCall"]["fallbackAfterSetupFailure"],
        "issue-finder.status"
    );
    let workflow = specs["recommendedWorkflow"].as_array().unwrap();
    let workflow_tools = workflow
        .iter()
        .map(|step| step["tool"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        workflow_tools,
        vec![
            "issue-finder.scout",
            "issue-finder.assess",
            "issue-finder.prepare",
            "issue-finder.read_context"
        ]
    );
    let read_context_step = workflow
        .iter()
        .find(|step| step["tool"] == "issue-finder.read_context")
        .expect("read_context workflow step");
    assert_eq!(read_context_step["deferred"], true);
    assert_eq!(
        read_context_step["firstSections"],
        serde_json::json!(["entry", "safety", "probe"])
    );
    let tools = specs["tools"].as_array().unwrap();
    let names = tools
        .iter()
        .map(|tool| {
            format!(
                "{}.{}",
                tool["namespace"].as_str().unwrap(),
                tool["name"].as_str().unwrap()
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "issue-finder.status",
            "issue-finder.scout",
            "issue-finder.assess",
            "issue-finder.prepare",
            "issue-finder.read_context",
            "issue-finder.memory_status",
            "issue-finder.memory_recall",
            "issue-finder.memory_dreams_list",
            "issue-finder.memory_dream_show",
            "issue-finder.memory_hints_list",
            "issue-finder.memory_hint_update",
            "issue-finder.memory_tombstone",
            "issue-finder.agents_list",
            "issue-finder.agent_capabilities",
            "issue-finder.sessions_list",
            "issue-finder.sessions_sync",
            "issue-finder.sessions_search",
            "issue-finder.sessions_read",
            "issue-finder.sessions_rename",
            "issue-finder.sessions_fork",
            "issue-finder.sessions_archive",
            "issue-finder.sessions_approve_mutation",
            "issue-finder.sessions_reject_mutation",
            "issue-finder.dispatch_status",
            "issue-finder.dispatch_events",
            "issue-finder.dispatch_artifacts",
            "issue-finder.dispatch_import_handoff",
            "issue-finder.dispatch_review_list",
            "issue-finder.dispatch_review_show",
            "issue-finder.dispatch_review_approve",
            "issue-finder.dispatch_review_reject",
            "issue-finder.dispatch",
            "issue-finder.dispatch_approve",
            "issue-finder.dispatch_reject",
            "issue-finder.dispatch_execute",
            "issue-finder.a2a_export_task",
            "issue-finder.a2a_approve_send",
            "issue-finder.a2a_reject_send",
            "issue-finder.a2a_import_result",
            "issue-finder.github_draft_tracking_comment",
            "issue-finder.github_draft_final_comment",
            "issue-finder.github_approve_comment",
            "issue-finder.github_reject_comment",
            "issue-finder.github_post_comment",
            "issue-finder.github_retry_comment",
            "issue-finder.github_interactions"
        ]
    );
    assert!(tools.iter().all(|tool| tool["inputSchema"].is_object()));
    let scout = tools
        .iter()
        .find(|tool| tool["name"] == "scout")
        .expect("scout tool spec");
    let scout_properties = scout["inputSchema"]["properties"].as_object().unwrap();
    assert!(scout_properties["repo"].is_object());
    assert!(
        !scout_properties.contains_key("minCategory"),
        "scout schema must not expose the removed minCategory noise parameter"
    );
    let status = tools
        .iter()
        .find(|tool| tool["name"] == "status")
        .expect("status tool spec");
    assert!(status["inputSchema"]["properties"]["checkAuth"].is_object());
    let read_context = tools
        .iter()
        .find(|tool| tool["name"] == "read_context")
        .expect("read_context tool spec");
    assert_eq!(read_context["deferLoading"], true);
    let memory_recall = tools
        .iter()
        .find(|tool| tool["name"] == "memory_recall")
        .expect("memory_recall tool spec");
    assert_eq!(
        memory_recall["inputSchema"]["required"],
        serde_json::json!(["issue"])
    );
    let agents_list = tools
        .iter()
        .find(|tool| tool["name"] == "agents_list")
        .expect("agents_list tool spec");
    assert_eq!(agents_list["inputSchema"]["additionalProperties"], false);
}

#[test]
fn tools_list_cli_outputs_single_json_workflow_entry_object() {
    let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .args(["tools", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1);
    let value = serde_json::from_str::<serde_json::Value>(stdout.trim()).unwrap();
    assert_eq!(value["kind"], "issue_finder_tool_specs");
    assert_eq!(
        value["quickStart"]["firstCall"]["defaultTool"],
        "issue-finder.scout"
    );
    assert!(value["recommendedWorkflow"].is_array());
    assert!(value["tools"].is_array());
}

#[test]
fn agents_list_cli_outputs_builtin_codex_profile() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .env("ISSUE_FINDER_HOME", &paths.home)
        .args(["agents", "list", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value = serde_json::from_str::<serde_json::Value>(stdout.trim()).unwrap();
    let agents = value.as_array().unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0]["id"], "codex");
    assert_eq!(agents[0]["adapter"], "codex_app_server");
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_read_tools_use_local_state_only() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths.clone()).unwrap();
    let task = runtime
        .store()
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
    let session = runtime
        .store()
        .create_session_link(NewAgentSessionLink {
            agent_id: "codex".to_string(),
            native_session_id: "thread_123".to_string(),
            issue_task_id: Some(task.id.clone()),
            display_name: "issue-finder: owner/repo#123 - Fix parser panic".to_string(),
            goal: Some("Fix owner/repo#123".to_string()),
            status: AgentSessionStatus::Linked,
            metadata_json: serde_json::json!({ "threadId": "thread_123" }),
        })
        .unwrap();
    let run = runtime
        .store()
        .create_dispatch_run(NewDispatchRun {
            issue_task_id: task.id.clone(),
            agent_id: "codex".to_string(),
            status: DispatchRunStatus::Proposed,
            requested_by: "test".to_string(),
            approval_state: ApprovalStatus::Pending,
            selected_session_link_id: Some(session.id.clone()),
        })
        .unwrap();
    runtime
        .store()
        .append_agent_event(NewAgentEvent {
            run_id: Some(run.id.clone()),
            session_link_id: Some(session.id),
            event_type: "thread_linked".to_string(),
            native_event_id: Some("event_1".to_string()),
            payload_json: serde_json::json!({ "source": "test" }),
        })
        .unwrap();
    runtime
        .store()
        .write_artifact(
            NewArtifact {
                issue_task_id: Some(task.id),
                run_id: Some(run.id.clone()),
                kind: "fix_result".to_string(),
                content_type: "application/json".to_string(),
                metadata_json: serde_json::json!({}),
            },
            br#"{"summary":"not executed"}"#,
        )
        .unwrap();

    let tool_runtime = IssueFinderToolRuntime::new(paths, Config::default());

    let agents = tool_runtime
        .execute(invocation("issue-finder.agents_list", "{}", "agents_list"))
        .await;
    assert!(agents.success, "{agents:?}");
    assert_eq!(agents.structured_content["agents"][0]["id"], "codex");

    let capabilities = tool_runtime
        .execute(invocation(
            "issue-finder.agent_capabilities",
            r#"{"agent":"codex"}"#,
            "agent_capabilities",
        ))
        .await;
    assert!(capabilities.success, "{capabilities:?}");
    let capability_items = capabilities.structured_content["agentCapabilities"]["capabilities"]
        .as_array()
        .unwrap();
    assert!(capability_items
        .iter()
        .any(|item| { item["capability"] == "open_pr" && item["status"] == "unsupported" }));
    let start_session = capability_items
        .iter()
        .find(|item| item["capability"] == "start_session")
        .expect("start_session capability");
    assert_eq!(start_session["details_json"]["binary"]["name"], "codex");
    assert!(start_session["details_json"]["binary"]["available"].is_boolean());
    let startup = &start_session["details_json"]["startup"];
    assert!(startup["supportedMethods"]
        .as_array()
        .unwrap()
        .iter()
        .any(|method| method == "thread/start"));
    match startup["probe"]["status"].as_str() {
        Some("local_cli_probe") => {
            assert_eq!(
                startup["probe"]["source"],
                "codex_cli_help_and_adapter_method_mapping"
            );
            assert_eq!(startup["connectionModes"][0]["mode"], "daemon_proxy");
        }
        Some("binary_unavailable") => {
            assert_eq!(startup["connectionModes"], serde_json::json!([]));
        }
        other => panic!("unexpected Codex startup probe status: {other:?}"),
    }
    for unsupported_capability in ["interrupt_run", "review_mode", "stream_events"] {
        assert!(
            capability_items.iter().any(|item| {
                item["capability"] == unsupported_capability && item["status"] == "unsupported"
            }),
            "{unsupported_capability} should not be advertised as usable before it is wired into the dispatch runtime"
        );
    }
    for experimental_capability in [
        "start_session",
        "resume_session",
        "fork_session",
        "rename_session",
        "list_sessions",
        "search_sessions",
        "read_transcript",
        "set_goal",
        "set_metadata",
        "archive_session",
    ] {
        assert!(
            capability_items.iter().any(|item| {
                item["capability"] == experimental_capability
                    && item["status"] == "experimental"
            }),
            "{experimental_capability} should stay visible as a wired Codex app-server runtime capability"
        );
    }

    let session_search = tool_runtime
        .execute(invocation(
            "issue-finder.sessions_search",
            r#"{"issue":"owner/repo#123","agent":"codex"}"#,
            "sessions_search",
        ))
        .await;
    assert!(session_search.success, "{session_search:?}");
    assert_eq!(
        session_search.structured_content["sessionSearch"]["sessions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let status_args = format!(r#"{{"runId":"{}"}}"#, run.id);
    let dispatch_status = tool_runtime
        .execute(invocation(
            "issue-finder.dispatch_status",
            &status_args,
            "dispatch_status",
        ))
        .await;
    assert!(dispatch_status.success, "{dispatch_status:?}");
    assert_eq!(
        dispatch_status.structured_content["dispatchStatus"]["run"]["id"],
        run.id
    );

    let dispatch_events = tool_runtime
        .execute(invocation(
            "issue-finder.dispatch_events",
            &status_args,
            "dispatch_events",
        ))
        .await;
    assert!(dispatch_events.success, "{dispatch_events:?}");
    assert_eq!(
        dispatch_events.structured_content["events"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let dispatch_artifacts = tool_runtime
        .execute(invocation(
            "issue-finder.dispatch_artifacts",
            &status_args,
            "dispatch_artifacts",
        ))
        .await;
    assert!(dispatch_artifacts.success, "{dispatch_artifacts:?}");
    assert_eq!(
        dispatch_artifacts.structured_content["artifacts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_package_a2a_and_proposal_tools_use_local_artifacts_only() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let issue = issue("owner/repo", 321);
    let workspace = PreparedWorkspace {
        info: WorkspaceInfo {
            path: dir.path().join("repo").to_string_lossy().to_string(),
            default_branch: "main".to_string(),
            branch: "issue-finder/321-fix-rust-cli-parser-regression".to_string(),
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
    };
    let handoff = Handoff::build(&issue, &workspace);
    let written = write_handoff(&paths, &handoff, &issue).unwrap();
    upsert_ready(&paths, &issue, 88, &written).unwrap();

    let runtime = IssueFinderToolRuntime::new(paths.clone(), Config::default());

    let import_args = serde_json::json!({ "inboxId": written.id }).to_string();
    let imported = runtime
        .execute(invocation(
            "issue-finder.dispatch_import_handoff",
            &import_args,
            "dispatch_import_handoff",
        ))
        .await;
    assert!(imported.success, "{imported:?}");
    assert_eq!(imported.status, "pending_issue_review");
    assert_eq!(
        imported.structured_content["packageImport"]["approvalRequest"]["approval_type"],
        "issue_review"
    );
    assert_eq!(
        imported.structured_content["packageImport"]["issueTask"]["issue_key"],
        "owner/repo#321"
    );
    assert!(imported.structured_content["packageImport"]["package"].is_null());
    let profile_snapshot_id = imported.structured_content["packageImport"]
        ["profileSnapshotArtifact"]["id"]
        .as_str()
        .unwrap();
    assert_eq!(
        imported.structured_content["packageImport"]["issueTask"]["profile_snapshot_artifact_id"],
        profile_snapshot_id
    );
    let review_id = imported.structured_content["packageImport"]["approvalRequest"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let review_list = runtime
        .execute(invocation(
            "issue-finder.dispatch_review_list",
            "{}",
            "dispatch_review_list",
        ))
        .await;
    assert!(review_list.success, "{review_list:?}");
    assert_eq!(
        review_list.structured_content["issueReviews"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let review_show_args = serde_json::json!({ "approvalRequestId": review_id }).to_string();
    let review_show = runtime
        .execute(invocation(
            "issue-finder.dispatch_review_show",
            &review_show_args,
            "dispatch_review_show",
        ))
        .await;
    assert!(review_show.success, "{review_show:?}");
    assert_eq!(
        review_show.structured_content["issueReview"]["approvalRequest"]["status"],
        "pending"
    );
    let review_approve = runtime
        .execute(invocation(
            "issue-finder.dispatch_review_approve",
            &review_show_args,
            "dispatch_review_approve",
        ))
        .await;
    assert!(review_approve.success, "{review_approve:?}");
    assert_eq!(review_approve.status, "approved");
    assert_eq!(
        review_approve.structured_content["issueReviewApproval"]["package"]["kind"],
        "issue_finder_task_package"
    );
    assert_eq!(
        review_approve.structured_content["issueReviewApproval"]["package"]["version"],
        2
    );
    assert_eq!(
        review_approve.structured_content["issueReviewApproval"]["package"]
            ["user_profile_snapshot"]["snapshot"]["profile"]["techStack"],
        serde_json::json!(["Rust", "TypeScript"])
    );
    assert_eq!(
        review_approve.structured_content["issueReviewApproval"]["issueTask"]["status"],
        "user_approved"
    );

    let proposal = runtime
        .execute(invocation(
            "issue-finder.dispatch",
            r#"{"issue":"owner/repo#321","agent":"codex","newSession":true}"#,
            "dispatch",
        ))
        .await;
    assert!(proposal.success, "{proposal:?}");
    assert_eq!(proposal.status, "pending_approval");
    assert_eq!(
        proposal.structured_content["dispatchProposal"]["approvalRequest"]["status"],
        "pending"
    );
    let run_id = proposal.structured_content["dispatchProposal"]["run"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let execute_args = serde_json::json!({ "runId": run_id }).to_string();
    let pending_execution = runtime
        .execute(invocation(
            "issue-finder.dispatch_execute",
            &execute_args,
            "dispatch_execute",
        ))
        .await;
    assert!(pending_execution.success, "{pending_execution:?}");
    assert_eq!(pending_execution.status, "pending_approval");
    assert_eq!(
        pending_execution.structured_content["approvalRequired"],
        true
    );

    let approve_args = serde_json::json!({ "runId": run_id }).to_string();
    let approval = runtime
        .execute(invocation(
            "issue-finder.dispatch_approve",
            &approve_args,
            "dispatch_approve",
        ))
        .await;
    assert!(approval.success, "{approval:?}");
    assert_eq!(approval.status, "approved");
    assert_eq!(
        approval.structured_content["dispatchApproval"]["run"]["approval_state"],
        "approved"
    );

    let a2a_export = runtime
        .execute(invocation(
            "issue-finder.a2a_export_task",
            r#"{"issue":"owner/repo#321"}"#,
            "a2a_export_task",
        ))
        .await;
    assert!(a2a_export.success, "{a2a_export:?}");
    assert_eq!(a2a_export.status, "pending_approval");
    assert_eq!(
        a2a_export.structured_content["a2aExport"]["task"]["task"]["taskType"],
        "fix_github_issue"
    );
    assert_eq!(
        a2a_export.structured_content["a2aExport"]["task"]["callback"]["importMode"],
        "local_artifact_only"
    );
    assert_eq!(
        a2a_export.structured_content["a2aExport"]["approvalRequest"]["approval_type"],
        "a2a_send"
    );
    let a2a_approval_id = a2a_export.structured_content["a2aExport"]["approvalRequest"]["id"]
        .as_str()
        .unwrap();
    let a2a_approve_args = serde_json::json!({ "approvalRequestId": a2a_approval_id }).to_string();
    let a2a_approval = runtime
        .execute(invocation(
            "issue-finder.a2a_approve_send",
            &a2a_approve_args,
            "a2a_approve_send",
        ))
        .await;
    assert!(a2a_approval.success, "{a2a_approval:?}");
    assert_eq!(a2a_approval.status, "approved");
    assert_eq!(
        a2a_approval.structured_content["a2aApproval"]["approvalRequest"]["status"],
        "approved"
    );

    let result_path = dir.path().join("fix_result.json");
    fs::write(&result_path, r#"{"summary":"fixed in local artifact"}"#).unwrap();
    let import_result_args = serde_json::json!({
        "runId": run_id,
        "path": result_path,
        "kind": "fix_result",
        "contentType": "application/json",
        "status": "completed"
    })
    .to_string();
    let imported_result = runtime
        .execute(invocation(
            "issue-finder.a2a_import_result",
            &import_result_args,
            "a2a_import_result",
        ))
        .await;
    assert!(imported_result.success, "{imported_result:?}");
    assert_eq!(
        imported_result.structured_content["a2aResultImport"]["run"]["status"],
        "completed"
    );
    assert_eq!(
        imported_result.structured_content["a2aResultImport"]["artifact"]["kind"],
        "fix_result"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_tool_auto_imports_ready_handoff_for_direct_proposal() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let issue = issue("owner/auto", 654);
    let workspace = PreparedWorkspace {
        info: WorkspaceInfo {
            path: dir.path().join("repo").to_string_lossy().to_string(),
            default_branch: "main".to_string(),
            branch: "issue-finder/654-fix-parser-panic".to_string(),
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
    };
    let handoff = Handoff::build(&issue, &workspace);
    let written = write_handoff(&paths, &handoff, &issue).unwrap();
    upsert_ready(&paths, &issue, 87, &written).unwrap();

    let runtime = IssueFinderToolRuntime::new(paths.clone(), Config::default());
    let proposal = runtime
        .execute(invocation(
            "issue-finder.dispatch",
            r#"{"issue":"owner/auto#654","agent":"codex","newSession":true}"#,
            "dispatch",
        ))
        .await;

    assert!(proposal.success, "{proposal:?}");
    assert_eq!(proposal.status, "pending_issue_review");
    assert_eq!(proposal.structured_content["blocked"], true);
    assert_eq!(proposal.structured_content["reviewRequired"], true);
    assert_eq!(proposal.structured_content["issueKey"], "owner/auto#654");
    assert!(proposal.structured_content["approvalRequestId"].is_string());
}

#[tokio::test(flavor = "current_thread")]
async fn projection_tools_auto_import_ready_handoff_when_needed() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let a2a_issue = issue("owner/a2a", 655);
    let github_issue = issue("owner/track", 656);
    let workspace = PreparedWorkspace {
        info: WorkspaceInfo {
            path: dir.path().join("repo").to_string_lossy().to_string(),
            default_branch: "main".to_string(),
            branch: "issue-finder/655-fix-parser-panic".to_string(),
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
    };
    let a2a_handoff = Handoff::build(&a2a_issue, &workspace);
    let a2a_written = write_handoff(&paths, &a2a_handoff, &a2a_issue).unwrap();
    upsert_ready(&paths, &a2a_issue, 86, &a2a_written).unwrap();
    let github_handoff = Handoff::build(&github_issue, &workspace);
    let github_written = write_handoff(&paths, &github_handoff, &github_issue).unwrap();
    upsert_ready(&paths, &github_issue, 85, &github_written).unwrap();

    let runtime = IssueFinderToolRuntime::new(paths.clone(), Config::default());
    let a2a_export = runtime
        .execute(invocation(
            "issue-finder.a2a_export_task",
            r#"{"issue":"owner/a2a#655"}"#,
            "a2a_export_task",
        ))
        .await;
    assert!(a2a_export.success, "{a2a_export:?}");
    assert_eq!(a2a_export.status, "pending_issue_review");
    assert_eq!(a2a_export.structured_content["issueKey"], "owner/a2a#655");
    assert_eq!(a2a_export.structured_content["reviewRequired"], true);
    assert!(a2a_export.structured_content["approvalRequestId"].is_string());

    let github_draft = runtime
        .execute(invocation(
            "issue-finder.github_draft_tracking_comment",
            r#"{"issue":"owner/track#656"}"#,
            "github_draft_tracking_comment",
        ))
        .await;
    assert!(github_draft.success, "{github_draft:?}");
    assert_eq!(github_draft.status, "pending_issue_review");
    assert_eq!(
        github_draft.structured_content["issueKey"],
        "owner/track#656"
    );
    assert_eq!(github_draft.structured_content["reviewRequired"], true);
    assert!(github_draft.structured_content["approvalRequestId"].is_string());
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_projection_tools_report_missing_task_package_as_structured_block() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = IssueFinderToolRuntime::new(paths, Config::default());

    for (tool, args) in [
        (
            "issue-finder.dispatch",
            r#"{"issue":"owner/missing#909","agent":"codex","newSession":true}"#,
        ),
        (
            "issue-finder.a2a_export_task",
            r#"{"issue":"owner/missing#909"}"#,
        ),
        (
            "issue-finder.github_draft_tracking_comment",
            r#"{"issue":"owner/missing#909"}"#,
        ),
    ] {
        let result = runtime.execute(invocation(tool, args, tool)).await;

        assert!(result.success, "{tool}: {result:?}");
        assert_eq!(result.status, "missing_task_package", "{tool}");
        assert_eq!(result.structured_content["blocked"], true, "{tool}");
        assert_eq!(
            result.structured_content["issueKey"], "owner/missing#909",
            "{tool}"
        );
        assert_eq!(
            result.structured_content["missingTaskPackage"], true,
            "{tool}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_tool_reports_missing_capability_as_structured_block() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths.clone()).unwrap();
    runtime
        .store()
        .create_agent_profile(NewAgentProfile {
            id: Some("limited".to_string()),
            kind: "test".to_string(),
            display_name: "Limited".to_string(),
            adapter: "codex_app_server".to_string(),
            config_json: serde_json::json!({}),
            enabled: true,
        })
        .unwrap();
    runtime
        .store()
        .upsert_agent_capability(NewAgentCapability {
            agent_id: "limited".to_string(),
            capability: AgentCapabilityName::StartSession,
            status: CapabilityStatus::Unsupported,
            details_json: serde_json::json!({ "reason": "test disabled" }),
        })
        .unwrap();
    runtime
        .store()
        .upsert_agent_capability(NewAgentCapability {
            agent_id: "limited".to_string(),
            capability: AgentCapabilityName::ReadTranscript,
            status: CapabilityStatus::Unsupported,
            details_json: serde_json::json!({ "reason": "test disabled" }),
        })
        .unwrap();
    let task = runtime
        .store()
        .upsert_issue_task(NewIssueTask {
            repo_full_name: "owner/repo".to_string(),
            issue_number: 777,
            title: "Fix parser panic".to_string(),
            url: "https://github.com/owner/repo/issues/777".to_string(),
            status: IssueTaskStatus::UserApproved,
            priority: Some(10),
            category: Some("high_value_ready".to_string()),
        })
        .unwrap();
    let package = IssueTaskPackage::new(IssueTaskPackageIssue {
        repo_full_name: "owner/repo".to_string(),
        number: 777,
        title: "Fix parser panic".to_string(),
        url: "https://github.com/owner/repo/issues/777".to_string(),
    });
    runtime
        .store()
        .write_task_package_artifact(&task.id, &package)
        .unwrap();
    let tool_runtime = IssueFinderToolRuntime::new(paths, Config::default());

    let result = tool_runtime
        .execute(invocation(
            "issue-finder.dispatch",
            r#"{"issue":"owner/repo#777","agent":"limited","newSession":true}"#,
            "dispatch_limited",
        ))
        .await;

    assert!(result.success, "{result:?}");
    assert_eq!(result.status, "unsupported_capability");
    assert_eq!(result.structured_content["blocked"], true);
    assert_eq!(
        result.structured_content["unsupportedCapability"],
        "start_session"
    );
    assert!(result.structured_content["reason"]
        .as_str()
        .unwrap()
        .contains("does not support capability start_session"));

    let session = runtime
        .store()
        .create_session_link(NewAgentSessionLink {
            agent_id: "limited".to_string(),
            native_session_id: "native_limited".to_string(),
            issue_task_id: Some(task.id),
            display_name: "limited session".to_string(),
            goal: None,
            status: AgentSessionStatus::Idle,
            metadata_json: serde_json::json!({}),
        })
        .unwrap();
    let read_args = serde_json::json!({ "sessionLinkId": session.id }).to_string();
    let read_result = tool_runtime
        .execute(invocation(
            "issue-finder.sessions_read",
            &read_args,
            "sessions_read_limited",
        ))
        .await;
    assert!(read_result.success, "{read_result:?}");
    assert_eq!(read_result.status, "unsupported_capability");
    assert_eq!(
        read_result.structured_content["unsupportedCapability"],
        "read_transcript"
    );
}

#[test]
fn tools_call_invalid_arguments_emits_single_json_object() {
    let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .args([
            "tools",
            "call",
            "issue-finder.scout",
            "--arguments",
            "[]",
            "--call-id",
            "call_test",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1);
    let value = serde_json::from_str::<serde_json::Value>(stdout.trim()).unwrap();
    assert_eq!(value["call_id"], "call_test");
    assert_eq!(value["success"], false);
    assert_eq!(value["status"], "invalid_arguments");
}

#[test]
fn tools_call_rejects_removed_scout_min_category_argument() {
    let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .args([
            "tools",
            "call",
            "issue-finder.scout",
            "--arguments",
            r#"{"limit":1,"minCategory":"high_value_ready"}"#,
            "--call-id",
            "min_category_call",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1);
    let value = serde_json::from_str::<serde_json::Value>(stdout.trim()).unwrap();
    assert_eq!(value["call_id"], "min_category_call");
    assert_eq!(value["success"], false);
    assert_eq!(value["status"], "invalid_arguments");
    assert!(value["structured_content"]["error"]["message"]
        .as_str()
        .unwrap()
        .contains("minCategory"));
}

#[test]
fn tools_call_status_reports_invalid_config_as_json() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    fs::create_dir_all(&paths.home).unwrap();
    fs::write(&paths.config, "github = [").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .env("ISSUE_FINDER_HOME", &paths.home)
        .env_remove("GITHUB_TOKEN")
        .args([
            "tools",
            "call",
            "issue-finder.status",
            "--arguments",
            r#"{"checkAuth":false}"#,
            "--call-id",
            "status_invalid_config",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1);
    let value = serde_json::from_str::<serde_json::Value>(stdout.trim()).unwrap();
    assert_eq!(value["call_id"], "status_invalid_config");
    assert_eq!(value["success"], true);
    assert_eq!(value["status"], "needs_setup");
    assert_eq!(value["structured_content"]["config"]["exists"], true);
    assert_eq!(value["structured_content"]["config"]["loadOk"], false);
    assert!(value["structured_content"]["config"]["loadError"].is_string());
    assert_eq!(
        value["structured_content"]["nextFixCommand"],
        "issue-finder init --force"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn tool_status_rejects_unknown_arguments() {
    let dir = tempdir().unwrap();
    let runtime = IssueFinderToolRuntime::new(test_paths(dir.path()), Config::default());

    let status = runtime
        .execute(invocation(
            "issue-finder.status",
            r#"{"checkAuth":false,"unexpected":true}"#,
            "status_unknown_arg",
        ))
        .await;

    assert!(!status.success);
    assert_eq!(status.status, "invalid_arguments");
    assert!(status.structured_content["error"]["message"]
        .as_str()
        .unwrap()
        .contains("unexpected"));
}

#[tokio::test(flavor = "current_thread")]
async fn tool_memory_status_and_hint_update_are_structured() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    seed_candidate_memory_hint(&paths);
    let runtime = IssueFinderToolRuntime::new(paths, Config::default());

    let status = runtime
        .execute(invocation(
            "issue-finder.memory_status",
            "{}",
            "memory_status_call",
        ))
        .await;
    assert!(status.success, "{status:?}");
    assert_eq!(status.status, "ok");
    assert_eq!(status.structured_content["kind"], "memory_status");
    assert_eq!(status.structured_content["counts"]["hints"], 1);

    let invalid = runtime
        .execute(invocation(
            "issue-finder.memory_hint_update",
            r#"{"hintId":"candidate-hint","action":"pin"}"#,
            "memory_hint_update_invalid",
        ))
        .await;
    assert!(!invalid.success, "{invalid:?}");
    assert_eq!(invalid.status, "invalid_transition");
    assert_eq!(
        invalid.structured_content["tool"],
        "issue-finder.memory_hint_update"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn tool_status_reports_missing_config_and_token() {
    let _env_lock = ENV_LOCK.lock().await;
    let _token_guard = EnvVarGuard::unset("GITHUB_TOKEN");
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = IssueFinderToolRuntime::new(paths.clone(), Config::default());

    let status = runtime
        .execute(invocation("issue-finder.status", "{}", "status_missing"))
        .await;

    assert!(status.success, "{status:?}");
    assert_eq!(status.status, "needs_setup");
    assert_eq!(
        status.structured_content["config"]["path"].as_str(),
        Some(paths.config.to_string_lossy().as_ref())
    );
    assert_eq!(status.structured_content["config"]["exists"], false);
    assert_eq!(
        status.structured_content["github"]["tokenSource"],
        "missing"
    );
    assert_eq!(status.structured_content["github"]["auth"]["checked"], true);
    assert_eq!(status.structured_content["github"]["auth"]["ok"], false);
    assert_eq!(
        status.structured_content["nextFixCommand"],
        r#"export GITHUB_TOKEN="$(gh auth token)""#
    );
}

#[tokio::test(flavor = "current_thread")]
async fn tool_status_reports_config_token_without_auth_check() {
    let _env_lock = ENV_LOCK.lock().await;
    let _token_guard = EnvVarGuard::unset("GITHUB_TOKEN");
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let mut config = Config::default();
    config.github.token = "config-token".to_string();
    config.save(&paths).unwrap();
    let runtime = IssueFinderToolRuntime::new(paths.clone(), config);

    let status = runtime
        .execute(invocation(
            "issue-finder.status",
            r#"{"checkAuth":false}"#,
            "status_config_token",
        ))
        .await;

    assert!(status.success, "{status:?}");
    assert_eq!(status.status, "ready");
    assert_eq!(status.structured_content["config"]["exists"], true);
    assert_eq!(status.structured_content["github"]["tokenSource"], "config");
    assert_eq!(
        status.structured_content["github"]["auth"]["checked"],
        false
    );
    assert_eq!(
        status.structured_content["nextFixCommand"],
        serde_json::Value::Null
    );
}

#[tokio::test(flavor = "current_thread")]
async fn tool_status_prefers_env_token_and_reports_auth_login() {
    let _env_lock = ENV_LOCK.lock().await;
    let _file_env_lock = env_lock::EnvLock::acquire();
    let _token_guard = EnvVarGuard::set("GITHUB_TOKEN", "env-token");
    let mock_github = start_mock_tool_github();
    let _api_env_guard = EnvVarGuard::set("ISSUE_FINDER_GITHUB_API_BASE", mock_github.base_url());

    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let mut config = Config::default();
    config.github.token = "config-token".to_string();
    config.save(&paths).unwrap();
    let runtime = IssueFinderToolRuntime::new(paths.clone(), config);

    let status = runtime
        .execute(invocation("issue-finder.status", "{}", "status_env_token"))
        .await;

    assert!(status.success, "{status:?}");
    assert_eq!(status.status, "ready");
    assert_eq!(
        status.structured_content["github"]["tokenSource"],
        "env:GITHUB_TOKEN"
    );
    assert_eq!(status.structured_content["github"]["auth"]["ok"], true);
    assert_eq!(
        status.structured_content["github"]["auth"]["login"],
        "tool-user"
    );

    mock_github.stop();
}

#[test]
fn daily_and_tool_prepare_gate_share_allowed_category_policy() {
    for category in [
        RecommendationCategory::HighValueReady,
        RecommendationCategory::HighValueNeedsScoping,
        RecommendationCategory::NicheButActionable,
        RecommendationCategory::ContestedOrLowTrust,
        RecommendationCategory::NeedsTriage,
        RecommendationCategory::FilteredLowDepth,
    ] {
        let assessment = ValueAssessment {
            category,
            recommendation_category: category,
            ..ValueAssessment::default()
        };
        assert_eq!(
            is_daily_prepare_candidate(&assessment),
            default_prepare_allowed(category)
        );
        let decision = prepare_gate_decision(&assessment, None);
        assert_eq!(
            matches!(decision, PrepareGateDecision::Allowed),
            default_prepare_allowed(category)
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn tool_runtime_uses_mocked_github_and_applies_prepare_gate() {
    let _env_lock = ENV_LOCK.lock().await;
    let _file_env_lock = env_lock::EnvLock::acquire();
    if !git_available() {
        return;
    }

    let mock_github = start_mock_tool_github();
    let _api_env_guard = EnvVarGuard::set("ISSUE_FINDER_GITHUB_API_BASE", mock_github.base_url());

    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let runtime = IssueFinderToolRuntime::new(paths.clone(), Config::default());

    let scout = runtime
        .execute(invocation(
            "issue-finder.scout",
            r#"{"limit":5,"refresh":true,"includeFiltered":false}"#,
            "scout_call",
        ))
        .await;
    assert!(scout.success, "{scout:?}");
    assert_eq!(scout.status, "ok");
    let scout_candidates = scout.structured_content["candidates"].as_array().unwrap();
    assert!(scout_candidates
        .iter()
        .any(|candidate| candidate["category"] == "high_value_ready"));
    assert!(scout_candidates
        .iter()
        .all(|candidate| candidate["category"] != "filtered_low_depth"));
    assert_eq!(scout.structured_content["filteredCount"], 2);
    assert!(scout_candidates[0]["gates"]["repoInfluence"]["status"].is_string());
    assert!(scout_candidates[0]["recommendation"]["finalFeedScore"].is_number());
    let events = load_events(&paths).unwrap();
    assert!(events.iter().any(|event| {
        event.event_type == RecommendationEventType::Shown
            && event.source == RecommendationEventSource::ToolScout
    }));

    let shown_count = events
        .iter()
        .filter(|event| event.event_type == RecommendationEventType::Shown)
        .count();
    let scout_no_record = runtime
        .execute(invocation(
            "issue-finder.scout",
            r#"{"limit":5,"refresh":false,"includeFiltered":false,"recordExposure":false}"#,
            "scout_no_record_call",
        ))
        .await;
    assert!(scout_no_record.success, "{scout_no_record:?}");
    let events_after_no_record = load_events(&paths).unwrap();
    assert_eq!(
        shown_count,
        events_after_no_record
            .iter()
            .filter(|event| event.event_type == RecommendationEventType::Shown)
            .count()
    );

    let assess = runtime
        .execute(invocation(
            "issue-finder.assess",
            r#"{"issue":"owner/niche#1"}"#,
            "assess_call",
        ))
        .await;
    assert!(assess.success, "{assess:?}");
    assert_eq!(assess.status, "ok");
    assert_eq!(
        assess.structured_content["assessment"]["category"],
        "niche_but_actionable"
    );
    assert_eq!(
        assess.structured_content["prepareGate"]["requiresBypass"],
        true
    );
    assert!(load_index(&paths).unwrap().items.is_empty());
    assert!(!paths.workspace_path_for("owner/niche").exists());
    assert!(fs::read_dir(&paths.inbox_dir).unwrap().next().is_none());
    assert!(load_events(&paths).unwrap().iter().any(|event| {
        event.event_type == RecommendationEventType::Read
            && event.source == RecommendationEventSource::ToolAssess
            && event.issue_key.repo_full_name == "owner/niche"
    }));

    let blocked = runtime
        .execute(invocation(
            "issue-finder.prepare",
            r#"{"issue":"owner/niche#1"}"#,
            "blocked_call",
        ))
        .await;
    assert!(blocked.success, "{blocked:?}");
    assert_eq!(blocked.status, "blocked_by_gate");
    assert_eq!(blocked.structured_content["success"], true);
    assert_eq!(
        blocked.structured_content["prepareGate"]["blockedCategory"],
        "niche_but_actionable"
    );
    assert!(!paths.workspace_path_for("owner/niche").exists());
    assert!(load_index(&paths).unwrap().items.is_empty());

    let missing_reason = runtime
        .execute(invocation(
            "issue-finder.prepare",
            r#"{"issue":"owner/niche#1","allowGateBypass":true,"bypassReason":" "}"#,
            "missing_reason_call",
        ))
        .await;
    assert!(!missing_reason.success);
    assert_eq!(missing_reason.status, "invalid_arguments");

    let remote = create_remote_repo(dir.path());
    clone_into_workspace(&remote, &paths, "owner/niche");
    let prepared = runtime
        .execute(invocation(
            "issue-finder.prepare",
            r#"{"issue":"owner/niche#1","allowGateBypass":true,"bypassReason":"Test bypass for niche issue"}"#,
            "prepared_call",
        ))
        .await;
    assert!(prepared.success, "{prepared:?}");
    assert_eq!(prepared.status, "prepared");
    assert_eq!(
        prepared.structured_content["gateBypass"]["reason"],
        "Test bypass for niche issue"
    );
    let handoff_json_path = prepared.structured_content["handoff"]["handoffJsonPath"]
        .as_str()
        .unwrap();
    let codex_path = prepared.structured_content["handoff"]["codexMarkdownPath"]
        .as_str()
        .unwrap();
    let events_path = prepared.structured_content["handoff"]["prepareEventsPath"]
        .as_str()
        .unwrap();
    assert!(PathBuf::from(handoff_json_path).exists());
    assert!(PathBuf::from(codex_path).exists());
    assert!(fs::read_to_string(events_path)
        .unwrap()
        .contains("Test bypass for niche issue"));
    assert!(fs::read_to_string(handoff_json_path)
        .unwrap()
        .contains("Prepare gate bypass: Test bypass for niche issue"));

    clone_into_workspace(&remote, &paths, "owner/ready");
    let human_prepared = workflow::prepare_from_input(
        &paths,
        &Config::default(),
        Some("owner/ready#1".to_string()),
        None,
    )
    .await
    .unwrap();
    let PrepareOutcome::Prepared(human_item) = human_prepared else {
        panic!("expected human prepare to prepare owner/ready");
    };
    assert!(PathBuf::from(&human_item.handoff_json_path).exists());

    let handoff_id = prepared.structured_content["handoff"]["id"]
        .as_str()
        .unwrap();
    let context = runtime
        .execute(invocation(
            "issue-finder.read_context",
            &format!(r#"{{"handoffId":"{handoff_id}","section":"entry"}}"#),
            "read_call",
        ))
        .await;
    assert!(context.success, "{context:?}");
    assert_eq!(context.status, "ok");
    assert!(context.structured_content["content"]
        .as_str()
        .unwrap()
        .contains("# Entry"));

    mock_github.stop();
}

#[tokio::test]
async fn tool_read_context_allows_fixed_sections_and_rejects_escape() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let handoff_dir = paths.inbox_item_dir("handoff-1");
    fs::create_dir_all(handoff_dir.join("context")).unwrap();
    fs::write(handoff_dir.join("handoff.json"), "{}").unwrap();
    fs::write(handoff_dir.join("agent-policy.json"), "{}").unwrap();
    fs::write(handoff_dir.join("probe.json"), "{}").unwrap();
    fs::write(handoff_dir.join("context/entry.md"), "abcdef").unwrap();
    fs::write(handoff_dir.join("context/repo.md"), "repo context").unwrap();
    upsert_ready(
        &paths,
        &issue("owner/context", 4),
        80,
        &WrittenHandoff {
            id: "handoff-1".to_string(),
            dir: handoff_dir.to_string_lossy().to_string(),
            handoff_json_path: handoff_dir
                .join("handoff.json")
                .to_string_lossy()
                .to_string(),
            handoff_md_path: handoff_dir.join("handoff.md").to_string_lossy().to_string(),
            codex_md_path: handoff_dir.join("codex.md").to_string_lossy().to_string(),
            agent_policy_path: handoff_dir
                .join("agent-policy.json")
                .to_string_lossy()
                .to_string(),
            probe_json_path: handoff_dir.join("probe.json").to_string_lossy().to_string(),
            prepare_events_path: handoff_dir
                .join("prepare-events.jsonl")
                .to_string_lossy()
                .to_string(),
        },
    )
    .unwrap();
    let runtime = IssueFinderToolRuntime::new(paths.clone(), Config::default());

    let truncated = runtime
        .execute(invocation(
            "issue-finder.read_context",
            r#"{"handoffId":"handoff-1","section":"entry","maxBytes":3}"#,
            "truncate_call",
        ))
        .await;
    assert!(truncated.success, "{truncated:?}");
    assert_eq!(truncated.structured_content["truncated"], true);
    assert_eq!(truncated.structured_content["content"], "abc");

    let traversal = runtime
        .execute(invocation(
            "issue-finder.read_context",
            r#"{"handoffId":"handoff-1","section":"../handoff.json"}"#,
            "traversal_call",
        ))
        .await;
    assert!(!traversal.success);
    assert_eq!(traversal.status, "invalid_arguments");

    #[cfg(unix)]
    {
        fs::remove_file(handoff_dir.join("context/repo.md")).unwrap();
        let outside = dir.path().join("outside.md");
        fs::write(&outside, "outside").unwrap();
        std::os::unix::fs::symlink(&outside, handoff_dir.join("context/repo.md")).unwrap();
        let escaped = runtime
            .execute(invocation(
                "issue-finder.read_context",
                r#"{"handoffId":"handoff-1","section":"repo"}"#,
                "escape_call",
            ))
            .await;
        assert!(!escaped.success);
    }
}

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, original }
    }

    fn unset(key: &'static str) -> Self {
        let original = std::env::var(key).ok();
        std::env::remove_var(key);
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

fn invocation(tool: &str, arguments: &str, call_id: &str) -> IssueFinderToolInvocation {
    IssueFinderToolInvocation::from_json_arguments(
        tool.to_string(),
        arguments,
        Some(call_id.to_string()),
        None,
    )
    .unwrap()
}

fn test_paths(root: &Path) -> IssueFinderPaths {
    IssueFinderPaths {
        home: root.join("issue-finder-home"),
        config: root.join("issue-finder-home/config.toml"),
        cache_dir: root.join("issue-finder-home/cache"),
        workspaces_dir: root.join("issue-finder-home/workspaces"),
        inbox_dir: root.join("issue-finder-home/inbox"),
        reports_dir: root.join("issue-finder-home/reports"),
    }
}

fn seed_candidate_memory_hint(paths: &IssueFinderPaths) {
    let store = MemoryStore::open(paths).unwrap();
    store
        .insert_dream_run(&MemoryDreamRun {
            id: "candidate-dream-run".to_string(),
            trigger: MemoryDreamTrigger::Manual,
            scope: MemoryDreamScope::Global,
            input_activation_run_ids_json: serde_json::json!([]),
            model_status: MemoryModelStatus::Disabled,
            created_at: Utc::now().to_rfc3339(),
        })
        .unwrap();
    store
        .insert_dream(&NewMemoryDream {
            id: "candidate-dream".to_string(),
            dream_run_id: "candidate-dream-run".to_string(),
            dream_type: MemoryDreamType::DiscoveryPolicy,
            summary: "Candidate policy".to_string(),
            evidence_node_ids_json: serde_json::json!([]),
            evidence_event_ids_json: serde_json::json!([]),
            evidence_hint_ids_json: serde_json::json!([]),
            status: MemoryDreamStatus::Candidate,
            confidence: 0.5,
            version: 1,
            created_at: Utc::now().to_rfc3339(),
            reviewed_at: None,
        })
        .unwrap();
    store
        .insert_hint(&NewMemoryHint {
            id: "candidate-hint".to_string(),
            dream_id: "candidate-dream".to_string(),
            hint_type: MemoryHintType::Ranking,
            scope_type: MemoryHintScopeType::Repo,
            scope_ref: "owner/repo".to_string(),
            summary: "Candidate ranking hint".to_string(),
            policy_json: serde_json::json!({"kind": "ranking_test"}),
            weight: 1.0,
            status: MemoryHintStatus::Candidate,
            created_at: Utc::now().to_rfc3339(),
            approved_at: None,
            expires_at: None,
        })
        .unwrap();
}

fn issue(repo_full_name: &str, number: u64) -> GitHubIssue {
    GitHubIssue {
        id: number,
        number,
        title: "Fix Rust CLI parser regression".to_string(),
        body: actionable_body(),
        labels: vec!["good first issue".to_string()],
        url: format!("https://github.com/{repo_full_name}/issues/{number}"),
        repo_full_name: repo_full_name.to_string(),
        repo_name: repo_full_name.split('/').nth(1).unwrap().to_string(),
        repo_description: "Rust CLI developer tools".to_string(),
        repo_stars: 0,
        created_at: Utc::now().to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
    }
}

struct MockToolGithub {
    base_url: String,
    shutdown: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl MockToolGithub {
    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn stop(mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
    }
}

impl Drop for MockToolGithub {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
    }
}

fn start_mock_tool_github() -> MockToolGithub {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let base_url_for_thread = base_url.clone();
    let search_count = Arc::new(AtomicUsize::new(0));
    let search_count_for_thread = Arc::clone(&search_count);
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_thread = Arc::clone(&shutdown);

    let handle = thread::spawn(move || {
        let started = Instant::now();
        while !shutdown_for_thread.load(Ordering::SeqCst)
            && started.elapsed() < Duration::from_secs(60)
        {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buffer = [0u8; 4096];
                    let bytes_read = stream.read(&mut buffer).unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
                    let response =
                        response_body(&request, &base_url_for_thread, &search_count_for_thread);
                    write_response(&mut stream, response.status, &response.body);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

    MockToolGithub {
        base_url,
        shutdown,
        handle: Some(handle),
    }
}

struct MockResponse {
    status: u16,
    body: String,
}

fn response_body(request: &str, base_url: &str, search_count: &AtomicUsize) -> MockResponse {
    let target = request_target(request);
    if target == "/user" {
        return ok_response(r#"{"login":"tool-user"}"#);
    }

    if target.starts_with("/search/issues") {
        let count = search_count.fetch_add(1, Ordering::SeqCst);
        let body = if count == 0 {
            search_body(base_url)
        } else {
            r#"{"items":[]}"#.to_string()
        };
        return ok_response(&body);
    }

    for repo in ["niche", "ready", "lowdepth"] {
        let prefix = format!("/repos/owner/{repo}");
        if target.starts_with(&format!("{prefix}/issues/1/comments")) {
            return ok_response(&comments_body());
        }
        if target.starts_with(&format!("{prefix}/issues/1/timeline")) {
            return ok_response("[]");
        }
        if target.starts_with(&format!("{prefix}/stargazers")) {
            return ok_response(&stargazers_body(repo));
        }
        if target.starts_with(&format!("{prefix}/forks")) {
            return ok_response(&forks_body(repo));
        }
        if target.starts_with(&format!("{prefix}/issues/1")) {
            return ok_response(&issue_body(repo));
        }
        if target == prefix {
            return ok_response(&repo_body(repo));
        }
    }

    MockResponse {
        status: 404,
        body: format!(
            r#"{{"message":"mock route not found","target":{}}}"#,
            serde_json::to_string(target).unwrap()
        ),
    }
}

fn request_target(request: &str) -> &str {
    request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("")
}

fn ok_response(body: &str) -> MockResponse {
    MockResponse {
        status: 200,
        body: body.to_string(),
    }
}

fn search_body(base_url: &str) -> String {
    format!(
        r#"{{
  "items": [
    {niche},
    {ready},
    {lowdepth}
  ]
}}"#,
        niche = search_item(base_url, "niche"),
        ready = search_item(base_url, "ready"),
        lowdepth = search_item(base_url, "lowdepth")
    )
}

fn search_item(base_url: &str, repo: &str) -> String {
    format!(
        r#"{{
      "id": 1,
      "number": 1,
      "title": "{title}",
      "body": "{body}",
      "html_url": "https://github.com/owner/{repo}/issues/1",
      "repository_url": "{base_url}/repos/owner/{repo}",
      "labels": [{{ "name": "good first issue" }}],
      "locked": false,
      "created_at": "{timestamp}",
      "updated_at": "{timestamp}"
    }}"#,
        title = issue_title(repo),
        body = json_string_literal(&issue_body_text(repo)),
        timestamp = Utc::now().to_rfc3339()
    )
}

fn issue_body(repo: &str) -> String {
    format!(
        r#"{{
  "id": 1,
  "number": 1,
  "title": "{title}",
  "body": "{body}",
  "html_url": "https://github.com/owner/{repo}/issues/1",
  "labels": [{{ "name": "good first issue" }}],
  "pull_request": null,
  "locked": false,
  "assignee": null,
  "assignees": [],
  "created_at": "{timestamp}",
  "updated_at": "{timestamp}",
  "comments": 1,
  "author_association": "CONTRIBUTOR",
  "user": {{ "login": "issue-author" }}
}}"#,
        title = issue_title(repo),
        body = json_string_literal(&issue_body_text(repo)),
        timestamp = Utc::now().to_rfc3339()
    )
}

fn repo_body(repo: &str) -> String {
    let (stars, forks, subscribers, open_issues) = match repo {
        "ready" | "lowdepth" => (2_500, 220, 50, 12),
        _ => (0, 0, 0, 12),
    };
    format!(
        r#"{{
  "full_name": "owner/{repo}",
  "name": "{repo}",
  "description": "Rust CLI parser developer tools",
  "stargazers_count": {stars},
  "forks_count": {forks},
  "subscribers_count": {subscribers},
  "open_issues_count": {open_issues},
  "pushed_at": "{timestamp}",
  "created_at": "2025-01-01T00:00:00Z",
  "updated_at": "{timestamp}",
  "default_branch": "main",
  "topics": ["rust", "cli", "parser"],
  "language": "Rust",
  "archived": false
}}"#,
        timestamp = Utc::now().to_rfc3339()
    )
}

fn comments_body() -> String {
    format!(
        r#"[{{
  "body": "Maintainer note: this is a good first contribution.",
  "author_association": "MEMBER",
  "created_at": "{}",
  "user": {{ "login": "maintainer" }}
}}]"#,
        Utc::now().to_rfc3339()
    )
}

fn stargazers_body(repo: &str) -> String {
    let count = if repo == "ready" || repo == "lowdepth" {
        20
    } else {
        0
    };
    let items = (0..count)
        .map(|index| {
            format!(
                r#"{{"starred_at":"{}","user":{{"login":"star-{index}"}}}}"#,
                Utc::now().to_rfc3339()
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
}

fn forks_body(repo: &str) -> String {
    let count = if repo == "ready" || repo == "lowdepth" {
        12
    } else {
        0
    };
    let items = (0..count)
        .map(|index| {
            format!(
                r#"{{"created_at":"{}","owner":{{"login":"fork-{index}"}}}}"#,
                Utc::now().to_rfc3339()
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
}

fn issue_title(repo: &str) -> &'static str {
    if repo == "lowdepth" {
        "Add JSON content"
    } else {
        "Fix Rust CLI parser regression"
    }
}

fn issue_body_text(repo: &str) -> String {
    if repo == "lowdepth" {
        "No code required. This can be done from your browser in under 60 seconds. Add JSON content.".to_string()
    } else {
        actionable_body()
    }
}

fn actionable_body() -> String {
    "Steps to reproduce: run the Rust CLI parser with a repeated flag. The parser currently panics in src/lib.rs. Expected behavior is a graceful error. Actual behavior is a panic. Suggested fix: guard the empty parse branch and verify with cargo test.".to_string()
}

fn json_string_literal(value: &str) -> String {
    serde_json::to_string(value)
        .unwrap()
        .trim_matches('"')
        .to_string()
}

fn write_response(stream: &mut std::net::TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Unknown",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).unwrap();
}

fn create_remote_repo(root: &Path) -> PathBuf {
    let source = root.join("source");
    let remote = root.join("remote.git");
    fs::create_dir_all(source.join("src")).unwrap();
    fs::write(
        source.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        source.join("src/lib.rs"),
        "pub fn parse() -> bool { true }\n",
    )
    .unwrap();

    run_git(root, &["init", "--bare", remote.to_str().unwrap()]);
    run_git(&source, &["init"]);
    run_git(&source, &["checkout", "-b", "main"]);
    run_git(&source, &["add", "."]);
    run_git(
        &source,
        &[
            "-c",
            "user.name=Issue Finder",
            "-c",
            "user.email=issue-finder@example.invalid",
            "commit",
            "-m",
            "initial",
        ],
    );
    run_git(
        &source,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    run_git(&source, &["push", "-u", "origin", "main"]);
    run_git(
        root,
        &[
            "--git-dir",
            remote.to_str().unwrap(),
            "symbolic-ref",
            "HEAD",
            "refs/heads/main",
        ],
    );
    remote
}

fn clone_into_workspace(remote: &Path, paths: &IssueFinderPaths, repo_full_name: &str) {
    let workspace = paths.workspace_path_for(repo_full_name);
    fs::create_dir_all(workspace.parent().unwrap()).unwrap();
    run_git(
        workspace.parent().unwrap(),
        &[
            "clone",
            remote.to_str().unwrap(),
            workspace.file_name().unwrap().to_str().unwrap(),
        ],
    );
}

fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}
