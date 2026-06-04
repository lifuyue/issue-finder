use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use patchbay_cli::config::Config;
use patchbay_cli::github::GitHubIssue;
use patchbay_cli::handoff::WrittenHandoff;
use patchbay_cli::inbox::{load_index, upsert_ready};
use patchbay_cli::paths::PatchbayPaths;
use patchbay_cli::tool_runtime::{list_tool_specs, PatchbayToolInvocation, PatchbayToolRuntime};
use patchbay_cli::workspace::git_available;
use tempfile::tempdir;

#[test]
fn tools_list_outputs_stable_patchbay_specs() {
    let specs = serde_json::to_value(list_tool_specs()).unwrap();
    assert_eq!(specs["kind"], "patchbay_tool_specs");
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
            "patchbay.scout",
            "patchbay.assess",
            "patchbay.prepare",
            "patchbay.read_context"
        ]
    );
    assert!(tools.iter().all(|tool| tool["inputSchema"].is_object()));
}

#[tokio::test]
async fn tool_runtime_uses_mocked_github_and_applies_prepare_gate() {
    if !git_available() {
        return;
    }

    let (base_url, handle) = start_mock_tool_github();
    std::env::set_var("PATCHBAY_GITHUB_API_BASE", &base_url);
    let _env_guard = EnvGuard;

    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let runtime = PatchbayToolRuntime::new(paths.clone(), Config::default());

    let scout = runtime
        .execute(invocation(
            "patchbay.scout",
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
    assert_eq!(scout.structured_content["filteredCount"], 1);
    assert!(scout_candidates[0]["gates"]["repoInfluence"]["status"].is_string());

    let assess = runtime
        .execute(invocation(
            "patchbay.assess",
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

    let blocked = runtime
        .execute(invocation(
            "patchbay.prepare",
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
            "patchbay.prepare",
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
            "patchbay.prepare",
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

    let handoff_id = prepared.structured_content["handoff"]["id"]
        .as_str()
        .unwrap();
    let context = runtime
        .execute(invocation(
            "patchbay.read_context",
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

    handle.join().unwrap();
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
    let runtime = PatchbayToolRuntime::new(paths.clone(), Config::default());

    let truncated = runtime
        .execute(invocation(
            "patchbay.read_context",
            r#"{"handoffId":"handoff-1","section":"entry","maxBytes":3}"#,
            "truncate_call",
        ))
        .await;
    assert!(truncated.success, "{truncated:?}");
    assert_eq!(truncated.structured_content["truncated"], true);
    assert_eq!(truncated.structured_content["content"], "abc");

    let traversal = runtime
        .execute(invocation(
            "patchbay.read_context",
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
                "patchbay.read_context",
                r#"{"handoffId":"handoff-1","section":"repo"}"#,
                "escape_call",
            ))
            .await;
        assert!(!escaped.success);
    }
}

struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        std::env::remove_var("PATCHBAY_GITHUB_API_BASE");
    }
}

fn invocation(tool: &str, arguments: &str, call_id: &str) -> PatchbayToolInvocation {
    PatchbayToolInvocation::from_json_arguments(
        tool.to_string(),
        arguments,
        Some(call_id.to_string()),
        None,
    )
    .unwrap()
}

fn test_paths(root: &Path) -> PatchbayPaths {
    PatchbayPaths {
        home: root.join("patchbay-home"),
        config: root.join("patchbay-home/config.toml"),
        cache_dir: root.join("patchbay-home/cache"),
        workspaces_dir: root.join("patchbay-home/workspaces"),
        inbox_dir: root.join("patchbay-home/inbox"),
        reports_dir: root.join("patchbay-home/reports"),
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

fn start_mock_tool_github() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let base_url_for_thread = base_url.clone();
    let search_count = Arc::new(AtomicUsize::new(0));
    let search_count_for_thread = Arc::clone(&search_count);

    let handle = thread::spawn(move || {
        let started = Instant::now();
        let mut last_request_at = Instant::now();
        let mut served = 0usize;
        while started.elapsed() < Duration::from_secs(10) {
            if served > 0 && last_request_at.elapsed() > Duration::from_millis(500) {
                break;
            }

            match listener.accept() {
                Ok((mut stream, _)) => {
                    last_request_at = Instant::now();
                    let mut buffer = [0u8; 4096];
                    let bytes_read = stream.read(&mut buffer).unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
                    let body =
                        response_body(&request, &base_url_for_thread, &search_count_for_thread);
                    write_response(&mut stream, &body);
                    served += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

    (base_url, handle)
}

fn response_body(request: &str, base_url: &str, search_count: &AtomicUsize) -> String {
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
            "user.name=Patchbay",
            "-c",
            "user.email=patchbay@example.invalid",
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

fn clone_into_workspace(remote: &Path, paths: &PatchbayPaths, repo_full_name: &str) {
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
