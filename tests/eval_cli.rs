use std::path::PathBuf;
use std::process::Command;

use tempfile::tempdir;

#[test]
fn eval_contract_exposes_versioned_runtime_and_oracle_boundaries() {
    let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .args(["eval", "contract", "--json"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let contract: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(contract["kind"], "issue_finder_eval_contract");
    assert_eq!(contract["version"], 2);
    assert_eq!(contract["defaultRuntime"], "codex_app_server");
    assert_eq!(
        contract["runtimeValidationLevels"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(contract["a2aTaskExportVersion"], 1);
    assert_eq!(contract["evidenceBundleVersion"], 1);
    assert_eq!(contract["verifierProtocolVersion"], 1);
    assert_eq!(contract["atifVersion"], "ATIF-v1.7");
    assert_eq!(
        contract["supportedEvaluationRuntimes"],
        serde_json::json!(["inspect", "harbor"])
    );
    assert_eq!(contract["hardGateIds"].as_array().unwrap().len(), 14);
    assert!(contract["terminationCauses"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("limit")));
    assert_eq!(contract["recoveryBoundaries"].as_array().unwrap().len(), 3);
    assert_eq!(contract["safetyInvariants"].as_array().unwrap().len(), 4);
    assert!(contract["supportedEvidenceExports"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("codex_runtime_eval")));
}

#[test]
fn eval_recovery_prepare_creates_randomized_approved_dispatch_state() {
    let home = tempdir().unwrap();
    let workspace = tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .env("ISSUE_FINDER_HOME", home.path())
        .args([
            "eval",
            "recovery-prepare",
            "--scenario",
            "E01",
            "--workspace",
            workspace.path().to_str().unwrap(),
            "--marker",
            "RANDOM-BOUNDARY-1",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let scenario: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(scenario["kind"], "issue_finder_recovery_eval_scenario");
    assert_eq!(scenario["scenario"], "E01");
    assert!(scenario["issueRef"]
        .as_str()
        .unwrap()
        .contains("random-boundary-1"));
    let status = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .env("ISSUE_FINDER_HOME", home.path())
        .args([
            "dispatch",
            "status",
            scenario["runId"].as_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(status.status.success());
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["run"]["status"], "approved");
    assert_eq!(status["run"]["approval_state"], "approved");
}

#[test]
fn eval_recommendation_offline_writes_report_files() {
    let output_dir = tempdir().unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .args([
            "eval",
            "recommendation",
            "--offline",
            "--output",
            output_dir.path().to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert!(status.success());
    assert!(output_dir.path().join("metrics.json").exists());
    assert!(output_dir.path().join("report.md").exists());
    assert!(output_dir.path().join("visible.jsonl").exists());

    let metrics = std::fs::read_to_string(output_dir.path().join("metrics.json")).unwrap();
    assert!(metrics.contains("\"samples\""));
    let visible = std::fs::read_to_string(output_dir.path().join("visible.jsonl")).unwrap();
    assert!(visible.contains("\"sampleId\""));
}

#[test]
fn eval_recommendation_accepts_external_dataset() {
    let output_dir = tempfile::tempdir().unwrap();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/recommendation_eval/datasets/core_quality.json");
    let status = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .args([
            "eval",
            "recommendation",
            "--offline",
            "--dataset",
            fixture.to_str().unwrap(),
            "--output",
            output_dir.path().to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(output_dir.path().join("metrics.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(report["datasets"].as_array().unwrap().len(), 1);
    assert_eq!(report["datasets"][0]["dataset"], "core_quality");
}

#[test]
fn eval_agent_loop_offline_writes_report_files() {
    let output_dir = tempdir().unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .args([
            "eval",
            "agent-loop",
            "--offline",
            "--output",
            output_dir.path().to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert!(status.success());
    assert!(output_dir.path().join("metrics.json").exists());
    assert!(output_dir.path().join("report.md").exists());

    let metrics = std::fs::read_to_string(output_dir.path().join("metrics.json")).unwrap();
    assert!(metrics.contains("agent_loop_eval_report"));
    assert!(metrics.contains("runtime_vs_quality"));
}
