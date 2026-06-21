use std::collections::BTreeSet;

use issue_finder::agent_loop_eval::{evaluate_builtin, run_offline_eval};
use tempfile::tempdir;

#[test]
fn agent_loop_eval_fixtures_cover_next_stage_loop_contracts() {
    let report = evaluate_builtin().unwrap();

    assert_eq!(report.kind, "agent_loop_eval_report");
    assert_eq!(report.metrics.total_samples, 6);
    assert_eq!(report.metrics.failed_samples, 0);
    assert_eq!(
        report
            .metrics
            .families
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "github_interaction_policy",
            "lifecycle_reactivation",
            "memory_governance",
            "package_quality",
            "runtime_vs_quality",
            "session_continuity",
        ])
    );
    assert!(report.samples.iter().all(|sample| sample.passed));
    assert!(report
        .samples
        .iter()
        .all(|sample| !sample.observations.is_empty()));
}

#[test]
fn agent_loop_eval_offline_writes_metrics_and_report_files() {
    let output_dir = tempdir().unwrap();
    let report = run_offline_eval(output_dir.path()).unwrap();

    assert_eq!(report.metrics.failed_samples, 0);
    assert!(output_dir.path().join("metrics.json").exists());
    assert!(output_dir.path().join("report.md").exists());

    let metrics = std::fs::read_to_string(output_dir.path().join("metrics.json")).unwrap();
    assert!(metrics.contains("agent_loop_eval_report"));
    assert!(metrics.contains("runtime_vs_quality"));
    let markdown = std::fs::read_to_string(output_dir.path().join("report.md")).unwrap();
    assert!(markdown.contains("# Agent Loop Eval"));
    assert!(markdown.contains("github_interaction_policy"));
}
