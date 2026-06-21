use std::collections::BTreeSet;
use std::path::Path;

use issue_finder::recommendation::eval::{
    builtin_datasets, evaluate_named_datasets, profile_coverage, write_offline_report_snapshot,
};

#[test]
fn recommendation_eval_fixtures_run_against_current_ranking_pipeline() {
    let report = evaluate_named_datasets(builtin_datasets());

    assert_eq!(report.datasets.len(), 10);
    assert_eq!(report.overall.samples, 101);
    assert!(report.overall.visible <= report.overall.samples);
    assert!((0.0..=1.0).contains(&report.overall.precision_at5));
    assert!((0.0..=1.0).contains(&report.overall.precision_at10));
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

    let dataset_names = report
        .datasets
        .iter()
        .map(|dataset| dataset.dataset.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        dataset_names,
        BTreeSet::from([
            "core_quality",
            "dispatch_outcome_replay",
            "feedback_replay",
            "lifecycle_reactivation",
            "profile_ai_agent_tools",
            "profile_backend_rust_go",
            "profile_devops_infra",
            "profile_frontend",
            "profile_python_data_cli",
            "source_trust",
        ])
    );

    let profiles = report
        .datasets
        .iter()
        .map(|dataset| dataset.profile.as_str())
        .collect::<BTreeSet<_>>();
    assert!(profiles.contains("default_cli_devtools"));
    assert!(profiles.contains("typescript_frontend"));
    assert!(profiles.contains("rust_backend_systems"));
    assert!(profiles.contains("python_data_cli"));
    assert!(profiles.contains("ai_agent_tools"));
    assert!(profiles.contains("devops_infra"));

    let coverage = profile_coverage(&report);
    assert!(coverage.values().all(|samples| *samples > 0));

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

    let json = serde_json::to_string_pretty(&report).expect("report should serialize");
    assert!(json.contains("precisionAt5"));
    assert!(json.contains("rankingInversions"));

    if let Some(output_dir) = std::env::var_os("ISSUE_FINDER_RECOMMENDATION_EVAL_REPORT_DIR") {
        write_offline_report_snapshot(&report, Path::new(&output_dir))
            .expect("offline report snapshot should be written");
    }
}
