use chrono::Utc;
use patchbay_cli::config::ProfileConfig;
use patchbay_cli::github::GitHubIssue;
use patchbay_cli::github_enrichment::{EnrichedIssue, TimestampedSample};
use patchbay_cli::value_scoring::{assess_issue, RecommendationCategory, RiskTag, ScoreBand};
use serde::Deserialize;

const SAMPLES: &str = include_str!("fixtures/recommendation_quality/samples.json");

#[derive(Debug, Deserialize)]
struct QualitySamples {
    samples: Vec<QualitySample>,
}

#[derive(Debug, Deserialize)]
struct QualitySample {
    id: String,
    issue: SampleIssue,
    enrichment: SampleEnrichment,
    expected: ExpectedRecommendation,
}

#[derive(Debug, Deserialize)]
struct SampleIssue {
    repo_full_name: String,
    repo_name: String,
    number: u64,
    title: String,
    body: String,
    labels: Vec<String>,
    repo_description: String,
    repo_stars: u64,
}

#[derive(Debug, Deserialize)]
struct SampleEnrichment {
    forks: u64,
    recent_stars: usize,
    recent_forks: usize,
    recent_repo_activity: bool,
    maintainer_attention: bool,
    comments_count: u64,
    open_issues: u64,
    topics: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedRecommendation {
    attention_band: ScoreBand,
    execution_band: ScoreBand,
    category: RecommendationCategory,
    required_risk_tags: Vec<RiskTag>,
    forbidden_risk_tags: Vec<RiskTag>,
}

#[test]
fn recommendation_quality_samples_match_product_rubric() {
    let samples = serde_json::from_str::<QualitySamples>(SAMPLES).unwrap();
    assert!(samples.samples.len() >= 9);

    for sample in samples.samples {
        let enriched = sample.enriched_issue();
        let assessment = assess_issue(&enriched, &default_profile());

        assert_eq!(
            assessment.attention_band, sample.expected.attention_band,
            "{} attention mismatch: {assessment:#?}",
            sample.id
        );
        assert_eq!(
            assessment.execution_band, sample.expected.execution_band,
            "{} execution mismatch: {assessment:#?}",
            sample.id
        );
        assert_eq!(
            assessment.recommendation_category, sample.expected.category,
            "{} category mismatch: {assessment:#?}",
            sample.id
        );

        for tag in &sample.expected.required_risk_tags {
            assert!(
                assessment.risk_tags.contains(tag),
                "{} missing required risk tag {tag}",
                sample.id
            );
        }
        for tag in &sample.expected.forbidden_risk_tags {
            assert!(
                !assessment.risk_tags.contains(tag),
                "{} unexpectedly had forbidden risk tag {tag}",
                sample.id
            );
        }

        if assessment.recommendation_category == RecommendationCategory::AgentReadyHighValue {
            assert_eq!(
                assessment.execution_band,
                ScoreBand::High,
                "{} agent-ready recommendation must have high execution",
                sample.id
            );
        }
        if assessment.recommendation_category == RecommendationCategory::HighAttentionLowDepth {
            assert_eq!(
                assessment.attention_band,
                ScoreBand::High,
                "{} low-depth recommendation must have high attention",
                sample.id
            );
            assert!(
                assessment.risk_tags.iter().any(|tag| {
                    matches!(
                        tag,
                        RiskTag::NoCodeRequired
                            | RiskTag::MicroContribution
                            | RiskTag::ContentFill
                            | RiskTag::ThinTask
                    )
                }),
                "{} low-depth recommendation must carry a low-depth tag",
                sample.id
            );
        }
    }
}

impl QualitySample {
    fn enriched_issue(&self) -> EnrichedIssue {
        let now = Utc::now().to_rfc3339();
        let issue = GitHubIssue {
            id: self.issue.number,
            number: self.issue.number,
            title: self.issue.title.clone(),
            body: self.issue.body.clone(),
            labels: self.issue.labels.clone(),
            url: format!(
                "https://github.com/{}/issues/{}",
                self.issue.repo_full_name, self.issue.number
            ),
            repo_full_name: self.issue.repo_full_name.clone(),
            repo_name: self.issue.repo_name.clone(),
            repo_description: self.issue.repo_description.clone(),
            repo_stars: self.issue.repo_stars,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        let mut enriched = EnrichedIssue::from_issue(&issue);
        enriched.repository.stars = issue.repo_stars;
        enriched.repository.forks = self.enrichment.forks;
        enriched.repository.open_issues = Some(self.enrichment.open_issues);
        enriched.repository.topics = self.enrichment.topics.clone();
        enriched.repository.pushed_at = self.enrichment.recent_repo_activity.then(|| now.clone());
        enriched.issue.comments_count = self.enrichment.comments_count;
        enriched.activity.recent_issue_activity = true;
        enriched.activity.recent_repo_activity = self.enrichment.recent_repo_activity;
        enriched.activity.maintainer_recent_response = self.enrichment.maintainer_attention;
        enriched.growth.recent_stargazer_sample = timestamp_samples(
            "repo:stargazers.sample_recent_100",
            self.enrichment.recent_stars,
        );
        enriched.growth.newest_fork_sample =
            timestamp_samples("repo:forks.sample_newest_100", self.enrichment.recent_forks);
        enriched
    }
}

fn default_profile() -> ProfileConfig {
    ProfileConfig {
        tech_stack: vec!["Rust".to_string(), "TypeScript".to_string()],
        keywords: vec!["cli".to_string(), "developer-tools".to_string()],
    }
}

fn timestamp_samples(prefix: &str, count: usize) -> Vec<TimestampedSample> {
    (0..count)
        .map(|index| TimestampedSample {
            source_ref: format!("{prefix}.{index}"),
            actor: Some(format!("actor-{index}")),
            timestamp: Some(Utc::now().to_rfc3339()),
        })
        .collect()
}
