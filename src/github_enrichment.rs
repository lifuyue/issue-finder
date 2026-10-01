use std::fs;
use std::time::Duration as StdDuration;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::availability::{
    AvailabilityDepth, AvailabilitySnapshot, CoverageStatus, PullRequestEvidence,
    PullRequestRelation,
};
use crate::competition::{
    assess_competition, is_issue_finder_projection_comment, CompetitionFacts,
    TimelineIssueReference,
};
use crate::config::Config;
use crate::github::GitHubIssue;
use crate::github_budget::{GitHubApiBudget, GitHubApiBudgetReport, GitHubRequestSource};
use crate::paths::{atomic_write, IssueFinderPaths};
use crate::system1::evidence::{
    CommentEvidence, CommentsEvidence, EvidenceSnapshot, EvidenceText, COMMENT_SAMPLE_LIMIT,
};

const ENRICHMENT_CACHE_TTL_MINUTES: i64 = 360;
const COMPETITION_COMPLETION_CACHE_TTL_MINUTES: i64 = 360;
const ENRICHMENT_HTTP_TIMEOUT: StdDuration = StdDuration::from_secs(10);
const RECENT_STARGAZER_SAMPLE_LIMIT: usize = 100;
const NEWEST_FORK_SAMPLE_LIMIT: usize = 100;
const ISSUE_COMMENT_LIMIT: usize = 30;
const ISSUE_TIMELINE_LIMIT: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnrichedIssue {
    pub issue: EnrichedIssueFacts,
    pub repository: EnrichedRepositoryFacts,
    pub activity: EnrichedActivityFacts,
    pub participants: EnrichedParticipants,
    pub comments: Vec<EnrichedComment>,
    #[serde(default = "default_competition_facts")]
    pub competition: CompetitionFacts,
    pub growth: EnrichedGrowthFacts,
    pub warnings: Vec<String>,
    pub source_fetched_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system1: Option<crate::system1::JudgmentSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<AvailabilitySnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnrichedIssueFacts {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub comments_count: u64,
    pub updated_at: String,
    pub created_at: String,
    pub author_association: String,
    pub url: String,
    pub repo_full_name: String,
    pub number: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnrichedRepositoryFacts {
    pub full_name: String,
    pub name: String,
    pub description: String,
    pub stars: u64,
    pub forks: u64,
    pub subscribers: Option<u64>,
    pub open_issues: Option<u64>,
    pub pushed_at: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub default_branch: Option<String>,
    pub archived: bool,
    pub topics: Vec<String>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnrichedActivityFacts {
    pub recent_issue_activity: bool,
    pub recent_repo_activity: bool,
    pub maintainer_recent_response: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnrichedParticipants {
    pub issue_author: Option<String>,
    pub commenters: Vec<String>,
    pub maintainer_commenters: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnrichedComment {
    pub source_ref: String,
    pub author: Option<String>,
    pub author_association: String,
    pub created_at: String,
    pub body_excerpt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnrichedGrowthFacts {
    pub recent_stargazer_sample: Vec<TimestampedSample>,
    pub newest_fork_sample: Vec<TimestampedSample>,
    pub stargazer_sample_limit: usize,
    pub fork_sample_limit: usize,
    pub confidence_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimestampedSample {
    pub source_ref: String,
    pub actor: Option<String>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RepoApiResponse {
    full_name: String,
    name: String,
    description: Option<String>,
    stargazers_count: Option<u64>,
    forks_count: Option<u64>,
    subscribers_count: Option<u64>,
    open_issues_count: Option<u64>,
    pushed_at: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    default_branch: Option<String>,
    archived: Option<bool>,
    topics: Option<Vec<String>>,
    language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IssueApiResponse {
    comments: Option<u64>,
    state: Option<String>,
    locked: Option<bool>,
    assignees: Option<Vec<UserApiResponse>>,
    author_association: Option<String>,
    user: Option<UserApiResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CommentApiResponse {
    id: Option<u64>,
    html_url: Option<String>,
    body: Option<String>,
    author_association: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    user: Option<UserApiResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserApiResponse {
    login: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StargazerApiResponse {
    StarredAt {
        starred_at: Option<String>,
        user: Option<UserApiResponse>,
    },
    User(UserApiResponse),
}

#[derive(Debug, Deserialize)]
struct ForkApiResponse {
    created_at: Option<String>,
    owner: Option<UserApiResponse>,
}

#[derive(Debug, Deserialize)]
struct TimelineApiResponse {
    event: Option<String>,
    created_at: Option<String>,
    source: Option<TimelineSourceApiResponse>,
}

#[derive(Debug, Deserialize)]
struct TimelineSourceApiResponse {
    issue: Option<TimelineIssueApiResponse>,
}

#[derive(Debug, Deserialize)]
struct TimelineIssueApiResponse {
    state: Option<String>,
    pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SourceCachePayload<T> {
    fetched_at: DateTime<Utc>,
    value: T,
}

struct CommentFetchRequest<'a> {
    paths: &'a IssueFinderPaths,
    owner: &'a str,
    repo: &'a str,
    number: u64,
    comments_count: u64,
    request_source: GitHubRequestSource,
    refresh: bool,
}

pub struct GitHubEnrichmentClient {
    http: reqwest::Client,
    token: String,
    api_base_url: String,
    budget: GitHubApiBudget,
}

impl GitHubEnrichmentClient {
    pub fn new(config: &Config) -> Result<Self> {
        Self::with_budget(config, GitHubApiBudget::from_env())
    }

    pub fn with_budget(config: &Config, budget: GitHubApiBudget) -> Result<Self> {
        Self::with_api_base_and_budget(
            config,
            std::env::var("ISSUE_FINDER_GITHUB_API_BASE")
                .unwrap_or_else(|_| "https://api.github.com".to_string()),
            budget,
        )
    }

    pub fn with_api_base(config: &Config, api_base_url: impl Into<String>) -> Result<Self> {
        Self::with_api_base_and_budget(config, api_base_url, GitHubApiBudget::from_env())
    }

    pub fn with_api_base_and_budget(
        config: &Config,
        api_base_url: impl Into<String>,
        budget: GitHubApiBudget,
    ) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent("issue-finder")
                .timeout(ENRICHMENT_HTTP_TIMEOUT)
                .build()?,
            token: config.resolved_github_token().token,
            api_base_url: api_base_url.into(),
            budget,
        })
    }

    pub fn request_stats(&self) -> GitHubApiBudgetReport {
        self.budget.report()
    }

    /// Fresh read-only checks, independent of model calls and enrichment cache age.
    /// Every request uses this client's existing authentication and shared API budget.
    pub async fn availability(
        &self,
        _paths: &IssueFinderPaths,
        issue: &GitHubIssue,
        depth: AvailabilityDepth,
    ) -> AvailabilitySnapshot {
        let mut snapshot = AvailabilitySnapshot::new(depth);
        snapshot.repo_full_name = issue.repo_full_name.clone();
        let Some((owner, repo)) = split_repo_full_name(&issue.repo_full_name) else {
            snapshot
                .uncertainties
                .push("Availability repository identity is invalid".into());
            return snapshot;
        };
        match self.fetch_issue_details(&owner, &repo, issue.number).await {
            Ok(details) => {
                snapshot.issue_state = details.state;
                snapshot.locked = details.locked;
                snapshot.assignees = details.assignees.and_then(|users| {
                    users
                        .into_iter()
                        .map(|user| user.login)
                        .collect::<Option<Vec<_>>>()
                });
            }
            Err(error) => snapshot
                .uncertainties
                .push(format!("Fresh issue status unavailable: {error}")),
        }
        match self
            .availability_json(
                &format!("/repos/{owner}/{repo}"),
                &[],
                GitHubRequestSource::EnrichmentRepoMetadata,
            )
            .await
        {
            Ok((value, _)) => {
                snapshot.archived = value.get("archived").and_then(serde_json::Value::as_bool);
                snapshot.default_branch = value
                    .get("default_branch")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
            }
            Err(error) => snapshot
                .uncertainties
                .push(format!("Fresh repository status unavailable: {error}")),
        }
        if snapshot.issue_state.is_none()
            || snapshot.assignees.is_none()
            || snapshot.locked.is_none()
            || snapshot.archived.is_none()
        {
            snapshot
                .uncertainties
                .push("GitHub issue/repository response omitted availability fields".into());
        }

        let max_pages = if depth == AvailabilityDepth::Final {
            3
        } else {
            2
        };
        let mut leads: Vec<(String, u64, PullRequestRelation, String)> = Vec::new();
        for page in 1..=max_pages {
            let result = self
                .availability_json(
                    &format!("/repos/{owner}/{repo}/issues/{}/timeline", issue.number),
                    &[("per_page", "100".into()), ("page", page.to_string())],
                    GitHubRequestSource::EnrichmentTimeline,
                )
                .await;
            match result {
                Ok((value, has_more)) => {
                    let Some(events) = value.as_array() else {
                        snapshot
                            .linked_coverage
                            .errors
                            .push("Timeline response was not an array".into());
                        break;
                    };
                    snapshot.linked_coverage.pages_fetched += 1;
                    snapshot.linked_coverage.items_fetched += events.len();
                    // A full page without a Link header cannot establish completeness.
                    let has_more = has_more.or((events.len() < 100).then_some(false));
                    snapshot.linked_coverage.has_more = has_more;
                    for (index, event) in events.iter().enumerate() {
                        if let Some(source) = event.pointer("/source/issue") {
                            if source.get("pull_request").is_some_and(|pr| !pr.is_null()) {
                                if let Some((full_name, number)) = pr_identity(source, &issue.url) {
                                    push_pr_lead(
                                        &mut leads,
                                        full_name,
                                        number,
                                        if event
                                            .get("will_close_target")
                                            .and_then(serde_json::Value::as_bool)
                                            == Some(true)
                                        {
                                            PullRequestRelation::ExplicitResolution
                                        } else {
                                            PullRequestRelation::Mention
                                        },
                                        format!("issue:timeline.page{page}.{index}"),
                                    );
                                } else {
                                    snapshot.uncertainties.push(
                                        "Timeline PR reference omitted a verifiable identity"
                                            .into(),
                                    );
                                    snapshot
                                        .linked_coverage
                                        .errors
                                        .push("Unverifiable timeline PR identity".into());
                                }
                            }
                        }
                    }
                    if has_more == Some(false) {
                        snapshot.linked_coverage.status = CoverageStatus::Complete;
                        break;
                    }
                    snapshot.linked_coverage.status = CoverageStatus::Partial;
                    if has_more.is_none() {
                        break;
                    }
                }
                Err(error) => {
                    snapshot.linked_coverage.errors.push(error.to_string());
                    break;
                }
            }
        }
        if !snapshot.linked_coverage.errors.is_empty() {
            snapshot.linked_coverage.status = if snapshot.linked_coverage.pages_fetched == 0 {
                CoverageStatus::Unavailable
            } else {
                CoverageStatus::Partial
            };
        }

        if depth == AvailabilityDepth::Final {
            let mut queries = vec![format!(
                "repo:{} is:pr \"#{}\" in:body",
                issue.repo_full_name, issue.number
            )];
            let terms = search_title_terms(&issue.title);
            if !terms.is_empty() {
                queries.push(format!(
                    "repo:{} is:pr {} in:title,body",
                    issue.repo_full_name,
                    terms.join(" ")
                ));
            }
            snapshot.search_coverage.status = CoverageStatus::Complete;
            for query in queries {
                snapshot.search_coverage.queries.push(query.clone());
                match self
                    .availability_json(
                        "/search/issues",
                        &[
                            ("q", query),
                            ("per_page", "10".into()),
                            ("page", "1".into()),
                        ],
                        GitHubRequestSource::EnrichmentTimeline,
                    )
                    .await
                {
                    Ok((value, has_more)) => {
                        let Some(items) = value.get("items").and_then(serde_json::Value::as_array)
                        else {
                            snapshot
                                .search_coverage
                                .errors
                                .push("PR search omitted items".into());
                            continue;
                        };
                        snapshot.search_coverage.pages_fetched += 1;
                        snapshot.search_coverage.items_fetched += items.len();
                        let total = value.get("total_count").and_then(serde_json::Value::as_u64);
                        let incomplete = value
                            .get("incomplete_results")
                            .and_then(serde_json::Value::as_bool);
                        let more = has_more.unwrap_or(false)
                            || total.is_none_or(|total| total > items.len() as u64)
                            || incomplete != Some(false);
                        snapshot.search_coverage.has_more =
                            Some(snapshot.search_coverage.has_more.unwrap_or(false) || more);
                        if more {
                            snapshot.search_coverage.status = CoverageStatus::Partial;
                        }
                        for item in items {
                            if item.get("pull_request").is_some() {
                                if let Some((full_name, number)) = pr_identity(item, &issue.url) {
                                    push_pr_lead(
                                        &mut leads,
                                        full_name,
                                        number,
                                        PullRequestRelation::SearchLead,
                                        "github:targeted_pr_search".into(),
                                    );
                                } else {
                                    snapshot
                                        .search_coverage
                                        .errors
                                        .push("Unverifiable search PR identity".into());
                                }
                            }
                        }
                    }
                    Err(error) => snapshot.search_coverage.errors.push(error.to_string()),
                }
            }
            if !snapshot.search_coverage.errors.is_empty() {
                snapshot.search_coverage.status = if snapshot.search_coverage.pages_fetched == 0 {
                    CoverageStatus::Unavailable
                } else {
                    CoverageStatus::Partial
                };
            }
            snapshot.uncertainties.push("Targeted PR searches cover only the recorded queries; unrelated wording or unindexed work may be absent".into());
        }
        let pr_limit = if depth == AvailabilityDepth::Final {
            12
        } else {
            6
        };
        if leads.len() > pr_limit {
            snapshot.linked_coverage.status = CoverageStatus::Partial;
            snapshot
                .uncertainties
                .push(format!("PR detail checks limited to {pr_limit} references"));
        }
        for (full_name, number, relation, source) in leads.into_iter().take(pr_limit) {
            let mut evidence = PullRequestEvidence {
                repo_full_name: full_name.clone(),
                number,
                url: None,
                state: None,
                draft: None,
                merged: None,
                merged_at: None,
                base_branch: None,
                relation,
                sources: vec![source],
                verified: false,
                errors: Vec::new(),
            };
            match self
                .availability_json(
                    &format!("/repos/{full_name}/pulls/{number}"),
                    &[],
                    GitHubRequestSource::EnrichmentTimeline,
                )
                .await
            {
                Ok((value, _)) => {
                    evidence.url = value
                        .get("html_url")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    evidence.state = value
                        .get("state")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    evidence.draft = value.get("draft").and_then(serde_json::Value::as_bool);
                    evidence.merged = value.get("merged").and_then(serde_json::Value::as_bool);
                    evidence.merged_at = value
                        .get("merged_at")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    evidence.base_branch = value
                        .pointer("/base/ref")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    evidence.verified = pr_identity(&value, &issue.url)
                        == Some((full_name.clone(), number))
                        && value
                            .pointer("/base/repo/full_name")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|name| name.eq_ignore_ascii_case(&full_name));
                    if !evidence.verified {
                        evidence
                            .errors
                            .push("PR API response identity did not match the requested PR".into());
                    }
                    if evidence.state.is_none()
                        || evidence.draft.is_none()
                        || evidence.merged.is_none()
                        || evidence.base_branch.is_none()
                        || (evidence.merged == Some(true) && evidence.merged_at.is_none())
                    {
                        evidence.errors.push(
                            "PR API response omitted state, draft, merge or base-branch evidence"
                                .into(),
                        );
                    }
                    if evidence.verified
                        && value
                            .get("body")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|body| explicitly_resolves(body, &full_name, issue))
                    {
                        evidence.relation = PullRequestRelation::ExplicitResolution;
                    }
                    if matches!(
                        evidence.relation,
                        PullRequestRelation::SearchLead | PullRequestRelation::Mention
                    ) {
                        snapshot.uncertainties.push(format!("PR lead {full_name}#{number} has no verified resolving relationship; mentions, title or keyword overlap do not establish competition"));
                    }
                    if evidence.merged == Some(true)
                        && evidence.relation == PullRequestRelation::ExplicitResolution
                        && (!full_name.eq_ignore_ascii_case(&snapshot.repo_full_name)
                            || evidence.base_branch != snapshot.default_branch
                            || snapshot.default_branch.is_none())
                    {
                        snapshot.uncertainties.push(format!("Merged resolving PR {full_name}#{number} does not establish a merge into this repository's default branch"));
                    }
                }
                Err(error) => evidence
                    .errors
                    .push(format!("Fresh PR details unavailable: {error}")),
            }
            snapshot.uncertainties.extend(
                evidence
                    .errors
                    .iter()
                    .map(|error| format!("{full_name}#{number}: {error}")),
            );
            snapshot.pull_requests.push(evidence);
        }
        for (name, coverage) in [
            ("Linked timeline", &snapshot.linked_coverage),
            ("PR search", &snapshot.search_coverage),
        ] {
            snapshot.uncertainties.extend(
                coverage
                    .errors
                    .iter()
                    .map(|error| format!("{name}: {error}")),
            );
            if coverage.status == CoverageStatus::Partial {
                snapshot
                    .uncertainties
                    .push(format!("{name} coverage is incomplete"));
            }
        }
        snapshot
    }

    async fn availability_json(
        &self,
        path: &str,
        query: &[(&str, String)],
        source: GitHubRequestSource,
    ) -> Result<(serde_json::Value, Option<bool>)> {
        self.record_request(source, path)?;
        let response = self
            .authorized(self.http.get(self.api_url(path)).query(query))
            .send()
            .await?;
        let response = require_success(response).await?;
        let has_more = match response.headers().get(reqwest::header::LINK) {
            Some(link) => Some(
                link.to_str()
                    .context("Invalid GitHub pagination Link header")?
                    .split(',')
                    .any(|part| {
                        part.split(';')
                            .skip(1)
                            .any(|param| param.trim() == "rel=\"next\"")
                    }),
            ),
            None => None,
        };
        Ok((response.json().await?, has_more))
    }

    /// Acquire full bodies independently from the legacy 500-character comment excerpts.
    /// Network requests share the client's authentication, timeout and GitHub request budget.
    pub async fn system1_evidence(
        &self,
        paths: &IssueFinderPaths,
        issue: &GitHubIssue,
        enriched: &EnrichedIssue,
        refresh: bool,
    ) -> Result<EvidenceSnapshot> {
        let mut snapshot = EvidenceSnapshot::from_issue(issue, enriched);
        let (owner, repo) = split_repo_full_name(&snapshot.repo_full_name)
            .context("Unable to split repository full name for System 1 evidence")?;
        let details = self
            .fetch_issue_details_cached(paths, &owner, &repo, issue.number, refresh)
            .await;
        let total_count = match details {
            Ok(details) => {
                snapshot.github_status.issue_state = details.state;
                snapshot.issue_author = details.user.and_then(|user| user.login);
                snapshot.issue_author_association = details
                    .author_association
                    .unwrap_or_else(|| "unknown".to_string());
                snapshot.github_status.assignees = details
                    .assignees
                    .map(|users| users.into_iter().filter_map(|user| user.login).collect());
                details.comments
            }
            Err(error) => {
                snapshot
                    .warnings
                    .push(format!("System 1 issue details unavailable: {error}"));
                None
            }
        };
        snapshot.comments = CommentsEvidence::unavailable(total_count);
        let key = snapshot.material_hash();
        let cache_path = paths.enrichment_source_cache_path("system1_comments_v1", &key);
        let cached: Option<Vec<CommentApiResponse>> = if refresh {
            None
        } else {
            match load_source_cache(&cache_path, ENRICHMENT_CACHE_TTL_MINUTES) {
                Ok(cached) => cached,
                Err(error) => {
                    snapshot
                        .warnings
                        .push(format!("System 1 comment cache unavailable: {error}"));
                    None
                }
            }
        };
        let raw_comments = if let Some(cached) = cached {
            self.budget
                .record_cache_hit(GitHubRequestSource::EnrichmentComments);
            Some(cached)
        } else {
            let pages = match total_count {
                Some(0) => Vec::new(),
                Some(count) => trailing_sample_pages(count, COMMENT_SAMPLE_LIMIT),
                None => vec![1],
            };
            let mut comments = Vec::new();
            let mut fetched = true;
            for page in pages.into_iter().take(2) {
                match self
                    .fetch_comments_page(
                        &owner,
                        &repo,
                        issue.number,
                        page,
                        GitHubRequestSource::EnrichmentComments,
                    )
                    .await
                {
                    Ok(page_comments) => comments.extend(page_comments),
                    Err(error) => {
                        fetched = false;
                        snapshot
                            .warnings
                            .push(format!("System 1 comments unavailable: {error}"));
                        break;
                    }
                }
            }
            if fetched {
                let comments = tail_limited(comments, COMMENT_SAMPLE_LIMIT);
                if let Err(error) = save_source_cache(&cache_path, &comments) {
                    snapshot
                        .warnings
                        .push(format!("System 1 comment cache write failed: {error}"));
                }
                Some(comments)
            } else {
                None
            }
        };
        snapshot.comments = match raw_comments {
            Some(comments) => CommentsEvidence::from_comments(
                total_count,
                comments
                    .into_iter()
                    .map(|comment| CommentEvidence {
                        id: comment.id,
                        url: comment.html_url,
                        author: comment.user.and_then(|user| user.login),
                        author_association: comment
                            .author_association
                            .unwrap_or_else(|| "unknown".to_string()),
                        created_at: comment.created_at,
                        updated_at: comment.updated_at,
                        body: comment
                            .body
                            .as_deref()
                            .map(|body| EvidenceText::bounded(body, usize::MAX))
                            .unwrap_or_else(EvidenceText::unavailable),
                    })
                    .collect(),
            ),
            None => CommentsEvidence::unavailable(total_count),
        };
        Ok(snapshot)
    }

    pub async fn enrich_issue(
        &self,
        paths: &IssueFinderPaths,
        issue: &GitHubIssue,
        refresh: bool,
    ) -> EnrichedIssue {
        self.enrich_issue_with_options(paths, issue, refresh, true)
            .await
    }

    pub async fn enrich_issue_with_options(
        &self,
        paths: &IssueFinderPaths,
        issue: &GitHubIssue,
        refresh: bool,
        include_competition_timeline: bool,
    ) -> EnrichedIssue {
        let cached = load_cached_enrichment(paths, issue).ok().flatten();
        let refresh = refresh
            || cached.as_ref().is_some_and(|cached| {
                if let (Ok(incoming), Ok(existing)) = (
                    DateTime::parse_from_rfc3339(&issue.updated_at),
                    DateTime::parse_from_rfc3339(&cached.issue.updated_at),
                ) {
                    if incoming < existing {
                        return false;
                    }
                }
                cached.issue.updated_at != issue.updated_at
                    || cached.issue.title != issue.title
                    || cached.issue.body != issue.body
            });
        if !refresh {
            if let Some(mut cached) = cached {
                if !include_competition_timeline || !competition_timeline_missing(&cached) {
                    canonicalize_enriched_issue_repo(&mut cached);
                    return cached;
                }
            }
        }

        let mut enriched = EnrichedIssue::from_issue(issue);
        let Some((owner, repo)) = split_repo_full_name(&issue.repo_full_name) else {
            enriched
                .warnings
                .push("Unable to split repository full name for enrichment".to_string());
            return enriched;
        };

        match self.fetch_repo_cached(paths, &owner, &repo, refresh).await {
            Ok(repo_facts) => {
                enriched.repository = repo_facts;
                canonicalize_enriched_issue_repo(&mut enriched);
            }
            Err(error) => enriched
                .warnings
                .push(format!("Repository metadata enrichment failed: {error}")),
        }

        match self
            .fetch_issue_details_cached(paths, &owner, &repo, issue.number, refresh)
            .await
        {
            Ok(details) => {
                enriched.issue.comments_count = details.comments.unwrap_or(0);
                enriched.issue.author_association = details
                    .author_association
                    .unwrap_or_else(|| "unknown".to_string());
                enriched.participants.issue_author = details.user.and_then(|user| user.login);
            }
            Err(error) => enriched
                .warnings
                .push(format!("Issue details enrichment failed: {error}")),
        }

        let mut competition_warnings = Vec::new();
        match self
            .fetch_comments_cached(CommentFetchRequest {
                paths,
                owner: &owner,
                repo: &repo,
                number: issue.number,
                comments_count: enriched.issue.comments_count,
                request_source: GitHubRequestSource::EnrichmentComments,
                refresh,
            })
            .await
        {
            Ok(comments) => {
                enriched.comments = comments;
                enriched.participants.commenters = unique_nonempty(
                    enriched
                        .comments
                        .iter()
                        .filter_map(|comment| comment.author.clone())
                        .collect(),
                );
                enriched.participants.maintainer_commenters = unique_nonempty(
                    enriched
                        .comments
                        .iter()
                        .filter(|comment| is_maintainer_association(&comment.author_association))
                        .filter_map(|comment| comment.author.clone())
                        .collect(),
                );
            }
            Err(error) => {
                enriched
                    .warnings
                    .push(format!("Issue comments enrichment failed: {error}"));
                if enriched.issue.comments_count > 0 {
                    competition_warnings.push(format!(
                        "Competition comment evidence enrichment failed: {error}"
                    ));
                }
            }
        }

        let competition_texts = competition_texts(&enriched);
        if include_competition_timeline {
            match self
                .fetch_timeline_refs_cached(
                    paths,
                    &owner,
                    &repo,
                    issue.number,
                    GitHubRequestSource::EnrichmentTimeline,
                    refresh,
                )
                .await
            {
                Ok(timeline_refs) => {
                    enriched.competition = assess_competition(
                        &timeline_refs,
                        &competition_texts,
                        competition_warnings,
                    );
                }
                Err(error) => {
                    competition_warnings
                        .push(format!("Competition timeline enrichment failed: {error}"));
                    enriched.competition =
                        assess_competition(&[], &competition_texts, competition_warnings);
                }
            }
        } else {
            competition_warnings.push("Competition timeline evidence was not fetched".to_string());
            enriched.competition =
                assess_competition(&[], &competition_texts, competition_warnings);
        }

        match self
            .fetch_recent_stargazers_cached(
                paths,
                &owner,
                &repo,
                enriched.repository.stars,
                refresh,
            )
            .await
        {
            Ok(samples) => enriched.growth.recent_stargazer_sample = samples,
            Err(error) => enriched
                .warnings
                .push(format!("Recent stargazer sample failed: {error}")),
        }

        match self
            .fetch_newest_forks_cached(paths, &owner, &repo, refresh)
            .await
        {
            Ok(samples) => enriched.growth.newest_fork_sample = samples,
            Err(error) => enriched
                .warnings
                .push(format!("Newest fork sample failed: {error}")),
        }

        enriched.recompute_activity();
        enriched.recompute_growth_notes();
        let _ = save_cached_enrichment(paths, issue, &enriched);
        enriched
    }

    pub async fn complete_competition_timeline(
        &self,
        paths: &IssueFinderPaths,
        issue: &GitHubIssue,
        current: &EnrichedIssue,
        refresh: bool,
    ) -> EnrichedIssue {
        if !competition_timeline_missing(current) {
            let mut current = current.clone();
            canonicalize_enriched_issue_repo(&mut current);
            return current;
        }

        if !refresh {
            if let Ok(Some(mut cached)) = load_cached_enrichment(paths, issue) {
                if !competition_timeline_missing(&cached) {
                    canonicalize_enriched_issue_repo(&mut cached);
                    return cached;
                }
            }
        }

        let mut enriched = current.clone();
        let Some((owner, repo)) = split_repo_full_name(&issue.repo_full_name) else {
            enriched
                .warnings
                .push("Unable to split repository full name for timeline completion".to_string());
            return enriched;
        };

        let mut competition_warnings = Vec::new();
        if competition_comment_evidence_failed(&enriched)
            || (enriched.issue.comments_count > 0 && enriched.comments.is_empty())
        {
            match self
                .fetch_comments_cached(CommentFetchRequest {
                    paths,
                    owner: &owner,
                    repo: &repo,
                    number: issue.number,
                    comments_count: enriched.issue.comments_count,
                    request_source: GitHubRequestSource::CompetitionCompletionComments,
                    refresh,
                })
                .await
            {
                Ok(comments) => {
                    enriched.comments = comments;
                    enriched.participants.commenters = unique_nonempty(
                        enriched
                            .comments
                            .iter()
                            .filter_map(|comment| comment.author.clone())
                            .collect(),
                    );
                    enriched.participants.maintainer_commenters = unique_nonempty(
                        enriched
                            .comments
                            .iter()
                            .filter(|comment| {
                                is_maintainer_association(&comment.author_association)
                            })
                            .filter_map(|comment| comment.author.clone())
                            .collect(),
                    );
                }
                Err(error) => {
                    enriched
                        .warnings
                        .push(format!("Issue comments completion failed: {error}"));
                    competition_warnings.push(format!(
                        "Competition comment evidence enrichment failed: {error}"
                    ));
                }
            }
        }

        let competition_texts = competition_texts(&enriched);
        match self
            .fetch_timeline_refs_cached(
                paths,
                &owner,
                &repo,
                issue.number,
                GitHubRequestSource::CompetitionCompletionTimeline,
                refresh,
            )
            .await
        {
            Ok(timeline_refs) => {
                enriched.competition =
                    assess_competition(&timeline_refs, &competition_texts, competition_warnings);
            }
            Err(error) => {
                competition_warnings
                    .push(format!("Competition timeline enrichment failed: {error}"));
                enriched.competition =
                    assess_competition(&[], &competition_texts, competition_warnings);
            }
        }
        enriched.source_fetched_at = Utc::now().to_rfc3339();
        canonicalize_enriched_issue_repo(&mut enriched);
        let _ = save_cached_enrichment(paths, issue, &enriched);
        enriched
    }

    async fn fetch_repo_cached(
        &self,
        paths: &IssueFinderPaths,
        owner: &str,
        repo: &str,
        refresh: bool,
    ) -> Result<EnrichedRepositoryFacts> {
        let key = repo_cache_key(owner, repo);
        let path = paths.enrichment_source_cache_path(
            GitHubRequestSource::EnrichmentRepoMetadata.as_str(),
            &key,
        );
        if !refresh {
            if let Some(cached) = load_source_cache(&path, ENRICHMENT_CACHE_TTL_MINUTES)? {
                self.budget
                    .record_cache_hit(GitHubRequestSource::EnrichmentRepoMetadata);
                return Ok(cached);
            }
        }

        let value = self.fetch_repo(owner, repo).await?;
        save_source_cache(&path, &value)?;
        Ok(value)
    }

    async fn fetch_issue_details_cached(
        &self,
        paths: &IssueFinderPaths,
        owner: &str,
        repo: &str,
        number: u64,
        refresh: bool,
    ) -> Result<IssueApiResponse> {
        let key = issue_cache_key(owner, repo, number);
        let path = paths.enrichment_source_cache_path(
            GitHubRequestSource::EnrichmentIssueDetails.as_str(),
            &key,
        );
        if !refresh {
            if let Some(cached) = load_source_cache(&path, ENRICHMENT_CACHE_TTL_MINUTES)? {
                self.budget
                    .record_cache_hit(GitHubRequestSource::EnrichmentIssueDetails);
                return Ok(cached);
            }
        }

        let value = self.fetch_issue_details(owner, repo, number).await?;
        save_source_cache(&path, &value)?;
        Ok(value)
    }

    async fn fetch_comments_cached(
        &self,
        request: CommentFetchRequest<'_>,
    ) -> Result<Vec<EnrichedComment>> {
        let CommentFetchRequest {
            paths,
            owner,
            repo,
            number,
            comments_count,
            request_source,
            refresh,
        } = request;
        let key = issue_cache_key(owner, repo, number);
        let path = paths.enrichment_source_cache_path(request_source.as_str(), &key);
        if !refresh {
            if let Some(cached) = load_source_cache(&path, ENRICHMENT_CACHE_TTL_MINUTES)? {
                self.budget.record_cache_hit(request_source);
                return Ok(cached);
            }
        }

        let value = self
            .fetch_comments(owner, repo, number, comments_count, request_source)
            .await?;
        save_source_cache(&path, &value)?;
        Ok(value)
    }

    async fn fetch_timeline_refs_cached(
        &self,
        paths: &IssueFinderPaths,
        owner: &str,
        repo: &str,
        number: u64,
        request_source: GitHubRequestSource,
        refresh: bool,
    ) -> Result<Vec<TimelineIssueReference>> {
        let key = issue_cache_key(owner, repo, number);
        let ttl = match request_source {
            GitHubRequestSource::CompetitionCompletionTimeline => {
                COMPETITION_COMPLETION_CACHE_TTL_MINUTES
            }
            _ => ENRICHMENT_CACHE_TTL_MINUTES,
        };
        let path = paths.enrichment_source_cache_path(request_source.as_str(), &key);
        if !refresh {
            if let Some(cached) = load_source_cache(&path, ttl)? {
                self.budget.record_cache_hit(request_source);
                return Ok(cached);
            }
        }

        let value = self
            .fetch_timeline_refs(owner, repo, number, request_source)
            .await?;
        save_source_cache(&path, &value)?;
        Ok(value)
    }

    async fn fetch_recent_stargazers_cached(
        &self,
        paths: &IssueFinderPaths,
        owner: &str,
        repo: &str,
        stars: u64,
        refresh: bool,
    ) -> Result<Vec<TimestampedSample>> {
        let key = repo_cache_key(owner, repo);
        let path = paths.enrichment_source_cache_path("enrichment_growth_stargazers", &key);
        if !refresh {
            if let Some(cached) = load_source_cache(&path, ENRICHMENT_CACHE_TTL_MINUTES)? {
                self.budget
                    .record_cache_hit(GitHubRequestSource::EnrichmentGrowth);
                return Ok(cached);
            }
        }

        let value = self.fetch_recent_stargazers(owner, repo, stars).await?;
        save_source_cache(&path, &value)?;
        Ok(value)
    }

    async fn fetch_newest_forks_cached(
        &self,
        paths: &IssueFinderPaths,
        owner: &str,
        repo: &str,
        refresh: bool,
    ) -> Result<Vec<TimestampedSample>> {
        let key = repo_cache_key(owner, repo);
        let path = paths.enrichment_source_cache_path("enrichment_growth_forks", &key);
        if !refresh {
            if let Some(cached) = load_source_cache(&path, ENRICHMENT_CACHE_TTL_MINUTES)? {
                self.budget
                    .record_cache_hit(GitHubRequestSource::EnrichmentGrowth);
                return Ok(cached);
            }
        }

        let value = self.fetch_newest_forks(owner, repo).await?;
        save_source_cache(&path, &value)?;
        Ok(value)
    }

    async fn fetch_repo(&self, owner: &str, repo: &str) -> Result<EnrichedRepositoryFacts> {
        self.record_request(
            GitHubRequestSource::EnrichmentRepoMetadata,
            format!("{owner}/{repo}"),
        )?;
        let response = self
            .authorized(
                self.http
                    .get(self.api_url(&format!("/repos/{owner}/{repo}"))),
            )
            .send()
            .await?;
        let repo = require_success(response)
            .await?
            .json::<RepoApiResponse>()
            .await?;
        Ok(EnrichedRepositoryFacts {
            full_name: repo.full_name,
            name: repo.name,
            description: repo.description.unwrap_or_default(),
            stars: repo.stargazers_count.unwrap_or_default(),
            forks: repo.forks_count.unwrap_or_default(),
            subscribers: repo.subscribers_count,
            open_issues: repo.open_issues_count,
            pushed_at: repo.pushed_at,
            created_at: repo.created_at,
            updated_at: repo.updated_at,
            default_branch: repo.default_branch,
            archived: repo.archived.unwrap_or(false),
            topics: repo.topics.unwrap_or_default(),
            language: repo.language,
        })
    }

    async fn fetch_issue_details(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<IssueApiResponse> {
        self.record_request(
            GitHubRequestSource::EnrichmentIssueDetails,
            format!("{owner}/{repo}#{number}"),
        )?;
        let response = self
            .authorized(
                self.http
                    .get(self.api_url(&format!("/repos/{owner}/{repo}/issues/{number}"))),
            )
            .send()
            .await?;
        require_success(response)
            .await?
            .json::<IssueApiResponse>()
            .await
            .map_err(Into::into)
    }

    async fn fetch_comments(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        comments_count: u64,
        request_source: GitHubRequestSource,
    ) -> Result<Vec<EnrichedComment>> {
        let mut comments = Vec::new();
        for page in trailing_sample_pages(comments_count, ISSUE_COMMENT_LIMIT) {
            comments.extend(
                self.fetch_comments_page(owner, repo, number, page, request_source)
                    .await?,
            );
        }
        let comments = tail_limited(comments, ISSUE_COMMENT_LIMIT);
        Ok(comments
            .into_iter()
            .enumerate()
            .map(|(index, comment)| EnrichedComment {
                source_ref: format!("issue:comments.{index}"),
                author: comment.user.and_then(|user| user.login),
                author_association: comment
                    .author_association
                    .unwrap_or_else(|| "unknown".to_string()),
                created_at: comment.created_at.unwrap_or_default(),
                body_excerpt: excerpt(comment.body.unwrap_or_default(), 500),
            })
            .collect())
    }

    async fn fetch_comments_page(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        page: u64,
        request_source: GitHubRequestSource,
    ) -> Result<Vec<CommentApiResponse>> {
        self.record_request(
            request_source,
            format!("{owner}/{repo}#{number}:comments:{page}"),
        )?;
        let response = self
            .authorized(
                self.http
                    .get(self.api_url(&format!("/repos/{owner}/{repo}/issues/{number}/comments")))
                    .query(&[
                        ("per_page", ISSUE_COMMENT_LIMIT.to_string()),
                        ("page", page.to_string()),
                    ]),
            )
            .send()
            .await?;
        require_success(response)
            .await?
            .json::<Vec<CommentApiResponse>>()
            .await
            .map_err(Into::into)
    }

    async fn fetch_recent_stargazers(
        &self,
        owner: &str,
        repo: &str,
        stars: u64,
    ) -> Result<Vec<TimestampedSample>> {
        let mut stargazers = Vec::new();
        for page in trailing_sample_pages(stars, RECENT_STARGAZER_SAMPLE_LIMIT) {
            stargazers.extend(self.fetch_stargazer_page(owner, repo, page).await?);
        }
        let stargazers = tail_limited(stargazers, RECENT_STARGAZER_SAMPLE_LIMIT);
        Ok(stargazers
            .into_iter()
            .enumerate()
            .map(|(index, item)| match item {
                StargazerApiResponse::StarredAt { starred_at, user } => TimestampedSample {
                    source_ref: format!("repo:stargazers.sample_recent_100.{index}"),
                    actor: user.and_then(|user| user.login),
                    timestamp: starred_at,
                },
                StargazerApiResponse::User(user) => TimestampedSample {
                    source_ref: format!("repo:stargazers.sample_recent_100.{index}"),
                    actor: user.login,
                    timestamp: None,
                },
            })
            .collect())
    }

    async fn fetch_stargazer_page(
        &self,
        owner: &str,
        repo: &str,
        page: u64,
    ) -> Result<Vec<StargazerApiResponse>> {
        self.record_request(
            GitHubRequestSource::EnrichmentGrowth,
            format!("{owner}/{repo}:stargazers:{page}"),
        )?;
        let response = self
            .authorized(
                self.http
                    .get(self.api_url(&format!("/repos/{owner}/{repo}/stargazers")))
                    .header("accept", "application/vnd.github.star+json")
                    .query(&[
                        ("per_page", RECENT_STARGAZER_SAMPLE_LIMIT.to_string()),
                        ("page", page.to_string()),
                    ]),
            )
            .send()
            .await?;
        require_success(response)
            .await?
            .json::<Vec<StargazerApiResponse>>()
            .await
            .map_err(Into::into)
    }

    async fn fetch_newest_forks(&self, owner: &str, repo: &str) -> Result<Vec<TimestampedSample>> {
        self.record_request(
            GitHubRequestSource::EnrichmentGrowth,
            format!("{owner}/{repo}:forks"),
        )?;
        let response = self
            .authorized(
                self.http
                    .get(self.api_url(&format!("/repos/{owner}/{repo}/forks")))
                    .query(&[
                        ("sort", "newest".to_string()),
                        ("per_page", NEWEST_FORK_SAMPLE_LIMIT.to_string()),
                    ]),
            )
            .send()
            .await?;
        let forks = require_success(response)
            .await?
            .json::<Vec<ForkApiResponse>>()
            .await?;
        Ok(forks
            .into_iter()
            .enumerate()
            .map(|(index, item)| TimestampedSample {
                source_ref: format!("repo:forks.sample_newest_100.{index}"),
                actor: item.owner.and_then(|user| user.login),
                timestamp: item.created_at,
            })
            .collect())
    }

    async fn fetch_timeline_refs(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        request_source: GitHubRequestSource,
    ) -> Result<Vec<TimelineIssueReference>> {
        self.record_request(request_source, format!("{owner}/{repo}#{number}:timeline"))?;
        let response = self
            .authorized(
                self.http
                    .get(self.api_url(&format!("/repos/{owner}/{repo}/issues/{number}/timeline")))
                    .header("accept", "application/vnd.github+json")
                    .query(&[("per_page", ISSUE_TIMELINE_LIMIT.to_string())]),
            )
            .send()
            .await?;
        let events = require_success(response)
            .await?
            .json::<Vec<TimelineApiResponse>>()
            .await?;

        Ok(events
            .into_iter()
            .enumerate()
            .filter_map(|(index, event)| {
                if event.event.as_deref() != Some("cross-referenced") {
                    return None;
                }
                let issue = event.source?.issue?;
                Some(TimelineIssueReference {
                    source_ref: format!("issue:timeline.{index}"),
                    state: issue.state,
                    is_pull_request: issue.pull_request.is_some(),
                    created_at: event.created_at,
                })
            })
            .filter(|item| item.is_pull_request)
            .collect())
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.token.trim().is_empty() {
            request
        } else {
            request.bearer_auth(self.token.trim())
        }
    }

    fn api_url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.api_base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    fn record_request(&self, source: GitHubRequestSource, detail: impl AsRef<str>) -> Result<()> {
        self.budget
            .record_network_request(source, detail)
            .map_err(Into::into)
    }
}

pub fn canonicalize_enriched_issue_repo(enriched: &mut EnrichedIssue) {
    let canonical = enriched.repository.full_name.trim();
    if canonical.is_empty() || enriched.issue.repo_full_name == canonical {
        return;
    }

    enriched.issue.repo_full_name = canonical.to_string();
    enriched.issue.url = format!(
        "https://github.com/{canonical}/issues/{}",
        enriched.issue.number
    );
}

impl EnrichedIssue {
    pub fn from_issue(issue: &GitHubIssue) -> Self {
        Self {
            issue: EnrichedIssueFacts {
                title: issue.title.clone(),
                body: issue.body.clone(),
                labels: issue.labels.clone(),
                comments_count: 0,
                updated_at: issue.updated_at.clone(),
                created_at: issue.created_at.clone(),
                author_association: "unknown".to_string(),
                url: issue.url.clone(),
                repo_full_name: issue.repo_full_name.clone(),
                number: issue.number,
            },
            repository: EnrichedRepositoryFacts {
                full_name: issue.repo_full_name.clone(),
                name: issue.repo_name.clone(),
                description: issue.repo_description.clone(),
                stars: issue.repo_stars,
                forks: 0,
                subscribers: None,
                open_issues: None,
                pushed_at: None,
                created_at: None,
                updated_at: None,
                default_branch: None,
                archived: false,
                topics: Vec::new(),
                language: None,
            },
            activity: EnrichedActivityFacts {
                recent_issue_activity: is_recent(&issue.updated_at, 14),
                recent_repo_activity: false,
                maintainer_recent_response: false,
            },
            participants: EnrichedParticipants {
                issue_author: None,
                commenters: Vec::new(),
                maintainer_commenters: Vec::new(),
            },
            comments: Vec::new(),
            competition: CompetitionFacts::missing_timeline(),
            growth: EnrichedGrowthFacts {
                recent_stargazer_sample: Vec::new(),
                newest_fork_sample: Vec::new(),
                stargazer_sample_limit: RECENT_STARGAZER_SAMPLE_LIMIT,
                fork_sample_limit: NEWEST_FORK_SAMPLE_LIMIT,
                confidence_notes: vec!["Growth evidence is missing or not yet sampled".to_string()],
            },
            warnings: Vec::new(),
            source_fetched_at: Utc::now().to_rfc3339(),
            system1: None,
            availability: None,
        }
    }

    fn recompute_activity(&mut self) {
        self.activity.recent_issue_activity = is_recent(&self.issue.updated_at, 14);
        self.activity.recent_repo_activity = self
            .repository
            .pushed_at
            .as_ref()
            .map(|timestamp| is_recent(timestamp, 30))
            .unwrap_or(false);
        self.activity.maintainer_recent_response = self.comments.iter().any(|comment| {
            is_maintainer_association(&comment.author_association)
                && is_recent(&comment.created_at, 7)
        });
    }

    fn recompute_growth_notes(&mut self) {
        self.growth.confidence_notes.clear();
        if self.growth.recent_stargazer_sample.is_empty() {
            self.growth
                .confidence_notes
                .push("No recent stargazer sample was available".to_string());
        } else if self
            .growth
            .recent_stargazer_sample
            .iter()
            .any(|sample| sample.timestamp.is_none())
        {
            self.growth.confidence_notes.push(
                "Some stargazer sample entries did not include starred_at timestamps".to_string(),
            );
        }

        if self.growth.newest_fork_sample.is_empty() {
            self.growth
                .confidence_notes
                .push("No newest fork sample was available".to_string());
        }

        if self.growth.confidence_notes.is_empty() {
            self.growth.confidence_notes.push(
                "Growth momentum is approximate because GitHub samples are capped".to_string(),
            );
        }
    }
}

pub fn star_velocity(sample: &[TimestampedSample], days: i64, now: DateTime<Utc>) -> usize {
    sample
        .iter()
        .filter(|item| timestamp_within_days(item.timestamp.as_deref(), days, now))
        .count()
}

pub fn fork_velocity(sample: &[TimestampedSample], days: i64, now: DateTime<Utc>) -> usize {
    star_velocity(sample, days, now)
}

fn default_competition_facts() -> CompetitionFacts {
    CompetitionFacts::missing_timeline()
}

fn competition_texts(enriched: &EnrichedIssue) -> Vec<String> {
    std::iter::once(enriched.issue.body.clone())
        .chain(
            enriched
                .comments
                .iter()
                .filter(|comment| !is_issue_finder_projection_comment(&comment.body_excerpt))
                .map(|comment| comment.body_excerpt.clone()),
        )
        .collect()
}

pub fn competition_timeline_missing(enriched: &EnrichedIssue) -> bool {
    enriched.competition.warnings.iter().any(|warning| {
        let lower = warning.to_lowercase();
        lower.contains("timeline evidence was not fetched")
            || lower.contains("timeline enrichment failed")
            || lower.contains("timeline completion skipped")
            || lower.contains("comment evidence enrichment failed")
    })
}

pub fn competition_timeline_not_fetched(enriched: &EnrichedIssue) -> bool {
    let warnings = &enriched.competition.warnings;
    if warnings.iter().any(|warning| {
        let lower = warning.to_lowercase();
        lower.contains("timeline enrichment failed")
            || lower.contains("timeline completion skipped")
    }) {
        return false;
    }
    warnings.iter().any(|warning| {
        let lower = warning.to_lowercase();
        lower.contains("timeline evidence was not fetched")
            || lower.contains("comment evidence enrichment failed")
    })
}

fn competition_comment_evidence_failed(enriched: &EnrichedIssue) -> bool {
    enriched.competition.warnings.iter().any(|warning| {
        warning
            .to_lowercase()
            .contains("comment evidence enrichment failed")
    })
}

fn load_cached_enrichment(
    paths: &IssueFinderPaths,
    issue: &GitHubIssue,
) -> Result<Option<EnrichedIssue>> {
    let path = paths.enrichment_cache_path(&issue.repo_full_name, issue.number);
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("unable to read {}", path.display()))?;
    let enriched = serde_json::from_str::<EnrichedIssue>(&raw)?;
    let fetched_at = DateTime::parse_from_rfc3339(&enriched.source_fetched_at)
        .map(|value| value.with_timezone(&Utc))?;
    if Utc::now() - fetched_at > Duration::minutes(ENRICHMENT_CACHE_TTL_MINUTES) {
        return Ok(None);
    }
    Ok(Some(enriched))
}

fn save_cached_enrichment(
    paths: &IssueFinderPaths,
    issue: &GitHubIssue,
    enriched: &EnrichedIssue,
) -> Result<()> {
    atomic_write(
        &paths.enrichment_cache_path(&issue.repo_full_name, issue.number),
        serde_json::to_vec_pretty(enriched)?,
    )
}

fn load_source_cache<T: DeserializeOwned>(
    path: &std::path::Path,
    ttl_minutes: i64,
) -> Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("unable to read {}", path.display()))?;
    let Ok(payload) = serde_json::from_str::<SourceCachePayload<T>>(&raw) else {
        return Ok(None);
    };
    if Utc::now() - payload.fetched_at > Duration::minutes(ttl_minutes) {
        return Ok(None);
    }
    Ok(Some(payload.value))
}

fn save_source_cache<T: Serialize>(path: &std::path::Path, value: &T) -> Result<()> {
    let payload = SourceCachePayload {
        fetched_at: Utc::now(),
        value,
    };
    atomic_write(path, serde_json::to_vec_pretty(&payload)?)
}

fn repo_cache_key(owner: &str, repo: &str) -> String {
    format!("{owner}/{repo}")
}

fn issue_cache_key(owner: &str, repo: &str, number: u64) -> String {
    format!("{owner}/{repo}#{number}")
}

async fn require_success(response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
        anyhow::bail!("GitHub rate limit or secondary throttle while enriching issue");
    }
    anyhow::bail!("GitHub enrichment request failed with {status}: {body}");
}

fn split_repo_full_name(repo_full_name: &str) -> Option<(String, String)> {
    let (owner, repo) = repo_full_name.split_once('/')?;
    Some((owner.to_string(), repo.to_string()))
}

fn push_pr_lead(
    leads: &mut Vec<(String, u64, PullRequestRelation, String)>,
    repo: String,
    number: u64,
    relation: PullRequestRelation,
    source: String,
) {
    if let Some((_, _, existing_relation, _)) = leads
        .iter_mut()
        .find(|(existing, n, _, _)| existing.eq_ignore_ascii_case(&repo) && *n == number)
    {
        if relation == PullRequestRelation::ExplicitResolution {
            *existing_relation = relation;
        }
    } else {
        leads.push((repo, number, relation, source));
    }
}

fn pr_identity(value: &serde_json::Value, issue_url: &str) -> Option<(String, u64)> {
    let url = url::Url::parse(value.get("html_url")?.as_str()?).ok()?;
    let target = url::Url::parse(issue_url).ok()?;
    if url.host_str() != target.host_str() {
        return None;
    }
    let parts = url.path_segments()?.collect::<Vec<_>>();
    if parts.len() != 4 || !matches!(parts[2], "pull" | "issues") {
        return None;
    }
    let number = parts[3].parse::<u64>().ok()?;
    if value.get("number").and_then(serde_json::Value::as_u64) != Some(number) {
        return None;
    }
    Some((format!("{}/{}", parts[0], parts[1]), number))
}

fn search_title_terms(title: &str) -> Vec<String> {
    title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|term| {
            term.len() > 3
                && !matches!(
                    term.as_str(),
                    "issue"
                        | "error"
                        | "with"
                        | "this"
                        | "that"
                        | "when"
                        | "from"
                        | "does"
                        | "should"
                        | "cannot"
                        | "support"
                        | "please"
                )
        })
        .take(3)
        .collect()
}

fn explicitly_resolves(body: &str, pr_repo: &str, issue: &GitHubIssue) -> bool {
    let mut in_code = false;
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            in_code = !in_code;
            continue;
        }
        if in_code || line.starts_with('>') || line.contains('`') || line.contains("<!--") {
            continue;
        }
        let words = line.split_whitespace().collect::<Vec<_>>();
        for (index, pair) in words.windows(2).enumerate() {
            let keyword = pair[0]
                .trim_matches(|c: char| !c.is_ascii_alphabetic())
                .to_ascii_lowercase();
            if !matches!(
                keyword.as_str(),
                "close"
                    | "closes"
                    | "closed"
                    | "fix"
                    | "fixes"
                    | "fixed"
                    | "resolve"
                    | "resolves"
                    | "resolved"
            ) {
                continue;
            }
            if index > 0
                && matches!(
                    words[index - 1].to_ascii_lowercase().as_str(),
                    "not" | "never" | "doesn't" | "don't"
                )
            {
                continue;
            }
            let reference = pair[1]
                .trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']' | ',' | '.' | ';'));
            if let Some(number) = reference
                .strip_prefix('#')
                .and_then(|number| number.parse::<u64>().ok())
            {
                if number == issue.number && pr_repo.eq_ignore_ascii_case(&issue.repo_full_name) {
                    return true;
                }
            }
            if let Some((repo, number)) = reference.rsplit_once('#') {
                if repo.eq_ignore_ascii_case(&issue.repo_full_name)
                    && number.parse::<u64>().ok() == Some(issue.number)
                {
                    return true;
                }
            }
            if let (Ok(reference), Ok(target)) =
                (url::Url::parse(reference), url::Url::parse(&issue.url))
            {
                if reference.host_str() == target.host_str()
                    && reference
                        .path()
                        .trim_end_matches('/')
                        .eq_ignore_ascii_case(target.path().trim_end_matches('/'))
                {
                    return true;
                }
            }
        }
    }
    false
}

fn trailing_sample_pages(total_count: u64, page_size: usize) -> Vec<u64> {
    if total_count == 0 {
        return vec![1];
    }

    let page_size = page_size as u64;
    let last_page = ((total_count.saturating_sub(1)) / page_size) + 1;
    let last_page_count = ((total_count.saturating_sub(1)) % page_size) + 1;
    if last_page > 1 && last_page_count < page_size {
        vec![last_page - 1, last_page]
    } else {
        vec![last_page]
    }
}

fn tail_limited<T>(items: Vec<T>, limit: usize) -> Vec<T> {
    let skip = items.len().saturating_sub(limit);
    items.into_iter().skip(skip).collect()
}

fn timestamp_within_days(value: Option<&str>, days: i64, now: DateTime<Utc>) -> bool {
    let Some(value) = value else {
        return false;
    };
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| now - timestamp.with_timezone(&Utc) <= Duration::days(days))
        .unwrap_or(false)
}

fn is_recent(timestamp: &str, days: i64) -> bool {
    timestamp_within_days(Some(timestamp), days, Utc::now())
}

fn is_maintainer_association(value: &str) -> bool {
    matches!(
        value.to_ascii_uppercase().as_str(),
        "OWNER" | "MEMBER" | "COLLABORATOR"
    )
}

fn unique_nonempty(values: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    values
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn excerpt(value: String, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod availability_http_tests {
    use super::*;
    use serde_json::{json, Value};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn issue() -> GitHubIssue {
        GitHubIssue {
            id: 3,
            number: 3,
            title: "Fix bug".into(),
            body: "Wrong behavior".into(),
            labels: vec![],
            url: "https://github.com/o/r/issues/3".into(),
            repo_full_name: "o/r".into(),
            repo_name: "r".into(),
            repo_description: String::new(),
            repo_stars: 1,
            created_at: "2026-09-01T00:00:00Z".into(),
            updated_at: "2026-09-01T00:00:00Z".into(),
        }
    }

    fn pr(body: &str, state: Option<&str>, merged: bool) -> Value {
        json!({"number": 9, "html_url":"https://github.com/o/r/pull/9", "state":state,
            "draft": false, "merged":merged, "merged_at": if merged { Some("2026-09-30T00:00:00Z") } else { None },
            "base":{"ref":"main", "repo":{"full_name":"o/r"}}, "body":body})
    }

    fn linked() -> Value {
        json!([{"event":"cross-referenced", "source":{"issue":{"number":9, "html_url":"https://github.com/o/r/pull/9", "state":"closed", "pull_request":{}}}}])
    }

    fn status() -> Value {
        json!({"state":"open", "assignees":[], "locked":false})
    }

    fn repo() -> Value {
        json!({"archived":false, "default_branch":"main"})
    }

    async fn run(
        responses: Vec<(Value, Option<&str>)>,
        budget: usize,
        depth: AvailabilityDepth,
        candidate: GitHubIssue,
    ) -> (AvailabilitySnapshot, Vec<String>, GitHubApiBudgetReport) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api_base_url = format!("http://{}", listener.local_addr().unwrap());
        let responses = responses
            .into_iter()
            .map(|(body, header)| (body, header.map(str::to_owned)))
            .collect::<Vec<_>>();
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for (body, link) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(StdDuration::from_secs(3)))
                    .unwrap();
                let mut bytes = [0; 8192];
                let count = stream.read(&mut bytes).unwrap();
                requests.push(
                    String::from_utf8_lossy(&bytes[..count])
                        .lines()
                        .next()
                        .unwrap()
                        .to_owned(),
                );
                let status = body
                    .get("_http_status")
                    .and_then(Value::as_u64)
                    .unwrap_or(200);
                let body = body.to_string();
                let link = link
                    .map(|value| format!("Link: {value}\r\n"))
                    .unwrap_or_default();
                write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\n{link}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            requests
        });
        let client = GitHubEnrichmentClient {
            http: reqwest::Client::builder()
                .timeout(StdDuration::from_secs(3))
                .build()
                .unwrap(),
            token: "fixture-token".into(),
            api_base_url,
            budget: GitHubApiBudget::with_total_budget(Some(budget)),
        };
        let dir = tempfile::tempdir().unwrap();
        let paths = IssueFinderPaths {
            home: dir.path().to_owned(),
            config: dir.path().join("config"),
            cache_dir: dir.path().join("cache"),
            workspaces_dir: dir.path().join("workspaces"),
            inbox_dir: dir.path().join("inbox"),
            reports_dir: dir.path().join("reports"),
        };
        let snapshot = client.availability(&paths, &candidate, depth).await;
        (snapshot, handle.join().unwrap(), client.request_stats())
    }

    #[tokio::test]
    async fn verifies_actual_merge_instead_of_timeline_closed_state() {
        for (body, state, merged, strong, repair) in [
            ("Fixes #3", Some("open"), false, true, false),
            ("Fixes #3", Some("closed"), true, false, true),
            ("Fixes #3", Some("closed"), false, false, false),
            ("Mentions #3", Some("open"), false, false, false),
        ] {
            let (facts, requests, _) = run(
                vec![
                    (status(), None),
                    (repo(), None),
                    (linked(), None),
                    (pr(body, state, merged), None),
                ],
                8,
                AvailabilityDepth::Initial,
                issue(),
            )
            .await;
            assert_eq!(requests.len(), 4);
            assert!(requests[3].starts_with("GET /repos/o/r/pulls/9 "));
            assert_eq!(facts.has_strong_open_competition(), strong);
            assert_eq!(facts.has_merged_resolution_evidence(), repair);
            assert!(facts.checks_complete());
        }
    }

    #[tokio::test]
    async fn follows_timeline_pagination_and_reports_bound_or_missing_link() {
        let (facts, requests, _) = run(
            vec![
                (status(), None),
                (repo(), None),
                (
                    json!([]),
                    Some("<https://api.github.com/page2>; rel=\"next\""),
                ),
                (linked(), None),
                (pr("Fixes #3", Some("open"), false), None),
            ],
            8,
            AvailabilityDepth::Initial,
            issue(),
        )
        .await;
        assert!(facts.has_strong_open_competition());
        assert_eq!(facts.linked_coverage.pages_fetched, 2);
        assert_eq!(facts.linked_coverage.status, CoverageStatus::Complete);
        assert!(requests[3].contains("page=2"));
        for responses in [
            vec![
                (status(), None),
                (repo(), None),
                (json!([]), Some("<ignored>; rel=\"next\"")),
                (json!([]), Some("<ignored>; rel=\"next\"")),
            ],
            vec![
                (status(), None),
                (repo(), None),
                (json!(vec![json!({"event":"commented"}); 100]), None),
            ],
        ] {
            let (facts, _, _) = run(responses, 8, AvailabilityDepth::Initial, issue()).await;
            assert_eq!(facts.linked_coverage.status, CoverageStatus::Partial);
            assert!(!facts.fresh_fix_available());
        }
    }

    #[tokio::test]
    async fn missing_pr_state_and_exhausted_budget_remain_unknown() {
        let (facts, _, _) = run(
            vec![
                (status(), None),
                (repo(), None),
                (linked(), None),
                (pr("Fixes #3", None, false), None),
            ],
            8,
            AvailabilityDepth::Initial,
            issue(),
        )
        .await;
        assert!(!facts.checks_complete());
        assert!(!facts.has_strong_open_competition());
        assert!(!facts.uncertainties.is_empty());
        let (facts, requests, report) = run(
            vec![(status(), None), (repo(), None)],
            2,
            AvailabilityDepth::Final,
            issue(),
        )
        .await;
        assert_eq!(requests.len(), 2);
        assert_eq!(report.total_network_requests, 2);
        assert_eq!(facts.linked_coverage.status, CoverageStatus::Unavailable);
        assert_eq!(facts.search_coverage.status, CoverageStatus::Unavailable);
        assert!(facts
            .uncertainties
            .iter()
            .any(|error| error.contains("budget exhausted")));
    }

    #[tokio::test]
    async fn http_failures_are_preserved_in_snapshot_instead_of_becoming_empty_evidence() {
        let (facts, _, _) = run(
            vec![
                (status(), None),
                (json!({"_http_status": 503}), None),
                (json!({"_http_status": 503}), None),
            ],
            8,
            AvailabilityDepth::Initial,
            issue(),
        )
        .await;
        assert_eq!(facts.issue_state.as_deref(), Some("open"));
        assert_eq!(facts.archived, None);
        assert_eq!(facts.linked_coverage.status, CoverageStatus::Unavailable);
        assert!(facts
            .uncertainties
            .iter()
            .any(|error| error.contains("503")));
        assert!(!facts.fresh_fix_available());
    }

    #[tokio::test]
    async fn unlinked_search_matches_are_uncertain_until_actual_resolution_verified() {
        let result = json!({"total_count":1,"incomplete_results":false,"items":[{"number":9,"html_url":"https://github.com/o/r/pull/9","pull_request":{},"title":"Fix bug"}]});
        let (facts, requests, _) = run(
            vec![
                (status(), None),
                (repo(), None),
                (json!([]), None),
                (result, None),
                (
                    pr("Another change for related symptoms", Some("open"), false),
                    None,
                ),
            ],
            8,
            AvailabilityDepth::Final,
            issue(),
        )
        .await;
        assert!(requests[3].starts_with("GET /search/issues?"));
        assert_eq!(
            facts.pull_requests[0].relation,
            PullRequestRelation::SearchLead
        );
        assert!(!facts.has_strong_open_competition());
        assert!(!facts.fresh_fix_available());
        assert!(facts
            .uncertainties
            .iter()
            .any(|warning| warning.contains("no verified resolving relationship")));
    }

    #[test]
    fn resolving_references_require_exact_issue_and_repository_without_quoted_examples() {
        let candidate = issue();
        assert!(explicitly_resolves(
            "Resolves o/r#3",
            "other/repo",
            &candidate
        ));
        assert!(explicitly_resolves(
            "Fixes https://github.com/o/r/issues/3.",
            "o/r",
            &candidate
        ));
        for body in [
            "Mentions #3",
            "Fixes #30",
            "Fixes other/r#3",
            "> Fixes #3",
            "```\nFixes #3\n```",
            "Does not fix #3",
        ] {
            assert!(!explicitly_resolves(body, "o/r", &candidate), "{body}");
        }
        assert!(!explicitly_resolves("Fixes #3", "other/repo", &candidate));
    }
}

#[cfg(test)]
mod system1_evidence_http_tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, Instant};

    use serde_json::{json, Value};

    use super::*;
    use crate::system1::evidence::CommentFetchStatus;

    fn issue() -> GitHubIssue {
        GitHubIssue {
            id: 3,
            number: 3,
            title: "Fix this typo".into(),
            body: "Correct a typo in README.".into(),
            labels: Vec::new(),
            url: "https://github.com/o/r/issues/3".into(),
            repo_full_name: "o/r".into(),
            repo_name: "r".into(),
            repo_description: String::new(),
            repo_stars: 1,
            created_at: "2026-09-01T00:00:00Z".into(),
            updated_at: "2026-09-01T00:00:00Z".into(),
        }
    }

    fn paths(dir: &std::path::Path) -> IssueFinderPaths {
        IssueFinderPaths {
            home: dir.to_owned(),
            config: dir.join("config.toml"),
            cache_dir: dir.join("cache"),
            workspaces_dir: dir.join("workspaces"),
            inbox_dir: dir.join("inbox"),
            reports_dir: dir.join("reports"),
        }
    }

    fn server(
        responses: Vec<(u16, Value)>,
    ) -> (GitHubEnrichmentClient, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let api_base_url = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut requests = Vec::new();
            for (status, body) in responses {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "missing mock GitHub request");
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("mock GitHub accept failed: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = [0; 8_192];
                let count = stream.read(&mut bytes).unwrap();
                requests.push(String::from_utf8_lossy(&bytes[..count]).into_owned());
                let body = body.to_string();
                write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            requests
        });
        // Construct directly so offline tests never resolve or forward real developer credentials.
        let client = GitHubEnrichmentClient {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap(),
            token: "fixture-token".into(),
            api_base_url,
            budget: GitHubApiBudget::with_total_budget(Some(5)),
        };
        (client, handle)
    }

    #[tokio::test]
    async fn preserves_full_comment_identity_and_reuses_versioned_evidence_cache() {
        let complete_body = format!(
            "{} I am no longer working on this.",
            "background ".repeat(100)
        );
        let (client, handle) = server(vec![
            (
                200,
                json!({"comments": 1, "state": "open", "assignees": [{"login": "assigned-person"}]}),
            ),
            (
                200,
                json!([{"id": 42, "html_url": "https://github.com/o/r/issues/3#issuecomment-42",
                "body": complete_body, "user": {"login": "participant"}, "author_association": "MEMBER",
                "created_at": "2026-09-01T01:00:00Z", "updated_at": "2026-09-02T01:00:00Z"}]),
            ),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        let issue = issue();
        let enriched = EnrichedIssue::from_issue(&issue);
        let snapshot = client
            .system1_evidence(&paths, &issue, &enriched, false)
            .await
            .unwrap();
        let requests = handle.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].contains("per_page=30&page=1"));
        assert_eq!(snapshot.comments.status, CommentFetchStatus::Complete);
        let comment = &snapshot.comments.comments[0];
        assert_eq!(comment.body.text, complete_body);
        assert_eq!(comment.id, Some(42));
        assert_eq!(comment.author_association, "MEMBER");
        assert_eq!(comment.updated_at.as_deref(), Some("2026-09-02T01:00:00Z"));
        assert_eq!(
            snapshot.github_status.assignees,
            Some(vec!["assigned-person".into()])
        );
        assert_eq!(snapshot.github_status.linked_merged_pr_count, None);
        let cached = client
            .system1_evidence(&paths, &issue, &enriched, false)
            .await
            .unwrap();
        assert_eq!(snapshot.material_hash(), cached.material_hash());
        assert_eq!(client.request_stats().total_network_requests, 2);
        let mut changed = issue.clone();
        changed.updated_at = "2026-09-03T01:00:00Z".into();
        let changed_enrichment = EnrichedIssue::from_issue(&changed);
        let unavailable = client
            .system1_evidence(&paths, &changed, &changed_enrichment, false)
            .await
            .unwrap();
        assert_eq!(unavailable.comments.status, CommentFetchStatus::Unavailable);
        assert_ne!(snapshot.material_hash(), unavailable.material_hash());
    }

    #[tokio::test]
    async fn comment_failure_and_missing_count_do_not_claim_an_empty_complete_timeline() {
        for (details, comment_status, expected) in [
            (json!({"comments": 1}), 503, CommentFetchStatus::Unavailable),
            (json!({}), 200, CommentFetchStatus::Sampled),
        ] {
            let (client, handle) =
                server(vec![(200, details.clone()), (comment_status, json!([]))]);
            let dir = tempfile::tempdir().unwrap();
            let issue = issue();
            let snapshot = client
                .system1_evidence(
                    &paths(dir.path()),
                    &issue,
                    &EnrichedIssue::from_issue(&issue),
                    false,
                )
                .await
                .unwrap();
            handle.join().unwrap();
            assert_eq!(snapshot.comments.status, expected);
            assert_eq!(snapshot.comments.total_count, details["comments"].as_u64());
            assert!(snapshot.comments.comments.is_empty());
            assert_eq!(snapshot.github_status.assignees, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use crate::competition::CompetitionFacts;
    use crate::github::GitHubIssue;

    use super::{
        canonicalize_enriched_issue_repo, competition_timeline_missing,
        competition_timeline_not_fetched, fork_velocity, star_velocity, tail_limited,
        trailing_sample_pages, EnrichedIssue, TimestampedSample,
    };

    fn sample(timestamp: &str) -> TimestampedSample {
        TimestampedSample {
            source_ref: "repo:stargazers.sample_recent_100.0".to_string(),
            actor: Some("user".to_string()),
            timestamp: Some(timestamp.to_string()),
        }
    }

    #[test]
    fn calculates_star_velocity_buckets() {
        let now = Utc.with_ymd_and_hms(2026, 6, 2, 0, 0, 0).unwrap();
        let samples = vec![
            sample("2026-06-01T00:00:00Z"),
            sample("2026-05-20T00:00:00Z"),
            sample("2026-04-01T00:00:00Z"),
        ];

        assert_eq!(star_velocity(&samples, 7, now), 1);
        assert_eq!(star_velocity(&samples, 14, now), 2);
        assert_eq!(star_velocity(&samples, 30, now), 2);
    }

    #[test]
    fn calculates_fork_velocity_proxy() {
        let now = Utc.with_ymd_and_hms(2026, 6, 2, 0, 0, 0).unwrap();
        let samples = vec![
            sample("2026-05-15T00:00:00Z"),
            sample("2026-04-01T00:00:00Z"),
        ];
        assert_eq!(fork_velocity(&samples, 30, now), 1);
    }

    #[test]
    fn samples_previous_page_when_last_page_is_partial() {
        assert_eq!(trailing_sample_pages(10_001, 100), vec![100, 101]);
        assert_eq!(trailing_sample_pages(10_000, 100), vec![100]);
        assert_eq!(trailing_sample_pages(31, 30), vec![1, 2]);
        assert_eq!(trailing_sample_pages(30, 30), vec![1]);
    }

    #[test]
    fn keeps_tail_entries_after_multi_page_sample() {
        let values = (0..101).collect::<Vec<_>>();
        let tail = tail_limited(values, 100);
        assert_eq!(tail.len(), 100);
        assert_eq!(tail[0], 1);
        assert_eq!(tail[99], 100);
    }

    #[test]
    fn distinguishes_not_fetched_from_failed_or_skipped_timeline() {
        let mut enriched = EnrichedIssue::from_issue(&issue());
        assert!(competition_timeline_missing(&enriched));
        assert!(competition_timeline_not_fetched(&enriched));

        enriched.competition = CompetitionFacts {
            warnings: vec!["Competition timeline enrichment failed: rate limit".to_string()],
            ..CompetitionFacts::default()
        };
        assert!(competition_timeline_missing(&enriched));
        assert!(!competition_timeline_not_fetched(&enriched));

        enriched.competition = CompetitionFacts {
            warnings: vec!["Competition timeline completion skipped by budget".to_string()],
            ..CompetitionFacts::default()
        };
        assert!(competition_timeline_missing(&enriched));
        assert!(!competition_timeline_not_fetched(&enriched));
    }

    #[test]
    fn canonicalizes_issue_repo_from_repository_full_name() {
        let mut enriched = EnrichedIssue::from_issue(&GitHubIssue {
            repo_full_name: "old-owner/repo".to_string(),
            repo_name: "repo".to_string(),
            url: "https://github.com/old-owner/repo/issues/1".to_string(),
            ..issue()
        });
        enriched.repository.full_name = "new-owner/repo".to_string();

        canonicalize_enriched_issue_repo(&mut enriched);

        assert_eq!(enriched.issue.repo_full_name, "new-owner/repo");
        assert_eq!(
            enriched.issue.url,
            "https://github.com/new-owner/repo/issues/1"
        );
    }

    fn issue() -> GitHubIssue {
        GitHubIssue {
            id: 1,
            number: 1,
            title: "Test issue".to_string(),
            body: "Test body".to_string(),
            labels: vec!["good first issue".to_string()],
            url: "https://github.com/owner/repo/issues/1".to_string(),
            repo_full_name: "owner/repo".to_string(),
            repo_name: "repo".to_string(),
            repo_description: "Test repository".to_string(),
            repo_stars: 100,
            created_at: "2026-06-01T00:00:00Z".to_string(),
            updated_at: "2026-06-01T00:00:00Z".to_string(),
        }
    }
}
