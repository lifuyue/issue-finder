use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use issue_finder::config::Config;
use issue_finder::discovery::{DiscoveryScope, RepositoryScope, SearchOptions};
use issue_finder::github::GitHubClient;
use issue_finder::github_budget::GitHubApiBudget;
use issue_finder::paths::IssueFinderPaths;
use issue_finder::recommendation::{RecommendationEngine, ScoutOptions};
use serde_json::{json, Value};
use tempfile::TempDir;

#[path = "support/env_lock.rs"]
mod env_lock;

#[tokio::test]
async fn agent_search_obeys_query_sort_page_and_repository_scope() {
    let _lock = env_lock::EnvLock::acquire();
    let server = MockServer::start(|request| {
        let page = query(request)["page"].parse::<usize>().unwrap();
        let mut items = if page == 2 {
            vec![issue("owner/repo", 1), issue("elsewhere/repo", 2)]
        } else {
            vec![issue("owner/repo", 1), issue("owner/repo", 3)]
        };
        if page == 3 {
            items[1]["state"] = json!("closed");
        }
        json!({"total_count":20,"incomplete_results":false,"items":items})
    });
    let (_dir, paths) = paths();
    let config = Config::default();
    let search: SearchOptions = serde_json::from_value(json!({
        "query":"panic label:bug", "sort":"comments", "order":"asc",
        "page":2,"perPage":2,"maxPages":2
    }))
    .unwrap();
    let scope = DiscoveryScope::repository(RepositoryScope::parse("owner/repo").unwrap());
    let client = GitHubClient::new(&config).unwrap();
    let result = client
        .search_candidates(&paths, true, &config.profile, &scope, &search)
        .await
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert!(request.starts_with("/search/issues?"));
        let parameters = query(request);
        assert_eq!(parameters["sort"], "comments");
        assert_eq!(parameters["order"], "asc");
        assert_eq!(parameters["per_page"], "2");
        assert_eq!(
            parameters["q"],
            "is:issue is:open archived:false no:assignee panic label:bug repo:owner/repo"
        );
    }
    assert_eq!(query(&requests[0])["page"], "2");
    assert_eq!(query(&requests[1])["page"], "3");
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.candidates[0].issue.number, 1);
    assert_eq!(result.candidates[0].source_lanes.len(), 2);
    let details = result.diagnostics.search.unwrap();
    assert_eq!(details.next_page, Some(4));
    assert_eq!(details.total_count, Some(20));
    assert_eq!(details.pages_scanned, 2);
    assert!(details.scan_limit_reached);
}

#[tokio::test]
async fn agent_search_cache_isolated_by_query_sort_scope_page_and_profile() {
    let _lock = env_lock::EnvLock::acquire();
    let server = MockServer::start(|_| json!({"total_count":0,"items":[]}));
    let (_dir, paths) = paths();
    let mut config = Config::default();
    let client = GitHubClient::new(&config).unwrap();
    let mut search: SearchOptions =
        serde_json::from_value(json!({"query":"panic","sort":"best_match"})).unwrap();
    for _ in 0..2 {
        client
            .search_candidates(
                &paths,
                false,
                &config.profile,
                &DiscoveryScope::Global,
                &search,
            )
            .await
            .unwrap();
    }
    assert_eq!(server.requests().len(), 1);
    assert!(!query(&server.requests()[0]).contains_key("sort"));
    search.query = "parser".to_string();
    client
        .search_candidates(
            &paths,
            false,
            &config.profile,
            &DiscoveryScope::Global,
            &search,
        )
        .await
        .unwrap();
    search.sort = issue_finder::discovery::SearchSort::Created;
    client
        .search_candidates(
            &paths,
            false,
            &config.profile,
            &DiscoveryScope::Global,
            &search,
        )
        .await
        .unwrap();
    let scope = DiscoveryScope::repository(RepositoryScope::parse("owner/repo").unwrap());
    client
        .search_candidates(&paths, false, &config.profile, &scope, &search)
        .await
        .unwrap();
    search.page = 2;
    client
        .search_candidates(&paths, false, &config.profile, &scope, &search)
        .await
        .unwrap();
    config.profile.keywords = vec!["compiler".to_string()];
    client
        .search_candidates(&paths, false, &config.profile, &scope, &search)
        .await
        .unwrap();
    assert_eq!(server.requests().len(), 6);
    assert_eq!(client.request_stats().cache_hits["discovery_global"], 1);
}

#[tokio::test]
async fn agent_search_retains_partial_pages_when_request_budget_is_exhausted() {
    let _lock = env_lock::EnvLock::acquire();
    let server = MockServer::start(|_| json!({"total_count":3,"items":[issue("owner/repo",1)]}));
    let (_dir, paths) = paths();
    let config = Config::default();
    let client =
        GitHubClient::with_budget(&config, GitHubApiBudget::with_total_budget(Some(1))).unwrap();
    let search: SearchOptions =
        serde_json::from_value(json!({"query":"panic","perPage":1,"maxPages":3})).unwrap();
    let result = client
        .search_candidates(
            &paths,
            true,
            &config.profile,
            &DiscoveryScope::Global,
            &search,
        )
        .await
        .unwrap();
    assert_eq!(server.requests().len(), 1);
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.diagnostics.search.unwrap().next_page, Some(2));
    assert!(result.diagnostics.stage_errors[0].contains("budget exhausted"));
    assert_eq!(
        client.request_stats().budget_exhausted["discovery_global"],
        1
    );
}

#[tokio::test]
async fn incomplete_search_results_warn_and_are_not_cached() {
    let _lock = env_lock::EnvLock::acquire();
    let server = MockServer::start(
        |_| json!({"total_count":1,"incomplete_results":true,"items":[issue("owner/repo",1)]}),
    );
    let (_dir, paths) = paths();
    let config = Config::default();
    let client = GitHubClient::new(&config).unwrap();
    for _ in 0..2 {
        let result = client
            .search_candidates(
                &paths,
                false,
                &config.profile,
                &DiscoveryScope::Global,
                &SearchOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(result.candidates.len(), 1);
        assert!(result.diagnostics.search.unwrap().incomplete_results);
        assert!(result.diagnostics.stage_errors[0].contains("incomplete search results"));
    }
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn agent_search_ranking_shares_budget_without_promoting_missing_evidence() {
    let _lock = env_lock::EnvLock::acquire();
    let server = MockServer::start(|request| {
        assert!(request.starts_with("/search/issues?"));
        json!({"total_count":1,"items":[issue("owner/repo",1)]})
    });
    let (_dir, paths) = paths();
    let config = Config::default();
    let engine = RecommendationEngine::new(&paths, &config);
    let mut options = ScoutOptions::cli();
    options.record_exposure = false;
    options.include_filtered = true;
    let search: SearchOptions =
        serde_json::from_value(json!({"query":"panic","apiBudget":1})).unwrap();
    let result = engine
        .scout_search(5, true, options, DiscoveryScope::Global, &search)
        .await
        .unwrap();
    assert_eq!(server.requests().len(), 1);
    assert_eq!(result.api_budget.total_network_requests, 1);
    assert_eq!(result.api_budget.total_budget, Some(1));
    assert_eq!(result.discovery_count, 1);
    assert!(result.ranked.is_empty());
    let details = result.diagnostics.search.as_ref().unwrap();
    assert_eq!(details.assessed_count, 1);
    assert_eq!(details.unassessed_count, 0);
    assert_eq!(details.enrichment_limit, 180);
    assert!(details.evidence_incomplete);
    assert_eq!(details.diagnostic_candidates.len(), 1);
    assert_eq!(
        details.diagnostic_candidates[0].issue_reference,
        "owner/repo#1"
    );
    assert!(!details.diagnostic_candidates[0].body_excerpt.is_empty());
    assert!(details.diagnostic_candidates[0]
        .reason
        .contains("not a recommendation"));
    assert!(result
        .diagnostics
        .stage_errors
        .iter()
        .any(|error| error.contains("evidence may be incomplete")));
}

#[test]
fn agent_search_rejects_conflicting_qualifiers_and_unbounded_requests() {
    let scope = DiscoveryScope::repository(RepositoryScope::parse("owner/repo").unwrap());
    for query in [
        "is:closed",
        "state:closed",
        "is:pr",
        "is:locked",
        "repo:another/repo",
    ] {
        let search = SearchOptions {
            query: query.to_string(),
            ..Default::default()
        };
        assert!(search.validate(&scope).is_err(), "{query}");
    }
    for value in [
        json!({"page":0}),
        json!({"perPage":101}),
        json!({"maxPages":11}),
        json!({"page":11,"perPage":100}),
        json!({"apiBudget":0}),
        json!({"query":"x".repeat(1025)}),
    ] {
        let search: SearchOptions = serde_json::from_value(value).unwrap();
        assert!(search.validate(&scope).is_err());
    }
}

fn issue(repository: &str, number: usize) -> Value {
    json!({
        "id":number, "number":number, "title":"Rust CLI parser panic on empty input",
        "body":"Reproduce by running the CLI with an empty argument. Expected: a parse error. Actual: panic in the parser. Add a regression test.",
        "html_url":format!("https://github.com/{repository}/issues/{number}"),
        "repository_url":format!("https://api.github.com/repos/{repository}"),
        "state":"open", "labels":[{"name":"bug"}], "locked":false,
        "assignee":null,"assignees":[], "created_at":"2026-09-01T00:00:00Z",
        "updated_at":"2026-09-07T00:00:00Z"
    })
}

fn paths() -> (TempDir, IssueFinderPaths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = IssueFinderPaths {
        home: dir.path().to_path_buf(),
        config: dir.path().join("config.toml"),
        cache_dir: dir.path().join("cache"),
        workspaces_dir: dir.path().join("workspaces"),
        inbox_dir: dir.path().join("inbox"),
        reports_dir: dir.path().join("reports"),
    };
    (dir, paths)
}

fn query(request: &str) -> HashMap<String, String> {
    url::Url::parse(&format!("http://localhost{request}"))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

struct MockServer {
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    previous_api_base: Option<String>,
}

impl MockServer {
    fn start(response: impl Fn(&str) -> Value + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let previous_api_base = std::env::var("ISSUE_FINDER_GITHUB_API_BASE").ok();
        std::env::set_var(
            "ISSUE_FINDER_GITHUB_API_BASE",
            format!("http://{}", listener.local_addr().unwrap()),
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_requests = requests.clone();
        let thread_stop = stop.clone();
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut buffer = [0; 8192];
                        let read = match stream.read(&mut buffer) {
                            Ok(0) => continue,
                            Ok(read) => read,
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                                ) =>
                            {
                                continue;
                            }
                            Err(error) => panic!("{error}"),
                        };
                        let request = String::from_utf8_lossy(&buffer[..read])
                            .split_whitespace()
                            .nth(1)
                            .unwrap()
                            .to_string();
                        thread_requests.lock().unwrap().push(request.clone());
                        let body = response(&request).to_string();
                        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        });
        Self {
            requests,
            stop,
            thread: Some(thread),
            previous_api_base,
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let joined = self.thread.take().unwrap().join();
        if !thread::panicking() {
            joined.unwrap();
        }
        if let Some(previous) = &self.previous_api_base {
            std::env::set_var("ISSUE_FINDER_GITHUB_API_BASE", previous);
        } else {
            std::env::remove_var("ISSUE_FINDER_GITHUB_API_BASE");
        }
    }
}
