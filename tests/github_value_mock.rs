use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration as StdDuration, Instant};

use chrono::{Duration, Utc};
use issue_finder::config::Config;
use issue_finder::paths::IssueFinderPaths;
use issue_finder::value_scoring::{GateStatus, RecommendationCategory, RiskTag};
use issue_finder::value_signals::ValueSignalKind;
use issue_finder::workflow;
use tempfile::tempdir;

#[path = "support/env_lock.rs"]
mod env_lock;

#[tokio::test]
async fn scout_reorders_candidates_after_value_enrichment() {
    let _env_lock = env_lock::EnvLock::acquire();
    let (base_url, handle) = start_mock_value_github();
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
    handle.join().unwrap();

    assert_eq!(ranked.len(), 2);
    assert_eq!(ranked[0].issue.repo_full_name, "owner/growth");
    assert_eq!(
        ranked[0].value_assessment.recommendation_category,
        RecommendationCategory::NicheButActionable
    );
    assert_eq!(
        ranked[0].value_assessment.gates.repo_influence.status,
        GateStatus::SoftFail
    );
    assert!(ranked[0]
        .value_assessment
        .signals
        .iter()
        .any(|signal| signal.kind == ValueSignalKind::GrowthMomentum));

    let noisy = ranked
        .iter()
        .find(|issue| issue.issue.repo_full_name == "owner/noisy")
        .expect("noisy candidate should be present");
    assert!(ranked[0].score > noisy.score);
    assert!(noisy
        .value_assessment
        .risk_tags
        .contains(&RiskTag::HighTriageLoad));

    assert!(!ranked
        .iter()
        .any(|issue| issue.issue.repo_full_name == "owner/impact"));
}

struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        std::env::remove_var("ISSUE_FINDER_GITHUB_API_BASE");
    }
}

fn start_mock_value_github() -> (String, thread::JoinHandle<()>) {
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

        while started.elapsed() < StdDuration::from_secs(10) {
            if served > 0 && last_request_at.elapsed() > StdDuration::from_secs(1) {
                break;
            }
            match listener.accept() {
                Ok((mut stream, _)) => {
                    last_request_at = Instant::now();
                    let mut buffer = [0u8; 4096];
                    let bytes_read = stream.read(&mut buffer).unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
                    let search_index = if request.contains("/search/issues") {
                        search_count_for_thread.fetch_add(1, Ordering::SeqCst)
                    } else {
                        search_count_for_thread.load(Ordering::SeqCst)
                    };
                    let body = response_body(&request, &base_url_for_thread, search_index);
                    write_response(&mut stream, &body);
                    served += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(StdDuration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

    (base_url, handle)
}

fn response_body(request: &str, base_url: &str, search_index: usize) -> String {
    if request.contains("/search/issues") {
        return if search_index == 0 {
            search_body(base_url)
        } else {
            r#"{"items":[]}"#.to_string()
        };
    }

    if request.contains("/repos/owner/noisy/issues/1/comments") {
        return comments_body("CONTRIBUTOR", "noisy-commenter");
    }
    if request.contains("/repos/owner/growth/issues/2/comments") {
        return comments_body("MEMBER", "growth-maintainer");
    }
    if request.contains("/repos/owner/impact/issues/3/comments") {
        return "[]".to_string();
    }

    if request.contains("/repos/owner/noisy/issues/1/timeline")
        || request.contains("/repos/owner/growth/issues/2/timeline")
        || request.contains("/repos/owner/impact/issues/3/timeline")
    {
        return "[]".to_string();
    }

    if request.contains("/repos/owner/noisy/issues/1") {
        return issue_details_body(60);
    }
    if request.contains("/repos/owner/growth/issues/2") {
        return issue_details_body(1);
    }
    if request.contains("/repos/owner/impact/issues/3") {
        return issue_details_body(0);
    }

    if request.contains("/repos/owner/noisy/stargazers")
        || request.contains("/repos/owner/impact/stargazers")
    {
        return "[]".to_string();
    }
    if request.contains("/repos/owner/growth/stargazers") {
        return stargazers_body(16);
    }

    if request.contains("/repos/owner/noisy/forks")
        || request.contains("/repos/owner/growth/forks")
        || request.contains("/repos/owner/impact/forks")
    {
        return "[]".to_string();
    }

    if request.contains("/repos/owner/noisy") {
        return noisy_repo_body();
    }
    if request.contains("/repos/owner/growth") {
        return growth_repo_body();
    }
    if request.contains("/repos/owner/impact") {
        return impact_repo_body();
    }

    r#"{"message":"not found"}"#.to_string()
}

fn search_body(base_url: &str) -> String {
    format!(
        r#"{{
  "items": [
    {{
      "id": 1,
      "number": 1,
      "title": "Fix Rust CLI parser",
      "body": "The parser currently panics when a subcommand contains repeated flags. Expected behavior is a graceful error in src/main.rs, actual behavior is a panic with a short stack trace. Verify the parser fix by running cargo test.",
      "html_url": "https://github.com/owner/noisy/issues/1",
      "repository_url": "{base_url}/repos/owner/noisy",
      "labels": [{{ "name": "good first issue" }}],
      "locked": false,
      "created_at": "{stale}",
      "updated_at": "{stale}"
    }},
    {{
      "id": 2,
      "number": 2,
      "title": "Improve dependency resolver diagnostics",
      "body": "Steps to reproduce: run the CLI package install command with a missing dependency. The resolver returns an unclear panic. Expected behavior is a helpful diagnostic in src/resolver.rs, actual behavior is a panic during package install. Suggested fix: guard the empty resolver result and verify with cargo test.",
      "html_url": "https://github.com/owner/growth/issues/2",
      "repository_url": "{base_url}/repos/owner/growth",
      "labels": [{{ "name": "good first issue" }}],
      "locked": false,
      "created_at": "{recent}",
      "updated_at": "{recent}"
    }},
    {{
      "id": 3,
      "number": 3,
      "title": "Needs product direction",
      "body": "Needs more discussion.",
      "html_url": "https://github.com/owner/impact/issues/3",
      "repository_url": "{base_url}/repos/owner/impact",
      "labels": [],
      "locked": false,
      "created_at": "{recent}",
      "updated_at": "{recent}"
    }}
  ]
}}"#,
        recent = Utc::now().to_rfc3339(),
        stale = (Utc::now() - Duration::days(90)).to_rfc3339()
    )
}

fn noisy_repo_body() -> String {
    repo_body(RepoBody {
        full_name: "owner/noisy",
        name: "noisy",
        description: "Rust CLI developer tools",
        stars: 55,
        forks: 3,
        open_issues: 1_200,
        pushed_at: (Utc::now() - Duration::days(90)).to_rfc3339(),
        topics: vec!["rust", "cli"],
    })
}

fn growth_repo_body() -> String {
    repo_body(RepoBody {
        full_name: "owner/growth",
        name: "growth",
        description: "Rust CLI package manager developer tools",
        stars: 80,
        forks: 4,
        open_issues: 12,
        pushed_at: Utc::now().to_rfc3339(),
        topics: vec!["package-manager", "cli", "rust"],
    })
}

fn impact_repo_body() -> String {
    repo_body(RepoBody {
        full_name: "owner/impact",
        name: "impact",
        description: "Large platform project",
        stars: 12_000,
        forks: 1_100,
        open_issues: 30,
        pushed_at: Utc::now().to_rfc3339(),
        topics: vec!["platform"],
    })
}

struct RepoBody<'a> {
    full_name: &'a str,
    name: &'a str,
    description: &'a str,
    stars: u64,
    forks: u64,
    open_issues: u64,
    pushed_at: String,
    topics: Vec<&'a str>,
}

fn repo_body(repo: RepoBody<'_>) -> String {
    let topics = repo
        .topics
        .into_iter()
        .map(|topic| format!(r#""{topic}""#))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{
  "full_name": "{full_name}",
  "name": "{name}",
  "description": "{description}",
  "stargazers_count": {stars},
  "forks_count": {forks},
  "subscribers_count": 0,
  "open_issues_count": {open_issues},
  "pushed_at": "{pushed_at}",
  "created_at": "2025-01-01T00:00:00Z",
  "updated_at": "{pushed_at}",
  "default_branch": "main",
  "topics": [{topics}],
  "language": "Rust",
  "archived": false
}}"#,
        full_name = repo.full_name,
        name = repo.name,
        description = repo.description,
        stars = repo.stars,
        forks = repo.forks,
        open_issues = repo.open_issues,
        pushed_at = repo.pushed_at
    )
}

fn issue_details_body(comments: u64) -> String {
    format!(
        r#"{{"comments":{comments},"author_association":"CONTRIBUTOR","user":{{"login":"author"}}}}"#
    )
}

fn comments_body(author_association: &str, login: &str) -> String {
    format!(
        r#"[{{"body":"comment","author_association":"{author_association}","created_at":"{}","user":{{"login":"{login}"}}}}]"#,
        Utc::now().to_rfc3339()
    )
}

fn stargazers_body(count: usize) -> String {
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

fn write_response(stream: &mut std::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).unwrap();
}
