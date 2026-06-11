use std::fs;
use std::time::Duration as StdDuration;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::competition::{assess_competition, CompetitionFacts, TimelineIssueReference};
use crate::config::Config;
use crate::github::GitHubIssue;
use crate::github_budget::{GitHubApiBudget, GitHubApiBudgetReport, GitHubRequestSource};
use crate::paths::{atomic_write, IssueFinderPaths};

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
    author_association: Option<String>,
    user: Option<UserApiResponse>,
}

#[derive(Debug, Deserialize)]
struct CommentApiResponse {
    body: Option<String>,
    author_association: Option<String>,
    created_at: Option<String>,
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
        if !refresh {
            if let Ok(Some(cached)) = load_cached_enrichment(paths, issue) {
                if !include_competition_timeline || !competition_timeline_missing(&cached) {
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
            Ok(repo_facts) => enriched.repository = repo_facts,
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
            return current.clone();
        }

        if !refresh {
            if let Ok(Some(cached)) = load_cached_enrichment(paths, issue) {
                if !competition_timeline_missing(&cached) {
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
mod tests {
    use chrono::{TimeZone, Utc};

    use crate::competition::CompetitionFacts;
    use crate::github::GitHubIssue;

    use super::{
        competition_timeline_missing, competition_timeline_not_fetched, fork_velocity,
        star_velocity, tail_limited, trailing_sample_pages, EnrichedIssue, TimestampedSample,
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
