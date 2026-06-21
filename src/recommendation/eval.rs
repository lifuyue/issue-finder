use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use crate::competition::{CompetitionBand, CompetitionFacts};
use crate::config::{Config, ProfileConfig};
use crate::github::GitHubIssue;
use crate::github_enrichment::{
    EnrichedComment, EnrichedIssue, EnrichedParticipants, TimestampedSample,
};
use crate::memory::outcome_projection::{ranking_adjustment_for_candidate, OutcomeFeedbackInput};
use crate::paths::{atomic_write, IssueFinderPaths};
use crate::recommendation::engine::{RecommendationEngine, ScoutOptions};
use crate::recommendation::events::IssueKey;
use crate::recommendation::events::RecommendationEventSource;
use crate::recommendation::feed_ranker::{
    apply_recommendation_assessments, displayable, sort_by_feed,
};
use crate::recommendation::state::RecommendationIssueState;
use crate::value_scoring::{assess_issue, RankedValueIssue, RiskTag};
use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};

const CORE_QUALITY: &str =
    include_str!("../../tests/fixtures/recommendation_eval/datasets/core_quality.json");
const PROFILE_FRONTEND: &str =
    include_str!("../../tests/fixtures/recommendation_eval/datasets/profile_frontend.json");
const PROFILE_BACKEND_RUST_GO: &str =
    include_str!("../../tests/fixtures/recommendation_eval/datasets/profile_backend_rust_go.json");
const PROFILE_PYTHON_DATA_CLI: &str =
    include_str!("../../tests/fixtures/recommendation_eval/datasets/profile_python_data_cli.json");
const PROFILE_AI_AGENT_TOOLS: &str =
    include_str!("../../tests/fixtures/recommendation_eval/datasets/profile_ai_agent_tools.json");
const PROFILE_DEVOPS_INFRA: &str =
    include_str!("../../tests/fixtures/recommendation_eval/datasets/profile_devops_infra.json");
const SOURCE_TRUST: &str =
    include_str!("../../tests/fixtures/recommendation_eval/datasets/source_trust.json");
const FEEDBACK_REPLAY: &str =
    include_str!("../../tests/fixtures/recommendation_eval/datasets/feedback_replay.json");
const DISPATCH_OUTCOME_REPLAY: &str =
    include_str!("../../tests/fixtures/recommendation_eval/datasets/dispatch_outcome_replay.json");
const LIFECYCLE_REACTIVATION: &str =
    include_str!("../../tests/fixtures/recommendation_eval/datasets/lifecycle_reactivation.json");

const LIVE_PROFILE_NAMES: [&str; 6] = [
    "default_cli_devtools",
    "typescript_frontend",
    "rust_backend_systems",
    "python_data_cli",
    "ai_agent_tools",
    "devops_infra",
];

#[derive(Debug, Deserialize)]
pub struct EvaluationDataset {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub samples: Vec<EvaluationSample>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationSample {
    pub id: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default = "default_source_tier")]
    pub source_tier: String,
    pub issue: SampleIssue,
    pub repository: SampleRepository,
    #[serde(default)]
    pub comments: Vec<SampleComment>,
    #[serde(default)]
    pub competition: SampleCompetition,
    #[serde(default)]
    pub activity: SampleActivity,
    #[serde(default)]
    pub growth: SampleGrowth,
    #[serde(default)]
    pub feedback: SampleFeedback,
    #[serde(default)]
    pub dispatch_outcomes: Vec<SampleDispatchOutcome>,
    pub expected: ExpectedOutcome,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleIssue {
    pub repo_full_name: String,
    #[serde(default)]
    pub repo_name: Option<String>,
    pub number: u64,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default = "default_created_age_days")]
    pub created_age_days: i64,
    #[serde(default = "default_updated_age_days")]
    pub updated_age_days: i64,
    #[serde(default)]
    pub comments_count: u64,
    #[serde(default = "default_author_association")]
    pub author_association: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleRepository {
    #[serde(default)]
    pub description: String,
    pub language: String,
    pub stars: u64,
    #[serde(default)]
    pub forks: u64,
    #[serde(default)]
    pub watchers: u64,
    #[serde(default)]
    pub open_issues: u64,
    #[serde(default)]
    pub topics: Vec<String>,
    #[serde(default)]
    pub pushed_at: Option<String>,
    #[serde(default = "default_pushed_age_days")]
    pub pushed_age_days: i64,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default = "default_repo_created_age_days")]
    pub created_age_days: i64,
    #[serde(default)]
    pub archived: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleComment {
    pub body: String,
    #[serde(default = "default_comment_association")]
    pub author_association: String,
    #[serde(default = "default_comment_age_days")]
    pub created_age_days: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleCompetition {
    #[serde(default = "default_timeline_missing")]
    pub timeline_missing: bool,
    #[serde(default)]
    pub open_pr_refs: usize,
    #[serde(default)]
    pub closed_pr_refs: usize,
    #[serde(default)]
    pub attempt_comments: usize,
    #[serde(default)]
    pub claim_comments: usize,
    #[serde(default)]
    pub working_comments: usize,
    #[serde(default)]
    pub fix_submitted_comments: usize,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleActivity {
    #[serde(default)]
    pub maintainer_recent_response: bool,
    #[serde(default)]
    pub recent_issue_activity: Option<bool>,
    #[serde(default)]
    pub recent_repo_activity: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleGrowth {
    #[serde(default)]
    pub recent_stars: usize,
    #[serde(default)]
    pub recent_forks: usize,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleFeedback {
    #[serde(default)]
    pub shown_count: u32,
    #[serde(default)]
    pub read_count: u32,
    #[serde(default)]
    pub prepared_count: u32,
    #[serde(default)]
    pub dismissed: bool,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub feedback_age_days: i64,
    #[serde(default)]
    pub last_seen_comments_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleDispatchOutcome {
    #[serde(default)]
    pub approved: bool,
    #[serde(default)]
    pub repo_full_name: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    pub outcome_kind: String,
    #[serde(default)]
    pub failure_class: Option<String>,
    #[serde(default)]
    pub task_class: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedOutcome {
    pub quality: ExpectedQuality,
    pub behavior: ExpectedBehavior,
    #[serde(default)]
    pub reject_reasons: Vec<String>,
    #[serde(default)]
    pub min_profile_fit: Option<i32>,
    #[serde(default)]
    pub max_rank_bucket: Option<usize>,
    #[serde(default)]
    pub must_have_risk_tags: Vec<RiskTag>,
    #[serde(default)]
    pub must_not_have_risk_tags: Vec<RiskTag>,
    #[serde(default)]
    pub min_memory_adjustment: Option<i32>,
    #[serde(default)]
    pub max_memory_adjustment: Option<i32>,
    #[serde(default)]
    pub min_reactivation_boost: Option<i32>,
    #[serde(default)]
    pub max_reactivation_boost: Option<i32>,
    #[serde(default)]
    pub min_feedback_penalty: Option<i32>,
    #[serde(default)]
    pub max_feedback_penalty: Option<i32>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedQuality {
    Reject,
    Weak,
    Good,
    Excellent,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedBehavior {
    VisibleTop,
    Visible,
    VisibleLower,
    Hidden,
    FallbackCandidate,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationReport {
    pub datasets: Vec<DatasetReport>,
    pub overall: Metrics,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatasetReport {
    pub dataset: String,
    pub profile: String,
    pub limit: usize,
    pub metrics: Metrics,
    pub failures: Vec<EvaluationFailure>,
    pub ranked: Vec<RankedSampleSummary>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub samples: usize,
    pub visible: usize,
    pub precision_at5: f64,
    pub precision_at10: f64,
    pub visible_fill_rate: f64,
    pub target_visible_fill_rate: f64,
    pub reject_leakage: usize,
    pub profile_mismatch_leakage: usize,
    pub stale_high_rank_leakage: usize,
    pub competition_leakage: usize,
    pub dashboard_noise_leakage: usize,
    pub ranking_inversions: usize,
    pub feedback_cooldown_passes: usize,
    pub feedback_cooldown_total: usize,
    pub dispatch_outcome_passes: usize,
    pub dispatch_outcome_total: usize,
    pub fallback_fill_rate: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationFailure {
    pub sample_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RankedSampleSummary {
    pub rank: usize,
    pub sample_id: String,
    pub key: String,
    pub title: String,
    pub expected_quality: ExpectedQuality,
    pub expected_behavior: ExpectedBehavior,
    pub final_feed_score: i32,
    pub freshness_boost: i32,
    pub feedback_penalty: i32,
    pub reactivation_boost: i32,
    pub memory_adjustment: i32,
    pub profile_fit: i32,
    pub visibility: String,
    pub source_tier: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveEvaluationReport {
    pub limit: usize,
    pub refresh: bool,
    pub profiles: Vec<LiveProfileReport>,
    pub summary: LiveEvaluationSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveProfileReport {
    pub profile: String,
    pub visible: usize,
    pub discovery_count: usize,
    pub filtered_count: usize,
    pub api_budget: crate::github_budget::GitHubApiBudgetReport,
    pub candidates: Vec<LiveCandidateSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveCandidateSummary {
    pub rank: usize,
    pub key: String,
    pub title: String,
    pub url: String,
    pub profile_fit: i32,
    pub visibility: String,
    pub source_tier: Option<String>,
    pub risk_tags: Vec<String>,
    pub competition: CompetitionFacts,
    pub missing_evidence: Vec<String>,
    pub manual_quality: Option<String>,
    pub manual_notes: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveEvaluationSummary {
    pub profiles: usize,
    pub min_visible: usize,
    pub max_visible: usize,
    pub total_visible: usize,
    pub total_network_requests: usize,
    pub budget_exhausted_profiles: usize,
}

struct RankedEvaluationSample<'a> {
    sample: &'a EvaluationSample,
    ranked: RankedValueIssue,
    source_tier: String,
}

struct LiveEvalHome {
    paths: IssueFinderPaths,
}

impl Drop for LiveEvalHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.paths.home);
    }
}

pub fn load_dataset(raw: &str) -> EvaluationDataset {
    serde_json::from_str(raw).expect("recommendation eval dataset should parse")
}

pub fn builtin_datasets() -> Vec<(&'static str, &'static str)> {
    vec![
        ("core_quality", CORE_QUALITY),
        ("profile_frontend", PROFILE_FRONTEND),
        ("profile_backend_rust_go", PROFILE_BACKEND_RUST_GO),
        ("profile_python_data_cli", PROFILE_PYTHON_DATA_CLI),
        ("profile_ai_agent_tools", PROFILE_AI_AGENT_TOOLS),
        ("profile_devops_infra", PROFILE_DEVOPS_INFRA),
        ("source_trust", SOURCE_TRUST),
        ("feedback_replay", FEEDBACK_REPLAY),
        ("dispatch_outcome_replay", DISPATCH_OUTCOME_REPLAY),
        ("lifecycle_reactivation", LIFECYCLE_REACTIVATION),
    ]
}

pub fn run_offline_eval(output_dir: &Path) -> Result<EvaluationReport> {
    let report = evaluate_named_datasets(builtin_datasets());
    write_offline_report_snapshot(&report, output_dir)?;
    Ok(report)
}

pub async fn run_live_eval(
    base_config: &Config,
    limit: usize,
    refresh: bool,
    output_dir: &Path,
) -> Result<LiveEvaluationReport> {
    let mut profiles = Vec::new();
    for profile_name in LIVE_PROFILE_NAMES {
        let mut config = base_config.clone();
        config.profile = profile_config(profile_name);
        let home = isolated_live_home(output_dir, profile_name)?;
        home.paths.ensure_layout()?;
        let result = RecommendationEngine::new(&home.paths, &config)
            .scout(
                limit,
                refresh,
                ScoutOptions {
                    include_filtered: false,
                    record_exposure: false,
                    source: RecommendationEventSource::CliScout,
                },
                crate::discovery::DiscoveryScope::Global,
            )
            .await?;
        let candidates = result
            .ranked
            .iter()
            .enumerate()
            .map(|(index, item)| live_candidate_summary(index + 1, item))
            .collect::<Vec<_>>();
        profiles.push(LiveProfileReport {
            profile: profile_name.to_string(),
            visible: result.ranked.len(),
            discovery_count: result.discovery_count,
            filtered_count: result.filtered_count,
            api_budget: result.api_budget,
            candidates,
        });
    }

    let summary = LiveEvaluationSummary {
        profiles: profiles.len(),
        min_visible: profiles
            .iter()
            .map(|profile| profile.visible)
            .min()
            .unwrap_or(0),
        max_visible: profiles
            .iter()
            .map(|profile| profile.visible)
            .max()
            .unwrap_or(0),
        total_visible: profiles.iter().map(|profile| profile.visible).sum(),
        total_network_requests: profiles
            .iter()
            .map(|profile| profile.api_budget.total_network_requests)
            .sum(),
        budget_exhausted_profiles: profiles
            .iter()
            .filter(|profile| !profile.api_budget.budget_exhausted.is_empty())
            .count(),
    };
    let report = LiveEvaluationReport {
        limit,
        refresh,
        profiles,
        summary,
    };
    write_live_report_snapshot(&report, output_dir)?;
    Ok(report)
}

pub fn evaluate_named_datasets(datasets: Vec<(&str, &str)>) -> EvaluationReport {
    let mut reports = Vec::new();
    for (name, raw) in datasets {
        let mut dataset = load_dataset(raw);
        if dataset.name.is_none() {
            dataset.name = Some(name.to_string());
        }
        reports.push(evaluate_dataset(&dataset));
    }
    let overall = aggregate_metrics(reports.iter().map(|report| &report.metrics));
    EvaluationReport {
        datasets: reports,
        overall,
    }
}

pub fn evaluate_dataset(dataset: &EvaluationDataset) -> DatasetReport {
    let dataset_name = dataset
        .name
        .clone()
        .unwrap_or_else(|| "unnamed_dataset".to_string());
    let profile_name = dataset
        .profile
        .clone()
        .or_else(|| {
            dataset
                .samples
                .iter()
                .find_map(|sample| sample.profile.clone())
        })
        .unwrap_or_else(|| "default_cli_devtools".to_string());
    let profile = profile_config(&profile_name);
    let mut ranked = dataset
        .samples
        .iter()
        .map(|sample| rank_sample(sample, &profile))
        .collect::<Vec<_>>();
    sort_by_feed_for_eval(&mut ranked);

    let metrics = metrics_for_ranked(&ranked, dataset.limit);
    let failures = failures_for_ranked(&ranked);
    let ranked = ranked
        .iter()
        .enumerate()
        .map(|(index, item)| RankedSampleSummary {
            rank: index + 1,
            sample_id: item.sample.id.clone(),
            key: format!(
                "{}#{}",
                item.ranked.issue.repo_full_name, item.ranked.issue.number
            ),
            title: item.ranked.issue.title.clone(),
            expected_quality: item.sample.expected.quality,
            expected_behavior: item.sample.expected.behavior,
            final_feed_score: item.ranked.recommendation.final_feed_score,
            freshness_boost: item.ranked.recommendation.freshness_boost,
            feedback_penalty: item.ranked.recommendation.feedback_penalty,
            reactivation_boost: item.ranked.recommendation.reactivation_boost,
            memory_adjustment: item.ranked.recommendation.memory_adjustment,
            profile_fit: item.ranked.value_assessment.profile_fit_score,
            visibility: item.ranked.recommendation.visibility.to_string(),
            source_tier: item.source_tier.clone(),
        })
        .collect();

    DatasetReport {
        dataset: dataset_name,
        profile: profile_name,
        limit: dataset.limit,
        metrics,
        failures,
        ranked,
    }
}

pub fn profile_config(name: &str) -> ProfileConfig {
    match name {
        "typescript_frontend" => ProfileConfig {
            tech_stack: vec![
                "TypeScript".to_string(),
                "JavaScript".to_string(),
                "React".to_string(),
            ],
            keywords: vec![
                "frontend".to_string(),
                "react".to_string(),
                "ui".to_string(),
                "browser".to_string(),
                "form".to_string(),
                "component".to_string(),
            ],
        },
        "rust_backend_systems" => ProfileConfig {
            tech_stack: vec!["Rust".to_string(), "Go".to_string()],
            keywords: vec![
                "cargo".to_string(),
                "compiler".to_string(),
                "performance".to_string(),
                "backend".to_string(),
            ],
        },
        "python_data_cli" => ProfileConfig {
            tech_stack: vec!["Python".to_string()],
            keywords: vec![
                "cli".to_string(),
                "data".to_string(),
                "pandas".to_string(),
                "testing".to_string(),
            ],
        },
        "ai_agent_tools" => ProfileConfig {
            tech_stack: vec!["Python".to_string(), "TypeScript".to_string()],
            keywords: vec![
                "ai".to_string(),
                "llm".to_string(),
                "agent".to_string(),
                "mcp".to_string(),
                "evaluation".to_string(),
                "model".to_string(),
                "openai".to_string(),
                "developer-tools".to_string(),
            ],
        },
        "devops_infra" => ProfileConfig {
            tech_stack: vec!["Go".to_string(), "YAML".to_string(), "Python".to_string()],
            keywords: vec![
                "kubernetes".to_string(),
                "docker".to_string(),
                "ci".to_string(),
                "gitops".to_string(),
                "cloud".to_string(),
                "infra".to_string(),
                "operator".to_string(),
            ],
        },
        _ => ProfileConfig {
            tech_stack: vec!["Rust".to_string(), "TypeScript".to_string()],
            keywords: vec!["cli".to_string(), "developer-tools".to_string()],
        },
    }
}

fn rank_sample<'a>(
    sample: &'a EvaluationSample,
    profile: &ProfileConfig,
) -> RankedEvaluationSample<'a> {
    let enriched = sample.enriched_issue();
    let issue = sample.github_issue();
    let value_assessment = assess_issue(&enriched, profile);
    let mut ranked = RankedValueIssue {
        issue,
        score: value_assessment.final_rank_score,
        value_assessment,
        enriched_issue: enriched,
        explanation: Vec::new(),
        recommendation: Default::default(),
    };
    ranked.explanation = ranked.value_assessment.explanation.clone();
    let states = sample.feedback_state(&ranked.issue);
    apply_recommendation_assessments(std::slice::from_mut(&mut ranked), &states);
    sample.apply_dispatch_outcome_replay(&mut ranked);
    RankedEvaluationSample {
        sample,
        ranked,
        source_tier: sample.source_tier.clone(),
    }
}

fn sort_by_feed_for_eval(ranked: &mut [RankedEvaluationSample<'_>]) {
    let mut keyed = ranked
        .iter()
        .map(|item| item.ranked.clone())
        .collect::<Vec<_>>();
    sort_by_feed(&mut keyed);
    let order = keyed
        .iter()
        .enumerate()
        .map(|(index, item)| {
            (
                format!("{}#{}", item.issue.repo_full_name, item.issue.number),
                index,
            )
        })
        .collect::<HashMap<_, _>>();
    ranked.sort_by_key(|item| {
        order
            .get(&format!(
                "{}#{}",
                item.ranked.issue.repo_full_name, item.ranked.issue.number
            ))
            .copied()
            .unwrap_or(usize::MAX)
    });
}

fn metrics_for_ranked(ranked: &[RankedEvaluationSample<'_>], limit: usize) -> Metrics {
    let visible = ranked
        .iter()
        .filter(|item| displayable(&item.ranked, false))
        .collect::<Vec<_>>();
    let top5 = visible.iter().take(5).copied().collect::<Vec<_>>();
    let top10 = visible.iter().take(10).copied().collect::<Vec<_>>();
    let reject_leakage = visible
        .iter()
        .filter(|item| item.sample.expected.quality == ExpectedQuality::Reject)
        .count();
    let profile_mismatch_leakage = visible
        .iter()
        .filter(|item| {
            item.sample
                .expected
                .reject_reasons_contains("profile_mismatch")
        })
        .count();
    let stale_high_rank_leakage = visible
        .iter()
        .take(10)
        .filter(|item| {
            item.sample.expected.reject_reasons_contains("stale")
                && item.ranked.recommendation.freshness_boost > 20
        })
        .count();
    let competition_leakage = visible
        .iter()
        .filter(|item| {
            item.sample
                .expected
                .reject_reasons_contains("claimed_or_pr")
        })
        .count();
    let dashboard_noise_leakage = visible
        .iter()
        .filter(|item| {
            item.sample
                .expected
                .reject_reasons_contains("dashboard_noise")
                || item.sample.expected.reject_reasons_contains("toy_no_code")
        })
        .count();
    let fallback_total = ranked
        .iter()
        .filter(|item| item.sample.expected.behavior == ExpectedBehavior::FallbackCandidate)
        .count();
    let fallback_visible = visible
        .iter()
        .filter(|item| item.sample.expected.behavior == ExpectedBehavior::FallbackCandidate)
        .count();
    let cooldown = ranked
        .iter()
        .filter(|item| item.sample.feedback.has_feedback())
        .collect::<Vec<_>>();
    let cooldown_passes = cooldown
        .iter()
        .filter(|item| {
            if item.sample.feedback.done || item.sample.feedback.dismissed {
                !displayable(&item.ranked, false)
            } else if item.sample.feedback.read_count > 0 {
                item.ranked.recommendation.feedback_penalty >= 35
            } else if item.sample.feedback.shown_count > 0 {
                item.ranked.recommendation.feedback_penalty > 0
            } else {
                true
            }
        })
        .count();
    let dispatch_outcome = ranked
        .iter()
        .filter(|item| !item.sample.dispatch_outcomes.is_empty())
        .collect::<Vec<_>>();
    let dispatch_outcome_passes = dispatch_outcome
        .iter()
        .filter(|item| memory_adjustment_matches_expectation(item))
        .count();

    Metrics {
        samples: ranked.len(),
        visible: visible.len(),
        precision_at5: precision(&top5),
        precision_at10: precision(&top10),
        visible_fill_rate: ratio(visible.len(), limit),
        target_visible_fill_rate: ratio(visible.len(), limit.max(1)),
        reject_leakage,
        profile_mismatch_leakage,
        stale_high_rank_leakage,
        competition_leakage,
        dashboard_noise_leakage,
        ranking_inversions: ranking_inversions(&visible),
        feedback_cooldown_passes: cooldown_passes,
        feedback_cooldown_total: cooldown.len(),
        dispatch_outcome_passes,
        dispatch_outcome_total: dispatch_outcome.len(),
        fallback_fill_rate: if fallback_total == 0 {
            1.0
        } else {
            ratio(fallback_visible, fallback_total)
        },
    }
}

fn failures_for_ranked(ranked: &[RankedEvaluationSample<'_>]) -> Vec<EvaluationFailure> {
    let mut failures = Vec::new();
    for (index, item) in ranked.iter().enumerate() {
        if item.sample.expected.quality == ExpectedQuality::Reject
            && displayable(&item.ranked, false)
        {
            failures.push(EvaluationFailure {
                sample_id: item.sample.id.clone(),
                reason: "reject sample is visible".to_string(),
            });
        }
        if item
            .sample
            .expected
            .min_profile_fit
            .is_some_and(|min| item.ranked.value_assessment.profile_fit_score < min)
        {
            failures.push(EvaluationFailure {
                sample_id: item.sample.id.clone(),
                reason: format!(
                    "profile fit {} below expected minimum",
                    item.ranked.value_assessment.profile_fit_score
                ),
            });
        }
        for tag in &item.sample.expected.must_have_risk_tags {
            if !item.ranked.value_assessment.risk_tags.contains(tag) {
                failures.push(EvaluationFailure {
                    sample_id: item.sample.id.clone(),
                    reason: format!("missing expected risk tag {tag}"),
                });
            }
        }
        for tag in &item.sample.expected.must_not_have_risk_tags {
            if item.ranked.value_assessment.risk_tags.contains(tag) {
                failures.push(EvaluationFailure {
                    sample_id: item.sample.id.clone(),
                    reason: format!("unexpected risk tag {tag}"),
                });
            }
        }
        if item
            .sample
            .expected
            .max_rank_bucket
            .is_some_and(|max_rank| index + 1 > max_rank)
        {
            failures.push(EvaluationFailure {
                sample_id: item.sample.id.clone(),
                reason: format!("rank {} exceeds expected maximum bucket", index + 1),
            });
        }
        if item
            .sample
            .expected
            .min_memory_adjustment
            .is_some_and(|min| item.ranked.recommendation.memory_adjustment < min)
        {
            failures.push(EvaluationFailure {
                sample_id: item.sample.id.clone(),
                reason: format!(
                    "memory adjustment {} below expected minimum",
                    item.ranked.recommendation.memory_adjustment
                ),
            });
        }
        if item
            .sample
            .expected
            .max_memory_adjustment
            .is_some_and(|max| item.ranked.recommendation.memory_adjustment > max)
        {
            failures.push(EvaluationFailure {
                sample_id: item.sample.id.clone(),
                reason: format!(
                    "memory adjustment {} above expected maximum",
                    item.ranked.recommendation.memory_adjustment
                ),
            });
        }
        if item
            .sample
            .expected
            .min_reactivation_boost
            .is_some_and(|min| item.ranked.recommendation.reactivation_boost < min)
        {
            failures.push(EvaluationFailure {
                sample_id: item.sample.id.clone(),
                reason: format!(
                    "reactivation boost {} below expected minimum",
                    item.ranked.recommendation.reactivation_boost
                ),
            });
        }
        if item
            .sample
            .expected
            .max_reactivation_boost
            .is_some_and(|max| item.ranked.recommendation.reactivation_boost > max)
        {
            failures.push(EvaluationFailure {
                sample_id: item.sample.id.clone(),
                reason: format!(
                    "reactivation boost {} above expected maximum",
                    item.ranked.recommendation.reactivation_boost
                ),
            });
        }
        if item
            .sample
            .expected
            .min_feedback_penalty
            .is_some_and(|min| item.ranked.recommendation.feedback_penalty < min)
        {
            failures.push(EvaluationFailure {
                sample_id: item.sample.id.clone(),
                reason: format!(
                    "feedback penalty {} below expected minimum",
                    item.ranked.recommendation.feedback_penalty
                ),
            });
        }
        if item
            .sample
            .expected
            .max_feedback_penalty
            .is_some_and(|max| item.ranked.recommendation.feedback_penalty > max)
        {
            failures.push(EvaluationFailure {
                sample_id: item.sample.id.clone(),
                reason: format!(
                    "feedback penalty {} above expected maximum",
                    item.ranked.recommendation.feedback_penalty
                ),
            });
        }
        if item.sample.expected.reasons.is_empty() {
            failures.push(EvaluationFailure {
                sample_id: item.sample.id.clone(),
                reason: "sample has no human expected reasons".to_string(),
            });
        }
    }
    failures
}

fn aggregate_metrics<'a>(metrics: impl Iterator<Item = &'a Metrics>) -> Metrics {
    let metrics = metrics.collect::<Vec<_>>();
    let mut aggregate = Metrics::default();
    for item in &metrics {
        aggregate.samples += item.samples;
        aggregate.visible += item.visible;
        aggregate.reject_leakage += item.reject_leakage;
        aggregate.profile_mismatch_leakage += item.profile_mismatch_leakage;
        aggregate.stale_high_rank_leakage += item.stale_high_rank_leakage;
        aggregate.competition_leakage += item.competition_leakage;
        aggregate.dashboard_noise_leakage += item.dashboard_noise_leakage;
        aggregate.ranking_inversions += item.ranking_inversions;
        aggregate.feedback_cooldown_passes += item.feedback_cooldown_passes;
        aggregate.feedback_cooldown_total += item.feedback_cooldown_total;
        aggregate.dispatch_outcome_passes += item.dispatch_outcome_passes;
        aggregate.dispatch_outcome_total += item.dispatch_outcome_total;
    }
    let count = metrics.len();
    if count > 0 {
        aggregate.precision_at5 =
            metrics.iter().map(|item| item.precision_at5).sum::<f64>() / count as f64;
        aggregate.precision_at10 =
            metrics.iter().map(|item| item.precision_at10).sum::<f64>() / count as f64;
        aggregate.visible_fill_rate = metrics
            .iter()
            .map(|item| item.visible_fill_rate)
            .sum::<f64>()
            / count as f64;
        aggregate.target_visible_fill_rate = metrics
            .iter()
            .map(|item| item.target_visible_fill_rate)
            .sum::<f64>()
            / count as f64;
        aggregate.fallback_fill_rate = metrics
            .iter()
            .map(|item| item.fallback_fill_rate)
            .sum::<f64>()
            / count as f64;
    }
    aggregate
}

fn precision(items: &[&RankedEvaluationSample<'_>]) -> f64 {
    if items.is_empty() {
        return 0.0;
    }
    let relevant = items
        .iter()
        .filter(|item| {
            matches!(
                item.sample.expected.quality,
                ExpectedQuality::Excellent | ExpectedQuality::Good
            )
        })
        .count();
    relevant as f64 / items.len() as f64
}

fn ranking_inversions(visible: &[&RankedEvaluationSample<'_>]) -> usize {
    let mut inversions = 0;
    for left_index in 0..visible.len() {
        for right_index in (left_index + 1)..visible.len() {
            let left = visible[left_index].sample.expected.quality;
            let right = visible[right_index].sample.expected.quality;
            if left < right {
                inversions += 1;
            }
        }
    }
    inversions
}

fn memory_adjustment_matches_expectation(item: &&RankedEvaluationSample<'_>) -> bool {
    let adjustment = item.ranked.recommendation.memory_adjustment;
    item.sample
        .expected
        .min_memory_adjustment
        .is_none_or(|min| adjustment >= min)
        && item
            .sample
            .expected
            .max_memory_adjustment
            .is_none_or(|max| adjustment <= max)
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        return 0.0;
    }
    (numerator as f64 / denominator as f64).min(1.0)
}

impl EvaluationSample {
    fn github_issue(&self) -> GitHubIssue {
        let repo_name = self.issue.repo_name.clone().unwrap_or_else(|| {
            self.issue
                .repo_full_name
                .split('/')
                .nth(1)
                .unwrap_or("repo")
                .to_string()
        });
        GitHubIssue {
            id: stable_id(&self.id),
            number: self.issue.number,
            title: self.issue.title.clone(),
            body: self.issue.body.clone(),
            labels: self.issue.labels.clone(),
            url: format!(
                "https://github.com/{}/issues/{}",
                self.issue.repo_full_name, self.issue.number
            ),
            repo_full_name: self.issue.repo_full_name.clone(),
            repo_name,
            repo_description: self.repository.description.clone(),
            repo_stars: self.repository.stars,
            created_at: timestamp_or_age(
                self.issue.created_at.as_deref(),
                self.issue.created_age_days,
            ),
            updated_at: timestamp_or_age(
                self.issue.updated_at.as_deref(),
                self.issue.updated_age_days,
            ),
        }
    }

    fn enriched_issue(&self) -> EnrichedIssue {
        let issue = self.github_issue();
        let mut enriched = EnrichedIssue::from_issue(&issue);
        enriched.issue.comments_count = self.issue.comments_count.max(self.comments.len() as u64);
        enriched.issue.author_association = self.issue.author_association.clone();
        enriched.repository.description = self.repository.description.clone();
        enriched.repository.stars = self.repository.stars;
        enriched.repository.forks = self.repository.forks;
        enriched.repository.subscribers = Some(self.repository.watchers);
        enriched.repository.open_issues = Some(self.repository.open_issues);
        enriched.repository.pushed_at = Some(timestamp_or_age(
            self.repository.pushed_at.as_deref(),
            self.repository.pushed_age_days,
        ));
        enriched.repository.created_at = Some(timestamp_or_age(
            self.repository.created_at.as_deref(),
            self.repository.created_age_days,
        ));
        enriched.repository.archived = self.repository.archived;
        enriched.repository.topics = self.repository.topics.clone();
        enriched.repository.language = Some(self.repository.language.clone());
        enriched.activity.recent_issue_activity = self
            .activity
            .recent_issue_activity
            .unwrap_or(self.issue.updated_age_days <= 14);
        enriched.activity.recent_repo_activity = self
            .activity
            .recent_repo_activity
            .unwrap_or(self.repository.pushed_age_days <= 30);
        enriched.activity.maintainer_recent_response = self.activity.maintainer_recent_response;
        enriched.comments = self
            .comments
            .iter()
            .enumerate()
            .map(|(index, comment)| EnrichedComment {
                source_ref: format!("issue:comments.{index}"),
                author: Some(format!("commenter-{index}")),
                author_association: comment.author_association.clone(),
                created_at: timestamp_or_age(None, comment.created_age_days),
                body_excerpt: comment.body.clone(),
            })
            .collect();
        let maintainer_commenters = enriched
            .comments
            .iter()
            .filter(|comment| {
                matches!(
                    comment.author_association.as_str(),
                    "OWNER" | "MEMBER" | "COLLABORATOR"
                )
            })
            .filter_map(|comment| comment.author.clone())
            .collect::<Vec<_>>();
        enriched.participants = EnrichedParticipants {
            issue_author: Some("issue-author".to_string()),
            commenters: enriched
                .comments
                .iter()
                .filter_map(|comment| comment.author.clone())
                .collect(),
            maintainer_commenters,
        };
        enriched.competition = self.competition_facts();
        enriched.growth.recent_stargazer_sample = timestamped_samples(
            "repo:stargazers.sample_recent_100",
            self.growth.recent_stars,
        );
        enriched.growth.newest_fork_sample =
            timestamped_samples("repo:forks.sample_newest_100", self.growth.recent_forks);
        enriched
    }

    fn competition_facts(&self) -> CompetitionFacts {
        let competition_points = (self.competition.open_pr_refs * 3
            + self.competition.closed_pr_refs
            + self.competition.attempt_comments
            + self.competition.claim_comments
            + self.competition.working_comments
            + self.competition.fix_submitted_comments) as i32;
        let competition_band = match competition_points {
            value if value <= 1 => CompetitionBand::Clear,
            value if value <= 3 => CompetitionBand::Light,
            value if value <= 7 => CompetitionBand::Contested,
            _ => CompetitionBand::Saturated,
        };
        CompetitionFacts {
            open_pr_refs: self.competition.open_pr_refs,
            closed_pr_refs: self.competition.closed_pr_refs,
            attempt_comments: self.competition.attempt_comments,
            claim_comments: self.competition.claim_comments,
            working_comments: self.competition.working_comments,
            fix_submitted_comments: self.competition.fix_submitted_comments,
            latest_competition_at: None,
            competition_points,
            competition_band,
            warnings: if self.competition.timeline_missing {
                vec!["Competition timeline evidence was not fetched".to_string()]
            } else {
                Vec::new()
            },
        }
    }

    fn feedback_state(&self, issue: &GitHubIssue) -> HashMap<IssueKey, RecommendationIssueState> {
        if !self.feedback.has_feedback() {
            return HashMap::new();
        }
        let timestamp = timestamp_or_age(None, self.feedback.feedback_age_days);
        let state = RecommendationIssueState {
            issue_key: IssueKey::from_issue(issue),
            shown_count: self.feedback.shown_count,
            read_count: self.feedback.read_count,
            prepared_count: self.feedback.prepared_count,
            dismissed: self.feedback.dismissed,
            done: self.feedback.done,
            restored_at: None,
            last_shown_at: (self.feedback.shown_count > 0).then(|| timestamp.clone()),
            last_read_at: (self.feedback.read_count > 0).then(|| timestamp.clone()),
            last_prepared_at: (self.feedback.prepared_count > 0).then(|| timestamp.clone()),
            last_feedback_at: Some(timestamp),
            last_seen_issue_updated_at: Some(issue.updated_at.clone()),
            last_seen_comments_count: Some(
                self.feedback
                    .last_seen_comments_count
                    .unwrap_or(self.issue.comments_count),
            ),
        };
        let mut states = HashMap::new();
        states.insert(state.issue_key.clone(), state);
        states
    }

    fn apply_dispatch_outcome_replay(&self, ranked: &mut RankedValueIssue) {
        if self.dispatch_outcomes.is_empty() {
            return;
        }
        let task_class = fixture_task_class(self);
        let outcomes = self
            .dispatch_outcomes
            .iter()
            .enumerate()
            .map(|(index, outcome)| OutcomeFeedbackInput {
                id: format!("{}:dispatch-outcome:{index}", self.id),
                approved: outcome.approved,
                issue_key: IssueKey::new(
                    outcome
                        .repo_full_name
                        .clone()
                        .unwrap_or_else(|| self.issue.repo_full_name.clone()),
                    self.issue.number,
                ),
                repo_scope: outcome.repo_full_name.clone(),
                agent_id: outcome.agent_id.clone(),
                outcome_kind: outcome.outcome_kind.clone(),
                failure_class: outcome.failure_class.clone(),
                task_class: outcome.task_class.clone(),
                validation_outcome: None,
            })
            .collect::<Vec<_>>();
        let adjustment =
            ranking_adjustment_for_candidate(&outcomes, &ranked.issue.repo_full_name, task_class);
        if adjustment == 0 {
            return;
        }
        ranked.recommendation.memory_adjustment = adjustment;
        ranked.recommendation.final_feed_score += adjustment;
        ranked.score = ranked.recommendation.final_feed_score;
        let sign = if adjustment > 0 { "+" } else { "" };
        ranked.recommendation.reasons.push(format!(
            "Dispatch outcome replay adjustment: {sign}{adjustment}"
        ));
        ranked.explanation.push(format!(
            "Dispatch outcome replay adjustment: {sign}{adjustment}"
        ));
    }
}

fn fixture_task_class(sample: &EvaluationSample) -> &'static str {
    let text = format!(
        "{}\n{}\n{}",
        sample.issue.title,
        sample.issue.body,
        sample.issue.labels.join(" ")
    )
    .to_ascii_lowercase();
    if text.contains("panic") && (text.contains("cli") || sample.repository.language == "Rust") {
        "rust_cli_panic"
    } else if text.contains("react") || text.contains("ui") || text.contains("frontend") {
        "frontend_ui_bug"
    } else if text.contains("doc") || text.contains("readme") {
        "docs_update"
    } else if text.contains("test") || text.contains("coverage") {
        "test_coverage"
    } else if text.contains("dependency") || text.contains("upgrade") {
        "dependency_upgrade"
    } else {
        "unknown_task"
    }
}

impl ExpectedOutcome {
    fn reject_reasons_contains(&self, reason: &str) -> bool {
        self.reject_reasons.iter().any(|value| value == reason)
    }
}

impl SampleFeedback {
    fn has_feedback(&self) -> bool {
        self.shown_count > 0
            || self.read_count > 0
            || self.prepared_count > 0
            || self.dismissed
            || self.done
    }
}

pub fn profile_coverage(report: &EvaluationReport) -> BTreeMap<String, usize> {
    report
        .datasets
        .iter()
        .map(|dataset| (dataset.profile.clone(), dataset.metrics.samples))
        .collect()
}

pub fn write_offline_report_snapshot(report: &EvaluationReport, output_dir: &Path) -> Result<()> {
    fs::create_dir_all(output_dir)?;
    atomic_write(
        &output_dir.join("metrics.json"),
        serde_json::to_vec_pretty(report)?,
    )?;
    atomic_write(
        &output_dir.join("report.md"),
        offline_markdown_report(report),
    )?;
    atomic_write(
        &output_dir.join("visible.jsonl"),
        offline_visible_jsonl(report),
    )?;
    Ok(())
}

pub fn write_live_report_snapshot(report: &LiveEvaluationReport, output_dir: &Path) -> Result<()> {
    fs::create_dir_all(output_dir)?;
    atomic_write(
        &output_dir.join("metrics.json"),
        serde_json::to_vec_pretty(report)?,
    )?;
    atomic_write(&output_dir.join("report.md"), live_markdown_report(report))?;
    atomic_write(
        &output_dir.join("visible.jsonl"),
        live_visible_jsonl(report),
    )?;
    Ok(())
}

fn offline_markdown_report(report: &EvaluationReport) -> String {
    let mut output = String::new();
    output.push_str("# Recommendation Evaluation\n\n");
    output.push_str("Generated by `issue-finder eval recommendation --offline` from deterministic offline fixtures. It records ranking pipeline behavior without network access.\n\n");
    output.push_str("## Overall Metrics\n\n");
    output.push_str(&format!(
        "- samples: {}\n- visible: {}\n- precision@5: {:.2}\n- precision@10: {:.2}\n- visible fill rate: {:.2}\n- reject leakage: {}\n- profile mismatch leakage: {}\n- stale high-rank leakage: {}\n- competition leakage: {}\n- dashboard noise leakage: {}\n- ranking inversions: {}\n- feedback cooldown: {}/{}\n- dispatch outcome replay: {}/{}\n- fallback fill rate: {:.2}\n\n",
        report.overall.samples,
        report.overall.visible,
        report.overall.precision_at5,
        report.overall.precision_at10,
        report.overall.visible_fill_rate,
        report.overall.reject_leakage,
        report.overall.profile_mismatch_leakage,
        report.overall.stale_high_rank_leakage,
        report.overall.competition_leakage,
        report.overall.dashboard_noise_leakage,
        report.overall.ranking_inversions,
        report.overall.feedback_cooldown_passes,
        report.overall.feedback_cooldown_total,
        report.overall.dispatch_outcome_passes,
        report.overall.dispatch_outcome_total,
        report.overall.fallback_fill_rate
    ));
    output.push_str("## Dataset Metrics\n\n");
    output.push_str(
        "| dataset | profile | samples | visible | p@5 | p@10 | inversions | failures |\n",
    );
    output.push_str("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |\n");
    for dataset in &report.datasets {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {:.2} | {:.2} | {} | {} |\n",
            dataset.dataset,
            dataset.profile,
            dataset.metrics.samples,
            dataset.metrics.visible,
            dataset.metrics.precision_at5,
            dataset.metrics.precision_at10,
            dataset.metrics.ranking_inversions,
            dataset.failures.len()
        ));
    }
    output.push_str("\n## Known Failure Signals\n\n");
    for dataset in &report.datasets {
        if dataset.failures.is_empty() {
            continue;
        }
        output.push_str(&format!("### {}\n\n", dataset.dataset));
        for failure in &dataset.failures {
            output.push_str(&format!("- `{}`: {}\n", failure.sample_id, failure.reason));
        }
        output.push('\n');
    }
    finish_markdown(output)
}

fn offline_visible_jsonl(report: &EvaluationReport) -> String {
    let mut output = String::new();
    for dataset in &report.datasets {
        for item in dataset
            .ranked
            .iter()
            .filter(|item| item.visibility == "visible")
        {
            let row = serde_json::json!({
                "dataset": dataset.dataset,
                "profile": dataset.profile,
                "rank": item.rank,
                "sampleId": item.sample_id,
                "key": item.key,
                "title": item.title,
                "expectedQuality": item.expected_quality,
                "expectedBehavior": item.expected_behavior,
                "finalFeedScore": item.final_feed_score,
                "freshnessBoost": item.freshness_boost,
                "memoryAdjustment": item.memory_adjustment,
                "profileFit": item.profile_fit,
                "sourceTier": item.source_tier,
            });
            output.push_str(&serde_json::to_string(&row).expect("visible row should serialize"));
            output.push('\n');
        }
    }
    output
}

fn live_markdown_report(report: &LiveEvaluationReport) -> String {
    let mut output = String::new();
    output.push_str("# Live Recommendation Evaluation\n\n");
    output.push_str("Generated by `issue-finder eval recommendation --live`. Manual review columns are placeholders and must be filled after reading issue body/comments.\n\n");
    output.push_str("## Summary\n\n");
    output.push_str(&format!(
        "- profiles: {}\n- limit: {}\n- refresh: {}\n- visible range: {}-{}\n- total visible: {}\n- total network requests: {}\n- budget exhausted profiles: {}\n\n",
        report.summary.profiles,
        report.limit,
        report.refresh,
        report.summary.min_visible,
        report.summary.max_visible,
        report.summary.total_visible,
        report.summary.total_network_requests,
        report.summary.budget_exhausted_profiles
    ));
    output.push_str("## Profiles\n\n");
    output.push_str(
        "| profile | visible | discovery | filtered | network requests | budget exhausted |\n",
    );
    output.push_str("| --- | ---: | ---: | ---: | ---: | ---: |\n");
    for profile in &report.profiles {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            profile.profile,
            profile.visible,
            profile.discovery_count,
            profile.filtered_count,
            profile.api_budget.total_network_requests,
            profile.api_budget.budget_exhausted.len()
        ));
    }
    output.push_str("\n## Manual Review\n\n");
    for profile in &report.profiles {
        output.push_str(&format!("### {}\n\n", profile.profile));
        output.push_str("| rank | issue | profileFit | visibility | competition warnings | manualQuality | manualNotes |\n");
        output.push_str("| ---: | --- | ---: | --- | --- | --- | --- |\n");
        for item in &profile.candidates {
            output.push_str(&format!(
                "| {} | {} | {} | {} | {} |  |  |\n",
                item.rank,
                item.key,
                item.profile_fit,
                item.visibility,
                item.competition.warnings.join("; ")
            ));
        }
        output.push('\n');
    }
    finish_markdown(output)
}

fn finish_markdown(mut output: String) -> String {
    while output.ends_with("\n\n") {
        output.pop();
    }
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

fn live_visible_jsonl(report: &LiveEvaluationReport) -> String {
    let mut output = String::new();
    for profile in &report.profiles {
        for item in &profile.candidates {
            let row = serde_json::to_string(item).expect("live visible row should serialize");
            output.push_str(&row);
            output.push('\n');
        }
    }
    output
}

fn live_candidate_summary(rank: usize, item: &RankedValueIssue) -> LiveCandidateSummary {
    LiveCandidateSummary {
        rank,
        key: format!("{}#{}", item.issue.repo_full_name, item.issue.number),
        title: item.issue.title.clone(),
        url: item.issue.url.clone(),
        profile_fit: item.value_assessment.profile_fit_score,
        visibility: item.recommendation.visibility.to_string(),
        source_tier: source_tier_from_explanation(&item.explanation),
        risk_tags: item
            .value_assessment
            .risk_tags
            .iter()
            .map(ToString::to_string)
            .collect(),
        competition: item.enriched_issue.competition.clone(),
        missing_evidence: item.value_assessment.missing_evidence.clone(),
        manual_quality: None,
        manual_notes: String::new(),
    }
}

fn source_tier_from_explanation(explanation: &[String]) -> Option<String> {
    explanation.iter().find_map(|item| {
        item.split_once("discovery trust `")
            .and_then(|(_, rest)| rest.split_once('`'))
            .map(|(tier, _)| tier.to_string())
    })
}

fn isolated_live_home(output_dir: &Path, profile_name: &str) -> Result<LiveEvalHome> {
    let stamp = Utc::now().timestamp_millis();
    let safe_profile = profile_name.replace('/', "__");
    let home = std::env::temp_dir().join(format!("issue-finder-eval-{stamp}-{safe_profile}"));
    fs::create_dir_all(&home)
        .with_context(|| format!("unable to create live eval home for {profile_name}"))?;
    fs::create_dir_all(output_dir)?;
    Ok(LiveEvalHome {
        paths: IssueFinderPaths {
            config: home.join("config.toml"),
            cache_dir: home.join("cache"),
            workspaces_dir: home.join("workspaces"),
            inbox_dir: home.join("inbox"),
            reports_dir: home.join("reports"),
            home,
        },
    })
}

fn timestamped_samples(prefix: &str, count: usize) -> Vec<TimestampedSample> {
    (0..count)
        .map(|index| TimestampedSample {
            source_ref: format!("{prefix}.{index}"),
            actor: Some(format!("actor-{index}")),
            timestamp: Some(timestamp_or_age(None, (index % 20) as i64)),
        })
        .collect()
}

fn timestamp_or_age(timestamp: Option<&str>, age_days: i64) -> String {
    timestamp
        .map(ToString::to_string)
        .unwrap_or_else(|| (Utc::now() - Duration::days(age_days)).to_rfc3339())
}

fn stable_id(value: &str) -> u64 {
    value.bytes().fold(0u64, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte as u64)
    })
}

fn default_limit() -> usize {
    15
}

fn default_source_tier() -> String {
    "gfi_trusted".to_string()
}

fn default_created_age_days() -> i64 {
    30
}

fn default_updated_age_days() -> i64 {
    7
}

fn default_pushed_age_days() -> i64 {
    7
}

fn default_repo_created_age_days() -> i64 {
    900
}

fn default_author_association() -> String {
    "NONE".to_string()
}

fn default_comment_association() -> String {
    "NONE".to_string()
}

fn default_comment_age_days() -> i64 {
    3
}

fn default_timeline_missing() -> bool {
    false
}
