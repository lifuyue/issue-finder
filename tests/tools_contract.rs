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
use issue_finder::github::GitHubIssue;
use issue_finder::handoff::WrittenHandoff;
use issue_finder::inbox::{load_index, upsert_ready};
use issue_finder::paths::IssueFinderPaths;
use issue_finder::prepare_gate::{
    default_prepare_allowed, prepare_gate_decision, PrepareGateDecision,
};
use issue_finder::recommendation::{
    load_events, RecommendationEventSource, RecommendationEventType,
};
use issue_finder::tool_runtime::{
    list_tool_specs, IssueFinderToolInvocation, IssueFinderToolRuntime,
};
use issue_finder::value_scoring::{
    is_daily_prepare_candidate, RecommendationCategory, ValueAssessment,
};
use issue_finder::workflow::{self, PrepareOutcome};
use issue_finder::workspace::git_available;
use tempfile::tempdir;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

#[test]
fn tools_list_outputs_stable_issue_finder_specs() {
    let specs = serde_json::to_value(list_tool_specs()).unwrap();
    assert_eq!(specs["kind"], "issue_finder_tool_specs");
    assert_eq!(specs["version"], 1);
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
            "issue-finder.read_context"
        ]
    );
    assert!(tools.iter().all(|tool| tool["inputSchema"].is_object()));
    let scout = tools
        .iter()
        .find(|tool| tool["name"] == "scout")
        .expect("scout tool spec");
    assert!(scout["inputSchema"]["properties"]["repo"].is_object());
    let status = tools
        .iter()
        .find(|tool| tool["name"] == "status")
        .expect("status tool spec");
    assert!(status["inputSchema"]["properties"]["checkAuth"].is_object());
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
                    let body =
                        response_body(&request, &base_url_for_thread, &search_count_for_thread);
                    write_response(&mut stream, &body);
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

fn response_body(request: &str, base_url: &str, search_count: &AtomicUsize) -> String {
    if request.starts_with("GET /user") {
        return r#"{"login":"tool-user"}"#.to_string();
    }

    if request.starts_with("GET /search/issues") {
        let count = search_count.fetch_add(1, Ordering::SeqCst);
        return if count == 0 {
            search_body(base_url)
        } else {
            r#"{"items":[]}"#.to_string()
        };
    }

    for repo in ["niche", "ready", "lowdepth"] {
        let prefix = format!("/repos/owner/{repo}");
        if request.contains(&format!("{prefix}/issues/1/comments")) {
            return comments_body();
        }
        if request.contains(&format!("{prefix}/issues/1/timeline")) {
            return "[]".to_string();
        }
        if request.contains(&format!("{prefix}/stargazers")) {
            return stargazers_body(repo);
        }
        if request.contains(&format!("{prefix}/forks")) {
            return forks_body(repo);
        }
        if request.contains(&format!("{prefix}/issues/1")) {
            return issue_body(repo);
        }
        if request.contains(&prefix) {
            return repo_body(repo);
        }
    }

    r#"{"message":"not found"}"#.to_string()
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

fn write_response(stream: &mut std::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
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
