use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(unix)]
use std::time::{Duration, Instant};

use issue_finder::github::GitHubIssue;
use issue_finder::paths::IssueFinderPaths;
use issue_finder::recommendation::events::{load_events, RecommendationEventType};
use issue_finder::session::{self, FinishOptions, PrepareOptions, TASK_FILE};
use serde_json::json;
use tempfile::{tempdir, TempDir};

struct Fixture {
    _temp: TempDir,
    checkout: PathBuf,
    root: PathBuf,
    paths: IssueFinderPaths,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempdir().unwrap();
        let directory = temp.path().canonicalize().unwrap();
        let checkout = directory.join("checkout");
        fs::create_dir(&checkout).unwrap();
        git(&checkout, &["init", "-b", "main"]);
        git(&checkout, &["config", "user.name", "Test"]);
        git(&checkout, &["config", "user.email", "test@example.invalid"]);
        fs::write(checkout.join("README.md"), "original\n").unwrap();
        fs::write(
            checkout.join("CONTRIBUTING.md"),
            "Read the repository instructions.\n",
        )
        .unwrap();
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
        let remote = directory.join("remote.git");
        git(
            &checkout,
            &[
                "clone",
                "--bare",
                &checkout.to_string_lossy(),
                &remote.to_string_lossy(),
            ],
        );
        // The production fetch stays on its requested GitHub URL; Git routes it to a local fixture.
        git(
            &checkout,
            &[
                "config",
                &format!(
                    "url.{}.insteadOf",
                    url::Url::from_directory_path(&remote).unwrap()
                ),
                "https://github.com/owner/repo.git",
            ],
        );
        let home = directory.join("state");
        let paths = IssueFinderPaths {
            config: home.join("config.toml"),
            cache_dir: home.join("cache"),
            workspaces_dir: home.join("workspaces"),
            inbox_dir: home.join("inbox"),
            reports_dir: home.join("reports"),
            home,
        };
        Self {
            _temp: temp,
            checkout,
            root: directory.join("contributions"),
            paths,
        }
    }

    fn prepare(&self) -> session::PreparedTask {
        session::prepare(
            &self.paths,
            &issue(),
            json!({"comments": [{"body": "Full discussion evidence"}]}),
            &PrepareOptions {
                checkout: self.checkout.clone(),
                workspace_root: self.root.clone(),
            },
        )
        .unwrap()
    }
}

fn issue() -> GitHubIssue {
    GitHubIssue {
        id: 7,
        number: 7,
        title: "Fix parser issue".to_string(),
        body: "Full issue body with steps to reproduce".to_string(),
        labels: vec!["bug".to_string()],
        url: "https://github.com/owner/repo/issues/7".to_string(),
        repo_full_name: "owner/repo".to_string(),
        repo_name: "repo".to_string(),
        repo_description: String::new(),
        repo_stars: 1,
        created_at: "2026-09-01T00:00:00Z".to_string(),
        updated_at: "2026-09-01T00:00:00Z".to_string(),
    }
}

fn git(path: &Path, argv: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(path)
        .args(argv)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {argv:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn options(checks: Vec<Vec<&str>>) -> FinishOptions {
    FinishOptions {
        checks: checks
            .into_iter()
            .map(|argv| argv.into_iter().map(str::to_owned).collect())
            .collect(),
        check_timeout_seconds: 5,
        summary: Some("Implementation reviewed by the current agent".to_string()),
    }
}

#[test]
fn clean_checkout_prepares_resumable_task_without_dispatch_or_approval_artifacts() {
    let fixture = Fixture::new();
    let base = git(&fixture.checkout, &["rev-parse", "HEAD"]);
    let prepared = fixture.prepare();
    assert_eq!(
        prepared.task.workspace.path,
        fixture.checkout.to_string_lossy()
    );
    assert_eq!(prepared.task.workspace.base_commit, base);
    assert!(prepared.task.workspace.branch.starts_with("fuyue/issue-7-"));
    assert_eq!(
        prepared.task.evidence["comments"][0]["body"],
        "Full discussion evidence"
    );
    assert!(prepared
        .task
        .repo_scan
        .discovered_files
        .contains(&"CONTRIBUTING.md".to_string()));
    assert!(!fixture.paths.dispatch_dir().exists());
    assert!(!fixture.checkout.join("agent-policy.json").exists());
    let status = session::task_status(&fixture.paths, &fixture.checkout).unwrap();
    assert_eq!(status.status, "in_progress");
    assert!(status.changes.changed_files.is_empty());
    assert_eq!(
        load_events(&fixture.paths).unwrap()[0].event_type,
        RecommendationEventType::Prepared
    );
}

#[test]
fn dirty_checkout_isolates_workspace_and_preserves_user_edits_and_branch() {
    let fixture = Fixture::new();
    fs::write(fixture.checkout.join("README.md"), "user's local changes\n").unwrap();
    let prepared = fixture.prepare();
    let workspace = Path::new(&prepared.task.workspace.path);
    assert_ne!(workspace, fixture.checkout);
    assert!(workspace.starts_with(&fixture.root));
    assert_eq!(
        git(&fixture.checkout, &["branch", "--show-current"]),
        "main"
    );
    assert_eq!(
        fs::read_to_string(fixture.checkout.join("README.md")).unwrap(),
        "user's local changes\n"
    );
    assert_eq!(
        fs::read_to_string(workspace.join("README.md")).unwrap(),
        "original\n"
    );
    assert!(session::task_status(&fixture.paths, workspace).is_ok());
}

#[test]
fn finish_accounts_for_committed_staged_and_untracked_changes_and_is_idempotent() {
    let fixture = Fixture::new();
    fixture.prepare();
    fs::write(fixture.checkout.join("README.md"), "committed fix\n").unwrap();
    git(&fixture.checkout, &["add", "README.md"]);
    git(&fixture.checkout, &["commit", "-m", "fix"]);
    fs::write(fixture.checkout.join("staged.txt"), "staged\n").unwrap();
    git(&fixture.checkout, &["add", "staged.txt"]);
    fs::write(fixture.checkout.join("untracked.txt"), "untracked\n").unwrap();
    let checks = options(vec![vec!["git", "diff", "--check"]]);
    let result = session::finish(&fixture.paths, &fixture.checkout, &checks).unwrap();
    assert_eq!(result.status, "completed");
    assert_eq!(
        result.changes.changed_files,
        ["README.md", "staged.txt", "untracked.txt"]
    );
    assert_eq!(result.changes.staged_files, ["staged.txt"]);
    assert_eq!(result.changes.untracked_files, ["untracked.txt"]);
    assert_eq!(result.checks[0].exit_code, 0);
    assert!(Path::new(&result.result_file).is_file());
    assert!(fixture.checkout.join(TASK_FILE).exists());
    assert_eq!(
        session::finish(&fixture.paths, &fixture.checkout, &checks).unwrap(),
        result
    );
    assert_eq!(
        load_events(&fixture.paths)
            .unwrap()
            .iter()
            .filter(|event| event.event_type == RecommendationEventType::Done)
            .count(),
        1
    );
    fs::write(fixture.checkout.join("untracked.txt"), "new changes\n").unwrap();
    assert_eq!(
        session::task_status(&fixture.paths, &fixture.checkout)
            .unwrap()
            .status,
        "in_progress"
    );
    let next = session::finish(&fixture.paths, &fixture.checkout, &checks).unwrap();
    assert_ne!(next.changes.fingerprint, result.changes.fingerprint);
}

#[test]
#[cfg(unix)]
fn failed_checks_keep_task_resumable_and_never_record_done_feedback() {
    let fixture = Fixture::new();
    fixture.prepare();
    fs::write(fixture.checkout.join("README.md"), "fix\n").unwrap();
    let result = session::finish(
        &fixture.paths,
        &fixture.checkout,
        &options(vec![vec!["sh", "-c", "printf failure >&2; exit 3"]]),
    )
    .unwrap();
    assert_eq!(result.status, "validation_failed");
    assert_eq!(result.checks[0].exit_code, 3);
    assert_eq!(result.checks[0].output_tail, "failure");
    assert_eq!(
        session::task_status(&fixture.paths, &fixture.checkout)
            .unwrap()
            .status,
        "validation_failed"
    );
    assert!(!load_events(&fixture.paths)
        .unwrap()
        .iter()
        .any(|event| event.event_type == RecommendationEventType::Done));
    let retry = session::finish(
        &fixture.paths,
        &fixture.checkout,
        &options(vec![vec!["git", "diff", "--check"]]),
    )
    .unwrap();
    assert_eq!(retry.status, "completed");
}

#[test]
fn resume_rejects_edited_metadata_wrong_branch_and_staged_task_file() {
    let fixture = Fixture::new();
    fixture.prepare();
    let task_file = fixture.checkout.join(TASK_FILE);
    let original = fs::read(&task_file).unwrap();
    let mut altered: serde_json::Value = serde_json::from_slice(&original).unwrap();
    altered["issue"]["number"] = json!(8);
    fs::write(&task_file, serde_json::to_vec(&altered).unwrap()).unwrap();
    assert!(session::task_status(&fixture.paths, &fixture.checkout)
        .unwrap_err()
        .to_string()
        .contains("recorded identity"));
    fs::write(&task_file, &original).unwrap();
    let branch = git(&fixture.checkout, &["branch", "--show-current"]);
    git(&fixture.checkout, &["switch", "main"]);
    assert!(session::task_status(&fixture.paths, &fixture.checkout)
        .unwrap_err()
        .to_string()
        .contains("branch"));
    git(&fixture.checkout, &["switch", &branch]);
    git(&fixture.checkout, &["add", TASK_FILE]);
    assert!(session::finish(
        &fixture.paths,
        &fixture.checkout,
        &options(vec![vec!["git", "diff", "--check"]])
    )
    .unwrap_err()
    .to_string()
    .contains("staged or committed"));
    git(
        &fixture.checkout,
        &["commit", "-m", "accidental task artifact"],
    );
    git(&fixture.checkout, &["rm", "--cached", TASK_FILE]);
    git(&fixture.checkout, &["commit", "-m", "remove artifact"]);
    assert!(session::task_status(&fixture.paths, &fixture.checkout)
        .unwrap_err()
        .to_string()
        .contains("staged or committed"));
}

#[test]
#[cfg(unix)]
fn finish_rejects_task_mutation_by_explicit_validation_command() {
    let fixture = Fixture::new();
    fixture.prepare();
    fs::write(fixture.checkout.join("README.md"), "fix\n").unwrap();
    let error = session::finish(
        &fixture.paths,
        &fixture.checkout,
        &options(vec![vec![
            "sh",
            "-c",
            "printf '{}' > .issue-finder-cli-task.json",
        ]]),
    )
    .unwrap_err();
    assert!(error.to_string().contains("invalid task state JSON"));
    assert!(!load_events(&fixture.paths)
        .unwrap()
        .iter()
        .any(|event| event.event_type == RecommendationEventType::Done));
}

#[test]
#[cfg(unix)]
fn validation_timeout_kills_process_group_and_bounds_output() {
    let fixture = Fixture::new();
    fixture.prepare();
    fs::write(fixture.checkout.join("README.md"), "fix\n").unwrap();
    let mut checks = options(vec![vec![
        "sh",
        "-c",
        "head -c 9000 /dev/zero; sleep 20 & wait",
    ]]);
    checks.check_timeout_seconds = 1;
    let start = Instant::now();
    let result = session::finish(&fixture.paths, &fixture.checkout, &checks).unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(result.status, "validation_failed");
    assert!(result.checks[0].timed_out);
    assert_eq!(result.checks[0].exit_code, 124);
    assert_eq!(result.checks[0].output_tail.len(), 6000);
}

#[test]
fn unchanged_workspace_never_counts_task_artifact_as_a_contribution() {
    let fixture = Fixture::new();
    fixture.prepare();
    let result = session::finish(
        &fixture.paths,
        &fixture.checkout,
        &options(vec![vec!["git", "diff", "--check"]]),
    )
    .unwrap();
    assert_eq!(result.status, "no_changes");
    assert!(result.changes.changed_files.is_empty());
    assert!(!load_events(&fixture.paths)
        .unwrap()
        .iter()
        .any(|event| event.event_type == RecommendationEventType::Done));
}

#[test]
#[cfg(unix)]
fn finish_requires_explicit_checks_and_passes_argv_without_shell_expansion() {
    let fixture = Fixture::new();
    fixture.prepare();
    fs::write(fixture.checkout.join("README.md"), "fix\n").unwrap();
    assert!(
        session::finish(&fixture.paths, &fixture.checkout, &options(vec![]))
            .unwrap_err()
            .to_string()
            .contains("1 to 20")
    );
    let result = session::finish(
        &fixture.paths,
        &fixture.checkout,
        &options(vec![vec!["printf", "%s", "literal; touch injected-file"]]),
    )
    .unwrap();
    assert_eq!(result.checks[0].output_tail, "literal; touch injected-file");
    assert!(!fixture.checkout.join("injected-file").exists());
}

#[test]
fn another_issue_isolates_existing_task_and_uses_remote_default_baseline() {
    let fixture = Fixture::new();
    let first = fixture.prepare();
    let original_base = first.task.workspace.base_commit.clone();
    fs::write(fixture.checkout.join("README.md"), "first issue change\n").unwrap();
    git(&fixture.checkout, &["add", "README.md"]);
    git(
        &fixture.checkout,
        &["commit", "-m", "first issue implementation"],
    );
    session::finish(
        &fixture.paths,
        &fixture.checkout,
        &options(vec![vec!["git", "diff", "--check"]]),
    )
    .unwrap();
    let first_head = git(&fixture.checkout, &["rev-parse", "HEAD"]);
    let mut another_issue = issue();
    another_issue.number = 8;
    another_issue.url = "https://github.com/owner/repo/issues/8".to_string();
    let next = session::prepare(
        &fixture.paths,
        &another_issue,
        json!({}),
        &PrepareOptions {
            checkout: fixture.checkout.clone(),
            workspace_root: fixture.root.clone(),
        },
    )
    .unwrap();
    assert_ne!(next.task.workspace.path, first.task.workspace.path);
    assert_eq!(next.task.workspace.base_commit, original_base);
    assert_eq!(git(&fixture.checkout, &["rev-parse", "HEAD"]), first_head);
    assert_eq!(
        session::task_status(&fixture.paths, &fixture.checkout)
            .unwrap()
            .status,
        "completed"
    );
    assert_eq!(
        fs::read_to_string(Path::new(&next.task.workspace.path).join("README.md")).unwrap(),
        "original\n"
    );
}

#[cfg(unix)]
#[test]
fn preparation_rejects_symlinked_destination_and_root_inside_checkout() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    fs::write(fixture.checkout.join("README.md"), "dirty\n").unwrap();
    let inside = PrepareOptions {
        checkout: fixture.checkout.clone(),
        workspace_root: fixture.checkout.join("contributions"),
    };
    assert!(
        session::prepare(&fixture.paths, &issue(), json!({}), &inside)
            .unwrap_err()
            .to_string()
            .contains("outside existing Git")
    );
    fs::create_dir_all(&fixture.root).unwrap();
    let link = fixture.root.with_file_name("linked-root");
    symlink(&fixture.root, &link).unwrap();
    let symlinked = PrepareOptions {
        checkout: fixture.checkout.clone(),
        workspace_root: link,
    };
    assert!(
        session::prepare(&fixture.paths, &issue(), json!({}), &symlinked)
            .unwrap_err()
            .to_string()
            .contains("symlinked")
    );
    assert_eq!(
        git(&fixture.checkout, &["branch", "--show-current"]),
        "main"
    );
}

#[test]
fn preparation_locks_the_checkout_across_state_roots_before_switching_branches() {
    let fixture = Fixture::new();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(fixture.checkout.join(".git/issue-finder-session.lock"))
        .unwrap();
    lock.lock().unwrap();
    let error = session::prepare(
        &fixture.paths,
        &issue(),
        json!({}),
        &PrepareOptions {
            checkout: fixture.checkout.clone(),
            workspace_root: fixture.root.clone(),
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("Another Issue Finder operation"));
    assert_eq!(
        git(&fixture.checkout, &["branch", "--show-current"]),
        "main"
    );
    assert!(!fixture.checkout.join(TASK_FILE).exists());
    assert!(!fixture.paths.sessions_dir().exists());
    drop(lock);
    // The persistent lock file does not represent a stale operation.
    let prepared = fixture.prepare();
    assert_eq!(
        prepared.task.workspace.path,
        fixture.checkout.to_string_lossy()
    );
    assert!(session::task_status(&fixture.paths, &fixture.checkout).is_ok());
}

#[test]
fn preparation_serializes_a_shared_contribution_root_before_creating_state() {
    let fixture = Fixture::new();
    fs::create_dir_all(&fixture.root).unwrap();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(fixture.root.join(".issue-finder-prepare.lock"))
        .unwrap();
    lock.lock().unwrap();
    let error = session::prepare(
        &fixture.paths,
        &issue(),
        json!({}),
        &PrepareOptions {
            checkout: fixture.checkout.clone(),
            workspace_root: fixture.root.clone(),
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("Another Issue Finder operation"));
    assert_eq!(
        git(&fixture.checkout, &["branch", "--show-current"]),
        "main"
    );
    assert!(!fixture.paths.sessions_dir().exists());
    drop(lock);
    assert!(fixture.prepare().task_file.ends_with(TASK_FILE));
}

#[test]
fn cached_source_uses_the_same_lock_as_an_explicit_checkout_in_another_root() {
    let fixture = Fixture::new();
    let cache = fixture.root.join("repositories/owner/repo");
    fs::create_dir_all(cache.parent().unwrap()).unwrap();
    fs::rename(&fixture.checkout, &cache).unwrap();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(cache.join(".git/issue-finder-session.lock"))
        .unwrap();
    lock.lock().unwrap();
    // A caller outside the repository reuses the cache while a different
    // caller already holds it as an explicit checkout for another root/home.
    let options = PrepareOptions {
        checkout: fixture._temp.path().to_path_buf(),
        workspace_root: fixture.root.clone(),
    };
    let error = session::prepare(&fixture.paths, &issue(), json!({}), &options).unwrap_err();
    assert!(error.to_string().contains("Another Issue Finder operation"));
    assert_eq!(git(&cache, &["branch", "--show-current"]), "main");
    assert!(!fixture.paths.sessions_dir().exists());
    drop(lock);
    let prepared = session::prepare(&fixture.paths, &issue(), json!({}), &options).unwrap();
    assert!(session::task_status(&fixture.paths, Path::new(&prepared.task.workspace.path)).is_ok());
}

#[test]
#[cfg(unix)]
fn concurrent_finish_never_runs_checks_twice_or_duplicates_completion_feedback() {
    let fixture = Fixture::new();
    fixture.prepare();
    fs::write(fixture.checkout.join("README.md"), "fix\n").unwrap();
    let marker = fixture.root.join("check-started");
    let release = fixture.root.join("check-release");
    let count = fixture.root.join("check-count");
    let checks = FinishOptions {
        checks: vec![vec![
            "sh".into(), "-c".into(),
            "printf started > \"$1\"; while [ ! -f \"$2\" ]; do sleep 0.02; done; printf run >> \"$3\"".into(),
            "check".into(), marker.to_string_lossy().into_owned(),
            release.to_string_lossy().into_owned(), count.to_string_lossy().into_owned(),
        ]],
        check_timeout_seconds: 5,
        summary: Some("Reviewed fix".into()),
    };
    std::thread::scope(|scope| {
        let running = scope.spawn(|| session::finish(&fixture.paths, &fixture.checkout, &checks));
        let started = Instant::now();
        while !marker.exists() && started.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists(), "first validation did not start");
        let error = session::finish(&fixture.paths, &fixture.checkout, &checks).unwrap_err();
        assert!(error.to_string().contains("Another Issue Finder operation"));
        fs::write(&release, "continue").unwrap();
        assert_eq!(running.join().unwrap().unwrap().status, "completed");
    });
    let retry = session::finish(&fixture.paths, &fixture.checkout, &checks).unwrap();
    assert_eq!(retry.status, "completed");
    assert!(retry.feedback_recorded);
    assert_eq!(fs::read_to_string(count).unwrap(), "run");
    assert_eq!(
        load_events(&fixture.paths)
            .unwrap()
            .iter()
            .filter(|event| event.event_type == RecommendationEventType::Done)
            .count(),
        1
    );
}

#[test]
fn completed_result_retries_failed_feedback_without_duplicating_a_recorded_event() {
    let fixture = Fixture::new();
    fixture.prepare();
    fs::write(fixture.checkout.join("README.md"), "fix\n").unwrap();
    let event_path = fixture.paths.recommendation_events_path();
    let original_events = fs::read(&event_path).unwrap();
    fs::remove_file(&event_path).unwrap();
    fs::create_dir(&event_path).unwrap();
    let checks = options(vec![vec!["git", "diff", "--check"]]);
    let first = session::finish(&fixture.paths, &fixture.checkout, &checks).unwrap();
    assert_eq!(first.status, "completed");
    assert!(!first.feedback_recorded);
    assert!(first
        .warnings
        .iter()
        .any(|warning| warning.contains("feedback failed")));
    fs::remove_dir(&event_path).unwrap();
    fs::write(&event_path, original_events).unwrap();
    let retry = session::finish(&fixture.paths, &fixture.checkout, &checks).unwrap();
    assert!(retry.feedback_recorded);
    assert_eq!(retry.finished_at, first.finished_at);
    assert!(retry.warnings.is_empty());

    // Simulate exit after the event append but before its projection flag was saved.
    let mut unacknowledged = retry.clone();
    unacknowledged.feedback_recorded = false;
    fs::write(
        &retry.result_file,
        serde_json::to_vec_pretty(&unacknowledged).unwrap(),
    )
    .unwrap();
    let reconciled = session::finish(&fixture.paths, &fixture.checkout, &checks).unwrap();
    assert!(reconciled.feedback_recorded);
    assert_eq!(
        load_events(&fixture.paths)
            .unwrap()
            .iter()
            .filter(|event| event.event_type == RecommendationEventType::Done)
            .count(),
        1
    );
}
