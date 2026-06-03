use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::config::ProfileConfig;
use crate::github_enrichment::{fork_velocity, star_velocity, EnrichedIssue};
use crate::scoring::normalize;
use crate::value_scoring::RiskTag;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValueSignalKind {
    EstablishedImpact,
    GrowthMomentum,
    RepoActivity,
    MaintainerAttention,
    ExternalReward,
    ContributionWindow,
    IssueClarity,
    CodePathReference,
    ReproductionSteps,
    AcceptanceCriteria,
    SuggestedFix,
    ValidationHint,
    ExecutionReadiness,
    IssueFit,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SignalAxis {
    Attention,
    Execution,
    ProfileFit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValueSignal {
    pub kind: ValueSignalKind,
    pub axis: SignalAxis,
    pub score_delta: i32,
    pub summary: String,
    pub evidence_refs: Vec<String>,
}

pub fn build_value_signals(enriched: &EnrichedIssue, profile: &ProfileConfig) -> Vec<ValueSignal> {
    let mut signals = Vec::new();

    if let Some(signal) = established_impact_signal(enriched) {
        signals.push(signal);
    }
    if let Some(signal) = growth_momentum_signal(enriched) {
        signals.push(signal);
    }
    if let Some(signal) = repo_activity_signal(enriched) {
        signals.push(signal);
    }
    if let Some(signal) = maintainer_attention_signal(enriched) {
        signals.push(signal);
    }
    if let Some(signal) = external_reward_signal(enriched) {
        signals.push(signal);
    }
    if let Some(signal) = contribution_window_signal(enriched) {
        signals.push(signal);
    }

    signals.extend(execution_signals(enriched));

    if let Some(signal) = issue_fit_signal(enriched, profile) {
        signals.push(signal);
    }

    signals
}

pub fn build_risk_tags(enriched: &EnrichedIssue) -> Vec<RiskTag> {
    let text = normalized_issue_text(enriched);
    let mut tags = Vec::new();

    if contains_any(
        &text,
        &[
            "no code required",
            "no coding required",
            "no prerequisites needed",
            "do not need to clone",
            "browser in under",
        ],
    ) {
        tags.push(RiskTag::NoCodeRequired);
    }
    if contains_any(
        &text,
        &[
            "under 60 seconds",
            "under 1 minute",
            "less than 1 minute",
            "1 minute",
            "<1 minute",
            "quick contribution",
        ],
    ) {
        tags.push(RiskTag::MicroContribution);
    }
    if contains_any(
        &text,
        &[
            "add grammar point",
            "add trivia question",
            "add new trivia",
            "add new grammar",
            "content contribution",
            "fill in",
            "glossary",
            "related terms",
        ],
    ) {
        tags.push(RiskTag::ContentFill);
    }
    if contains_any(
        &text,
        &[
            "variation",
            "generated issue",
            "create the new test file",
            "complete coverage for",
            "mock integrations",
        ],
    ) {
        tags.push(RiskTag::TemplateLike);
    }
    if enriched
        .issue
        .labels
        .iter()
        .map(|label| normalize(label))
        .any(|label| contains_any(&label, &["gssoc", "gssoc26", "gssoc2026", "hacktoberfest"]))
    {
        tags.push(RiskTag::EventNoise);
    }
    if is_thin_task(enriched, &text) {
        tags.push(RiskTag::ThinTask);
    }
    if enriched.issue.comments_count >= 20
        || enriched.repository.open_issues.unwrap_or(0) >= 500
        || contains_any(
            &text,
            &["293 test files", "audit stale", "complete coverage"],
        )
    {
        tags.push(RiskTag::HighTriageLoad);
    }
    if !enriched.activity.maintainer_recent_response {
        tags.push(RiskTag::MissingMaintainerSignal);
    }
    if !has_validation_hint(&text) {
        tags.push(RiskTag::WeakValidationPath);
    }

    tags.sort_by_key(|tag| tag.to_string());
    tags.dedup();
    tags
}

pub fn risk_penalty(tags: &[RiskTag]) -> i32 {
    tags.iter()
        .map(|tag| match tag {
            RiskTag::NoCodeRequired => 35,
            RiskTag::MicroContribution => 25,
            RiskTag::ContentFill => 25,
            RiskTag::TemplateLike => 20,
            RiskTag::EventNoise => 18,
            RiskTag::ThinTask => 18,
            RiskTag::HighTriageLoad => 25,
            RiskTag::MissingMaintainerSignal => 8,
            RiskTag::WeakValidationPath => 10,
        })
        .sum::<i32>()
        .clamp(0, 100)
}

fn established_impact_signal(enriched: &EnrichedIssue) -> Option<ValueSignal> {
    let stars = enriched.repository.stars;
    let forks = enriched.repository.forks;
    let subscribers = enriched.repository.subscribers.unwrap_or(0);
    let delta = if stars >= 10_000 || forks >= 1_000 {
        35
    } else if stars >= 2_000 || forks >= 200 {
        28
    } else if stars >= 500 || forks >= 50 || subscribers >= 100 {
        20
    } else if stars >= 50 || forks >= 25 {
        12
    } else {
        return None;
    };

    Some(ValueSignal {
        kind: ValueSignalKind::EstablishedImpact,
        axis: SignalAxis::Attention,
        score_delta: delta,
        summary: format!("Repository has visible impact with {stars} stars and {forks} forks."),
        evidence_refs: refs(["repo:stargazers_count", "repo:forks_count"]),
    })
}

fn growth_momentum_signal(enriched: &EnrichedIssue) -> Option<ValueSignal> {
    let now = Utc::now();
    let stars_7d = star_velocity(&enriched.growth.recent_stargazer_sample, 7, now);
    let stars_30d = star_velocity(&enriched.growth.recent_stargazer_sample, 30, now);
    let forks_30d = fork_velocity(&enriched.growth.newest_fork_sample, 30, now);
    let attention_to_size = stars_30d as f64 / (enriched.repository.stars.max(1) as f64);

    let delta = if stars_7d >= 15 || stars_30d >= 40 || forks_30d >= 25 {
        35
    } else if forks_30d >= 8 || attention_to_size >= 0.15 {
        30
    } else if stars_7d >= 5 || stars_30d >= 15 || forks_30d >= 3 || attention_to_size >= 0.05 {
        22
    } else if stars_30d > 0 || forks_30d > 0 {
        12
    } else {
        return None;
    };

    Some(ValueSignal {
        kind: ValueSignalKind::GrowthMomentum,
        axis: SignalAxis::Attention,
        score_delta: delta,
        summary: format!(
            "Sampled recent attention: {stars_7d} stars in 7d, {stars_30d} stars in 30d, {forks_30d} forks in 30d."
        ),
        evidence_refs: refs([
            "repo:stargazers.sample_recent_100",
            "repo:forks.sample_newest_100",
        ]),
    })
}

fn repo_activity_signal(enriched: &EnrichedIssue) -> Option<ValueSignal> {
    if !enriched.activity.recent_repo_activity {
        return None;
    }
    Some(ValueSignal {
        kind: ValueSignalKind::RepoActivity,
        axis: SignalAxis::Attention,
        score_delta: 10,
        summary: "Repository was pushed recently.".to_string(),
        evidence_refs: refs(["repo:pushed_at"]),
    })
}

fn maintainer_attention_signal(enriched: &EnrichedIssue) -> Option<ValueSignal> {
    if !enriched.activity.maintainer_recent_response {
        return None;
    }
    Some(ValueSignal {
        kind: ValueSignalKind::MaintainerAttention,
        axis: SignalAxis::Attention,
        score_delta: 12,
        summary: "A maintainer recently responded in the issue thread.".to_string(),
        evidence_refs: refs(["issue:comments", "issue:author_association"]),
    })
}

fn external_reward_signal(enriched: &EnrichedIssue) -> Option<ValueSignal> {
    let labels = normalize(&enriched.issue.labels.join(" "));
    let text = normalized_issue_text(enriched);
    if !contains_any(&labels, &["bounty", "ai agent friendly"])
        && !contains_any(&text, &["bounty", "ai agent friendly"])
    {
        return None;
    }
    Some(ValueSignal {
        kind: ValueSignalKind::ExternalReward,
        axis: SignalAxis::Attention,
        score_delta: 25,
        summary: "Issue carries explicit external reward or agent-friendly contribution signals."
            .to_string(),
        evidence_refs: refs(["issue:labels", "issue:body"]),
    })
}

fn contribution_window_signal(enriched: &EnrichedIssue) -> Option<ValueSignal> {
    if !enriched.activity.recent_issue_activity {
        return None;
    }
    Some(ValueSignal {
        kind: ValueSignalKind::ContributionWindow,
        axis: SignalAxis::Attention,
        score_delta: 8,
        summary: "Issue activity is recent enough for an active contribution window.".to_string(),
        evidence_refs: refs(["issue:updated_at"]),
    })
}

fn execution_signals(enriched: &EnrichedIssue) -> Vec<ValueSignal> {
    let text = normalized_issue_text(enriched);
    let mut signals = Vec::new();
    let body_len = enriched.issue.body.trim().len();

    if body_len >= 120 || has_actionable_language(&text) {
        let delta = if body_len >= 400 && has_actionable_language(&text) {
            25
        } else if body_len >= 120 {
            18
        } else {
            12
        };
        signals.push(ValueSignal {
            kind: ValueSignalKind::IssueClarity,
            axis: SignalAxis::Execution,
            score_delta: delta,
            summary: "Issue contains enough detail to start investigation.".to_string(),
            evidence_refs: refs(["issue:title", "issue:body"]),
        });
    }

    if has_file_path_reference(&enriched.issue.body) {
        signals.push(ValueSignal {
            kind: ValueSignalKind::CodePathReference,
            axis: SignalAxis::Execution,
            score_delta: 25,
            summary: "Issue references a likely code path or file.".to_string(),
            evidence_refs: refs(["issue:body"]),
        });
    }

    if contains_any(&text, &["steps to reproduce", "step to reproduce", "repro"]) {
        signals.push(ValueSignal {
            kind: ValueSignalKind::ReproductionSteps,
            axis: SignalAxis::Execution,
            score_delta: 25,
            summary: "Issue includes reproduction guidance.".to_string(),
            evidence_refs: refs(["issue:body"]),
        });
    }

    if contains_any(
        &text,
        &[
            "acceptance criteria",
            "expected behavior",
            "expected behaviour",
            "expected",
        ],
    ) {
        signals.push(ValueSignal {
            kind: ValueSignalKind::AcceptanceCriteria,
            axis: SignalAxis::Execution,
            score_delta: 20,
            summary: "Issue includes expected behavior or acceptance criteria.".to_string(),
            evidence_refs: refs(["issue:body"]),
        });
    }

    if contains_any(
        &text,
        &["suggested fix", "suggested solution", "fix should"],
    ) {
        signals.push(ValueSignal {
            kind: ValueSignalKind::SuggestedFix,
            axis: SignalAxis::Execution,
            score_delta: 15,
            summary: "Issue includes a suggested fix or implementation direction.".to_string(),
            evidence_refs: refs(["issue:body"]),
        });
    }

    if has_validation_hint(&text) {
        signals.push(ValueSignal {
            kind: ValueSignalKind::ValidationHint,
            axis: SignalAxis::Execution,
            score_delta: 15,
            summary: "Issue includes validation or test guidance.".to_string(),
            evidence_refs: refs(["issue:body"]),
        });
    }

    let good_first = enriched
        .issue
        .labels
        .iter()
        .any(|label| normalize(label).contains("good first issue"));
    if good_first || has_actionable_language(&text) {
        signals.push(ValueSignal {
            kind: ValueSignalKind::ExecutionReadiness,
            axis: SignalAxis::Execution,
            score_delta: if good_first && has_actionable_language(&text) {
                15
            } else {
                10
            },
            summary: "Issue has beginner-friendly or actionable execution signals.".to_string(),
            evidence_refs: refs(["issue:labels", "issue:body"]),
        });
    }

    signals
}

fn issue_fit_signal(enriched: &EnrichedIssue, profile: &ProfileConfig) -> Option<ValueSignal> {
    let searchable = normalize(&format!(
        "{} {} {} {} {}",
        enriched.issue.title,
        enriched.issue.body,
        enriched.repository.full_name,
        enriched.repository.description,
        enriched.repository.topics.join(" ")
    ));
    let matched = matching_profile_terms(&searchable, profile);
    if matched.is_empty() {
        return None;
    }
    let delta = (matched.len() as i32 * 18).min(100);
    Some(ValueSignal {
        kind: ValueSignalKind::IssueFit,
        axis: SignalAxis::ProfileFit,
        score_delta: delta,
        summary: format!("Matched profile terms: {}.", matched.join(", ")),
        evidence_refs: refs(["profile:terms", "issue:title", "repo:topics"]),
    })
}

fn matching_profile_terms(searchable: &str, profile: &ProfileConfig) -> Vec<String> {
    let searchable_tokens = searchable.split_whitespace().collect::<Vec<_>>();
    let mut matched = Vec::new();
    for term in profile_terms(profile) {
        let term_tokens = term.split_whitespace().collect::<Vec<_>>();
        let is_match = if term.len() < 3 {
            searchable_tokens.iter().any(|token| *token == term)
        } else if term_tokens.len() == 1 {
            searchable_tokens.iter().any(|token| *token == term)
        } else {
            searchable.contains(&term)
        };
        if is_match && !matched.contains(&term) {
            matched.push(term);
        }
    }
    matched.sort();
    matched
}

fn profile_terms(profile: &ProfileConfig) -> Vec<String> {
    let mut terms = Vec::new();
    for item in profile.tech_stack.iter().chain(profile.keywords.iter()) {
        let normalized = normalize(item);
        if !normalized.is_empty() {
            terms.push(normalized.clone());
        }
        terms.extend(aliases(&normalized).iter().map(|alias| alias.to_string()));
    }
    terms.sort();
    terms.dedup();
    terms
}

fn aliases(term: &str) -> &'static [&'static str] {
    match term {
        "typescript" => &["ts", "tsx"],
        "javascript" => &["js", "jsx"],
        "node js" => &["node", "nodejs", "npm"],
        "react" => &["jsx", "tsx", "component", "hooks"],
        "python" => &["py", "pytest"],
        "go" => &["golang"],
        "rust" => &["cargo", "rs"],
        "cli" => &["command line"],
        _ => &[],
    }
}

fn normalized_issue_text(enriched: &EnrichedIssue) -> String {
    normalize(&format!(
        "{} {} {}",
        enriched.issue.title,
        enriched.issue.body,
        enriched.issue.labels.join(" ")
    ))
}

fn has_actionable_language(text: &str) -> bool {
    contains_any(
        text,
        &[
            "steps to reproduce",
            "step to reproduce",
            "expected",
            "actual",
            "acceptance criteria",
            "stack trace",
            "repro",
            "suggested fix",
            "file",
        ],
    )
}

fn has_file_path_reference(text: &str) -> bool {
    text.split_whitespace()
        .map(|token| {
            token.trim_matches(|ch: char| {
                !ch.is_ascii_alphanumeric() && ch != '/' && ch != '.' && ch != '-' && ch != '_'
            })
        })
        .any(|token| {
            token.contains('/')
                && [
                    ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".md", ".json", ".css",
                    ".scss", ".html", ".sql",
                ]
                .iter()
                .any(|suffix| token.ends_with(suffix))
        })
}

fn has_validation_hint(text: &str) -> bool {
    contains_any(
        text,
        &[
            "test",
            "tests",
            "testing",
            "verify",
            "validation",
            "coverage",
            "chrome devtools",
            "emulate",
            "reproduce",
        ],
    )
}

fn is_thin_task(enriched: &EnrichedIssue, text: &str) -> bool {
    enriched.issue.body.trim().len() < 500
        && contains_any(
            text,
            &[
                "jsdoc",
                "add concise comments",
                "add comments",
                "short summary",
                "link this issue",
            ],
        )
}

fn contains_any(text: &str, values: &[&str]) -> bool {
    values.iter().any(|value| text.contains(value))
}

fn refs<const N: usize>(values: [&str; N]) -> Vec<String> {
    values.into_iter().map(ToOwned::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{build_risk_tags, build_value_signals, SignalAxis, ValueSignalKind};
    use crate::config::ProfileConfig;
    use crate::github::GitHubIssue;
    use crate::github_enrichment::{EnrichedIssue, TimestampedSample};
    use crate::value_scoring::RiskTag;

    fn enriched(title: &str, body: &str) -> EnrichedIssue {
        let issue = GitHubIssue {
            id: 1,
            number: 1,
            title: title.to_string(),
            body: body.to_string(),
            labels: vec!["good first issue".to_string()],
            url: "https://github.com/owner/repo/issues/1".to_string(),
            repo_full_name: "owner/repo".to_string(),
            repo_name: "repo".to_string(),
            repo_description: "Rust CLI".to_string(),
            repo_stars: 2_500,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        };
        let mut enriched = EnrichedIssue::from_issue(&issue);
        enriched.repository.forks = 300;
        enriched.repository.pushed_at = Some(Utc::now().to_rfc3339());
        enriched.activity.recent_repo_activity = true;
        enriched.growth.recent_stargazer_sample = vec![TimestampedSample {
            source_ref: "repo:stargazers.sample_recent_100.0".to_string(),
            actor: Some("user".to_string()),
            timestamp: Some(Utc::now().to_rfc3339()),
        }];
        enriched
    }

    #[test]
    fn extracts_attention_execution_and_profile_signals() {
        let enriched = enriched(
            "Fix Rust CLI parser",
            "Steps to reproduce: run cargo test. Expected no panic. Actual panic in src/main.rs. Suggested fix: guard empty input.",
        );
        let signals = build_value_signals(
            &enriched,
            &ProfileConfig {
                tech_stack: vec!["Rust".to_string()],
                keywords: vec!["cli".to_string()],
            },
        );
        assert!(signals.iter().any(|signal| {
            signal.kind == ValueSignalKind::EstablishedImpact
                && signal.axis == SignalAxis::Attention
        }));
        assert!(signals.iter().any(|signal| {
            signal.kind == ValueSignalKind::ReproductionSteps
                && signal.axis == SignalAxis::Execution
        }));
        assert!(signals
            .iter()
            .any(|signal| signal.kind == ValueSignalKind::IssueFit));
    }

    #[test]
    fn detects_low_depth_risks() {
        let enriched = enriched(
            "Add new Grammar Point",
            "No Code Required. This contribution can be done from your browser in under 60 seconds. Add Grammar Point content.",
        );
        let tags = build_risk_tags(&enriched);
        assert!(tags.contains(&RiskTag::NoCodeRequired));
        assert!(tags.contains(&RiskTag::MicroContribution));
        assert!(tags.contains(&RiskTag::ContentFill));
    }

    #[test]
    fn short_alias_matching_is_token_aware() {
        let mut enriched = enriched(
            "User profile",
            "This task has words like firstable and statuslike, but no configured technology token.",
        );
        enriched.repository.description = "General utility".to_string();
        enriched.repository.topics.clear();
        let signals = build_value_signals(
            &enriched,
            &ProfileConfig {
                tech_stack: vec!["Rust".to_string(), "TypeScript".to_string()],
                keywords: vec![],
            },
        );
        assert!(!signals
            .iter()
            .any(|signal| signal.kind == ValueSignalKind::IssueFit));
    }
}
