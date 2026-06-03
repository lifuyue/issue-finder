use chrono::{Duration, Utc};
use patchbay_cli::config::ProfileConfig;
use patchbay_cli::github::GitHubIssue;
use patchbay_cli::github_enrichment::{EnrichedIssue, TimestampedSample};
use patchbay_cli::value_scoring::{
    aggregate_signals, assess_issue, RecommendationCategory, RiskTag, ScoreBand,
};
use patchbay_cli::value_signals::{SignalAxis, ValueSignal, ValueSignalKind};

#[test]
fn high_attention_and_high_execution_classifies_agent_ready() {
    let enriched = fixture()
        .stars(2_500)
        .forks(220)
        .recent_stargazers(20)
        .recent_forks(12)
        .maintainer_response()
        .good_first_actionable_issue()
        .build();

    let assessment = assess_issue(&enriched, &matching_profile());

    assert_eq!(assessment.attention_band, ScoreBand::High);
    assert_eq!(assessment.execution_band, ScoreBand::High);
    assert_eq!(
        assessment.recommendation_category,
        RecommendationCategory::AgentReadyHighValue
    );
    assert_has_signal(&assessment.signals, ValueSignalKind::EstablishedImpact);
    assert_has_signal(&assessment.signals, ValueSignalKind::GrowthMomentum);
    assert_has_signal(&assessment.signals, ValueSignalKind::ReproductionSteps);
}

#[test]
fn high_attention_low_depth_keeps_visibility_but_marks_risks() {
    let enriched = fixture()
        .stars(2_500)
        .forks(220)
        .recent_stargazers(20)
        .body(
            "No Code Required. This contribution can be done from your browser in under 60 seconds. Add Grammar Point content.",
        )
        .title("Add new Grammar Point")
        .labels(["good first issue", "hacktoberfest"])
        .build();

    let assessment = assess_issue(&enriched, &matching_profile());

    assert_eq!(assessment.attention_band, ScoreBand::High);
    assert_eq!(
        assessment.recommendation_category,
        RecommendationCategory::HighAttentionLowDepth
    );
    assert!(assessment.risk_tags.contains(&RiskTag::NoCodeRequired));
    assert!(assessment.risk_tags.contains(&RiskTag::MicroContribution));
    assert!(assessment.risk_tags.contains(&RiskTag::ContentFill));
}

#[test]
fn low_attention_but_clear_issue_is_niche_but_actionable() {
    let enriched = fixture()
        .stars(0)
        .forks(0)
        .recent_repo_activity()
        .good_first_actionable_issue()
        .topics(["rust", "cli", "parser"])
        .build();

    let assessment = assess_issue(&enriched, &matching_profile());

    assert_ne!(assessment.attention_band, ScoreBand::High);
    assert_eq!(assessment.execution_band, ScoreBand::High);
    assert_eq!(
        assessment.recommendation_category,
        RecommendationCategory::NicheButActionable
    );
}

#[test]
fn high_attention_with_template_noise_needs_triage() {
    let enriched = fixture()
        .stars(150)
        .forks(500)
        .recent_stargazers(50)
        .recent_forks(50)
        .comments_count(60)
        .open_issues(1_200)
        .body(
            "Create the new test file ProfileOptimizerModal.mock-integrations.test.tsx. Variation 9 for GSSoC. Complete coverage for asynchronous service layer mocking.",
        )
        .title("test(ProfileOptimizerModal-mock-integrations): verify Variation 9")
        .labels(["good first issue", "GSSoC 2026", "tests"])
        .build();

    let assessment = assess_issue(&enriched, &matching_profile());

    assert_eq!(assessment.attention_band, ScoreBand::High);
    assert_eq!(
        assessment.recommendation_category,
        RecommendationCategory::NeedsTriage
    );
    assert!(assessment.risk_tags.contains(&RiskTag::TemplateLike));
    assert!(assessment.risk_tags.contains(&RiskTag::EventNoise));
    assert!(assessment.risk_tags.contains(&RiskTag::HighTriageLoad));
}

#[test]
fn profile_fit_is_token_aware_for_short_aliases() {
    let enriched = fixture()
        .body("This issue mentions strings and status words but does not reference the configured technologies.")
        .title("Update user settings")
        .repo_description("General utility")
        .topics([])
        .build();

    let matching = assess_issue(
        &enriched,
        &ProfileConfig {
            tech_stack: vec!["Rust".to_string(), "TypeScript".to_string()],
            keywords: Vec::new(),
        },
    );

    assert_eq!(matching.profile_fit_score, 0);
}

#[test]
fn aggregate_signals_applies_axis_scores_and_formula() {
    let assessment = aggregate_signals(
        vec![
            signal(
                ValueSignalKind::EstablishedImpact,
                SignalAxis::Attention,
                35,
            ),
            signal(ValueSignalKind::GrowthMomentum, SignalAxis::Attention, 35),
            signal(ValueSignalKind::IssueClarity, SignalAxis::Execution, 25),
            signal(
                ValueSignalKind::ReproductionSteps,
                SignalAxis::Execution,
                25,
            ),
            signal(ValueSignalKind::IssueFit, SignalAxis::ProfileFit, 50),
        ],
        vec![],
        &fixture().build(),
    );

    assert_eq!(assessment.attention_score, 70);
    assert_eq!(assessment.execution_score, 50);
    assert_eq!(assessment.profile_fit_score, 50);
    assert!(assessment.final_rank_score > 50);
}

fn fixture() -> EnrichedIssueFixture {
    EnrichedIssueFixture::default()
}

#[derive(Debug, Clone)]
struct EnrichedIssueFixture {
    title: String,
    body: String,
    labels: Vec<String>,
    repo_description: String,
    repo_stars: u64,
    forks: u64,
    subscribers: Option<u64>,
    open_issues: Option<u64>,
    issue_updated_at: String,
    repo_pushed_at: Option<String>,
    topics: Vec<String>,
    comments_count: u64,
    recent_stargazers: usize,
    recent_forks: usize,
    maintainer_response: bool,
}

impl Default for EnrichedIssueFixture {
    fn default() -> Self {
        Self {
            title: "Fix Rust CLI parser".to_string(),
            body: actionable_body(),
            labels: vec!["good first issue".to_string()],
            repo_description: "Rust CLI developer tools".to_string(),
            repo_stars: 0,
            forks: 0,
            subscribers: None,
            open_issues: Some(12),
            issue_updated_at: recent_timestamp(),
            repo_pushed_at: Some(recent_timestamp()),
            topics: vec!["rust".to_string(), "cli".to_string()],
            comments_count: 1,
            recent_stargazers: 0,
            recent_forks: 0,
            maintainer_response: false,
        }
    }
}

impl EnrichedIssueFixture {
    fn title(mut self, value: &str) -> Self {
        self.title = value.to_string();
        self
    }

    fn body(mut self, value: &str) -> Self {
        self.body = value.to_string();
        self
    }

    fn repo_description(mut self, value: &str) -> Self {
        self.repo_description = value.to_string();
        self
    }

    fn labels<const N: usize>(mut self, values: [&str; N]) -> Self {
        self.labels = values.into_iter().map(ToOwned::to_owned).collect();
        self
    }

    fn stars(mut self, value: u64) -> Self {
        self.repo_stars = value;
        self
    }

    fn forks(mut self, value: u64) -> Self {
        self.forks = value;
        self
    }

    fn open_issues(mut self, value: u64) -> Self {
        self.open_issues = Some(value);
        self
    }

    fn comments_count(mut self, value: u64) -> Self {
        self.comments_count = value;
        self
    }

    fn topics<const N: usize>(mut self, values: [&str; N]) -> Self {
        self.topics = values.into_iter().map(ToOwned::to_owned).collect();
        self
    }

    fn recent_repo_activity(mut self) -> Self {
        self.repo_pushed_at = Some(recent_timestamp());
        self
    }

    fn good_first_actionable_issue(mut self) -> Self {
        self.title = "Fix Rust CLI parser regression".to_string();
        self.body = actionable_body();
        self.labels = vec!["good first issue".to_string()];
        self
    }

    fn recent_stargazers(mut self, count: usize) -> Self {
        self.recent_stargazers = count;
        self
    }

    fn recent_forks(mut self, count: usize) -> Self {
        self.recent_forks = count;
        self
    }

    fn maintainer_response(mut self) -> Self {
        self.maintainer_response = true;
        self
    }

    fn build(self) -> EnrichedIssue {
        let issue = GitHubIssue {
            id: 1,
            number: 1,
            title: self.title,
            body: self.body,
            labels: self.labels,
            url: "https://github.com/owner/repo/issues/1".to_string(),
            repo_full_name: "owner/repo".to_string(),
            repo_name: "repo".to_string(),
            repo_description: self.repo_description,
            repo_stars: self.repo_stars,
            created_at: self.issue_updated_at.clone(),
            updated_at: self.issue_updated_at,
        };
        let mut enriched = EnrichedIssue::from_issue(&issue);
        enriched.repository.stars = issue.repo_stars;
        enriched.repository.forks = self.forks;
        enriched.repository.subscribers = self.subscribers;
        enriched.repository.open_issues = self.open_issues;
        enriched.repository.pushed_at = self.repo_pushed_at.clone();
        enriched.repository.topics = self.topics;
        enriched.issue.comments_count = self.comments_count;
        enriched.activity.recent_issue_activity = is_recent_timestamp(&enriched.issue.updated_at);
        enriched.activity.recent_repo_activity = self
            .repo_pushed_at
            .as_deref()
            .map(is_recent_timestamp)
            .unwrap_or(false);
        enriched.activity.maintainer_recent_response = self.maintainer_response;
        enriched.growth.recent_stargazer_sample =
            timestamp_samples("repo:stargazers.sample_recent_100", self.recent_stargazers);
        enriched.growth.newest_fork_sample =
            timestamp_samples("repo:forks.sample_newest_100", self.recent_forks);
        enriched
    }
}

fn matching_profile() -> ProfileConfig {
    ProfileConfig {
        tech_stack: vec!["Rust".to_string()],
        keywords: vec!["cli".to_string(), "parser".to_string()],
    }
}

fn actionable_body() -> String {
    "Steps to reproduce: run `cargo test`. The parser currently panics when a subcommand contains repeated flags. Expected behavior is a graceful error in src/main.rs, actual behavior is a panic with a short stack trace. Suggested fix: guard empty input and verify with tests.".to_string()
}

fn recent_timestamp() -> String {
    Utc::now().to_rfc3339()
}

fn is_recent_timestamp(value: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|timestamp| Utc::now() - timestamp.with_timezone(&Utc) <= Duration::days(14))
        .unwrap_or(false)
}

fn timestamp_samples(prefix: &str, count: usize) -> Vec<TimestampedSample> {
    (0..count)
        .map(|index| TimestampedSample {
            source_ref: format!("{prefix}.{index}"),
            actor: Some(format!("actor-{index}")),
            timestamp: Some(recent_timestamp()),
        })
        .collect()
}

fn signal(kind: ValueSignalKind, axis: SignalAxis, delta: i32) -> ValueSignal {
    ValueSignal {
        kind,
        axis,
        score_delta: delta,
        summary: "summary".to_string(),
        evidence_refs: vec!["issue:body".to_string()],
    }
}

fn assert_has_signal(signals: &[ValueSignal], kind: ValueSignalKind) {
    assert!(
        signals.iter().any(|signal| signal.kind == kind),
        "expected signal {kind:?}, got {signals:?}"
    );
}
