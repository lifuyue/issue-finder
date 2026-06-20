use issue_finder::memory::run_offline_eval;
use tempfile::tempdir;

#[test]
fn memory_eval_offline_writes_metrics_and_report_files() {
    let dir = tempdir().unwrap();
    let report = run_offline_eval(dir.path()).unwrap();

    assert_eq!(report.kind, "memory_eval_report");
    assert_eq!(report.metrics.total_samples, 8);
    assert_eq!(report.metrics.failed_samples, 0);
    assert!(report.metrics.dimensions.contains_key("factual_recall"));
    assert!(report.metrics.dimensions.contains_key("write_back_safety"));
    assert!(dir.path().join("metrics.json").exists());
    assert!(dir.path().join("report.md").exists());

    let metrics = std::fs::read_to_string(dir.path().join("metrics.json")).unwrap();
    assert!(metrics.contains("totalSamples"));
    let markdown = std::fs::read_to_string(dir.path().join("report.md")).unwrap();
    assert!(markdown.contains("# Memory Eval"));
    assert!(markdown.contains("Expected:"));
}
