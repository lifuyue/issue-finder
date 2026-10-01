use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use tempfile::tempdir;

#[test]
fn session_cli_exposes_only_current_agent_tools_and_reports_errors_as_json() {
    let temp = tempdir().unwrap();
    for args in [
        vec!["tools", "list"],
        vec!["tools", "--profile", "session", "list"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
            .env("ISSUE_FINDER_HOME", temp.path())
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        let catalog: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(catalog["sessionContractVersion"], 2);
        let names = catalog["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, ["scout", "assess"]);
    }
    let (ok, output) = call(
        temp.path(),
        "http://127.0.0.1:1",
        "issue-finder.scout",
        json!({"limit":0}),
    );
    assert!(!ok);
    assert_eq!(output["status"], "invalid_arguments");
    for removed in [
        "status",
        "prepare",
        "task_status",
        "finish",
        "feedback",
        "dispatch",
    ] {
        let (ok, output) = call(
            temp.path(),
            "http://127.0.0.1:1",
            &format!("issue-finder.{removed}"),
            json!({}),
        );
        assert!(!ok);
        assert_eq!(output["status"], "forbidden_tool");
    }
    assert!(!temp.path().join("sessions").exists());
}

#[test]
fn session_assess_reads_paged_discussion_and_leaves_execution_to_codex() {
    let server = Server::start();
    let temp = tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let state = root.join("state");
    let (ok, assessment) = call(
        &state,
        &server.url,
        "issue-finder.assess",
        json!({"issue":"owner/repo#1","commentsPerPage":1}),
    );
    assert!(ok, "{assessment}");
    let issue = &assessment["structured_content"]["issue"];
    assert_eq!(
        issue["comments"][0]["body"],
        "complete comment ".repeat(100)
    );
    assert_eq!(issue["nextCommentsPage"], 2);
    assert_eq!(issue["commentsTruncated"], true);
    assert!(!state
        .join("inbox")
        .read_dir()
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false));
    let (ok, page2) = call(
        &state,
        &server.url,
        "issue-finder.assess",
        json!({"issue":"owner/repo#1","commentsPerPage":1,"commentsPage":2}),
    );
    assert!(ok);
    assert_eq!(page2["structured_content"]["issue"]["comments"][0]["id"], 2);
    assert!(page2["structured_content"]["issue"]["nextCommentsPage"].is_null());

    let data = &assessment["structured_content"];
    assert_eq!(data["sessionContractVersion"], 2);
    assert!(data["assessment"]["scores"].is_object());
    assert!(data.get("prepareGate").is_none());
    // A low-depth documentation recommendation is information, never repair authorization.
    assert!(data["assessment"]["gates"]["lowDepth"]["reasons"].is_array());
    assert!(!state.join("sessions").exists());
    assert!(!state.join("dispatch").exists());
    assert!(fs::read_dir(state.join("workspaces"))
        .unwrap()
        .next()
        .is_none());
    assert!(fs::read_dir(state.join("reports"))
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn session_assess_preserves_discussion_for_unavailable_issues_and_pull_requests() {
    let server = Server::start();
    let temp = tempdir().unwrap();
    for number in [2, 4] {
        let (ok, output) = call(
            temp.path(),
            &server.url,
            "issue-finder.assess",
            json!({"issue":format!("owner/repo#{number}")}),
        );
        assert!(ok, "{output}");
        assert_eq!(output["status"], "issue_unavailable");
        assert_eq!(output["structured_content"]["issue"]["number"], number);
        assert!(output["structured_content"]["issue"]["comments"].is_array());
    }
    for (number, warning) in [(3, "assignee"), (6, "locked")] {
        let (ok, assessed) = call(
            temp.path(),
            &server.url,
            "issue-finder.assess",
            json!({"issue":format!("owner/repo#{number}")}),
        );
        assert!(ok, "{assessed}");
        assert_ne!(assessed["status"], "issue_unavailable");
        assert!(assessed["structured_content"]["assessment"].is_object());
        assert!(assessed["structured_content"]["issueWarnings"][0]
            .as_str()
            .unwrap()
            .contains(warning));
    }
    assert!(!temp.path().join("sessions").exists());
}

#[test]
fn session_scout_accepts_per_call_preferences_and_exposes_budget_exhaustion() {
    let server = Server::start();
    let temp = tempdir().unwrap();
    let (ok, output) = call(
        temp.path(),
        &server.url,
        "issue-finder.scout",
        json!({
            "repo":"owner/repo","search":{"query":"parser","maxPages":1,"apiBudget":2},
            "profile":{"techStack":["Python"],"keywords":["parser"]},"recordExposure":false
        }),
    );
    assert!(ok, "{output}");
    assert_eq!(output["status"], "partial");
    let data = &output["structured_content"];
    assert_eq!(data["apiBudget"]["totalNetworkRequests"], 2);
    assert_eq!(data["profile"]["techStack"], json!(["Python"]));
    assert!(data["diagnostics"]["search"]["effectiveQuery"]
        .as_str()
        .unwrap()
        .contains("repo:owner/repo"));
    for candidate in data["candidates"].as_array().unwrap() {
        assert!(candidate.get("prepareGate").is_none());
    }
    assert!(!temp.path().join("config.toml").exists());
}

#[test]
fn session_business_calls_report_configuration_and_authentication_errors_directly() {
    use issue_finder::config::Config;
    use issue_finder::paths::IssueFinderPaths;
    use issue_finder::tool_runtime::{IssueFinderToolInvocation, IssueFinderToolRuntime};

    let temp = tempdir().unwrap();
    fs::write(temp.path().join("config.toml"), "invalid = [").unwrap();
    for tool in ["scout", "assess"] {
        let (_, output) = call(
            temp.path(),
            "http://127.0.0.1:1",
            &format!("issue-finder.{tool}"),
            if tool == "scout" {
                json!({})
            } else {
                json!({"issue":"owner/repo#1"})
            },
        );
        assert_eq!(output["success"], false);
        let serialized = output.to_string();
        assert!(serialized.contains("config.toml"));
        assert!(!serialized.contains("call issue-finder.status"));
        assert!(!serialized.contains("fixture-token"));
    }

    let isolated = tempdir().unwrap();
    let missing_auth = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .env("ISSUE_FINDER_HOME", isolated.path())
        .env("PATH", "")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .args([
            "tools",
            "--profile",
            "session",
            "call",
            "issue-finder.assess",
            "--arguments",
            r#"{"issue":"owner/repo#1"}"#,
        ])
        .output()
        .unwrap();
    let output: Value = serde_json::from_slice(&missing_auth.stdout).unwrap();
    assert!(!missing_auth.status.success());
    assert_eq!(output["success"], false);
    assert!(output
        .to_string()
        .contains("GitHub authentication is unavailable"));
    assert!(!output.to_string().contains("issue-finder.status"));

    // The runtime path must also return the supplied configuration error directly.
    let paths = IssueFinderPaths {
        home: isolated.path().to_path_buf(),
        config: isolated.path().join("config.toml"),
        cache_dir: isolated.path().join("cache"),
        workspaces_dir: isolated.path().join("workspaces"),
        inbox_dir: isolated.path().join("inbox"),
        reports_dir: isolated.path().join("reports"),
    };
    let runtime = IssueFinderToolRuntime::session(paths, Config::default())
        .with_config_load_error("invalid configuration fixture".into());
    let output = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(runtime.execute(IssueFinderToolInvocation {
            call_id: "config-error".into(),
            turn_id: None,
            tool_name: "issue-finder.scout".into(),
            arguments: json!({}),
        }));
    assert!(!output.success);
    assert!(output
        .structured_content
        .to_string()
        .contains("invalid configuration fixture"));
}

#[test]
fn session_assess_reports_runtime_github_auth_and_network_failures() {
    let server = Server::start();
    let temp = tempdir().unwrap();
    let (ok, unauthorized) = call(
        temp.path(),
        &server.url,
        "issue-finder.assess",
        json!({"issue":"owner/repo#5"}),
    );
    assert!(!ok);
    assert_eq!(unauthorized["status"], "system_error");
    assert!(unauthorized.to_string().contains("401"));
    assert!(!unauthorized.to_string().contains("fixture-token"));
    let (ok, disconnected) = call(
        temp.path(),
        "http://127.0.0.1:1",
        "issue-finder.assess",
        json!({"issue":"owner/repo#1"}),
    );
    assert!(!ok);
    assert_eq!(disconnected["status"], "system_error");
    assert!(disconnected.to_string().contains("127.0.0.1:1"));
}

#[test]
fn gh_token_only_reaches_all_assessment_clients_without_gh_or_secret_persistence() {
    let token = "gh-only-business-fixture";
    let server = Server::start_with_token(token);
    let temp = tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .env("ISSUE_FINDER_HOME", temp.path())
        .env("ISSUE_FINDER_GITHUB_API_BASE", &server.url)
        .env("ISSUE_FINDER_GITHUB_API_BUDGET_TOTAL", "1200")
        .env("PATH", "")
        .env("GH_TOKEN", token)
        .env_remove("GITHUB_TOKEN")
        .args([
            "tools",
            "call",
            "issue-finder.assess",
            "--arguments",
            r#"{"issue":"owner/repo#1"}"#,
        ])
        .output()
        .unwrap();
    let assessment: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{assessment}");
    assert!(assessment["structured_content"]["assessment"].is_object());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(token));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(token));
    assert!(!temp.path().join("config.toml").exists());
}

fn call(home: &Path, url: &str, tool: &str, args: Value) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .env("ISSUE_FINDER_HOME", home)
        .env("ISSUE_FINDER_CODEX_BIN", home.join("not-installed-codex"))
        .env("ISSUE_FINDER_GITHUB_API_BASE", url)
        .env_remove("GITHUB_TOKEN")
        .env("GH_TOKEN", "fixture-token")
        .env("ISSUE_FINDER_GITHUB_API_BUDGET_TOTAL", "1200")
        .args([
            "tools",
            "--profile",
            "session",
            "call",
            tool,
            "--arguments",
            &args.to_string(),
        ])
        .output()
        .unwrap();
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid output: {error}; stdout={}, stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), value)
}

struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn start() -> Self {
        Self::start_with_token("fixture-token")
    }

    fn start_with_token(expected_token: &str) -> Self {
        let expected_token = expected_token.to_owned();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let base = url.clone();
        let thread = thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => respond(stream, &base, &expected_token),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        });
        Self {
            url,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let joined = self.thread.take().unwrap().join();
        if !thread::panicking() {
            assert!(joined.is_ok(), "mock server failed");
        }
    }
}

fn respond(mut stream: TcpStream, base: &str, expected_token: &str) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut buffer = [0; 16_384];
    let length = match stream.read(&mut buffer) {
        Ok(length) if length > 0 => length,
        _ => return,
    };
    let request = String::from_utf8_lossy(&buffer[..length]);
    assert!(
        request
            .lines()
            .any(|line| line
                .eq_ignore_ascii_case(&format!("authorization: Bearer {expected_token}"))),
        "mock request did not use the selected fixture credential"
    );
    let target = request.split_whitespace().nth(1).unwrap();
    let path = target.split('?').next().unwrap();
    if path == "/repos/owner/repo/issues/5" {
        let body = r#"{"message":"Bad credentials"}"#;
        write!(stream, "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        return;
    }
    let now = chrono::Utc::now().to_rfc3339();
    let issue = |number| {
        json!({
            "id":number,"number":number,"title":"Fix typo in parser documentation","body":"Correct a documentation typo.",
            "html_url":format!("https://github.com/owner/repo/{}/{number}", if number == 4 {"pull"} else {"issues"}),"repository_url":format!("{base}/repos/owner/repo"),
            "pull_request":if number == 4 {json!({})} else {Value::Null},
            "state":if number == 2 {"closed"} else {"open"},"locked":number == 6,
            "assignees":if number == 3 {json!([{"login":"other-contributor"}])} else {json!([])},"assignee":null,"labels":[{"name":"good first issue"}],
            "created_at":now,"updated_at":now,"comments":2,"user":{"login":"author"},"author_association":"CONTRIBUTOR"
        })
    };
    let comment = |id| json!({"id":id,"html_url":format!("https://github.com/owner/repo/issues/1#issuecomment-{id}"),"body":"complete comment ".repeat(100),"user":{"login":"maintainer"},"author_association":"MEMBER","created_at":now,"updated_at":now});
    let body = if path == "/repos/owner/repo/issues/1" {
        issue(1)
    } else if path == "/repos/owner/repo/issues/2" {
        issue(2)
    } else if path == "/repos/owner/repo/issues/3" {
        issue(3)
    } else if path == "/repos/owner/repo/issues/4" {
        issue(4)
    } else if path == "/repos/owner/repo/issues/6" {
        issue(6)
    } else if path == "/search/issues" {
        json!({"total_count":1,"incomplete_results":false,"items":[issue(1)]})
    } else if path.ends_with("/comments") {
        if target.contains("per_page=1&") || target.ends_with("per_page=1") {
            if target.contains("page=2") {
                json!([comment(2)])
            } else {
                json!([comment(1)])
            }
        } else {
            json!([comment(1), comment(2)])
        }
    } else if path == "/repos/owner/repo" {
        json!({"full_name":"owner/repo","name":"repo","description":"Parser tools","stargazers_count":2500,"forks_count":220,"subscribers_count":50,"open_issues_count":12,"pushed_at":now,"created_at":"2025-01-01T00:00:00Z","updated_at":now,"default_branch":"main","topics":["parser"],"language":"Rust","archived":false})
    } else {
        json!([])
    };
    let raw = body.to_string();
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", raw.len(), raw).unwrap();
}
