use issue_finder::memory::run_offline_eval;
use tempfile::tempdir;

#[test]
fn memory_eval_offline_writes_metrics_and_report_files() {
    let dir = tempdir().unwrap();
    let report = run_offline_eval(dir.path()).unwrap();

    assert!(!report.samples.is_empty());
    assert_eq!(report.metrics.failed_samples, 0);
    assert!(report
        .metrics
        .dimensions
        .contains_key("over_recall_prevention"));
    assert!(report.metrics.dimensions.contains_key("deletion_tombstone"));
    assert!(report.samples.iter().all(|sample| !sample
        .observed_behavior
        .contains("Covered by deterministic")));
    assert!(dir.path().join("metrics.json").exists());
    assert!(dir.path().join("report.md").exists());
}
