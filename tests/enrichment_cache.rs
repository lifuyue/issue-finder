use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use issue_finder::config::Config;
use issue_finder::github::GitHubIssue;
use issue_finder::github_budget::GitHubApiBudget;
use issue_finder::github_enrichment::{competition_timeline_missing, GitHubEnrichmentClient};
use issue_finder::paths::IssueFinderPaths;
use issue_finder::value_scoring::assess_issue;
use tempfile::tempdir;
use tokio::sync::Mutex;

static ENRICHMENT_TEST_LOCK: Mutex<()> = Mutex::const_new(());

#[tokio::test(flavor = "current_thread")]
async fn enrichment_cache_is_used_unless_refresh_is_passed() {
    let _test_lock = ENRICHMENT_TEST_LOCK.lock().await;
    let (base_url, handle) = start_enrichment_server();

    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let config = test_config();
    let issue = issue();
    let client = GitHubEnrichmentClient::with_api_base(&config, base_url).unwrap();
    let enriched = client.enrich_issue(&paths, &issue, true).await;
    handle.join().unwrap();

    assert_eq!(enriched.repository.forks, 42);
    assert!(!enriched.comments.is_empty());

    let cached = GitHubEnrichmentClient::with_api_base(&config, "http://127.0.0.1:9")
        .unwrap()
        .enrich_issue(&paths, &issue, false)
        .await;
    assert_eq!(cached.repository.forks, 42);
    assert!(cached.warnings.is_empty());

    let budget = GitHubApiBudget::with_total_budget(Some(0));
    let cached_without_network = GitHubEnrichmentClient::with_api_base_and_budget(
        &config,
        "http://127.0.0.1:9",
        budget.clone(),
    )
    .unwrap()
    .enrich_issue(&paths, &issue, false)
    .await;
    assert_eq!(cached_without_network.repository.forks, 42);
    assert_eq!(budget.report().total_network_requests, 0);

    let refreshed = GitHubEnrichmentClient::with_api_base(&config, "http://127.0.0.1:9")
        .unwrap()
        .enrich_issue(&paths, &issue, true)
        .await;
    assert_ne!(refreshed.repository.forks, 42);
    assert!(!refreshed.warnings.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn stale_enrichment_cache_refreshes_from_network() {
    let _test_lock = ENRICHMENT_TEST_LOCK.lock().await;
    let (base_url, handle) = start_enrichment_server();

    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let config = test_config();
    let issue = issue();
    let mut stale = issue_finder::github_enrichment::EnrichedIssue::from_issue(&issue);
    stale.repository.forks = 7;
    stale.source_fetched_at = (Utc::now() - chrono::Duration::minutes(480)).to_rfc3339();
    let cache_path = paths.enrichment_cache_path(&issue.repo_full_name, issue.number);
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    std::fs::write(cache_path, serde_json::to_vec_pretty(&stale).unwrap()).unwrap();

    let refreshed = GitHubEnrichmentClient::with_api_base(&config, base_url)
        .unwrap()
        .enrich_issue(&paths, &issue, false)
        .await;
    handle.join().unwrap();

    assert_eq!(refreshed.repository.forks, 42);
    assert!(refreshed.warnings.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn partial_enrichment_failure_still_produces_assessment() {
    let _test_lock = ENRICHMENT_TEST_LOCK.lock().await;
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let config = test_config();
    let issue = issue();
    let enriched = GitHubEnrichmentClient::with_api_base(&config, "http://127.0.0.1:9")
        .unwrap()
        .enrich_issue(&paths, &issue, true)
        .await;

    assert!(!enriched.warnings.is_empty());
    let assessment = assess_issue(&enriched, &config.profile);
    assert!(assessment.final_rank_score >= 0);
    assert!(!assessment.missing_evidence.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn budget_exhaustion_is_deterministic_and_explainable() {
    let _test_lock = ENRICHMENT_TEST_LOCK.lock().await;
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let config = test_config();
    let issue = issue();
    let budget = GitHubApiBudget::with_total_budget(Some(0));
    let enriched = GitHubEnrichmentClient::with_api_base_and_budget(
        &config,
        "http://127.0.0.1:9",
        budget.clone(),
    )
    .unwrap()
    .enrich_issue(&paths, &issue, true)
    .await;

    assert!(enriched
        .warnings
        .iter()
        .any(|warning| warning.contains("GitHub API budget exhausted")));
    assert!(enriched
        .competition
        .warnings
        .iter()
        .any(|warning| warning.contains("GitHub API budget exhausted")));
    assert_eq!(budget.report().total_network_requests, 0);
    assert!(!budget.report().budget_exhausted.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn enrichment_samples_tail_pages_for_recent_stars_and_comments() {
    let _test_lock = ENRICHMENT_TEST_LOCK.lock().await;
    let (base_url, handle) = start_tail_sampling_server();

    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let config = test_config();
    let issue = issue();
    let enriched = GitHubEnrichmentClient::with_api_base(&config, base_url)
        .unwrap()
        .enrich_issue(&paths, &issue, true)
        .await;
    handle.join().unwrap();

    let star_actors = enriched
        .growth
        .recent_stargazer_sample
        .iter()
        .filter_map(|sample| sample.actor.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(star_actors.len(), 100);
    assert!(!star_actors.contains(&"old-star-0"));
    assert!(star_actors.contains(&"recent-star"));

    let comment_authors = enriched
        .comments
        .iter()
        .filter_map(|comment| comment.author.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(comment_authors.len(), 30);
    assert!(!comment_authors.contains(&"old-comment-0"));
    assert!(comment_authors.contains(&"recent-maintainer"));
    assert!(enriched.activity.maintainer_recent_response);
}

#[tokio::test(flavor = "current_thread")]
async fn timeline_completion_updates_missing_competition_cache() {
    let _test_lock = ENRICHMENT_TEST_LOCK.lock().await;
    let (base_url, handle) = start_timeline_completion_server();

    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let config = test_config();
    let issue = issue();
    let current = issue_finder::github_enrichment::EnrichedIssue::from_issue(&issue);
    assert!(competition_timeline_missing(&current));

    let client = GitHubEnrichmentClient::with_api_base(&config, base_url).unwrap();
    let completed = client
        .complete_competition_timeline(&paths, &issue, &current, true)
        .await;
    handle.join().unwrap();

    assert_eq!(completed.competition.open_pr_refs, 1);
    assert!(completed.competition.warnings.is_empty());
    assert!(!competition_timeline_missing(&completed));

    let cached = GitHubEnrichmentClient::with_api_base(&config, "http://127.0.0.1:9")
        .unwrap()
        .complete_competition_timeline(&paths, &issue, &current, false)
        .await;
    assert_eq!(cached.competition.open_pr_refs, 1);
    assert!(!competition_timeline_missing(&cached));

    let budget = GitHubApiBudget::with_total_budget(Some(0));
    let cached_without_network = GitHubEnrichmentClient::with_api_base_and_budget(
        &config,
        "http://127.0.0.1:9",
        budget.clone(),
    )
    .unwrap()
    .complete_competition_timeline(&paths, &issue, &current, false)
    .await;
    assert_eq!(cached_without_network.competition.open_pr_refs, 1);
    assert_eq!(budget.report().total_network_requests, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn fresh_cache_preserves_ranking_assessment() {
    let _test_lock = ENRICHMENT_TEST_LOCK.lock().await;
    let (base_url, handle) = start_enrichment_server();

    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let config = test_config();
    let issue = issue();
    let enriched = GitHubEnrichmentClient::with_api_base(&config, base_url)
        .unwrap()
        .enrich_issue(&paths, &issue, true)
        .await;
    handle.join().unwrap();

    let cached = GitHubEnrichmentClient::with_api_base(&config, "http://127.0.0.1:9")
        .unwrap()
        .enrich_issue(&paths, &issue, false)
        .await;

    let live_assessment = assess_issue(&enriched, &config.profile);
    let cached_assessment = assess_issue(&cached, &config.profile);
    assert_eq!(
        cached_assessment.final_rank_score,
        live_assessment.final_rank_score
    );
}

fn test_paths(root: &std::path::Path) -> IssueFinderPaths {
    IssueFinderPaths {
        home: root.join("issue-finder-home"),
        config: root.join("issue-finder-home/config.toml"),
        cache_dir: root.join("issue-finder-home/cache"),
        workspaces_dir: root.join("issue-finder-home/workspaces"),
        inbox_dir: root.join("issue-finder-home/inbox"),
        reports_dir: root.join("issue-finder-home/reports"),
    }
}

fn test_config() -> Config {
    let mut config = Config::default();
    config.github.token.clear();
    config
}

fn issue() -> GitHubIssue {
    GitHubIssue {
        id: 1,
        number: 12,
        title: "Fix Rust CLI parser".to_string(),
        body: "Expected graceful behavior in src/main.rs".to_string(),
        labels: vec!["good first issue".to_string()],
        url: "https://github.com/owner/repo/issues/12".to_string(),
        repo_full_name: "owner/repo".to_string(),
        repo_name: "repo".to_string(),
        repo_description: "Rust CLI developer tools".to_string(),
        repo_stars: 123,
        created_at: Utc::now().to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
    }
}

fn start_enrichment_server() -> (String, thread::JoinHandle<()>) {
    start_json_server(6, |request| {
        if request.starts_with("GET /repos/owner/repo/issues/12/comments") {
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
            "{}".to_string()
        }
    })
}

fn start_tail_sampling_server() -> (String, thread::JoinHandle<()>) {
    start_json_server(7, |request| {
        if request.starts_with("GET /repos/owner/repo/issues/12/comments") {
            if request.contains("&page=1") {
                comments_tail_page_body(0, 30)
            } else {
                recent_comment_page_body()
            }
        } else if request.starts_with("GET /repos/owner/repo/issues/12/timeline") {
            "[]".to_string()
        } else if request.starts_with("GET /repos/owner/repo/issues/12") {
            tail_issue_body()
        } else if request.starts_with("GET /repos/owner/repo/stargazers") {
            if request.contains("&page=1") {
                stargazers_tail_page_body()
            } else {
                recent_stargazer_page_body()
            }
        } else if request.starts_with("GET /repos/owner/repo/forks") {
            "[]".to_string()
        } else if request.starts_with("GET /repos/owner/repo") {
            tail_repo_body()
        } else {
            "{}".to_string()
        }
    })
}

fn start_timeline_completion_server() -> (String, thread::JoinHandle<()>) {
    start_json_server(1, |request| {
        if request.starts_with("GET /repos/owner/repo/issues/12/timeline") {
            open_pr_timeline_body()
        } else {
            "{}".to_string()
        }
    })
}

fn start_json_server<F>(minimum_requests: usize, responder: F) -> (String, thread::JoinHandle<()>)
where
    F: Fn(&str) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let started = Instant::now();
        let mut last_request_at = Instant::now();
        let mut served = 0usize;
        while started.elapsed() < Duration::from_secs(10) {
            if served >= minimum_requests && last_request_at.elapsed() > Duration::from_secs(1) {
                break;
            }
            match listener.accept() {
                Ok((mut stream, _)) => {
                    if let Some(request) = read_http_request(&mut stream) {
                        last_request_at = Instant::now();
                        let body = responder(&request);
                        write_response(&mut stream, &body);
                        served += 1;
                    }
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

fn read_http_request(stream: &mut TcpStream) -> Option<String> {
    stream.set_read_timeout(Some(Duration::from_secs(1))).ok()?;
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        let bytes_read = stream.read(&mut buffer).ok()?;
        if bytes_read == 0 {
            return None;
        }
        request.extend_from_slice(&buffer[..bytes_read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if request.len() > 16 * 1024 {
            return None;
        }
    }
    Some(String::from_utf8_lossy(&request).into_owned())
}

fn repo_body() -> String {
    r#"{
  "full_name": "owner/repo",
  "name": "repo",
  "description": "Rust CLI developer tools",
  "stargazers_count": 123,
  "forks_count": 42,
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
    r#"{"comments":1,"author_association":"CONTRIBUTOR","user":{"login":"author"}}"#.to_string()
}

fn comments_body() -> String {
    r#"[{"body":"Maintainer note","author_association":"MEMBER","created_at":"2026-06-01T00:00:00Z","user":{"login":"maintainer"}}]"#.to_string()
}

fn stargazers_body() -> String {
    r#"[{"starred_at":"2026-06-01T00:00:00Z","user":{"login":"star"}}]"#.to_string()
}

fn forks_body() -> String {
    r#"[{"created_at":"2026-05-20T00:00:00Z","owner":{"login":"fork"}}]"#.to_string()
}

fn tail_repo_body() -> String {
    r#"{
  "full_name": "owner/repo",
  "name": "repo",
  "description": "Rust CLI developer tools",
  "stargazers_count": 101,
  "forks_count": 0,
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

fn tail_issue_body() -> String {
    r#"{"comments":31,"author_association":"CONTRIBUTOR","user":{"login":"author"}}"#.to_string()
}

fn stargazers_tail_page_body() -> String {
    let timestamp = Utc::now().to_rfc3339();
    let items = (0..100)
        .map(|index| {
            format!(r#"{{"starred_at":"{timestamp}","user":{{"login":"old-star-{index}"}}}}"#)
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
}

fn recent_stargazer_page_body() -> String {
    format!(
        r#"[{{"starred_at":"{}","user":{{"login":"recent-star"}}}}]"#,
        Utc::now().to_rfc3339()
    )
}

fn comments_tail_page_body(start: usize, count: usize) -> String {
    let timestamp = Utc::now().to_rfc3339();
    let items = (start..start + count)
        .map(|index| {
            format!(
                r#"{{"body":"old comment {index}","author_association":"CONTRIBUTOR","created_at":"{timestamp}","user":{{"login":"old-comment-{index}"}}}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
}

fn recent_comment_page_body() -> String {
    format!(
        r#"[{{"body":"recent maintainer reply","author_association":"MEMBER","created_at":"{}","user":{{"login":"recent-maintainer"}}}}]"#,
        Utc::now().to_rfc3339()
    )
}

fn open_pr_timeline_body() -> String {
    r#"[{"event":"cross-referenced","created_at":"2026-06-01T00:00:00Z","source":{"issue":{"state":"open","pull_request":{}}}}]"#.to_string()
}

fn write_response(stream: &mut std::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).unwrap();
}
