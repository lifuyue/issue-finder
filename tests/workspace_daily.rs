use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use patchbay_cli::config::Config;
use patchbay_cli::github::GitHubIssue;
use patchbay_cli::github_enrichment::EnrichedIssue;
use patchbay_cli::handoff::handoff_id;
use patchbay_cli::inbox::{load_index, InboxStatus};
use patchbay_cli::paths::PatchbayPaths;
use patchbay_cli::value_scoring::{
    RankedValueIssue, RecommendationCategory, ScoreBand, ValueAssessment,
};
use patchbay_cli::workflow::{self, prepare_value_issue};
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
    let ranked = vec![ranked_value(success, 72, 60), ranked_value(failure, 68, 55)];

    let (report, _) = workflow::daily_from_ranked(&paths, &config, ranked, 2, 2)
        .await
        .unwrap();

    assert_eq!(report.prepared.len(), 1);
    assert_eq!(report.failed.len(), 1);
    assert!(PathBuf::from(&report.prepared[0].codex_md_path).exists());

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
async fn daily_skips_low_attention_triage_candidates() {
    if !git_available() {
        return;
    }

    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let remote = create_remote_repo(dir.path());
    clone_into_workspace(&remote, &paths, "owner/lowgate");
    clone_into_workspace(&remote, &paths, "owner/success");

    let config = Config::default();
    let low_gate = ranked_value(issue("owner/lowgate", 5), 35, 20);
    let success = ranked_value(issue("owner/success", 6), 70, 55);
    let (report, _) = workflow::daily_from_ranked(&paths, &config, vec![low_gate, success], 2, 1)
        .await
        .unwrap();

    assert_eq!(report.prepared.len(), 1);
    assert_eq!(report.prepared[0].repo_full_name, "owner/success");
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
        ranked_value(failwrite, 70, 55),
        ranked_value(success, 69, 55),
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

#[tokio::test]
async fn explicit_prepare_writes_low_execution_warning_and_assessment_fields() {
    if !git_available() {
        return;
    }

    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let remote = create_remote_repo(dir.path());
    clone_into_workspace(&remote, &paths, "owner/explicit");

    let config = Config::default();
    let ranked = ranked_value(issue("owner/explicit", 7), 80, 20);
    let outcome = prepare_value_issue(&paths, &config, ranked, true)
        .await
        .unwrap();
    let workflow::PrepareOutcome::Prepared(item) = outcome else {
        panic!("expected prepared outcome");
    };

    let handoff = fs::read_to_string(item.handoff_json_path).unwrap();
    assert!(handoff.contains("\"value_assessment\""));
    assert!(handoff.contains("\"final_rank_score\""));
    assert!(handoff.contains("\"attention_score\""));
    assert!(handoff.contains("\"execution_score\""));
    assert!(handoff.contains("\"evidence_pack\""));
    assert!(handoff.contains("\"context_pack\""));
    assert!(handoff.contains("Explicit prepare bypassed low execution score 20"));
    let codex = fs::read_to_string(item.codex_md_path).unwrap();
    assert!(codex.contains("Use the local skill at:"));
    assert!(codex.contains("context/entry.md"));
    assert!(codex.contains("context/safety.md"));
}

#[tokio::test]
async fn prepare_preserves_llm_summary_enhancement() {
    if !git_available() {
        return;
    }

    let (base_url, handle) = start_llm_server();
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let remote = create_remote_repo(dir.path());
    clone_into_workspace(&remote, &paths, "owner/llm");

    let mut config = Config::default();
    config.llm.enabled = true;
    config.llm.base_url = base_url;
    config.llm.api_key = "test-key".to_string();
    let ranked = ranked_value(issue("owner/llm", 8), 80, 60);

    let outcome = prepare_value_issue(&paths, &config, ranked, true)
        .await
        .unwrap();
    handle.join().unwrap();
    let workflow::PrepareOutcome::Prepared(item) = outcome else {
        panic!("expected prepared outcome");
    };

    let handoff = fs::read_to_string(item.handoff_json_path).unwrap();
    assert!(handoff.contains("\"llm_enhancement\""));
    assert!(handoff.contains("\"status\": \"success\""));
    let markdown = fs::read_to_string(item.handoff_md_path).unwrap();
    assert!(markdown.contains("## LLM Summary"));
    assert!(markdown.contains("Mock LLM summary"));
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

fn ranked_value(
    issue: GitHubIssue,
    final_rank_score: i32,
    execution_score: i32,
) -> RankedValueIssue {
    let enriched_issue = EnrichedIssue::from_issue(&issue);
    let attention_score = final_rank_score;
    let recommendation_category = if attention_score < 60 && execution_score < 40 {
        RecommendationCategory::NeedsTriage
    } else if attention_score >= 70 && execution_score >= 70 {
        RecommendationCategory::AgentReadyHighValue
    } else if attention_score >= 70 {
        RecommendationCategory::HighAttention
    } else {
        RecommendationCategory::NicheButActionable
    };
    RankedValueIssue {
        issue,
        score: final_rank_score,
        value_assessment: ValueAssessment {
            final_rank_score,
            attention_score,
            execution_score,
            profile_fit_score: 40,
            risk_penalty: if recommendation_category == RecommendationCategory::NeedsTriage {
                50
            } else {
                5
            },
            recommendation_category,
            attention_band: if attention_score >= 70 {
                ScoreBand::High
            } else if attention_score >= 40 {
                ScoreBand::Medium
            } else {
                ScoreBand::Low
            },
            execution_band: if execution_score >= 70 {
                ScoreBand::High
            } else if execution_score >= 40 {
                ScoreBand::Medium
            } else {
                ScoreBand::Low
            },
            signals: Vec::new(),
            risk_tags: Vec::new(),
            missing_evidence: Vec::new(),
            explanation: vec!["test recommendation evidence".to_string()],
        },
        enriched_issue,
        explanation: vec!["test recommendation evidence".to_string()],
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

fn start_llm_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let started = Instant::now();
        let mut served = 0usize;
        while served < 2 && started.elapsed() < Duration::from_secs(5) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buffer = [0u8; 4096];
                    let _ = stream.read(&mut buffer).unwrap_or(0);
                    let body = r#"{"choices":[{"message":{"content":"Mock LLM summary"}}]}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    stream.write_all(response.as_bytes()).unwrap();
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
