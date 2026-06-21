use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::mpsc;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use issue_finder::config::Config;
use issue_finder::dispatch::{
    ApprovalStatus, DispatchOutcomeFailureClass, DispatchOutcomeKind, DispatchRunStatus,
    DispatchRuntime, DispatchValidationOutcome, GitHubCommentWriter, GitHubInteractionDecisionKind,
    GitHubInteractionStatus, GitHubInteractionType, IssueTaskPackage, IssueTaskPackageIssue,
    IssueTaskStatus, NewArtifact, NewDispatchRun, NewDispatchRunOutcome, NewIssueTask,
    PostedGitHubComment, ReqwestGitHubCommentWriter,
};
use issue_finder::paths::IssueFinderPaths;
use serde_json::json;
use tempfile::tempdir;

#[path = "support/env_lock.rs"]
mod env_lock;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn default_tracking_comment_records_no_comment_decision() {
    let dir = tempdir().unwrap();
    let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
    let task = imported_issue_task(&runtime);

    let result = runtime
        .draft_github_tracking_comment("owner/repo#123", None)
        .unwrap();
    assert_eq!(result.issue_task.id, task.id);
    assert_eq!(
        result.decision.decision_kind,
        GitHubInteractionDecisionKind::NoComment
    );
    assert_eq!(result.decision.reason_code, "tracking_default_silence");
    assert!(result.draft.is_none());
    assert!(runtime
        .store()
        .list_github_interactions_for_issue_task(&task.id)
        .unwrap()
        .is_empty());
    let decisions = runtime
        .store()
        .list_github_interaction_decisions_for_issue_task(&task.id)
        .unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].decision_kind,
        GitHubInteractionDecisionKind::NoComment
    );
}

#[test]
fn explicit_tracking_comment_requires_approval_before_posting() {
    let dir = tempdir().unwrap();
    let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
    let task = imported_issue_task(&runtime);

    let result = runtime
        .draft_github_tracking_comment(
            "owner/repo#123",
            Some(
                "I have a concrete fix plan and will post a PR link after validation.".to_string(),
            ),
        )
        .unwrap();
    assert_eq!(
        result.decision.decision_kind,
        GitHubInteractionDecisionKind::Tracking
    );
    assert_eq!(result.decision.reason_code, "explicit_tracking_body");
    let draft = result.draft.as_ref().unwrap();
    assert_eq!(draft.issue_task.id, task.id);
    assert_eq!(
        draft.interaction.interaction_type,
        GitHubInteractionType::TrackingComment
    );
    assert_eq!(draft.interaction.status, GitHubInteractionStatus::Draft);
    assert_eq!(draft.approval_request.status, ApprovalStatus::Pending);
    assert_eq!(draft.approval_request.run_id, None);
    assert_eq!(
        result.decision.github_interaction_id.as_deref(),
        Some(draft.interaction.id.as_str())
    );

    let body = String::from_utf8(
        runtime
            .store()
            .read_artifact_bytes(&draft.body_artifact.id)
            .unwrap(),
    )
    .unwrap();
    assert!(body.starts_with("<!-- issue-finder:tracking_comment -->"));
    assert!(body.contains("concrete fix plan"));

    let mut writer = FakeGitHubWriter::default();
    let error = runtime
        .post_github_interaction_with_writer(&mut writer, &draft.interaction.id)
        .unwrap_err();
    assert!(error.to_string().contains("not approved"));
    assert!(writer.calls.is_empty());

    let approved = runtime
        .approve_github_interaction(&draft.interaction.id)
        .unwrap();
    assert_eq!(
        approved.interaction.status,
        GitHubInteractionStatus::Approved
    );
    assert_eq!(approved.approval_request.status, ApprovalStatus::Approved);

    let posted = runtime
        .post_github_interaction_with_writer(&mut writer, &draft.interaction.id)
        .unwrap();
    assert_eq!(posted.interaction.status, GitHubInteractionStatus::Posted);
    assert_eq!(
        posted.interaction.github_comment_id.as_deref(),
        Some("comment-1")
    );
    assert_eq!(writer.calls.len(), 1);
    assert_eq!(writer.calls[0].repo_full_name, "owner/repo");
    assert_eq!(writer.calls[0].issue_number, 123);
    assert_eq!(writer.calls[0].body, body);

    let interactions = runtime.list_github_interactions("owner/repo#123").unwrap();
    assert_eq!(interactions.len(), 1);
    assert_eq!(interactions[0].status, GitHubInteractionStatus::Posted);
    assert_eq!(
        runtime.store().get_issue_task(&task.id).unwrap().status,
        IssueTaskStatus::UserApproved
    );
}

#[test]
fn duplicate_tracking_comment_records_no_comment_decision() {
    let dir = tempdir().unwrap();
    let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
    let task = imported_issue_task(&runtime);

    let first = runtime
        .draft_github_tracking_comment("owner/repo#123", Some("custom body".to_string()))
        .unwrap();
    assert!(first.draft.is_some());

    let duplicate = runtime
        .draft_github_tracking_comment("owner/repo#123", Some("another body".to_string()))
        .unwrap();
    assert_eq!(
        duplicate.decision.decision_kind,
        GitHubInteractionDecisionKind::NoComment
    );
    assert_eq!(
        duplicate.decision.reason_code,
        "duplicate_tracking_interaction"
    );
    assert!(duplicate.draft.is_none());
    assert_eq!(
        runtime
            .store()
            .list_github_interactions_for_issue_task(&task.id)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn rejected_github_comment_cannot_be_posted() {
    let dir = tempdir().unwrap();
    let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
    imported_issue_task(&runtime);

    let result = runtime
        .draft_github_tracking_comment("owner/repo#123", Some("custom body".to_string()))
        .unwrap();
    let draft = result.draft.as_ref().unwrap();
    let rejected = runtime
        .reject_github_interaction(&draft.interaction.id)
        .unwrap();
    assert_eq!(
        rejected.interaction.status,
        GitHubInteractionStatus::Rejected
    );
    assert_eq!(rejected.approval_request.status, ApprovalStatus::Rejected);

    let mut writer = FakeGitHubWriter::default();
    let error = runtime
        .post_github_interaction_with_writer(&mut writer, &draft.interaction.id)
        .unwrap_err();
    assert!(error.to_string().contains("not approved"));
    assert!(writer.calls.is_empty());
}

#[test]
fn failed_github_comment_post_can_be_retried() {
    let dir = tempdir().unwrap();
    let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
    imported_issue_task(&runtime);

    let result = runtime
        .draft_github_tracking_comment("owner/repo#123", Some("custom tracking body".to_string()))
        .unwrap();
    let draft = result.draft.as_ref().unwrap();
    runtime
        .approve_github_interaction(&draft.interaction.id)
        .unwrap();

    let mut writer = FakeGitHubWriter {
        failures_remaining: 1,
        ..FakeGitHubWriter::default()
    };
    let error = runtime
        .post_github_interaction_with_writer(&mut writer, &draft.interaction.id)
        .unwrap_err();
    assert!(error.to_string().contains("transient GitHub failure"));
    let failed = runtime
        .store()
        .get_github_interaction(&draft.interaction.id)
        .unwrap();
    assert_eq!(failed.status, GitHubInteractionStatus::Failed);
    assert_eq!(writer.calls.len(), 1);

    let posted = runtime
        .retry_github_interaction_with_writer(&mut writer, &draft.interaction.id)
        .unwrap();
    assert_eq!(posted.interaction.status, GitHubInteractionStatus::Posted);
    assert_eq!(
        posted.interaction.github_comment_id.as_deref(),
        Some("comment-2")
    );
    assert_eq!(writer.calls.len(), 2);
}

#[test]
fn final_github_comment_is_derived_from_fix_result_artifact() {
    let dir = tempdir().unwrap();
    let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
    let task = imported_issue_task(&runtime);
    let run = runtime
        .store()
        .create_dispatch_run(NewDispatchRun {
            issue_task_id: task.id.clone(),
            agent_id: "codex".to_string(),
            status: DispatchRunStatus::Completed,
            requested_by: "test".to_string(),
            approval_state: ApprovalStatus::Approved,
            selected_session_link_id: None,
        })
        .unwrap();
    let fix_result = runtime
        .store()
        .write_artifact(
            NewArtifact {
                issue_task_id: Some(task.id.clone()),
                run_id: Some(run.id.clone()),
                kind: "fix_result".to_string(),
                content_type: "application/json".to_string(),
                metadata_json: json!({ "source": "test" }),
            },
            br#"{"suggestedGitHubReply":"Fixed locally; validation passed."}"#,
        )
        .unwrap();
    runtime
        .store()
        .set_dispatch_run_result_artifact(&run.id, &fix_result.id)
        .unwrap();
    runtime
        .store()
        .record_dispatch_run_outcome(NewDispatchRunOutcome {
            run_id: run.id.clone(),
            idempotency_key: "final-success".to_string(),
            outcome_kind: DispatchOutcomeKind::FixReady,
            failure_class: None,
            failure_detail: None,
            task_class: None,
            validation_outcome: Some(DispatchValidationOutcome::Passed),
            result_artifact_id: Some(fix_result.id.clone()),
            metadata_json: json!({ "source": "test" }),
        })
        .unwrap();

    let result = runtime.draft_github_final_comment(&run.id, None).unwrap();
    assert_eq!(
        result.decision.decision_kind,
        GitHubInteractionDecisionKind::Final
    );
    assert_eq!(result.decision.reason_code, "final_suggested_reply");
    let draft = result.draft.as_ref().unwrap();
    assert_eq!(
        draft.interaction.interaction_type,
        GitHubInteractionType::FinalComment
    );
    assert_eq!(draft.body_artifact.run_id.as_deref(), Some(run.id.as_str()));
    let body = String::from_utf8(
        runtime
            .store()
            .read_artifact_bytes(&draft.body_artifact.id)
            .unwrap(),
    )
    .unwrap();
    assert!(body.starts_with("<!-- issue-finder:final_comment -->"));
    assert!(body.contains("Fixed locally; validation passed."));

    let mut writer = FakeGitHubWriter::default();
    runtime
        .approve_github_interaction(&draft.interaction.id)
        .unwrap();
    let posted = runtime
        .post_github_interaction_with_writer(&mut writer, &draft.interaction.id)
        .unwrap();
    assert_eq!(posted.interaction.status, GitHubInteractionStatus::Posted);
    assert_eq!(
        runtime.store().get_issue_task(&task.id).unwrap().status,
        IssueTaskStatus::GithubPosted
    );
}

#[test]
fn final_github_comment_requires_explicit_suggested_reply() {
    let dir = tempdir().unwrap();
    let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
    let task = imported_issue_task(&runtime);
    let run = runtime
        .store()
        .create_dispatch_run(NewDispatchRun {
            issue_task_id: task.id.clone(),
            agent_id: "codex".to_string(),
            status: DispatchRunStatus::Completed,
            requested_by: "test".to_string(),
            approval_state: ApprovalStatus::Approved,
            selected_session_link_id: None,
        })
        .unwrap();
    let fix_result = runtime
        .store()
        .write_artifact(
            NewArtifact {
                issue_task_id: Some(task.id.clone()),
                run_id: Some(run.id.clone()),
                kind: "fix_result".to_string(),
                content_type: "application/json".to_string(),
                metadata_json: json!({ "source": "test" }),
            },
            br#"{"summary":"Fixed locally; validation passed."}"#,
        )
        .unwrap();
    runtime
        .store()
        .record_dispatch_run_outcome(NewDispatchRunOutcome {
            run_id: run.id.clone(),
            idempotency_key: "final-missing-reply".to_string(),
            outcome_kind: DispatchOutcomeKind::FixReady,
            failure_class: None,
            failure_detail: None,
            task_class: None,
            validation_outcome: Some(DispatchValidationOutcome::Passed),
            result_artifact_id: Some(fix_result.id.clone()),
            metadata_json: json!({ "source": "test" }),
        })
        .unwrap();

    let result = runtime.draft_github_final_comment(&run.id, None).unwrap();
    assert_eq!(
        result.decision.decision_kind,
        GitHubInteractionDecisionKind::NoReply
    );
    assert_eq!(
        result.decision.reason_code,
        "missing_suggested_github_reply"
    );
    assert!(result.draft.is_none());
    assert!(runtime
        .store()
        .list_github_interactions_for_issue_task(&task.id)
        .unwrap()
        .is_empty());
}

#[test]
fn context_gap_with_suggested_reply_drafts_clarification_comment() {
    let dir = tempdir().unwrap();
    let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
    let task = imported_issue_task(&runtime);
    let run = runtime
        .store()
        .create_dispatch_run(NewDispatchRun {
            issue_task_id: task.id.clone(),
            agent_id: "codex".to_string(),
            status: DispatchRunStatus::NeedsUser,
            requested_by: "test".to_string(),
            approval_state: ApprovalStatus::Approved,
            selected_session_link_id: None,
        })
        .unwrap();
    let fix_result = runtime
        .store()
        .write_artifact(
            NewArtifact {
                issue_task_id: Some(task.id.clone()),
                run_id: Some(run.id.clone()),
                kind: "fix_result".to_string(),
                content_type: "application/json".to_string(),
                metadata_json: json!({ "source": "test" }),
            },
            br#"{"status":"needs_user","suggestedGitHubReply":"Could a maintainer confirm the expected behavior for empty config files?"}"#,
        )
        .unwrap();
    runtime
        .store()
        .record_dispatch_run_outcome(NewDispatchRunOutcome {
            run_id: run.id.clone(),
            idempotency_key: "clarification-needed".to_string(),
            outcome_kind: DispatchOutcomeKind::NeedsUser,
            failure_class: Some(DispatchOutcomeFailureClass::ContextInsufficient),
            failure_detail: Some("expected behavior is unclear".to_string()),
            task_class: None,
            validation_outcome: None,
            result_artifact_id: Some(fix_result.id.clone()),
            metadata_json: json!({ "source": "test" }),
        })
        .unwrap();

    let result = runtime.draft_github_final_comment(&run.id, None).unwrap();
    assert_eq!(
        result.decision.decision_kind,
        GitHubInteractionDecisionKind::Clarification
    );
    assert_eq!(result.decision.reason_code, "context_gap_suggested_reply");
    let draft = result.draft.as_ref().unwrap();
    assert_eq!(
        draft.interaction.interaction_type,
        GitHubInteractionType::ClarificationComment
    );
    let body = String::from_utf8(
        runtime
            .store()
            .read_artifact_bytes(&draft.body_artifact.id)
            .unwrap(),
    )
    .unwrap();
    assert!(body.starts_with("<!-- issue-finder:clarification_comment -->"));
    assert!(body.contains("confirm the expected behavior"));
}

#[test]
fn reqwest_github_comment_writer_posts_to_configured_mock_api() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _file_env_lock = env_lock::EnvLock::acquire();
    let mock = start_mock_comment_server();
    let _api_guard = EnvVarGuard::set("ISSUE_FINDER_GITHUB_API_BASE", mock.base_url.clone());
    let _token_guard = EnvVarGuard::remove("GITHUB_TOKEN");
    let mut config = Config::default();
    config.github.token = "test-token".to_string();
    let mut writer = ReqwestGitHubCommentWriter::from_config(&config).unwrap();

    let posted = writer
        .post_issue_comment("owner/repo", 123, "tracking body")
        .unwrap();
    let request = mock.join();

    assert_eq!(posted.id, "98765");
    assert_eq!(
        posted.url,
        "https://github.com/owner/repo/issues/123#issuecomment-98765"
    );
    assert!(request.starts_with("POST /repos/owner/repo/issues/123/comments HTTP/1.1"));
    assert!(request.contains("authorization: Bearer test-token"));
    assert!(request.contains(r#""body":"tracking body""#));
}

#[derive(Debug, Default)]
struct FakeGitHubWriter {
    calls: Vec<FakeGitHubCall>,
    failures_remaining: usize,
}

struct MockCommentServer {
    base_url: String,
    receiver: mpsc::Receiver<String>,
    handle: thread::JoinHandle<()>,
}

impl MockCommentServer {
    fn join(self) -> String {
        let request = self
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("mock GitHub comment request");
        self.handle.join().unwrap();
        request
    }
}

fn start_mock_comment_server() -> MockCommentServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_http_request(&mut stream);
        let body = r#"{"id":98765,"html_url":"https://github.com/owner/repo/issues/123#issuecomment-98765"}"#;
        let response = format!(
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
        sender.send(request).unwrap();
    });
    MockCommentServer {
        base_url,
        receiver,
        handle,
    }
}

fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut buffer = Vec::new();
    loop {
        let mut chunk = [0_u8; 1024];
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(bytes_read) => {
                buffer.extend_from_slice(&chunk[..bytes_read]);
                if request_is_complete(&buffer) {
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(error) => panic!("mock GitHub request read failed: {error}"),
        }
    }
    String::from_utf8(buffer).unwrap()
}

fn request_is_complete(buffer: &[u8]) -> bool {
    let header_end = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4);
    let Some(header_end) = header_end else {
        return false;
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    buffer.len() >= header_end + content_length
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: String) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::remove_var(key);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            std::env::set_var(self.key, previous);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

#[derive(Debug)]
struct FakeGitHubCall {
    repo_full_name: String,
    issue_number: u64,
    body: String,
}

impl GitHubCommentWriter for FakeGitHubWriter {
    fn post_issue_comment(
        &mut self,
        repo_full_name: &str,
        issue_number: u64,
        body: &str,
    ) -> Result<PostedGitHubComment> {
        self.calls.push(FakeGitHubCall {
            repo_full_name: repo_full_name.to_string(),
            issue_number,
            body: body.to_string(),
        });
        if self.failures_remaining > 0 {
            self.failures_remaining -= 1;
            anyhow::bail!("transient GitHub failure");
        }
        Ok(PostedGitHubComment {
            id: format!("comment-{}", self.calls.len()),
            url: format!(
                "https://github.com/{repo_full_name}/issues/{issue_number}#issuecomment-1"
            ),
        })
    }
}

fn imported_issue_task(runtime: &DispatchRuntime) -> issue_finder::dispatch::IssueTask {
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
    let home = root.to_path_buf();
    IssueFinderPaths {
        config: home.join("config.toml"),
        cache_dir: home.join("cache"),
        workspaces_dir: home.join("workspaces"),
        inbox_dir: home.join("inbox"),
        reports_dir: home.join("reports"),
        home,
    }
}
