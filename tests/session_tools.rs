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
    let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .env("ISSUE_FINDER_HOME", temp.path())
        .args(["tools", "--profile", "session", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let catalog: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(catalog["sessionContractVersion"], 1);
    let names = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "status",
            "scout",
            "assess",
            "prepare",
            "task_status",
            "finish",
            "feedback"
        ]
    );
    let (ok, output) = call(
        temp.path(),
        "http://127.0.0.1:1",
        "issue-finder.scout",
        json!({"limit":0}),
    );
    assert!(!ok);
    assert_eq!(output["status"], "invalid_arguments");
    let (ok, output) = call(
        temp.path(),
        "http://127.0.0.1:1",
        "issue-finder.dispatch",
        json!({}),
    );
    assert!(!ok);
    assert_eq!(output["status"], "forbidden_tool");
    let (ok, output) = call(
        temp.path(),
        "http://127.0.0.1:1",
        "issue-finder.status",
        json!({"checkAuth":false}),
    );
    assert!(ok);
    assert_eq!(output["status"], "ready");
    assert_eq!(output["structured_content"]["config"]["exists"], false);
    assert!(!String::from_utf8(serde_json::to_vec(&output).unwrap())
        .unwrap()
        .contains("fixture-token"));
}

#[test]
fn session_cli_reads_full_discussion_and_finishes_without_dispatch() {
    let server = Server::start();
    let temp = tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let state = root.join("state");
    let checkout = root.join("checkout");
    fs::create_dir(&checkout).unwrap();
    git(&checkout, &["init", "-b", "main"]);
    git(&checkout, &["config", "user.name", "Test"]);
    git(&checkout, &["config", "user.email", "test@example.invalid"]);
    fs::write(checkout.join("README.md"), "old text\n").unwrap();
    git(&checkout, &["add", "."]);
    git(&checkout, &["commit", "-m", "initial"]);
    git(
        &checkout,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ],
    );

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

    let (_, closed) = call(
        &state,
        &server.url,
        "issue-finder.prepare",
        json!({"issue":"owner/repo#2","checkout":checkout,"workspaceRoot":root.join("workspaces")}),
    );
    assert_eq!(closed["status"], "issue_unavailable");
    assert_eq!(git(&checkout, &["branch", "--show-current"]), "main");
    let (_, gate) = call(
        &state,
        &server.url,
        "issue-finder.prepare",
        json!({"issue":"owner/repo#1","checkout":checkout,"workspaceRoot":root.join("workspaces")}),
    );
    assert_eq!(gate["status"], "blocked_by_gate");
    let (ok, prepared) = call(
        &state,
        &server.url,
        "issue-finder.prepare",
        json!({"issue":"owner/repo#1","checkout":checkout,"workspaceRoot":root.join("workspaces"),"allowGateBypass":true,"bypassReason":"Explicitly requested small documentation issue; reviewed scope and evidence."}),
    );
    assert!(ok, "{prepared}");
    assert_eq!(prepared["status"], "prepared");
    assert_eq!(
        prepared["structured_content"]["task"]["workspace"]["path"],
        checkout.to_str().unwrap()
    );
    assert!(prepared["structured_content"]["task"]["workspace"]["baseCommit"].is_string());
    assert!(!state.join("dispatch").exists());
    fs::write(checkout.join("README.md"), "corrected text\n").unwrap();
    let (ok, failed) = call(
        &state,
        &server.url,
        "issue-finder.finish",
        json!({"workspace":checkout,"checks":[["git","diff","--exit-code"]]}),
    );
    assert!(!ok);
    assert_eq!(failed["status"], "validation_failed");
    let (ok, finished) = call(
        &state,
        &server.url,
        "issue-finder.finish",
        json!({"workspace":checkout,"checks":[["git","diff","--check"]],"summary":"Corrected documentation and reviewed diff."}),
    );
    assert!(ok, "{finished}");
    assert_eq!(finished["status"], "completed");
    assert_eq!(
        finished["structured_content"]["changes"]["changedFiles"],
        json!(["README.md"])
    );
    assert!(Path::new(
        finished["structured_content"]["resultFile"]
            .as_str()
            .unwrap()
    )
    .exists());
    let (ok, resumed) = call(
        &state,
        &server.url,
        "issue-finder.task_status",
        json!({"workspace":checkout}),
    );
    assert!(ok);
    assert_eq!(resumed["status"], "completed");
}

#[test]
fn session_assess_preserves_discussion_for_unavailable_issues_and_pull_requests() {
    let server = Server::start();
    let temp = tempdir().unwrap();
    for number in [2, 3, 4] {
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
    assert!(!temp.path().join("config.toml").exists());
}

fn call(home: &Path, url: &str, tool: &str, args: Value) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .env("ISSUE_FINDER_HOME", home)
        .env("ISSUE_FINDER_GITHUB_API_BASE", url)
        .env("GITHUB_TOKEN", "fixture-token")
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

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let base = url.clone();
        let thread = thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => respond(stream, &base),
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

fn respond(mut stream: TcpStream, base: &str) {
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
    let target = request.split_whitespace().nth(1).unwrap();
    let path = target.split('?').next().unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    let issue = |number| {
        json!({
            "id":number,"number":number,"title":"Fix typo in parser documentation","body":"Correct a documentation typo.",
            "html_url":format!("https://github.com/owner/repo/{}/{number}", if number == 4 {"pull"} else {"issues"}),"repository_url":format!("{base}/repos/owner/repo"),
            "pull_request":if number == 4 {json!({})} else {Value::Null},
            "state":if number == 2 {"closed"} else {"open"},"locked":false,
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
