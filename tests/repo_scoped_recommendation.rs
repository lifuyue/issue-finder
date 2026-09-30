use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;

use chrono::Utc;
use issue_finder::config::Config;
use issue_finder::paths::IssueFinderPaths;
use issue_finder::recommendation::{
    DiscoveryScope, RecommendationEventSource, RepositoryScope, ScoutOptions,
};
use issue_finder::workflow;
use tempfile::tempdir;

#[path = "support/env_lock.rs"]
mod env_lock;

#[tokio::test]
async fn repo_scoped_scout_returns_same_repo_results_without_global_repo_cap() {
    let _env_lock = env_lock::EnvLock::acquire();
    let server = start_repo_scoped_mock_github();
    std::env::set_var("ISSUE_FINDER_GITHUB_API_BASE", server.base_url.clone());
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
    let scope = DiscoveryScope::repository(RepositoryScope::parse("owner/repo").unwrap());

    let result = workflow::scout_with_options(
        &paths,
        &Config::default(),
        3,
        true,
        ScoutOptions {
            include_filtered: true,
            record_exposure: false,
            source: RecommendationEventSource::CliScout,
        },
        scope,
    )
    .await
    .unwrap();
    let requests = server.requests();
    server.join();

    assert_eq!(result.diagnostics.scope, "repository");
    assert_eq!(result.diagnostics.repository.as_deref(), Some("owner/repo"));
    assert_eq!(
        result.ranked.len(),
        3,
        "ranked candidates: {:#?}; diagnostics: {:#?}; requests: {:#?}",
        result
            .ranked
            .iter()
            .map(|candidate| (
                candidate.issue.number,
                &candidate.issue.title,
                candidate.recommendation.visibility,
                &candidate.recommendation.reasons,
            ))
            .collect::<Vec<_>>(),
        result.diagnostics,
        requests,
    );
    assert!(result
        .ranked
        .iter()
        .all(|candidate| candidate.issue.repo_full_name == "owner/repo"));
    assert!(result
        .diagnostics
        .discovery_stages
        .iter()
        .any(|stage| stage.lane == "repo_scoped:beginner_label:good_first_issue"));

    assert!(
        requests.iter().all(|request| {
            !request.contains("/repos/")
                || request.contains("/repos/owner/repo")
                || request.contains("/repos/owner/repo/")
        }),
        "{requests:#?}"
    );
    assert!(
        requests.iter().all(|request| {
            !request.contains("/search/issues") || request.contains("repo%3Aowner%2Frepo")
        }),
        "{requests:#?}"
    );
}

#[tokio::test]
async fn codex_scout_recovers_legacy_hidden_candidates_without_rewriting_feedback() {
    use issue_finder::recommendation::engine::RecommendationEngine;
    use issue_finder::recommendation::events::{
        load_events, record_event_for_key, IssueKey, RecommendationEventType,
    };

    let _env_lock = env_lock::EnvLock::acquire();
    let server = start_repo_scoped_mock_github();
    std::env::set_var("ISSUE_FINDER_GITHUB_API_BASE", &server.base_url);
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
    for (number, event_type) in [
        (1, RecommendationEventType::Done),
        (2, RecommendationEventType::Dismissed),
        (3, RecommendationEventType::Prepared),
    ] {
        record_event_for_key(
            &paths,
            IssueKey::new("owner/repo", number),
            event_type,
            RecommendationEventSource::FeedbackCommand,
        )
        .unwrap();
    }
    let original_events = load_events(&paths).unwrap();
    let config = Config::default();
    let scope = DiscoveryScope::repository(RepositoryScope::parse("owner/repo").unwrap());
    let options = ScoutOptions {
        include_filtered: true,
        record_exposure: false,
        source: RecommendationEventSource::ToolScout,
    };
    let legacy = RecommendationEngine::new(&paths, &config)
        .scout(3, false, options, scope.clone())
        .await
        .unwrap();
    assert_eq!(legacy.ranked.len(), 1);
    assert_eq!(legacy.ranked[0].issue.number, 3);

    let engine = RecommendationEngine::for_codex(&paths, &config);
    let recovered = engine
        .scout(3, false, options, scope.clone())
        .await
        .unwrap();
    assert_eq!(
        recovered.ranked.len(),
        3,
        "Codex must not reuse a legacy result cache that hides candidates"
    );
    assert!(recovered
        .ranked
        .iter()
        .all(|item| item.recommendation.feedback_penalty == 0));
    assert_eq!(load_events(&paths).unwrap(), original_events);

    engine
        .scout(
            3,
            false,
            ScoutOptions {
                record_exposure: true,
                ..options
            },
            scope.clone(),
        )
        .await
        .unwrap();
    let after_shown = engine.scout(3, false, options, scope).await.unwrap();
    assert!(after_shown
        .ranked
        .iter()
        .all(|item| item.recommendation.feedback_penalty > 0));
    let issue = after_shown.ranked[0].issue.clone();
    engine
        .assess_issue(
            issue.clone(),
            false,
            true,
            RecommendationEventSource::ToolAssess,
        )
        .await
        .unwrap();
    let reread = engine
        .assess_issue(issue, false, false, RecommendationEventSource::ToolAssess)
        .await
        .unwrap();
    assert!(
        reread.recommendation.feedback_penalty
            > after_shown.ranked[0].recommendation.feedback_penalty
    );
    let events = load_events(&paths).unwrap();
    assert_eq!(&events[..original_events.len()], original_events.as_slice());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == RecommendationEventType::Shown)
            .count(),
        3
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == RecommendationEventType::Read)
            .count(),
        1
    );
    server.join();
}

#[test]
fn repository_scope_rejects_issue_urls() {
    let error = RepositoryScope::parse("https://github.com/owner/repo/issues/12")
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "expected owner/repo or https://github.com/owner/repo"
    );
}

struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        std::env::remove_var("ISSUE_FINDER_GITHUB_API_BASE");
    }
}

struct MockGithubServer {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
    shutdown: Arc<AtomicBool>,
    handle: thread::JoinHandle<()>,
}

impl MockGithubServer {
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    fn join(self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.handle.join().unwrap();
    }
}

fn start_repo_scoped_mock_github() -> MockGithubServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let base_url_for_thread = base_url.clone();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let requests_for_thread = Arc::clone(&requests);
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_thread = Arc::clone(&shutdown);

    let handle = thread::spawn(move || {
        while !shutdown_for_thread.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let base_url = base_url_for_thread.clone();
                    let requests = Arc::clone(&requests_for_thread);
                    thread::spawn(move || {
                        let mut buffer = [0u8; 4096];
                        let bytes_read = stream.read(&mut buffer).unwrap_or(0);
                        let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
                        let first_line = request.lines().next().unwrap_or_default().to_string();
                        requests.lock().unwrap().push(first_line);
                        let body = response_body(&request, &base_url);
                        write_response(&mut stream, &body);
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

    MockGithubServer {
        base_url,
        requests,
        shutdown,
        handle,
    }
}

fn response_body(request: &str, base_url: &str) -> String {
    if request.contains("/search/issues") {
        return r#"{"items":[]}"#.to_string();
    }

    if request.contains("/repos/owner/repo/issues/1/comments")
        || request.contains("/repos/owner/repo/issues/2/comments")
        || request.contains("/repos/owner/repo/issues/3/comments")
    {
        return "[]".to_string();
    }
    if request.contains("/repos/owner/repo/issues/1/timeline")
        || request.contains("/repos/owner/repo/issues/2/timeline")
        || request.contains("/repos/owner/repo/issues/3/timeline")
    {
        return "[]".to_string();
    }
    if request.contains("/repos/owner/repo/issues/1") {
        return issue_detail_body();
    }
    if request.contains("/repos/owner/repo/issues/2") {
        return issue_detail_body();
    }
    if request.contains("/repos/owner/repo/issues/3") {
        return issue_detail_body();
    }

    if request.contains("/repos/owner/repo/issues")
        && (request.contains("labels=good%20first%20issue")
            || request.contains("labels=good+first+issue"))
    {
        return repo_issue_list_body(base_url);
    }
    if request.contains("/repos/owner/repo/issues") {
        return "[]".to_string();
    }

    if request.contains("/repos/owner/repo/stargazers")
        || request.contains("/repos/owner/repo/forks")
    {
        return "[]".to_string();
    }
    if request.contains("/repos/owner/repo") {
        return repo_body();
    }

    "[]".to_string()
}

fn repo_issue_list_body(base_url: &str) -> String {
    format!(
        "[{},{},{}]",
        issue_list_item(base_url, 1),
        issue_list_item(base_url, 2),
        issue_list_item(base_url, 3)
    )
}

fn issue_list_item(base_url: &str, number: u64) -> String {
    let now = Utc::now().to_rfc3339();
    format!(
        r#"{{
  "id": {number},
  "number": {number},
  "title": "Fix reproducible Rust CLI bug {number}",
  "body": "Expected behavior differs from actual behavior in src/lib.rs. Reproduction steps are clear and a focused test can cover the fix.",
  "html_url": "https://github.com/owner/repo/issues/{number}",
  "labels": [{{"name":"good first issue"}}],
  "pull_request": null,
  "locked": false,
  "assignee": null,
  "assignees": [],
  "created_at": "{now}",
  "updated_at": "{now}",
  "repository_url": "{base_url}/repos/owner/repo"
}}"#
    )
}

fn issue_detail_body() -> String {
    r#"{
  "comments": 0,
  "author_association": "CONTRIBUTOR",
  "user": {"login": "issue-author"}
}"#
    .to_string()
}

fn repo_body() -> String {
    let now = Utc::now().to_rfc3339();
    format!(
        r#"{{
  "full_name": "owner/repo",
  "name": "repo",
  "description": "Rust CLI developer tools",
  "stargazers_count": 5000,
  "forks_count": 300,
  "subscribers_count": 120,
  "open_issues_count": 20,
  "pushed_at": "{now}",
  "created_at": "2020-01-01T00:00:00Z",
  "updated_at": "{now}",
  "default_branch": "main",
  "archived": false,
  "topics": ["rust", "cli", "developer-tools"],
  "language": "Rust"
}}"#
    )
}

fn write_response(stream: &mut std::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).unwrap();
}
