use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use patchbay_cli::config::Config;
use patchbay_cli::github::GitHubIssue;
use patchbay_cli::handoff::handoff_id;
use patchbay_cli::inbox::{load_index, InboxStatus};
use patchbay_cli::paths::PatchbayPaths;
use patchbay_cli::scoring::score_issue;
use patchbay_cli::workflow;
use patchbay_cli::workspace::{git_available, prepare_workspace};
use tempfile::tempdir;

#[test]
fn prepares_existing_local_git_workspace() {
    if !git_available() {
        return;
    }

    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let remote = create_remote_repo(dir.path());
    clone_into_workspace(&remote, &paths, "owner/success");

    let issue = issue("owner/success", 11);
    let workspace = prepare_workspace(&paths, &issue).unwrap();

    assert_eq!(workspace.info.default_branch, "main");
    assert_eq!(workspace.info.branch, "patchbay/11-fix-rust-cli-parser");
    assert!(!workspace.info.dirty);
    assert!(workspace
        .scan
        .candidate_files
        .iter()
        .any(|file| file.path == "src/lib.rs"));
    assert!(workspace
        .scan
        .validation_commands
        .iter()
        .any(|command| command.command == "cargo test"));
}

#[test]
fn workspace_prepare_fails_when_patchbay_branch_cannot_be_created() {
    if !git_available() {
        return;
    }

    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let remote = create_remote_repo(dir.path());
    clone_into_workspace(&remote, &paths, "owner/conflict");

    let workspace = paths.workspace_path_for("owner/conflict");
    run_git(&workspace, &["checkout", "-b", "patchbay"]);
    run_git(&workspace, &["checkout", "main"]);

    let error = prepare_workspace(&paths, &issue("owner/conflict", 12))
        .unwrap_err()
        .to_string();
    assert!(error.contains("git checkout -b patchbay/12-fix-rust-cli-parser"));
}

#[tokio::test]
async fn daily_continues_after_single_prepare_failure() {
    if !git_available() {
        return;
    }

    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let remote = create_remote_repo(dir.path());
    clone_into_workspace(&remote, &paths, "owner/success");
    fs::create_dir_all(paths.workspace_path_for("owner/fail")).unwrap();

    let config = Config::default();
    let success = issue("owner/success", 1);
    let failure = issue("owner/fail", 2);
    let ranked = vec![
        score_issue(success, &config.profile),
        score_issue(failure, &config.profile),
    ];

    let (report, _) = workflow::daily_from_ranked(&paths, &config, ranked, 2, 2)
        .await
        .unwrap();

    assert_eq!(report.prepared.len(), 1);
    assert_eq!(report.failed.len(), 1);

    let index = load_index(&paths).unwrap();
    assert!(index
        .items
        .iter()
        .any(|item| item.status == InboxStatus::Ready));
    assert!(index
        .items
        .iter()
        .any(|item| item.status == InboxStatus::PrepareFailed));
}

#[tokio::test]
async fn daily_continues_after_single_output_write_failure() {
    if !git_available() {
        return;
    }

    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let remote = create_remote_repo(dir.path());
    clone_into_workspace(&remote, &paths, "owner/failwrite");
    clone_into_workspace(&remote, &paths, "owner/success");

    let config = Config::default();
    let failwrite = issue("owner/failwrite", 3);
    let success = issue("owner/success", 4);
    fs::write(
        paths.inbox_item_dir(&handoff_id(&failwrite)),
        "not a directory",
    )
    .unwrap();
    let ranked = vec![
        score_issue(failwrite, &config.profile),
        score_issue(success, &config.profile),
    ];

    let (report, report_path) = workflow::daily_from_ranked(&paths, &config, ranked, 2, 2)
        .await
        .unwrap();

    assert_eq!(report.prepared.len(), 1);
    assert_eq!(report.failed.len(), 1);
    assert!(fs::read_to_string(report_path)
        .unwrap()
        .contains("Failed preparation count: 1"));
}

fn test_paths(root: &Path) -> PatchbayPaths {
    PatchbayPaths {
        home: root.join("patchbay-home"),
        config: root.join("patchbay-home/config.toml"),
        cache_dir: root.join("patchbay-home/cache"),
        workspaces_dir: root.join("patchbay-home/workspaces"),
        inbox_dir: root.join("patchbay-home/inbox"),
        reports_dir: root.join("patchbay-home/reports"),
    }
}

fn issue(repo_full_name: &str, number: u64) -> GitHubIssue {
    GitHubIssue {
        id: number,
        number,
        title: "Fix Rust CLI parser".to_string(),
        body: "Expected graceful behavior in src/lib.rs".to_string(),
        labels: vec!["good first issue".to_string()],
        url: format!("https://github.com/{repo_full_name}/issues/{number}"),
        repo_full_name: repo_full_name.to_string(),
        repo_name: repo_full_name.split('/').nth(1).unwrap().to_string(),
        repo_description: "Rust CLI".to_string(),
        repo_stars: 10,
        created_at: Utc::now().to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
    }
}

fn create_remote_repo(root: &Path) -> PathBuf {
    let source = root.join("source");
    let remote = root.join("remote.git");
    fs::create_dir_all(source.join("src")).unwrap();
    fs::write(
        source.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        source.join("src/lib.rs"),
        "pub fn parse() -> bool { true }\n",
    )
    .unwrap();

    run_git(root, &["init", "--bare", remote.to_str().unwrap()]);
    run_git(&source, &["init"]);
    run_git(&source, &["checkout", "-b", "main"]);
    run_git(&source, &["add", "."]);
    run_git(
        &source,
        &[
            "-c",
            "user.name=Patchbay",
            "-c",
            "user.email=patchbay@example.invalid",
            "commit",
            "-m",
            "initial",
        ],
    );
    run_git(
        &source,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    run_git(&source, &["push", "-u", "origin", "main"]);
    run_git(
        root,
        &[
            "--git-dir",
            remote.to_str().unwrap(),
            "symbolic-ref",
            "HEAD",
            "refs/heads/main",
        ],
    );

    remote
}

fn clone_into_workspace(remote: &Path, paths: &PatchbayPaths, repo_full_name: &str) {
    let workspace = paths.workspace_path_for(repo_full_name);
    fs::create_dir_all(workspace.parent().unwrap()).unwrap();
    run_git(
        workspace.parent().unwrap(),
        &[
            "clone",
            remote.to_str().unwrap(),
            workspace.file_name().unwrap().to_str().unwrap(),
        ],
    );
}

fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}
