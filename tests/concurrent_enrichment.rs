use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{Duration as ChronoDuration, Utc};
use issue_finder::config::Config;
use issue_finder::paths::IssueFinderPaths;
use issue_finder::workflow;
use tempfile::tempdir;

#[path = "support/env_lock.rs"]
mod env_lock;

const SLOW_TOP_DELAY: Duration = Duration::from_millis(800);
const OTHER_DETAIL_DELAY: Duration = Duration::from_millis(350);

#[tokio::test]
async fn scout_enriches_candidates_concurrently_and_keeps_feed_order() {
    let _env_lock = env_lock::EnvLock::acquire();
    let (base_url, server) = start_concurrent_mock_github();
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

    let ranked = workflow::scout(&paths, &Config::default(), 10, true)
        .await
        .unwrap();
    server.handle.join().unwrap();

    assert!(
        server.max_detail_concurrency.load(Ordering::SeqCst) >= 2,
        "issue detail requests were not enriched concurrently"
    );
    assert!(
        ranked.len() >= 2,
        "failed endpoint should not fail the whole scout: {ranked:?}"
    );
    assert_eq!(ranked[0].issue.repo_full_name, "owner/slow-top");
    assert!(ranked
        .iter()
        .any(|item| item.issue.repo_full_name == "owner/fast-one"));
}

struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        std::env::remove_var("ISSUE_FINDER_GITHUB_API_BASE");
    }
}

struct MockGithubServer {
    handle: thread::JoinHandle<()>,
    max_detail_concurrency: Arc<AtomicUsize>,
}

fn start_concurrent_mock_github() -> (String, MockGithubServer) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let base_url_for_thread = base_url.clone();
    let active_detail_requests = Arc::new(AtomicUsize::new(0));
    let max_detail_concurrency = Arc::new(AtomicUsize::new(0));
    let search_count = Arc::new(AtomicUsize::new(0));
    let active_detail_requests_for_thread = Arc::clone(&active_detail_requests);
    let max_detail_concurrency_for_thread = Arc::clone(&max_detail_concurrency);
    let search_count_for_thread = Arc::clone(&search_count);

    let handle = thread::spawn(move || {
        let started = Instant::now();
        let mut last_request_at = Instant::now();
        let mut served = 0usize;

        while started.elapsed() < Duration::from_secs(10) {
            if served > 0 && last_request_at.elapsed() > Duration::from_millis(2_500) {
                break;
            }

            match listener.accept() {
                Ok((mut stream, _)) => {
                    last_request_at = Instant::now();
                    served += 1;
                    let base_url = base_url_for_thread.clone();
                    let active_detail_requests = Arc::clone(&active_detail_requests_for_thread);
                    let max_detail_concurrency = Arc::clone(&max_detail_concurrency_for_thread);
                    let search_count = Arc::clone(&search_count_for_thread);
                    thread::spawn(move || {
                        let request = read_request(&mut stream);
                        let search_index = if request.starts_with("GET /search/issues") {
                            search_count.fetch_add(1, Ordering::SeqCst)
                        } else {
                            search_count.load(Ordering::SeqCst)
                        };
                        let response = response_for(&request, &base_url, search_index);
                        if response.tracks_detail_concurrency {
                            let in_flight =
                                active_detail_requests.fetch_add(1, Ordering::SeqCst) + 1;
                            max_detail_concurrency.fetch_max(in_flight, Ordering::SeqCst);
                        }
                        if response.delay > Duration::ZERO {
                            thread::sleep(response.delay);
                        }
                        if response.tracks_detail_concurrency {
                            active_detail_requests.fetch_sub(1, Ordering::SeqCst);
                        }
                        write_response(&mut stream, response.status, &response.body);
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

    (
        base_url,
        MockGithubServer {
            handle,
            max_detail_concurrency,
        },
    )
}

struct MockResponse {
    status: u16,
    delay: Duration,
    body: String,
    tracks_detail_concurrency: bool,
}

fn response_for(request: &str, base_url: &str, search_index: usize) -> MockResponse {
    if request.starts_with("GET /search/issues") {
        if search_index > 0 {
            return ok(r#"{"items":[]}"#.to_string());
        }
        return ok(search_body(base_url));
    }

    for repo in ["slow-top", "fast-one", "fast-two", "broken"] {
        let details_path = format!("/repos/owner/{repo}/issues/{}", issue_number(repo));
        if request.starts_with(&format!("GET {details_path}/timeline")) {
            return ok("[]".to_string());
        }
        if request.starts_with(&format!("GET {details_path}/comments")) {
            return ok(comments_body());
        }
        if request.starts_with(&format!("GET {details_path}")) {
            if repo == "broken" {
                return MockResponse {
                    status: 500,
                    delay: OTHER_DETAIL_DELAY,
                    body: r#"{"message":"temporary failure"}"#.to_string(),
                    tracks_detail_concurrency: true,
                };
            }
            return MockResponse {
                status: 200,
                delay: if repo == "slow-top" {
                    SLOW_TOP_DELAY
                } else {
                    OTHER_DETAIL_DELAY
                },
                body: issue_details_body(),
                tracks_detail_concurrency: true,
            };
        }
        if request.starts_with(&format!("GET /repos/owner/{repo}/stargazers")) {
            return ok(stargazers_body(repo));
        }
        if request.starts_with(&format!("GET /repos/owner/{repo}/forks")) {
            return ok("[]".to_string());
        }
        if request.starts_with(&format!("GET /repos/owner/{repo}")) {
            return ok(repo_body(repo));
        }
    }

    MockResponse {
        status: 404,
        delay: Duration::ZERO,
        body: r#"{"message":"not found"}"#.to_string(),
        tracks_detail_concurrency: false,
    }
}

fn ok(body: String) -> MockResponse {
    MockResponse {
        status: 200,
        delay: Duration::ZERO,
        body,
        tracks_detail_concurrency: false,
    }
}

fn search_body(base_url: &str) -> String {
    let recent = Utc::now().to_rfc3339();
    let stale = (Utc::now() - ChronoDuration::days(20)).to_rfc3339();
    format!(
        r#"{{
  "items": [
    {slow_top},
    {fast_one},
    {fast_two},
    {broken}
  ]
}}"#,
        slow_top = search_item(
            1,
            "slow-top",
            "Fix Rust CLI resolver panic with missing lockfile",
            "Steps to reproduce: run the Rust CLI resolver without a lockfile. Expected behavior is a helpful diagnostic in src/resolver.rs. Actual behavior is a panic. Suggested fix: guard the missing lockfile path and verify with cargo test.",
            &recent,
            base_url
        ),
        fast_one = search_item(
            2,
            "fast-one",
            "Fix TypeScript CLI help output mismatch",
            "Expected behavior: the TypeScript CLI help text should show the documented flag. Actual behavior: it prints the old flag. Reproduce with npm test in src/cli.",
            &recent,
            base_url
        ),
        fast_two = search_item(
            3,
            "fast-two",
            "Fix Rust CLI config default warning",
            "Expected behavior: the Rust CLI should show one warning for a missing config file. Actual behavior: it shows two warnings. Reproduce with cargo test.",
            &stale,
            base_url
        ),
        broken = search_item(
            4,
            "broken",
            "Fix Rust CLI temporary issue details failure",
            "Expected behavior: the Rust CLI should continue when optional metadata fails. Actual behavior is noisy logs.",
            &recent,
            base_url
        )
    )
}

fn search_item(
    number: u64,
    repo: &str,
    title: &str,
    body: &str,
    updated_at: &str,
    base_url: &str,
) -> String {
    format!(
        r#"{{
      "id": {number},
      "number": {number},
      "title": "{title}",
      "body": "{body}",
      "html_url": "https://github.com/owner/{repo}/issues/{number}",
      "repository_url": "{base_url}/repos/owner/{repo}",
      "labels": [{{ "name": "good first issue" }}],
      "locked": false,
      "created_at": "{updated_at}",
      "updated_at": "{updated_at}"
    }}"#
    )
}

fn repo_body(repo: &str) -> String {
    let stars = if repo == "slow-top" { 50_000 } else { 200 };
    let forks = if repo == "slow-top" { 2_500 } else { 25 };
    let watchers = if repo == "slow-top" { 250 } else { 5 };
    format!(
        r#"{{
  "full_name": "owner/{repo}",
  "name": "{repo}",
  "description": "Rust CLI developer tools",
  "stargazers_count": {stars},
  "forks_count": {forks},
  "subscribers_count": {watchers},
  "open_issues_count": 20,
  "pushed_at": "{}",
  "created_at": "2025-01-01T00:00:00Z",
  "updated_at": "{}",
  "default_branch": "main",
  "topics": ["rust", "cli"],
  "language": "Rust",
  "archived": false
}}"#,
        Utc::now().to_rfc3339(),
        Utc::now().to_rfc3339()
    )
}

fn issue_details_body() -> String {
    r#"{"comments":1,"author_association":"CONTRIBUTOR","user":{"login":"author"}}"#.to_string()
}

fn comments_body() -> String {
    format!(
        r#"[{{"body":"Maintainer confirms this remains open.","author_association":"MEMBER","created_at":"{}","user":{{"login":"maintainer"}}}}]"#,
        Utc::now().to_rfc3339()
    )
}

fn stargazers_body(repo: &str) -> String {
    if repo == "slow-top" {
        format!(
            r#"[{{"starred_at":"{}","user":{{"login":"star-user"}}}}]"#,
            Utc::now().to_rfc3339()
        )
    } else {
        "[]".to_string()
    }
}

fn issue_number(repo: &str) -> u64 {
    match repo {
        "slow-top" => 1,
        "fast-one" => 2,
        "fast-two" => 3,
        "broken" => 4,
        _ => 0,
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(bytes_read) => {
                request.extend_from_slice(&buffer[..bytes_read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n")
                    || request.len() >= 16 * 1024
                {
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
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&request).to_string()
}

fn write_response(stream: &mut std::net::TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).unwrap();
}
