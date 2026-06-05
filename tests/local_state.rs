use std::path::PathBuf;

use chrono::Utc;
use issue_finder::github::GitHubIssue;
use issue_finder::handoff::{write_handoff, Handoff};
use issue_finder::inbox::{load_index, upsert_ready, InboxStatus};
use issue_finder::paths::IssueFinderPaths;
use issue_finder::recommendation::{load_state_map, IssueKey, RecommendationEventType};
use issue_finder::repo_scan::{CandidateFile, RepoScan, ValidationCommand};
use issue_finder::report::{write_daily_report, DailyReport, PreparedReportItem};
use issue_finder::workflow;
use issue_finder::workspace::{PreparedWorkspace, WorkspaceInfo};
use tempfile::tempdir;

#[test]
fn writes_handoff_inbox_and_report_under_issue_finder_home() {
    let dir = tempdir().unwrap();
    let paths = IssueFinderPaths {
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
            branch: "issue-finder/123-fix-accessible-button-label".to_string(),
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

    let item_dir = PathBuf::from(&written.dir);
    let handoff_json = std::fs::read_to_string(&written.handoff_json_path).unwrap();
    let handoff_value = serde_json::from_str::<serde_json::Value>(&handoff_json).unwrap();
    assert_eq!(handoff_value["version"], 1);
    assert_eq!(handoff_value["context_pack"]["version"], 1);
    assert_eq!(
        handoff_value["context_pack"]["kind"],
        "issue_finder_progressive_handoff_pack"
    );
    assert_eq!(handoff_value["context_pack"]["entrypoint"], "./codex.md");
    assert!(handoff_value["context_pack"]["body"].is_null());
    assert!(handoff_value["value_assessment"]["final_rank_score"].is_number());
    assert!(handoff_value["value_assessment"]["value_score"].is_null());
    assert!(handoff_value["value_assessment"]["opportunity_type"].is_null());

    let codex = std::fs::read_to_string(&written.codex_md_path).unwrap();
    assert!(codex.contains("Use the local skill at:"));
    assert!(codex.contains(item_dir.to_string_lossy().as_ref()));
    assert!(codex.contains("/context/entry.md"));
    assert!(codex.contains("/context/safety.md"));
    assert!(codex.contains("Do not read every context file at once"));

    let entry = std::fs::read_to_string(item_dir.join("context/entry.md")).unwrap();
    assert!(entry.contains("## Next Reads"));
    assert!(!entry.contains("Expected a useful label in src/button.rs"));
    let value = std::fs::read_to_string(item_dir.join("context/value.md")).unwrap();
    assert!(value.contains("# Recommendation Assessment"));
    assert!(value.contains("## Evidence Pack: High Attention"));
    let repo = std::fs::read_to_string(item_dir.join("context/repo.md")).unwrap();
    assert!(repo.contains("src/button.rs"));
    let validation = std::fs::read_to_string(item_dir.join("context/validation.md")).unwrap();
    assert!(validation.contains("`cargo test`"));
    let safety = std::fs::read_to_string(item_dir.join("context/safety.md")).unwrap();
    assert!(
        safety.contains("Issue Finder does not install dependencies, commit, push, or create PRs")
    );
    let probe = std::fs::read_to_string(item_dir.join("context/probe.md")).unwrap();
    assert!(probe.contains("# Probe"));
    let policy = serde_json::from_str::<serde_json::Value>(
        &std::fs::read_to_string(item_dir.join("agent-policy.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(policy["kind"], "issue_finder_agent_policy");
    assert!(
        policy["permission_profile"]["filesystem"]["protected_roots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|root| root.as_str().unwrap().ends_with("/.git"))
    );
    let probe_json = serde_json::from_str::<serde_json::Value>(
        &std::fs::read_to_string(item_dir.join("probe.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(probe_json["kind"], "issue_finder_probe_pack");
    assert!(item_dir.join("prepare-events.jsonl").exists());
    let skill =
        std::fs::read_to_string(item_dir.join(".agents/skills/issue-finder/SKILL.md")).unwrap();
    assert!(skill.starts_with("# issue-finder"));
    assert!(skill.contains("Read context/entry.md and context/safety.md first"));
    let refs =
        std::fs::read_to_string(item_dir.join(".agents/skills/issue-finder/refs.json")).unwrap();
    let refs = serde_json::from_str::<serde_json::Value>(&refs).unwrap();
    assert_eq!(refs["skill"], "issue-finder");
    assert_eq!(refs["default_load"][0], "context/entry.md");

    let index = load_index(&paths).unwrap();
    assert_eq!(index.items.len(), 1);
    assert_eq!(index.items[0].status, InboxStatus::Ready);
    assert_eq!(index.items[0].codex_md_path, written.codex_md_path);
    assert_eq!(index.items[0].agent_policy_path, written.agent_policy_path);
    assert!(workflow::read_handoff(&paths, &written.id, true)
        .unwrap()
        .contains("\"kind\": \"issue_finder_handoff\""));
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
            final_rank_score: 88,
            feed_score: 588,
            freshness_boost: 0,
            feedback_penalty: 0,
            reactivation_boost: 0,
            recommendation_visibility: "visible".to_string(),
            recommendation_reasons: Vec::new(),
            attention_score: 80,
            execution_score: 70,
            profile_fit_score: 40,
            risk_penalty: 5,
            recommendation_category: "high_value_ready".to_string(),
            risk_tags: Vec::new(),
            why_it_is_worth_doing: "High attention evidence".to_string(),
            biggest_risk: "none".to_string(),
            missing_evidence: Vec::new(),
            handoff_json_path: written.handoff_json_path,
            handoff_md_path: written.handoff_md_path,
            codex_md_path: written.codex_md_path,
            agent_policy_path: written.agent_policy_path,
            probe_json_path: written.probe_json_path,
            prepare_events_path: written.prepare_events_path,
            readiness_score: 80,
            readiness_band: "high".to_string(),
            probe_status: "not_run".to_string(),
            probe_warnings: Vec::new(),
        }],
        failed: Vec::new(),
    };
    let report_path = write_daily_report(&paths, &report).unwrap();
    assert!(std::fs::read_to_string(report_path)
        .unwrap()
        .contains("Codex: "));
}

#[test]
fn feedback_events_derive_state_for_non_inbox_issue() {
    let dir = tempdir().unwrap();
    let paths = IssueFinderPaths {
        home: dir.path().to_path_buf(),
        config: dir.path().join("config.toml"),
        cache_dir: dir.path().join("cache"),
        workspaces_dir: dir.path().join("workspaces"),
        inbox_dir: dir.path().join("inbox"),
        reports_dir: dir.path().join("reports"),
    };
    paths.ensure_layout().unwrap();

    workflow::record_feedback(&paths, "owner/repo#77", RecommendationEventType::Read).unwrap();
    workflow::record_feedback(&paths, "owner/repo#77", RecommendationEventType::Dismissed).unwrap();
    let state = load_state_map(&paths)
        .unwrap()
        .remove(&IssueKey::new("owner/repo", 77))
        .unwrap();
    assert_eq!(state.read_count, 1);
    assert!(state.dismissed);

    workflow::record_feedback(&paths, "owner/repo#77", RecommendationEventType::Restored).unwrap();
    let rendered = workflow::render_feedback_state(&paths, "owner/repo#77").unwrap();
    assert!(rendered.contains("shown: 0"));
    assert!(rendered.contains("dismissed: false"));
}
