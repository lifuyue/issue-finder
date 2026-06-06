use std::collections::{BTreeMap, HashMap};

use chrono::{Duration, Utc};
use issue_finder::competition::{CompetitionBand, CompetitionFacts};
use issue_finder::config::ProfileConfig;
use issue_finder::github::GitHubIssue;
use issue_finder::github_enrichment::{
    EnrichedComment, EnrichedIssue, EnrichedParticipants, TimestampedSample,
};
use issue_finder::recommendation::events::IssueKey;
use issue_finder::recommendation::feed_ranker::{
    apply_recommendation_assessments, displayable, sort_by_feed,
};
use issue_finder::recommendation::state::RecommendationIssueState;
use issue_finder::value_scoring::{assess_issue, RankedValueIssue, RiskTag};
use serde::{Deserialize, Serialize};

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
    pub profile_fit: i32,
    pub visibility: String,
    pub source_tier: String,
}

struct RankedEvaluationSample<'a> {
    sample: &'a EvaluationSample,
    ranked: RankedValueIssue,
    source_tier: String,
}

pub fn load_dataset(raw: &str) -> EvaluationDataset {
    serde_json::from_str(raw).expect("recommendation eval dataset should parse")
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
            tech_stack: vec!["TypeScript".to_string(), "JavaScript".to_string()],
            keywords: vec![
                "frontend".to_string(),
                "react".to_string(),
                "ui".to_string(),
                "browser".to_string(),
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
                "developer-tools".to_string(),
            ],
        },
        "devops_infra" => ProfileConfig {
            tech_stack: vec!["Go".to_string(), "TypeScript".to_string()],
            keywords: vec![
                "kubernetes".to_string(),
                "docker".to_string(),
                "ci".to_string(),
                "infrastructure".to_string(),
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
        .filter(|item| item.sample.expected.reject_reasons_contains("stale"))
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
            last_seen_comments_count: Some(self.issue.comments_count),
        };
        let mut states = HashMap::new();
        states.insert(state.issue_key.clone(), state);
        states
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
