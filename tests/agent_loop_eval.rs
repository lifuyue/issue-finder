use issue_finder::agent_loop_eval::evaluate_builtin;

#[test]
fn agent_loop_eval_fixtures_cover_agent_loop_contracts() {
    let report = evaluate_builtin().unwrap();

    assert!(!report.samples.is_empty());
    assert_eq!(report.metrics.failed_samples, 0);
    assert!(report.samples.iter().all(|sample| sample.passed));
    assert!(report
        .samples
        .iter()
        .all(|sample| !sample.observations.is_empty()));
}
