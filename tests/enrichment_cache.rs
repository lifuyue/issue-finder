use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use patchbay_cli::config::Config;
use patchbay_cli::github::GitHubIssue;
use patchbay_cli::github_enrichment::GitHubEnrichmentClient;
use patchbay_cli::paths::PatchbayPaths;
use patchbay_cli::value_scoring::assess_issue;
use tempfile::tempdir;

#[tokio::test]
async fn enrichment_cache_is_used_unless_refresh_is_passed() {
    let (base_url, handle) = start_enrichment_server();

    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let config = Config::default();
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

    let refreshed = GitHubEnrichmentClient::with_api_base(&config, "http://127.0.0.1:9")
        .unwrap()
        .enrich_issue(&paths, &issue, true)
        .await;
    assert_ne!(refreshed.repository.forks, 42);
    assert!(!refreshed.warnings.is_empty());
}

#[tokio::test]
async fn partial_enrichment_failure_still_produces_assessment() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let config = Config::default();
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

#[tokio::test]
async fn enrichment_samples_tail_pages_for_recent_stars_and_comments() {
    let (base_url, handle) = start_tail_sampling_server();

    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let config = Config::default();
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

fn test_paths(root: &std::path::Path) -> PatchbayPaths {
    PatchbayPaths {
        home: root.join("patchbay-home"),
        config: root.join("patchbay-home/config.toml"),
        cache_dir: root.join("patchbay-home/cache"),
        workspaces_dir: root.join("patchbay-home/workspaces"),
        inbox_dir: root.join("patchbay-home/inbox"),
        reports_dir: root.join("patchbay-home/reports"),
    }
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
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let started = Instant::now();
        let mut served = 0usize;
        while served < 6 && started.elapsed() < Duration::from_secs(5) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buffer = [0u8; 4096];
                    let bytes_read = stream.read(&mut buffer).unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
                    let body = if request.starts_with("GET /repos/owner/repo/issues/12/comments") {
                        comments_body()
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

fn start_tail_sampling_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let started = Instant::now();
        let mut served = 0usize;
        while served < 7 && started.elapsed() < Duration::from_secs(5) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buffer = [0u8; 4096];
                    let bytes_read = stream.read(&mut buffer).unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
                    let body = if request.starts_with("GET /repos/owner/repo/issues/12/comments") {
                        if request.contains("&page=1") {
                            comments_tail_page_body(0, 30)
                        } else {
                            recent_comment_page_body()
                        }
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

fn write_response(stream: &mut std::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).unwrap();
}
