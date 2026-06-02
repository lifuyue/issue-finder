use chrono::Utc;
use patchbay_cli::github::GitHubIssue;
use patchbay_cli::handoff::{write_handoff, Handoff};
use patchbay_cli::inbox::{load_index, upsert_ready, InboxStatus};
use patchbay_cli::paths::PatchbayPaths;
use patchbay_cli::repo_scan::{CandidateFile, RepoScan, ValidationCommand};
use patchbay_cli::report::{write_daily_report, DailyReport, PreparedReportItem};
use patchbay_cli::workflow;
use patchbay_cli::workspace::{PreparedWorkspace, WorkspaceInfo};
use tempfile::tempdir;

#[test]
fn writes_handoff_inbox_and_report_under_patchbay_home() {
    let dir = tempdir().unwrap();
    let paths = PatchbayPaths {
        home: dir.path().to_path_buf(),
        config: dir.path().join("config.toml"),
        cache_dir: dir.path().join("cache"),
        workspaces_dir: dir.path().join("workspaces"),
        inbox_dir: dir.path().join("inbox"),
        reports_dir: dir.path().join("reports"),
    };
    paths.ensure_layout().unwrap();

    let issue = GitHubIssue {
        id: 1,
        number: 123,
        title: "Fix accessible button label".to_string(),
        body: "Expected a useful label in src/button.rs".to_string(),
        labels: vec!["good first issue".to_string()],
        url: "https://github.com/owner/repo/issues/123".to_string(),
        repo_full_name: "owner/repo".to_string(),
        repo_name: "repo".to_string(),
        repo_description: "Rust CLI".to_string(),
        repo_stars: 42,
        created_at: Utc::now().to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
    };
    let workspace = PreparedWorkspace {
        info: WorkspaceInfo {
            path: paths
                .workspace_path_for("owner/repo")
                .to_string_lossy()
                .to_string(),
            default_branch: "main".to_string(),
            branch: "patchbay/123-fix-accessible-button-label".to_string(),
            dirty: false,
        },
        scan: RepoScan {
            discovered_files: vec!["src/button.rs".to_string()],
            candidate_files: vec![CandidateFile {
                path: "src/button.rs".to_string(),
                reason: "Issue body referenced this path".to_string(),
            }],
            validation_commands: vec![ValidationCommand {
                command: "cargo test".to_string(),
                reason: "Detected Cargo.toml".to_string(),
            }],
            warnings: Vec::new(),
        },
        warnings: Vec::new(),
    };

    let handoff = Handoff::build(&issue, &workspace);
    let written = write_handoff(&paths, &handoff, &issue).unwrap();
    upsert_ready(&paths, &issue, 88, &written).unwrap();

    let index = load_index(&paths).unwrap();
    assert_eq!(index.items.len(), 1);
    assert_eq!(index.items[0].status, InboxStatus::Ready);
    assert!(workflow::read_handoff(&paths, &written.id, true)
        .unwrap()
        .contains("\"kind\": \"patchbay_handoff\""));
    assert!(workflow::read_handoff(&paths, &written.id, false)
        .unwrap()
        .contains("# Handoff: owner/repo#123"));

    let report = DailyReport {
        run_timestamp: Utc::now().to_rfc3339(),
        discovery_count: 1,
        prepared: vec![PreparedReportItem {
            id: written.id,
            repo_full_name: issue.repo_full_name,
            issue_number: issue.number,
            title: issue.title,
            score: 88,
            handoff_json_path: written.handoff_json_path,
            handoff_md_path: written.handoff_md_path,
        }],
        failed: Vec::new(),
    };
    let report_path = write_daily_report(&paths, &report).unwrap();
    assert!(std::fs::read_to_string(report_path)
        .unwrap()
        .contains("Prepared handoff count: 1"));
}
