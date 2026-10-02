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

#[path = "support/github_auth.rs"]
mod github_auth;

#[tokio::test]
async fn repo_scoped_scout_returns_same_repo_results_without_global_repo_cap() {
    let _env_lock = env_lock::EnvLock::acquire();
    let _auth_guard = github_auth::GitHubAuthGuard::clear();
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
    let _auth_guard = github_auth::GitHubAuthGuard::clear();
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
    let mut config = Config::default();
    config.decision.codex_binary = dir.path().join("missing-codex").display().to_string();
    config.decision.provider = issue_finder::config::DecisionProvider::Codex;
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

    if request.contains("/repos/owner/repo/issues/") && request.contains("/comments") {
        return "[]".to_string();
    }
    if request.contains("/repos/owner/repo/issues/") && request.contains("/timeline") {
        return "[]".to_string();
    }
    if request.contains("/repos/owner/repo/issues/") {
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
  "state": "open", "locked": false, "assignees": [],
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

struct InspectableDecisionProvider {
    requests: Mutex<Vec<issue_finder::decision::contract::DecisionRequest>>,
    fail: bool,
    version: &'static str,
}

impl issue_finder::decision::contract::Provider for InspectableDecisionProvider {
    fn fingerprint(&self) -> String {
        format!(
            "offline-alternate-provider:semantic-integration-{}",
            self.version
        )
    }

    fn decide<'a>(
        &'a self,
        request: &'a issue_finder::decision::contract::DecisionRequest,
    ) -> issue_finder::decision::contract::DecisionFuture<'a> {
        use issue_finder::decision::contract::*;
        Box::pin(async move {
            self.requests.lock().unwrap().push(request.clone());
            if self.fail {
                return Err(ProviderError::new(
                    "authentication_failed",
                    "offline expired login",
                ));
            }
            let precise_correction = request.candidate_id.ends_with(":3");
            let answers = request
                .questions
                .iter()
                .map(|question| {
                    let choice = match question.id.as_str() {
                        "task_type" if precise_correction => "concrete_change",
                        "task_type" => "support_question",
                        "description_quality" if precise_correction => "clear",
                        "description_quality" => "unclear",
                        "contribution_signal" if precise_correction => "interest_only",
                        "contribution_signal" => "not_observed",
                        "scope" => "bounded",
                        "preference_match" => "matches",
                        "task_shape" if precise_correction => "documentation",
                        "task_shape" => "implementation",
                        "verification_clues" => "present",
                        "maintainer_signal" => "not_observed",
                        _ => panic!("unexpected semantic question {}", question.id),
                    };
                    QuestionResponse {
                        question_id: question.id.clone(),
                        status: AnswerStatus::Answered,
                        answer: Some(Answer::Choice(choice.into())),
                        probabilities: None,
                    }
                })
                .collect();
            Ok(DecisionResponse {
                candidate_id: request.candidate_id.clone(),
                input_id: request.input_id.clone(),
                status: ResponseStatus::Complete,
                answers,
                metadata: ProviderMetadata {
                    provider: "offline_alternate".into(),
                    model: "finite-fixture".into(),
                    reasoning_effort: "none".into(),
                    ..ProviderMetadata::default()
                },
            })
        })
    }
}

fn semantic_test_paths(directory: &std::path::Path) -> IssueFinderPaths {
    IssueFinderPaths {
        home: directory.into(),
        config: directory.join("config.toml"),
        cache_dir: directory.join("cache"),
        workspaces_dir: directory.join("workspaces"),
        inbox_dir: directory.join("inbox"),
        reports_dir: directory.join("reports"),
    }
}

/// Extend the existing GitHub fixture with a short precise correction and a
/// participation question which the legacy PR keyword policy hides.
fn start_semantic_mock_github() -> MockGithubServer {
    start_semantic_mock_github_with(3, Arc::new(AtomicBool::new(false)))
}

fn start_semantic_mock_github_with(count: usize, close_first: Arc<AtomicBool>) -> MockGithubServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let mut listing: serde_json::Value =
        serde_json::from_str(&repo_issue_list_body(&base_url)).unwrap();
    for number in 4..=count {
        listing
            .as_array_mut()
            .unwrap()
            .push(serde_json::from_str(&issue_list_item(&base_url, number as u64)).unwrap());
    }
    let frozen_time = Utc::now().to_rfc3339();
    for item in listing.as_array_mut().unwrap() {
        item["created_at"] = serde_json::json!(frozen_time);
        item["updated_at"] = serde_json::json!(frozen_time);
    }
    listing[2]["title"] = serde_json::json!("Correct CLI documentation spelling");
    listing[2]["body"] = serde_json::json!(
        "In docs/cli.md, replace `adress` with `address` in the --output example."
    );
    let listing = listing.to_string();
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
                    let listing = listing.clone();
                    let base_url = base_url_for_thread.clone();
                    let requests = Arc::clone(&requests_for_thread);
                    let close_first = close_first.clone();
                    thread::spawn(move || {
                        let mut buffer = [0u8; 4096];
                        let bytes_read = stream.read(&mut buffer).unwrap_or(0);
                        let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
                        requests
                            .lock()
                            .unwrap()
                            .push(request.lines().next().unwrap_or_default().into());
                        let body = if request.contains("/search/issues") {
                            r#"{"items":[],"total_count":0,"incomplete_results":false}"#.into()
                        } else if request
                            .lines()
                            .next()
                            .is_some_and(|line| line == "GET /repos/owner/repo/issues/1 HTTP/1.1")
                            && close_first.load(Ordering::SeqCst)
                        {
                            r#"{"state":"closed","locked":false,"assignees":[],"comments":0}"#
                                .into()
                        } else if request.contains("/repos/owner/repo/issues/3/comments") {
                            r#"[{"id":303,"html_url":"https://github.com/owner/repo/issues/3#issuecomment-303","user":{"login":"interested-contributor"},"author_association":"CONTRIBUTOR","created_at":"2026-09-30T00:00:00Z","updated_at":"2026-09-30T00:00:00Z","body":"I am not working on this. Should I make a PR if I take it later?"}]"#.into()
                        } else if request.contains("/repos/owner/repo/issues/3")
                            && !request.contains("/timeline")
                        {
                            r#"{"comments":1,"state":"open","locked":false,"assignees":[],"author_association":"CONTRIBUTOR","user":{"login":"issue-author"}}"#.into()
                        } else if request.contains("/repos/owner/repo/issues")
                            && (request.contains("labels=good%20first%20issue")
                                || request.contains("labels=good+first+issue"))
                        {
                            listing
                        } else {
                            response_body(&request, &base_url)
                        };
                        write_response(&mut stream, &body);
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10))
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

#[tokio::test]
async fn decision_selects_boundary_correction_before_visibility_and_reuses_only_identical_input() {
    use issue_finder::decision::contract::Provider;
    use issue_finder::decision::questions::ContributionSignal;
    use issue_finder::decision::{JudgmentSnapshot, JudgmentStatus};
    use issue_finder::recommendation::engine::RecommendationEngine;

    let _env_lock = env_lock::EnvLock::acquire();
    let _auth_guard = github_auth::GitHubAuthGuard::clear();
    let server = start_semantic_mock_github();
    std::env::set_var("ISSUE_FINDER_GITHUB_API_BASE", &server.base_url);
    let _env_guard = EnvGuard;
    let directory = tempdir().unwrap();
    let paths = semantic_test_paths(directory.path());
    let mut config = Config::default();
    config.decision.candidate_budget = 3;
    let provider = Arc::new(InspectableDecisionProvider {
        requests: Mutex::new(Vec::new()),
        fail: false,
        version: "v1",
    });
    let options = ScoutOptions {
        include_filtered: false,
        record_exposure: false,
        source: RecommendationEventSource::ToolScout,
    };
    let scope = DiscoveryScope::repository(RepositoryScope::parse("owner/repo").unwrap());
    let engine = RecommendationEngine::with_decision_provider(&paths, &config, provider.clone());
    let selected = engine
        .scout(1, false, options, scope.clone())
        .await
        .unwrap();
    assert_eq!(selected.ranked.len(), 1);
    assert_eq!(
        selected.ranked[0].issue.number, 3,
        "a good candidate beyond the presentation cutoff must reach decision model"
    );
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        3,
        "the bounded pool must be classified before one result is selected"
    );
    let correction = &selected.ranked[0];
    assert_eq!(
        correction.enriched_issue.competition.fix_submitted_comments,
        0
    );
    assert_eq!(correction.enriched_issue.competition.working_comments, 0);
    assert!(!correction
        .recommendation
        .reasons
        .iter()
        .any(|reason| reason.contains("open PR") || reason.contains("thin task")));
    let snapshot = correction.enriched_issue.decision.as_ref().unwrap();
    assert_eq!(snapshot.status, JudgmentStatus::Completed);
    let answers = snapshot.answers.as_ref().unwrap();
    assert_eq!(
        answers.contribution_signal,
        Some(ContributionSignal::InterestOnly)
    );
    assert_eq!(answers.task_shape, None);
    let evidence = snapshot.evidence.as_ref().unwrap();
    assert!(evidence.comments.comments[0]
        .body
        .text
        .contains("Should I make a PR"));
    assert_eq!(
        evidence.comments.comments[0].author_association,
        "CONTRIBUTOR"
    );
    let saved: JudgmentSnapshot =
        serde_json::from_slice(&std::fs::read(snapshot.snapshot_path.as_ref().unwrap()).unwrap())
            .unwrap();
    assert_eq!(saved.input_id, snapshot.input_id);
    assert_eq!(
        saved.provider_fingerprint.as_deref(),
        Some(provider.fingerprint().as_str())
    );
    assert_eq!(saved.response, snapshot.response);
    let requests = provider.requests.lock().unwrap().clone();
    let first = requests
        .iter()
        .find(|request| request.candidate_id.ends_with(":3"))
        .unwrap();
    assert!(!serde_json::to_string(first)
        .unwrap()
        .contains("claim_comments"));
    let cached = engine
        .scout(1, false, options, scope.clone())
        .await
        .unwrap();
    assert_eq!(cached.ranked[0].issue.number, 3);
    assert!(
        cached.ranked[0]
            .enriched_issue
            .decision
            .as_ref()
            .unwrap()
            .cache_hit
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 3);

    let mut changed = config.clone();
    changed
        .profile
        .keywords
        .push("explicit-new-user-interest".into());
    let changed_engine =
        RecommendationEngine::with_decision_provider(&paths, &changed, provider.clone());
    let changed_result = changed_engine
        .scout(1, false, options, scope.clone())
        .await
        .unwrap();
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        6,
        "changed user preferences must invalidate all input-dependent judgments"
    );
    let changed_snapshot = changed_result.ranked[0]
        .enriched_issue
        .decision
        .as_ref()
        .unwrap();
    assert!(!changed_snapshot.cache_hit);
    assert_ne!(changed_snapshot.input_id, snapshot.input_id);
    assert_eq!(
        changed_snapshot.evidence.as_ref().unwrap().material_hash(),
        evidence.material_hash(),
        "only question context changed, not the GitHub material"
    );
    let replaced_provider = Arc::new(InspectableDecisionProvider {
        requests: Mutex::new(Vec::new()),
        fail: false,
        version: "v2",
    });
    let replaced_engine =
        RecommendationEngine::with_decision_provider(&paths, &changed, replaced_provider.clone());
    let replaced = replaced_engine
        .scout(1, false, options, scope)
        .await
        .unwrap();
    let replaced_snapshot = replaced.ranked[0].enriched_issue.decision.as_ref().unwrap();
    assert_eq!(
        replaced_provider.requests.lock().unwrap().len(),
        3,
        "changed provider configuration must invalidate stored judgments"
    );
    assert!(!replaced_snapshot.cache_hit);
    assert_eq!(replaced_snapshot.input_id, changed_snapshot.input_id);
    assert_ne!(
        replaced_snapshot.provider_fingerprint,
        changed_snapshot.provider_fingerprint
    );

    let mut changed_material = replaced_snapshot.evidence.clone().unwrap();
    changed_material.body = issue_finder::decision::evidence::EvidenceText::bounded(
        "In docs/cli.md replace `adress` with `address`; also correct the corresponding heading.",
        12_000,
    );
    let rejudged = issue_finder::decision::judge(
        &paths,
        replaced_provider.as_ref(),
        changed_material.clone(),
        &changed.profile,
        false,
    )
    .await
    .unwrap();
    assert!(!rejudged.cache_hit);
    assert_ne!(rejudged.input_id, replaced_snapshot.input_id);
    assert_eq!(
        replaced_provider.requests.lock().unwrap().len(),
        4,
        "changed material must reach the provider even without a refresh flag"
    );
    let same_material = issue_finder::decision::judge(
        &paths,
        replaced_provider.as_ref(),
        changed_material,
        &changed.profile,
        false,
    )
    .await
    .unwrap();
    assert!(same_material.cache_hit);
    assert_eq!(replaced_provider.requests.lock().unwrap().len(), 4);
    server.join();
}

#[tokio::test]
async fn decision_failure_and_budget_skips_stay_visible_without_keyword_fallback() {
    use issue_finder::decision::JudgmentStatus;
    use issue_finder::recommendation::engine::RecommendationEngine;

    let _env_lock = env_lock::EnvLock::acquire();
    let _auth_guard = github_auth::GitHubAuthGuard::clear();
    let server = start_semantic_mock_github();
    std::env::set_var("ISSUE_FINDER_GITHUB_API_BASE", &server.base_url);
    let _env_guard = EnvGuard;
    let directory = tempdir().unwrap();
    let paths = semantic_test_paths(directory.path());
    let mut config = Config::default();
    config.decision.candidate_budget = 2;
    let provider = Arc::new(InspectableDecisionProvider {
        requests: Mutex::new(Vec::new()),
        fail: true,
        version: "v1",
    });
    let engine = RecommendationEngine::with_decision_provider(&paths, &config, provider.clone());
    let result = engine
        .scout(
            3,
            false,
            ScoutOptions {
                include_filtered: false,
                record_exposure: false,
                source: RecommendationEventSource::ToolScout,
            },
            DiscoveryScope::repository(RepositoryScope::parse("owner/repo").unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(
        result.ranked.len(),
        3,
        "failed semantics must not imply low issue quality or revive keyword hiding"
    );
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        2,
        "each selected issue fails explicitly without retrying or cancelling other slots"
    );
    let statuses: Vec<_> = result
        .ranked
        .iter()
        .map(|candidate| candidate.enriched_issue.decision.as_ref().unwrap().status)
        .collect();
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == JudgmentStatus::Failed)
            .count(),
        2
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == JudgmentStatus::SkippedBudget)
            .count(),
        1
    );
    assert!(result
        .diagnostics
        .stage_errors
        .iter()
        .any(|error| error.contains("authentication_failed")));
    assert!(result.ranked.iter().all(|candidate| candidate
        .enriched_issue
        .decision
        .as_ref()
        .unwrap()
        .answers
        .is_none()));
    server.join();
}

#[test]
fn decision_concurrency_defaults_to_four_and_has_no_product_upper_limit() {
    use issue_finder::config::DecisionConfig;
    assert_eq!(DecisionConfig::default().concurrency, 4);
    for value in [1, 4, 9, 1024, 100_000] {
        let config: DecisionConfig = toml::from_str(&format!("concurrency = {value}")).unwrap();
        assert!(config.validate().is_ok());
    }
    let config: DecisionConfig = toml::from_str("concurrency = 0").unwrap();
    assert!(config.validate().is_err());
    assert!(toml::from_str::<DecisionConfig>("concurrency = -1").is_err());
}

struct ConcurrentSemanticProvider {
    active: std::sync::atomic::AtomicUsize,
    peak: std::sync::atomic::AtomicUsize,
    events: Mutex<Vec<(String, bool)>>,
    close_first: Arc<AtomicBool>,
}

impl issue_finder::decision::contract::Provider for ConcurrentSemanticProvider {
    fn fingerprint(&self) -> String {
        "offline-concurrent-v2".into()
    }
    fn decide<'a>(
        &'a self,
        request: &'a issue_finder::decision::contract::DecisionRequest,
    ) -> issue_finder::decision::contract::DecisionFuture<'a> {
        use issue_finder::decision::contract::*;
        Box::pin(async move {
            assert_eq!(request.questions.len(), 7);
            assert!(!request
                .questions
                .iter()
                .any(|question| question.id == "task_shape"));
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            self.events
                .lock()
                .unwrap()
                .push((request.candidate_id.clone(), true));
            let number: u64 = request
                .candidate_id
                .rsplit(':')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            tokio::time::sleep(Duration::from_millis(120 + (6 - number) * 10)).await;
            if number == 1 {
                self.close_first.store(true, Ordering::SeqCst);
            }
            self.events
                .lock()
                .unwrap()
                .push((request.candidate_id.clone(), false));
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(DecisionResponse {
                candidate_id: request.candidate_id.clone(),
                input_id: request.input_id.clone(),
                status: ResponseStatus::Complete,
                answers: request
                    .questions
                    .iter()
                    .map(|question| QuestionResponse {
                        question_id: question.id.clone(),
                        status: AnswerStatus::Answered,
                        answer: Some(Answer::Choice(
                            match question.id.as_str() {
                                "task_type" => "concrete_change",
                                "description_quality" => "clear",
                                "contribution_signal" => "not_observed",
                                "scope" => "bounded",
                                "preference_match" => "matches",
                                "verification_clues" => "present",
                                "maintainer_signal" => "not_observed",
                                _ => panic!("unexpected question"),
                            }
                            .into(),
                        )),
                        probabilities: None,
                    })
                    .collect(),
                metadata: ProviderMetadata {
                    provider: "offline".into(),
                    model: "fixture".into(),
                    reasoning_effort: "none".into(),
                    duration_ms: Some(150),
                    usage: None,
                    ..ProviderMetadata::default()
                },
            })
        })
    }
}

#[tokio::test]
async fn decision_runs_four_slots_refills_and_rechecks_facts_before_returning_recommendations() {
    use issue_finder::decision::replay::ScoutReplay;
    use issue_finder::recommendation::engine::RecommendationEngine;
    let _lock = env_lock::EnvLock::acquire();
    let _auth = github_auth::GitHubAuthGuard::clear();
    let close_first = Arc::new(AtomicBool::new(false));
    let server = start_semantic_mock_github_with(5, close_first.clone());
    std::env::set_var("ISSUE_FINDER_GITHUB_API_BASE", &server.base_url);
    let _guard = EnvGuard;
    let directory = tempdir().unwrap();
    let paths = semantic_test_paths(directory.path());
    let mut config = Config::default();
    config.decision.candidate_budget = 5;
    let provider = Arc::new(ConcurrentSemanticProvider {
        active: 0.into(),
        peak: 0.into(),
        events: Mutex::new(Vec::new()),
        close_first,
    });
    let engine = RecommendationEngine::with_decision_provider(&paths, &config, provider.clone());
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        engine.scout(
            3,
            false,
            ScoutOptions {
                include_filtered: false,
                record_exposure: false,
                source: RecommendationEventSource::ToolScout,
            },
            DiscoveryScope::repository(RepositoryScope::parse("owner/repo").unwrap()),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(provider.peak.load(Ordering::SeqCst), 4);
    let events = provider.events.lock().unwrap();
    assert_eq!(events.iter().filter(|(_, started)| *started).count(), 5);
    let fifth_start = events
        .iter()
        .enumerate()
        .filter(|(_, (_, start))| *start)
        .nth(4)
        .unwrap()
        .0;
    assert!(events[..fifth_start].iter().any(|(_, start)| !start));
    assert_eq!(result.ranked.len(), 3);
    assert!(
        result.ranked.iter().all(|item| item.issue.number != 1),
        "issue closed while screening must be replaced before the user sees recommendations"
    );
    assert!(result.ranked.iter().all(|item| item
        .enriched_issue
        .availability
        .as_ref()
        .unwrap()
        .depth
        == issue_finder::availability::AvailabilityDepth::Final));
    let mut legacy = serde_json::to_value(&result.diagnostics).unwrap();
    let object = legacy.as_object_mut().unwrap();
    let execution_json = object.remove("decisionExecution").unwrap();
    let replay_path_json = object.remove("decisionReplayPath").unwrap();
    object.insert("system1Execution".into(), execution_json);
    object.insert("system1ReplayPath".into(), replay_path_json);
    let restored: issue_finder::discovery::DiscoveryDiagnostics =
        serde_json::from_value(legacy).unwrap();
    assert_eq!(restored, result.diagnostics);
    let execution = result.diagnostics.decision_execution.as_ref().unwrap();
    assert_eq!(execution.peak_in_flight, 4);
    assert_eq!(execution.tasks.len(), 5);
    assert!(execution.tasks.iter().any(|task| task.queue_wait_ms >= 100));
    let replay = ScoutReplay::load(std::path::Path::new(
        result.diagnostics.decision_replay_path.as_ref().unwrap(),
    ))
    .unwrap();
    let closed = replay
        .ranked
        .iter()
        .find(|item| item.issue.number == 1)
        .unwrap();
    assert_eq!(
        closed
            .enriched_issue
            .availability
            .as_ref()
            .unwrap()
            .issue_state
            .as_deref(),
        Some("closed")
    );
    let reranked = replay.replay().unwrap();
    assert_eq!(
        reranked
            .iter()
            .map(|item| (
                &item.issue.repo_full_name,
                item.issue.number,
                item.score,
                item.recommendation.visibility
            ))
            .collect::<Vec<_>>(),
        replay
            .ranked
            .iter()
            .map(|item| (
                &item.issue.repo_full_name,
                item.issue.number,
                item.score,
                item.recommendation.visibility
            ))
            .collect::<Vec<_>>()
    );
    server.join();
}
