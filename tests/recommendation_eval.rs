use std::path::Path;

use issue_finder::recommendation::eval::{
    builtin_datasets, evaluate_named_datasets, write_offline_report_snapshot,
};

#[test]
fn recommendation_eval_fixtures_run_against_current_ranking_pipeline() {
    let report = evaluate_named_datasets(builtin_datasets());

    assert!(!report.datasets.is_empty());
    assert!(report.overall.samples > 0);
    assert_eq!(
        report.overall.reject_leakage, 0,
        "V2 quality gate should hide reject samples"
    );
    assert_eq!(
        report.overall.dashboard_noise_leakage, 0,
        "V2 quality gate should hide dashboard and toy/no-code noise"
    );
    assert_eq!(
        report.overall.competition_leakage, 0,
        "V2 quality gate should hide claimed or PR-contested samples"
    );
    assert!(
        report.overall.profile_mismatch_leakage <= 1,
        "V2 should keep profile mismatch leakage within the stage target"
    );
    assert_eq!(
        report.overall.stale_high_rank_leakage, 0,
        "V2 freshness policy should prevent stale samples from receiving high freshness"
    );
    assert_eq!(
        report.overall.feedback_cooldown_passes, report.overall.feedback_cooldown_total,
        "V2 feedback cooldown samples should all pass"
    );
    assert_eq!(
        report.overall.dispatch_outcome_passes, report.overall.dispatch_outcome_total,
        "dispatch outcome replay samples should all pass"
    );

    for dataset in &report.datasets {
        assert_eq!(dataset.metrics.samples, dataset.ranked.len());
        assert!(
            !dataset.ranked.is_empty(),
            "{} should include ranked samples",
            dataset.dataset
        );
        assert!(
            dataset.failures.is_empty(),
            "{} should not have expectation failures: {:?}",
            dataset.dataset,
            dataset.failures
        );
        assert!(
            dataset
                .ranked
                .iter()
                .enumerate()
                .all(|(index, item)| item.rank == index + 1),
            "{} rank values should be stable and contiguous",
            dataset.dataset
        );
    }

    if let Some(output_dir) = std::env::var_os("ISSUE_FINDER_RECOMMENDATION_EVAL_REPORT_DIR") {
        write_offline_report_snapshot(&report, Path::new(&output_dir))
            .expect("offline report snapshot should be written");
    }
}

#[test]
fn codex_feedback_replay_recovers_lifecycle_candidates_and_keeps_exposure_cooldown() {
    let report = evaluate_named_datasets(vec![(
        "codex_feedback_replay",
        include_str!("fixtures/recommendation_eval/datasets/codex_feedback_replay.json"),
    )]);
    let dataset = &report.datasets[0];
    assert!(dataset.failures.is_empty(), "{:?}", dataset.failures);
    assert_eq!(dataset.metrics.visible, dataset.metrics.samples);
    assert_eq!(
        dataset.metrics.feedback_cooldown_passes,
        dataset.metrics.feedback_cooldown_total
    );
}
