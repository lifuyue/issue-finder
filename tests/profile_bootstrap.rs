use std::fs;
use std::process::Command;

use issue_finder::profile_bootstrap::bootstrap_profile;
use serde_json::json;
use tempfile::tempdir;

#[test]
fn profile_bootstrap_scans_full_supported_sources_and_drafts_profile() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let cli_project = home.join("Projects/cli-tool");
    let web_project = home.join("Projects/web-ui");
    fs::create_dir_all(&cli_project).unwrap();
    fs::create_dir_all(&web_project).unwrap();
    fs::create_dir_all(home.join(".codex/memories")).unwrap();

    fs::write(
        cli_project.join("Cargo.toml"),
        r#"
[package]
name = "cli-tool"
version = "0.1.0"

[dependencies]
clap = "4"
tokio = "1"
serde = "1"
"#,
    )
    .unwrap();
    fs::write(
        web_project.join("package.json"),
        r#"{"dependencies":{"react":"latest","vite":"latest","typescript":"latest"},"devDependencies":{"playwright":"latest"}}"#,
    )
    .unwrap();

    let session_index = [
        json!({
            "cwd": cli_project,
            "title": "Rust CLI parser work",
            "timestamp": "2026-06-01T00:00:00Z"
        })
        .to_string(),
        json!({
            "workspacePath": web_project,
            "summary": "React UI component testing",
            "timestamp": "2026-06-02T00:00:00Z"
        })
        .to_string(),
        json!({
            "currentWorkingDirectory": home.join("Projects/cli-tool"),
            "title": "Cargo tokio developer tools",
            "timestamp": "2026-06-03T00:00:00Z"
        })
        .to_string(),
    ]
    .join("\n");
    fs::write(home.join(".codex/session_index.jsonl"), session_index).unwrap();

    fs::write(
        home.join(".codex/history.jsonl"),
        format!(
            "not json\n{}\n",
            json!({
                "cwd": web_project,
                "title": "Vite frontend testing",
                "timestamp": "2026-06-04T00:00:00Z"
            })
        ),
    )
    .unwrap();

    fs::write(
        home.join(".codex/memories/profile.md"),
        format!(
            "Current project {} focuses on Rust CLI developer-tools and MCP.\n",
            home.join("Projects/cli-tool").display()
        ),
    )
    .unwrap();

    let report = bootstrap_profile(home).unwrap();
    assert_eq!(report.kind, "issue_finder_profile_bootstrap_report");
    assert_eq!(report.scan_scope.scan_depth, "root_manifest_only");
    assert!(report.scan_scope.full_supported_source_scan);
    assert_eq!(report.scan_scope.conversation_body_mode, "disabled");

    let session_source = report
        .agent_sources
        .iter()
        .find(|source| source.path.ends_with(".codex/session_index.jsonl"))
        .unwrap();
    assert_eq!(session_source.records_seen, 3);
    assert_eq!(session_source.records_parsed, 3);
    let history_source = report
        .agent_sources
        .iter()
        .find(|source| source.path.ends_with(".codex/history.jsonl"))
        .unwrap();
    assert_eq!(history_source.records_seen, 2);
    assert_eq!(history_source.records_parsed, 1);
    assert!(history_source
        .warnings
        .iter()
        .any(|warning| warning.code == "invalid_jsonl_record"));

    assert_eq!(report.active_projects.len(), 2);
    let cli = report
        .active_projects
        .iter()
        .find(|project| project.path.ends_with("Projects/cli-tool"))
        .unwrap();
    assert_eq!(cli.session_count, 2);
    assert_eq!(cli.memory_count, 1);
    assert_eq!(cli.manifest_count, 1);
    let web = report
        .active_projects
        .iter()
        .find(|project| project.path.ends_with("Projects/web-ui"))
        .unwrap();
    assert_eq!(web.session_count, 2);
    assert_eq!(web.memory_count, 0);
    assert_eq!(web.manifest_count, 1);

    assert!(report
        .tech_stack_evidence
        .iter()
        .any(|evidence| evidence.term == "Rust" && !evidence.manifest_refs.is_empty()));
    assert!(report
        .tech_stack_evidence
        .iter()
        .any(|evidence| evidence.term == "TypeScript" && !evidence.manifest_refs.is_empty()));
    assert!(report
        .keyword_evidence
        .iter()
        .any(|evidence| evidence.term == "cli" && !evidence.project_refs.is_empty()));
    assert!(report
        .recommended_profile
        .tech_stack
        .contains(&"Rust".to_string()));
    assert!(report
        .recommended_profile
        .tech_stack
        .contains(&"TypeScript".to_string()));
    assert!(report
        .recommended_profile
        .keywords
        .contains(&"cli".to_string()));
    assert!(!home.join(".issue-finder/config.toml").exists());
}

#[test]
fn profile_bootstrap_handles_empty_home_without_failing() {
    let dir = tempdir().unwrap();
    let report = bootstrap_profile(dir.path()).unwrap();
    assert!(report.agent_sources.is_empty());
    assert!(report.active_projects.is_empty());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "no_agent_sources_found"));
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "no_active_projects_found"));
}

#[test]
fn profile_bootstrap_cli_json_outputs_single_object() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".codex")).unwrap();
    fs::write(
        dir.path().join(".codex/session_index.jsonl"),
        json!({
            "cwd": dir.path(),
            "title": "Rust CLI",
            "timestamp": "2026-06-01T00:00:00Z"
        })
        .to_string(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .args([
            "profile",
            "bootstrap",
            "--json",
            "--scan-root",
            dir.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1);
    let value = serde_json::from_str::<serde_json::Value>(stdout.trim()).unwrap();
    assert_eq!(value["kind"], "issue_finder_profile_bootstrap_report");
    assert_eq!(value["scanScope"]["conversationBodyMode"], "disabled");
}
