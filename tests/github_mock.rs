use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use issue_finder::config::Config;
use issue_finder::paths::IssueFinderPaths;
use issue_finder::workflow;
use tempfile::tempdir;

#[path = "support/env_lock.rs"]
mod env_lock;

#[tokio::test]
async fn scout_uses_mocked_github_search_responses() {
    let _env_lock = env_lock::EnvLock::acquire();
    let (base_url, handle) = start_mock_github();
    std::env::set_var("ISSUE_FINDER_GITHUB_API_BASE", &base_url);
    let _env_guard = EnvGuard;

    let dir = tempdir().unwrap();
    let paths = IssueFinderPaths {
        home: dir.path().to_path_buf(),
        config: dir.path().join("config.toml"),
        cache_dir: dir.path().join("cache"),
        workspaces_dir: dir.path().join("workspaces"),
        inbox_dir: dir.path().join("inbox"),
        reports_dir: dir.path().join("reports"),
    };
    let config = Config::default();

    let ranked = workflow::scout(&paths, &config, 10, true).await.unwrap();

    handle.join().unwrap();

    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].issue.repo_full_name, "owner/repo");
    assert_eq!(ranked[0].issue.number, 12);
    assert!(ranked[0].score > 0);
    assert!(!ranked[0].value_assessment.signals.is_empty());
    assert!(!ranked[0].value_assessment.explanation.is_empty());
}

struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        std::env::remove_var("ISSUE_FINDER_GITHUB_API_BASE");
    }
}

fn start_mock_github() -> (String, thread::JoinHandle<()>) {
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

        while started.elapsed() < Duration::from_secs(5) {
            if served >= 9 && last_request_at.elapsed() > Duration::from_millis(250) {
                break;
            }
            match listener.accept() {
                Ok((mut stream, _)) => {
                    last_request_at = Instant::now();
                    let mut buffer = [0u8; 4096];
                    let bytes_read = stream.read(&mut buffer).unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
                    let body = if request.starts_with("GET /search/issues") {
                        let count = search_count_for_thread.fetch_add(1, Ordering::SeqCst);
                        if count == 0 {
                            search_body(&base_url_for_thread)
                        } else {
                            "{\"items\":[]}".to_string()
                        }
                    } else if request.starts_with("GET /repos/owner/repo/issues/12/comments") {
                        comments_body()
                    } else if request.starts_with("GET /repos/owner/repo/issues/12/timeline") {
                        "[]".to_string()
                    } else if request.starts_with("GET /repos/owner/repo/issues/12") {
                        issue_body()
                    } else if request.starts_with("GET /repos/owner/repo/stargazers") {
                        stargazers_body()
                    } else if request.starts_with("GET /repos/owner/repo/forks") {
                        forks_body()
                    } else if request.starts_with("GET /repos/owner/repo") {
                        repo_body()
                    } else {
                        "{\"message\":\"not found\"}".to_string()
                    };
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

fn search_body(base_url: &str) -> String {
    format!(
        r#"{{
  "items": [
    {{
      "id": 99,
      "number": 12,
      "title": "Fix Rust CLI parser",
      "body": "Expected graceful behavior in src/main.rs",
      "html_url": "https://github.com/owner/repo/issues/12",
      "repository_url": "{base_url}/repos/owner/repo",
      "labels": [{{ "name": "good first issue" }}],
      "locked": false,
      "created_at": "2026-06-01T00:00:00Z",
      "updated_at": "2026-06-02T00:00:00Z"
    }}
  ]
}}"#
    )
}

fn repo_body() -> String {
    r#"{
  "full_name": "owner/repo",
  "name": "repo",
  "description": "Rust CLI developer tools",
  "stargazers_count": 1234,
  "forks_count": 170,
  "subscribers_count": 90,
  "open_issues_count": 12,
  "pushed_at": "2026-06-01T00:00:00Z",
  "created_at": "2025-01-01T00:00:00Z",
  "updated_at": "2026-06-01T00:00:00Z",
  "default_branch": "main",
  "topics": ["rust", "cli"],
  "language": "Rust",
  "archived": false
}"#
    .to_string()
}

fn issue_body() -> String {
    r#"{
  "comments": 1,
  "author_association": "CONTRIBUTOR",
  "user": { "login": "issue-author" }
}"#
    .to_string()
}

fn comments_body() -> String {
    r#"[
  {
    "body": "Maintainer note: this is a good first contribution.",
    "author_association": "MEMBER",
    "created_at": "2026-06-01T12:00:00Z",
    "user": { "login": "maintainer" }
  }
]"#
    .to_string()
}

fn stargazers_body() -> String {
    r#"[
  {
    "starred_at": "2026-06-01T00:00:00Z",
    "user": { "login": "star-user" }
  }
]"#
    .to_string()
}

fn forks_body() -> String {
    r#"[
  {
    "created_at": "2026-05-20T00:00:00Z",
    "owner": { "login": "fork-user" }
  }
]"#
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
