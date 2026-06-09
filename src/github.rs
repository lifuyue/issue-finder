use std::collections::{HashMap, HashSet};
use std::fs;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use futures::stream::{self, StreamExt};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::{Config, ProfileConfig};
use crate::discovery::{
    gfi_repositories, merge_candidates, overlay_repositories, profile_trusted_repositories,
    DiscoveryCandidate, DiscoveryOutput, DiscoveryScope, DiscoveryStageStats, RepoTrustTier,
    RepositoryScope, TrustedRepository,
};
use crate::errors::IssueFinderError;
use crate::github_budget::{GitHubApiBudget, GitHubApiBudgetReport, GitHubRequestSource};
use crate::paths::{atomic_write, IssueFinderPaths};

const SEARCH_CACHE_TTL_MINUTES: i64 = 180;
const FALLBACK_DISCOVERY_CACHE_TTL_MINUTES: i64 = 360;
const TRUSTED_OVERLAY_REPO_REQUEST_LIMIT: usize = 15;
const PROFILE_TRUSTED_REPO_REQUEST_LIMIT: usize = 20;
const GFI_REPO_REQUEST_LIMIT: usize = 20;
const GLOBAL_SEARCH_REQUEST_LIMIT: usize = 0;
const FALLBACK_TRUSTED_REPO_REQUEST_LIMIT: usize = 20;
const FALLBACK_TRUSTED_REPO_CANDIDATE_LIMIT: usize = 3;
const FALLBACK_TRUSTED_REPO_PER_PAGE: usize = 20;
const FALLBACK_GLOBAL_SEARCH_REQUEST_LIMIT: usize = 8;
const FALLBACK_GLOBAL_SEARCH_PER_PAGE: usize = 15;
const TRUSTED_OVERLAY_REPO_CANDIDATE_LIMIT: usize = 8;
const PROFILE_TRUSTED_REPO_CANDIDATE_LIMIT: usize = 10;
const GFI_REPO_CANDIDATE_LIMIT: usize = 4;
const TRUSTED_LABEL_PER_PAGE: usize = 8;
const PROFILE_TRUSTED_LABEL_PER_PAGE: usize = 20;
const GLOBAL_SEARCH_PER_PAGE: usize = 30;
const DISCOVERY_SEARCH_CONCURRENCY_LIMIT: usize = 1;
const REPO_SCOPED_LABEL_PER_PAGE: usize = 30;
const REPO_SCOPED_SEARCH_PER_PAGE: usize = 30;
const REPO_SCOPED_RECENT_PER_PAGE: usize = 100;

const BEGINNER_LABELS: [&str; 8] = [
    "good first issue",
    "good-first-issue",
    "beginner",
    "beginner-friendly",
    "easy",
    "starter",
    "help wanted",
    "low-hanging-fruit",
];

const FALLBACK_TRUSTED_LABELS: [&str; 3] = ["good first issue", "good-first-issue", "help wanted"];
const REPO_SCOPED_BEGINNER_LABELS: [&str; 6] = [
    "good first issue",
    "good-first-issue",
    "beginner",
    "beginner-friendly",
    "easy",
    "starter",
];
const REPO_SCOPED_ACTIONABLE_KEYWORDS: [&str; 6] =
    ["bug", "repro", "expected actual", "panic", "error", "test"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl IssueRef {
    pub fn repo_full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    pub fn parse(value: &str) -> Result<Self> {
        if value.starts_with("http://") || value.starts_with("https://") {
            return Self::parse_url(value);
        }

        let (repo_part, number_part) = value
            .split_once('#')
            .ok_or(IssueFinderError::InvalidIssueReference)?;
        let (owner, repo) = repo_part
            .split_once('/')
            .ok_or(IssueFinderError::InvalidIssueReference)?;
        let number = number_part
            .parse::<u64>()
            .map_err(|_| IssueFinderError::InvalidIssueReference)?;

        if owner.trim().is_empty() || repo.trim().is_empty() || number == 0 {
            return Err(IssueFinderError::InvalidIssueReference.into());
        }

        Ok(Self {
            owner: owner.to_string(),
            repo: repo.to_string(),
            number,
        })
    }

    pub fn parse_url(value: &str) -> Result<Self> {
        let url = Url::parse(value).map_err(|_| IssueFinderError::InvalidIssueReference)?;
        if url.host_str() != Some("github.com") {
            return Err(IssueFinderError::InvalidIssueReference.into());
        }

        let parts = url
            .path_segments()
            .ok_or(IssueFinderError::InvalidIssueReference)?;
        let segments = parts.collect::<Vec<_>>();
        if segments.len() < 4 || segments[2] != "issues" {
            return Err(IssueFinderError::InvalidIssueReference.into());
        }

        let number = segments[3]
            .parse::<u64>()
            .map_err(|_| IssueFinderError::InvalidIssueReference)?;

        Ok(Self {
            owner: segments[0].to_string(),
            repo: segments[1].to_string(),
            number,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitHubIssue {
    pub id: u64,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub url: String,
    pub repo_full_name: String,
    pub repo_name: String,
    pub repo_description: String,
    pub repo_stars: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DiscoveryCachePayload {
    fetched_at: DateTime<Utc>,
    candidates: Vec<DiscoveryCandidate>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    items: Vec<SearchIssue>,
}

#[derive(Debug, Deserialize)]
struct SearchIssue {
    id: u64,
    number: u64,
    title: String,
    body: Option<String>,
    html_url: String,
    repository_url: String,
    labels: Vec<GitHubLabel>,
    pull_request: Option<serde_json::Value>,
    locked: bool,
    assignee: Option<serde_json::Value>,
    assignees: Option<Vec<serde_json::Value>>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum GitHubLabel {
    Name(String),
    Object { name: Option<String> },
}

#[derive(Debug, Deserialize)]
struct RepoResponse {
    full_name: String,
    name: String,
    description: Option<String>,
    stargazers_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct IssueResponse {
    id: u64,
    number: u64,
    title: String,
    body: Option<String>,
    html_url: String,
    labels: Vec<GitHubLabel>,
    pull_request: Option<serde_json::Value>,
    locked: bool,
    assignee: Option<serde_json::Value>,
    assignees: Option<Vec<serde_json::Value>>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone)]
struct RepoMetadata {
    full_name: String,
    name: String,
    description: String,
    stars: u64,
}

pub struct GitHubClient {
    http: reqwest::Client,
    token: String,
    api_base_url: String,
    budget: GitHubApiBudget,
}

impl GitHubClient {
    pub fn new(config: &Config) -> Result<Self> {
        Self::with_budget(config, GitHubApiBudget::from_env())
    }

    pub fn with_budget(config: &Config, budget: GitHubApiBudget) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("issue-finder")
            .build()?;
        Ok(Self {
            http,
            token: config.github.token.clone(),
            api_base_url: std::env::var("ISSUE_FINDER_GITHUB_API_BASE")
                .unwrap_or_else(|_| "https://api.github.com".to_string()),
            budget,
        })
    }

    #[cfg(test)]
    pub fn with_api_base_and_budget(
        config: &Config,
        api_base_url: impl Into<String>,
        budget: GitHubApiBudget,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("issue-finder")
            .build()?;
        Ok(Self {
            http,
            token: config.github.token.clone(),
            api_base_url: api_base_url.into(),
            budget,
        })
    }

    pub fn request_stats(&self) -> GitHubApiBudgetReport {
        self.budget.report()
    }

    pub async fn discover_issues(
        &self,
        paths: &IssueFinderPaths,
        refresh: bool,
    ) -> Result<Vec<GitHubIssue>> {
        let profile = Config::default().profile;
        Ok(self
            .discover_candidates(paths, refresh, &profile)
            .await?
            .into_iter()
            .map(|candidate| candidate.issue)
            .collect())
    }

    pub async fn discover_candidates(
        &self,
        paths: &IssueFinderPaths,
        refresh: bool,
        profile: &ProfileConfig,
    ) -> Result<Vec<DiscoveryCandidate>> {
        let mut lanes = primary_trusted_repository_lanes(profile)?
            .into_iter()
            .map(|(repository, trust_tier)| SearchLaneRequest::trusted(repository, trust_tier))
            .collect::<Vec<_>>();

        lanes.extend(
            global_search_lanes(profile)
                .into_iter()
                .take(GLOBAL_SEARCH_REQUEST_LIMIT)
                .map(SearchLaneRequest::global),
        );

        let lane_results = stream::iter(lanes.into_iter().map(|lane| async move {
            self.fetch_lane_candidates_cached(paths, refresh, lane, profile)
                .await
        }))
        .buffer_unordered(DISCOVERY_SEARCH_CONCURRENCY_LIMIT)
        .collect::<Vec<_>>()
        .await;

        let candidates = collect_lane_candidates(lane_results)?;

        let candidates = merge_candidates(candidates, profile);
        Ok(candidates)
    }

    pub async fn discover_trusted_fallback_candidates(
        &self,
        paths: &IssueFinderPaths,
        refresh: bool,
        profile: &ProfileConfig,
    ) -> Result<Vec<DiscoveryCandidate>> {
        let profile_repositories =
            profile_trusted_repositories(profile, FALLBACK_TRUSTED_REPO_REQUEST_LIMIT)?;
        let mut repositories = profile_repositories.clone();
        repositories.extend(gfi_repositories(
            profile,
            FALLBACK_TRUSTED_REPO_REQUEST_LIMIT,
        )?);
        let repositories = dedupe_repositories(repositories)
            .into_iter()
            .take(FALLBACK_TRUSTED_REPO_REQUEST_LIMIT)
            .collect::<Vec<_>>();
        let requests = fallback_trusted_repository_requests(&profile_repositories, &repositories);

        let lane_results = stream::iter(requests.into_iter().map(|request| async move {
            self.list_trusted_repository_fallback_lane_cached(paths, refresh, request, profile)
                .await
        }))
        .buffer_unordered(DISCOVERY_SEARCH_CONCURRENCY_LIMIT)
        .collect::<Vec<_>>()
        .await;

        let candidates = collect_fallback_lane_candidates(lane_results);
        let candidates = merge_candidates(candidates, profile);
        Ok(cap_candidates_per_repo(
            candidates,
            FALLBACK_TRUSTED_REPO_CANDIDATE_LIMIT,
        ))
    }

    pub async fn discover_global_fallback_candidates(
        &self,
        paths: &IssueFinderPaths,
        refresh: bool,
        profile: &ProfileConfig,
    ) -> Result<Vec<DiscoveryCandidate>> {
        let lanes = fallback_global_search_lanes(profile)
            .into_iter()
            .take(FALLBACK_GLOBAL_SEARCH_REQUEST_LIMIT)
            .collect::<Vec<_>>();
        let search_spacing = self.fallback_global_search_spacing();
        let mut lane_results = Vec::new();
        for lane in lanes {
            let requests_before = self.request_stats().total_network_requests;
            let result = self
                .search_lane_cached(SearchLaneCacheRequest {
                    paths,
                    refresh,
                    query: &lane.query,
                    per_page: FALLBACK_GLOBAL_SEARCH_PER_PAGE,
                    lane_id: &lane.id,
                    request_source: GitHubRequestSource::DiscoveryFallbackGlobal,
                    profile,
                })
                .await;
            if result.as_ref().is_err_and(is_rate_limit_error) {
                break;
            }
            let made_network_request =
                self.request_stats().total_network_requests > requests_before;
            lane_results.push(result);
            if made_network_request && !search_spacing.is_zero() {
                tokio::time::sleep(search_spacing).await;
            }
        }

        let candidates = collect_fallback_lane_candidates(lane_results);
        let mut candidates = merge_candidates(candidates, profile);
        candidates.truncate(30);
        Ok(candidates)
    }

    pub async fn discover_repository_beginner_candidates(
        &self,
        paths: &IssueFinderPaths,
        refresh: bool,
        repository: &RepositoryScope,
        profile: &ProfileConfig,
    ) -> Result<DiscoveryOutput> {
        let mut diagnostics = DiscoveryScope::repository(repository.clone()).diagnostics();
        let mut candidates = Vec::new();

        for label in REPO_SCOPED_BEGINNER_LABELS {
            let lane_id = repo_scoped_label_lane_id("beginner_label", label);
            let output = self
                .list_repository_label_lane_cached(RepositoryLabelLaneRequest {
                    paths,
                    refresh,
                    repository,
                    label,
                    stage: "beginner_label",
                    lane_id: &lane_id,
                    per_page: REPO_SCOPED_LABEL_PER_PAGE,
                    profile,
                })
                .await?;
            diagnostics.discovery_stages.push(output.stats);
            candidates.extend(output.candidates);
        }

        Ok(DiscoveryOutput {
            candidates: merge_candidates(candidates, profile),
            diagnostics,
        })
    }

    pub async fn discover_repository_signal_candidates(
        &self,
        paths: &IssueFinderPaths,
        refresh: bool,
        repository: &RepositoryScope,
        profile: &ProfileConfig,
    ) -> DiscoveryOutput {
        let mut diagnostics = DiscoveryScope::repository(repository.clone()).diagnostics();
        let mut candidates = Vec::new();

        let help_wanted_lane = "repo_scoped:help_wanted".to_string();
        match self
            .list_repository_label_lane_cached(RepositoryLabelLaneRequest {
                paths,
                refresh,
                repository,
                label: "help wanted",
                stage: "help_wanted",
                lane_id: &help_wanted_lane,
                per_page: REPO_SCOPED_LABEL_PER_PAGE,
                profile,
            })
            .await
        {
            Ok(output) => {
                diagnostics.discovery_stages.push(output.stats);
                candidates.extend(output.candidates);
            }
            Err(error) => diagnostics
                .stage_errors
                .push(format!("{help_wanted_lane}: {error}")),
        }

        let mut search_rate_limited = false;

        for term in crate::scoring::profile_terms(profile)
            .into_iter()
            .filter(|term| term.len() >= 3)
            .take(6)
        {
            if search_rate_limited {
                break;
            }
            let lane_id = format!("repo_scoped:profile_term:{}", lane_fragment(&term));
            let query = repo_scoped_search_query(repository, &term);
            match self
                .search_repository_lane_cached(RepositorySearchLaneRequest {
                    paths,
                    refresh,
                    repository,
                    query: &query,
                    stage: "profile_term",
                    lane_id: &lane_id,
                    per_page: REPO_SCOPED_SEARCH_PER_PAGE,
                    profile,
                })
                .await
            {
                Ok(output) => {
                    diagnostics.discovery_stages.push(output.stats);
                    candidates.extend(output.candidates);
                }
                Err(error) => {
                    search_rate_limited = is_rate_limit_error(&error);
                    diagnostics.stage_errors.push(format!("{lane_id}: {error}"));
                }
            }
        }

        for keyword in REPO_SCOPED_ACTIONABLE_KEYWORDS {
            if search_rate_limited {
                break;
            }
            let lane_id = format!("repo_scoped:actionable_keyword:{}", lane_fragment(keyword));
            let query = repo_scoped_search_query(repository, keyword);
            match self
                .search_repository_lane_cached(RepositorySearchLaneRequest {
                    paths,
                    refresh,
                    repository,
                    query: &query,
                    stage: "actionable_keyword",
                    lane_id: &lane_id,
                    per_page: REPO_SCOPED_SEARCH_PER_PAGE,
                    profile,
                })
                .await
            {
                Ok(output) => {
                    diagnostics.discovery_stages.push(output.stats);
                    candidates.extend(output.candidates);
                }
                Err(error) => {
                    search_rate_limited = is_rate_limit_error(&error);
                    diagnostics.stage_errors.push(format!("{lane_id}: {error}"));
                }
            }
        }

        DiscoveryOutput {
            candidates: merge_candidates(candidates, profile),
            diagnostics,
        }
    }

    pub async fn discover_repository_recent_candidates(
        &self,
        paths: &IssueFinderPaths,
        refresh: bool,
        repository: &RepositoryScope,
        profile: &ProfileConfig,
        window: usize,
    ) -> Result<DiscoveryOutput> {
        let mut diagnostics = DiscoveryScope::repository(repository.clone()).diagnostics();
        let lane_id = format!("repo_scoped:recent_open:{window}");
        let output = self
            .list_repository_recent_open_cached(RepositoryRecentLaneRequest {
                paths,
                refresh,
                repository,
                window,
                lane_id: &lane_id,
                profile,
            })
            .await?;
        diagnostics.discovery_stages.push(output.stats);

        Ok(DiscoveryOutput {
            candidates: merge_candidates(output.candidates, profile),
            diagnostics,
        })
    }

    async fn fetch_lane_candidates_cached(
        &self,
        paths: &IssueFinderPaths,
        refresh: bool,
        lane: SearchLaneRequest,
        profile: &ProfileConfig,
    ) -> Result<Vec<DiscoveryCandidate>> {
        let source = lane.request_source();
        let cache_key = lane.cache_key();
        if !refresh {
            if let Some(cached) =
                load_cached_candidates(paths, source, &cache_key, SEARCH_CACHE_TTL_MINUTES)?
            {
                self.budget.record_cache_hit(source);
                return Ok(cached);
            }
        }

        let candidates = self.fetch_lane_candidates(lane, profile).await?;
        save_cached_candidates(paths, source, &cache_key, &candidates)?;
        Ok(candidates)
    }

    async fn list_trusted_repository_fallback_lane_cached(
        &self,
        paths: &IssueFinderPaths,
        refresh: bool,
        request: FallbackTrustedRequest,
        profile: &ProfileConfig,
    ) -> Result<Vec<DiscoveryCandidate>> {
        let label_id = normalize_label(request.label).replace(' ', "-");
        let id = format!(
            "fallback_trusted:{label_id}:{}",
            request.repository.full_name()
        );
        if !refresh {
            if let Some(cached) = load_cached_candidates(
                paths,
                GitHubRequestSource::DiscoveryFallbackTrusted,
                &id,
                FALLBACK_DISCOVERY_CACHE_TTL_MINUTES,
            )? {
                self.budget
                    .record_cache_hit(GitHubRequestSource::DiscoveryFallbackTrusted);
                return Ok(cached);
            }
        }

        let candidates = self
            .list_trusted_repository_fallback_lane(
                &request.repository,
                request.label,
                &id,
                request.trust_tier,
                FALLBACK_TRUSTED_REPO_CANDIDATE_LIMIT,
                profile,
            )
            .await?;
        save_cached_candidates(
            paths,
            GitHubRequestSource::DiscoveryFallbackTrusted,
            &id,
            &candidates,
        )?;
        Ok(candidates)
    }

    async fn search_lane_cached(
        &self,
        request: SearchLaneCacheRequest<'_>,
    ) -> Result<Vec<DiscoveryCandidate>> {
        let SearchLaneCacheRequest {
            paths,
            refresh,
            query,
            per_page,
            lane_id,
            request_source,
            profile,
        } = request;
        if !refresh {
            if let Some(cached) = load_cached_candidates(
                paths,
                request_source,
                lane_id,
                FALLBACK_DISCOVERY_CACHE_TTL_MINUTES,
            )? {
                self.budget.record_cache_hit(request_source);
                return Ok(cached);
            }
        }

        let candidates = self
            .search_lane(
                query,
                per_page,
                lane_id,
                request_source,
                RepoTrustTier::Global,
                profile,
            )
            .await?;
        save_cached_candidates(paths, request_source, lane_id, &candidates)?;
        Ok(candidates)
    }

    async fn list_repository_label_lane_cached(
        &self,
        request: RepositoryLabelLaneRequest<'_>,
    ) -> Result<RepositoryLaneOutput> {
        if !request.refresh {
            if let Some(cached) = load_cached_candidates(
                request.paths,
                GitHubRequestSource::DiscoveryRepository,
                request.lane_id,
                SEARCH_CACHE_TTL_MINUTES,
            )? {
                self.budget
                    .record_cache_hit(GitHubRequestSource::DiscoveryRepository);
                let stats = DiscoveryStageStats::new(
                    request.stage,
                    request.lane_id,
                    request.per_page,
                    cached.len(),
                    cached.len(),
                );
                return Ok(RepositoryLaneOutput {
                    candidates: cached,
                    stats,
                });
            }
        }

        let output = self.list_repository_label_lane(&request).await?;
        save_cached_candidates(
            request.paths,
            GitHubRequestSource::DiscoveryRepository,
            request.lane_id,
            &output.candidates,
        )?;
        Ok(output)
    }

    async fn search_repository_lane_cached(
        &self,
        request: RepositorySearchLaneRequest<'_>,
    ) -> Result<RepositoryLaneOutput> {
        if !request.refresh {
            if let Some(cached) = load_cached_candidates(
                request.paths,
                GitHubRequestSource::DiscoveryRepository,
                request.lane_id,
                SEARCH_CACHE_TTL_MINUTES,
            )? {
                self.budget
                    .record_cache_hit(GitHubRequestSource::DiscoveryRepository);
                let stats = DiscoveryStageStats::new(
                    request.stage,
                    request.lane_id,
                    request.per_page,
                    cached.len(),
                    cached.len(),
                );
                return Ok(RepositoryLaneOutput {
                    candidates: cached,
                    stats,
                });
            }
        }

        let output = self.search_repository_lane(&request).await?;
        save_cached_candidates(
            request.paths,
            GitHubRequestSource::DiscoveryRepository,
            request.lane_id,
            &output.candidates,
        )?;
        Ok(output)
    }

    async fn list_repository_recent_open_cached(
        &self,
        request: RepositoryRecentLaneRequest<'_>,
    ) -> Result<RepositoryLaneOutput> {
        if !request.refresh {
            if let Some(cached) = load_cached_candidates(
                request.paths,
                GitHubRequestSource::DiscoveryRepository,
                request.lane_id,
                SEARCH_CACHE_TTL_MINUTES,
            )? {
                self.budget
                    .record_cache_hit(GitHubRequestSource::DiscoveryRepository);
                let stats = DiscoveryStageStats::new(
                    "recent_open",
                    request.lane_id,
                    request.window,
                    cached.len(),
                    cached.len(),
                );
                return Ok(RepositoryLaneOutput {
                    candidates: cached,
                    stats,
                });
            }
        }

        let output = self.list_repository_recent_open(&request).await?;
        save_cached_candidates(
            request.paths,
            GitHubRequestSource::DiscoveryRepository,
            request.lane_id,
            &output.candidates,
        )?;
        Ok(output)
    }

    async fn list_repository_label_lane(
        &self,
        request: &RepositoryLabelLaneRequest<'_>,
    ) -> Result<RepositoryLaneOutput> {
        let per_page = request.per_page.to_string();
        let url = self.api_url(&format!(
            "/repos/{}/{}/issues",
            request.repository.owner, request.repository.repo
        ));
        self.record_request(GitHubRequestSource::DiscoveryRepository, request.lane_id)?;
        let response = self
            .authorized(self.http.get(url))
            .query(&[
                ("state", "open"),
                ("labels", request.label),
                ("sort", "updated"),
                ("direction", "desc"),
                ("per_page", per_page.as_str()),
            ])
            .send()
            .await?;
        let response = require_success(response).await?;
        let issues = response.json::<Vec<IssueResponse>>().await?;
        let returned = issues.len();
        let candidates = repository_issue_candidates(
            request.repository,
            issues,
            request.lane_id,
            request.profile,
        );
        let stats = DiscoveryStageStats::new(
            request.stage,
            request.lane_id,
            request.per_page,
            returned,
            candidates.len(),
        );
        Ok(RepositoryLaneOutput { candidates, stats })
    }

    async fn search_repository_lane(
        &self,
        request: &RepositorySearchLaneRequest<'_>,
    ) -> Result<RepositoryLaneOutput> {
        let per_page = request.per_page.to_string();
        let url = self.api_url("/search/issues");
        self.record_request(GitHubRequestSource::DiscoveryRepository, request.lane_id)?;
        let response = self
            .authorized(self.http.get(url))
            .query(&[
                ("q", request.query),
                ("sort", "updated"),
                ("order", "desc"),
                ("per_page", per_page.as_str()),
            ])
            .send()
            .await?;

        let response = require_success(response).await?;
        let payload = response.json::<SearchResponse>().await?;
        let returned = payload.items.len();
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();

        for item in payload.items {
            if !should_include_issue(
                item.pull_request.is_some(),
                item.locked,
                item.assignee.is_some(),
                item.assignees
                    .as_ref()
                    .map(|items| !items.is_empty())
                    .unwrap_or(false),
                &item.labels,
            ) {
                continue;
            }

            let Ok((owner, repo)) = parse_repo_api_url(&item.repository_url) else {
                continue;
            };
            if owner != request.repository.owner || repo != request.repository.repo {
                continue;
            }

            let key = format!("{}#{}", request.repository.full_name(), item.number);
            if !seen.insert(key) {
                continue;
            }

            let issue = GitHubIssue {
                id: item.id,
                number: item.number,
                title: item.title,
                body: item.body.unwrap_or_default(),
                labels: extract_label_names(&item.labels),
                url: item.html_url,
                repo_full_name: request.repository.full_name(),
                repo_name: request.repository.repo.clone(),
                repo_description: String::new(),
                repo_stars: 0,
                created_at: item.created_at,
                updated_at: item.updated_at,
            };
            candidates.push(DiscoveryCandidate::new(
                issue,
                request.lane_id.to_string(),
                RepoTrustTier::Global,
                request.profile,
            ));
        }

        let stats = DiscoveryStageStats::new(
            request.stage,
            request.lane_id,
            request.per_page,
            returned,
            candidates.len(),
        );
        Ok(RepositoryLaneOutput { candidates, stats })
    }

    async fn list_repository_recent_open(
        &self,
        request: &RepositoryRecentLaneRequest<'_>,
    ) -> Result<RepositoryLaneOutput> {
        let mut returned = 0usize;
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        let pages = request.window.div_ceil(REPO_SCOPED_RECENT_PER_PAGE);

        for page in 1..=pages {
            let per_page = REPO_SCOPED_RECENT_PER_PAGE.to_string();
            let page_text = page.to_string();
            let url = self.api_url(&format!(
                "/repos/{}/{}/issues",
                request.repository.owner, request.repository.repo
            ));
            self.record_request(
                GitHubRequestSource::DiscoveryRepository,
                format!("{}:page-{page}", request.lane_id),
            )?;
            let response = self
                .authorized(self.http.get(url))
                .query(&[
                    ("state", "open"),
                    ("sort", "updated"),
                    ("direction", "desc"),
                    ("per_page", per_page.as_str()),
                    ("page", page_text.as_str()),
                ])
                .send()
                .await?;
            let response = require_success(response).await?;
            let issues = response.json::<Vec<IssueResponse>>().await?;
            let issue_count = issues.len();
            returned += issue_count;

            for item in issues {
                if !should_include_issue(
                    item.pull_request.is_some(),
                    item.locked,
                    item.assignee.is_some(),
                    item.assignees
                        .as_ref()
                        .map(|items| !items.is_empty())
                        .unwrap_or(false),
                    &item.labels,
                ) {
                    continue;
                }
                let key = format!("{}#{}", request.repository.full_name(), item.number);
                if !seen.insert(key) {
                    continue;
                }
                let issue = GitHubIssue {
                    id: item.id,
                    number: item.number,
                    title: item.title,
                    body: item.body.unwrap_or_default(),
                    labels: extract_label_names(&item.labels),
                    url: item.html_url,
                    repo_full_name: request.repository.full_name(),
                    repo_name: request.repository.repo.clone(),
                    repo_description: String::new(),
                    repo_stars: 0,
                    created_at: item.created_at,
                    updated_at: item.updated_at,
                };
                candidates.push(DiscoveryCandidate::new(
                    issue,
                    request.lane_id.to_string(),
                    RepoTrustTier::Global,
                    request.profile,
                ));
            }

            if issue_count < REPO_SCOPED_RECENT_PER_PAGE {
                break;
            }
        }

        let stats = DiscoveryStageStats::new(
            "recent_open",
            request.lane_id,
            request.window,
            returned,
            candidates.len(),
        );
        Ok(RepositoryLaneOutput { candidates, stats })
    }

    async fn fetch_lane_candidates(
        &self,
        lane: SearchLaneRequest,
        profile: &ProfileConfig,
    ) -> Result<Vec<DiscoveryCandidate>> {
        match lane {
            SearchLaneRequest::TrustedRepo {
                id,
                repository,
                trust_tier,
                candidate_limit,
                per_page,
            } => {
                self.list_trusted_repository_lane(
                    &repository,
                    &id,
                    trust_tier,
                    candidate_limit,
                    per_page,
                    profile,
                )
                .await
            }
            SearchLaneRequest::Global {
                id,
                query,
                per_page,
            } => {
                self.search_lane(
                    &query,
                    per_page,
                    &id,
                    GitHubRequestSource::DiscoveryGlobal,
                    RepoTrustTier::Global,
                    profile,
                )
                .await
            }
        }
    }

    async fn list_trusted_repository_fallback_lane(
        &self,
        repository: &TrustedRepository,
        label: &str,
        lane_id: &str,
        trust_tier: RepoTrustTier,
        candidate_limit: usize,
        profile: &ProfileConfig,
    ) -> Result<Vec<DiscoveryCandidate>> {
        let per_page = FALLBACK_TRUSTED_REPO_PER_PAGE.to_string();
        let url = self.api_url(&format!(
            "/repos/{}/{}/issues",
            repository.owner, repository.name
        ));
        self.record_request(GitHubRequestSource::DiscoveryFallbackTrusted, lane_id)?;
        let response = self
            .authorized(self.http.get(url))
            .query(&[
                ("state", "open"),
                ("labels", label),
                ("sort", "updated"),
                ("direction", "desc"),
                ("per_page", per_page.as_str()),
            ])
            .send()
            .await?;
        let response = require_success(response).await?;
        let issues = response.json::<Vec<IssueResponse>>().await?;

        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        for item in issues {
            if candidates.len() >= candidate_limit {
                break;
            }
            if !has_beginner_label(&item.labels) {
                continue;
            }
            if !should_include_issue(
                item.pull_request.is_some(),
                item.locked,
                item.assignee.is_some(),
                item.assignees
                    .as_ref()
                    .map(|items| !items.is_empty())
                    .unwrap_or(false),
                &item.labels,
            ) {
                continue;
            }

            let key = format!("{}#{}", repository.full_name(), item.number);
            if !seen.insert(key) {
                continue;
            }

            let issue = GitHubIssue {
                id: item.id,
                number: item.number,
                title: item.title,
                body: item.body.unwrap_or_default(),
                labels: extract_label_names(&item.labels),
                url: item.html_url,
                repo_full_name: repository.full_name(),
                repo_name: repository.name.clone(),
                repo_description: String::new(),
                repo_stars: 0,
                created_at: item.created_at,
                updated_at: item.updated_at,
            };
            candidates.push(DiscoveryCandidate::new(
                issue,
                lane_id.to_string(),
                trust_tier,
                profile,
            ));
        }

        Ok(candidates)
    }

    async fn list_trusted_repository_lane(
        &self,
        repository: &TrustedRepository,
        lane_id: &str,
        trust_tier: RepoTrustTier,
        candidate_limit: usize,
        per_page: usize,
        profile: &ProfileConfig,
    ) -> Result<Vec<DiscoveryCandidate>> {
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();

        for label in BEGINNER_LABELS {
            if candidates.len() >= candidate_limit {
                break;
            }

            let per_page = per_page.to_string();
            let url = self.api_url(&format!(
                "/repos/{}/{}/issues",
                repository.owner, repository.name
            ));
            self.record_request(
                trusted_repo_request_source(trust_tier),
                format!("{lane_id}:{label}"),
            )?;
            let response = self
                .authorized(self.http.get(url))
                .query(&[
                    ("state", "open"),
                    ("labels", label),
                    ("sort", "updated"),
                    ("direction", "desc"),
                    ("per_page", per_page.as_str()),
                ])
                .send()
                .await?;
            let response = require_success(response).await?;
            let issues = response.json::<Vec<IssueResponse>>().await?;

            for item in issues {
                if candidates.len() >= candidate_limit {
                    break;
                }
                if !should_include_issue(
                    item.pull_request.is_some(),
                    item.locked,
                    item.assignee.is_some(),
                    item.assignees
                        .as_ref()
                        .map(|items| !items.is_empty())
                        .unwrap_or(false),
                    &item.labels,
                ) {
                    continue;
                }

                let key = format!("{}#{}", repository.full_name(), item.number);
                if !seen.insert(key) {
                    continue;
                }

                let issue = GitHubIssue {
                    id: item.id,
                    number: item.number,
                    title: item.title,
                    body: item.body.unwrap_or_default(),
                    labels: extract_label_names(&item.labels),
                    url: item.html_url,
                    repo_full_name: repository.full_name(),
                    repo_name: repository.name.clone(),
                    repo_description: String::new(),
                    repo_stars: 0,
                    created_at: item.created_at,
                    updated_at: item.updated_at,
                };
                candidates.push(DiscoveryCandidate::new(
                    issue,
                    lane_id.to_string(),
                    trust_tier,
                    profile,
                ));
            }
        }

        Ok(candidates)
    }

    async fn search_lane(
        &self,
        query: &str,
        per_page: usize,
        lane_id: &str,
        request_source: GitHubRequestSource,
        trust_tier: RepoTrustTier,
        profile: &ProfileConfig,
    ) -> Result<Vec<DiscoveryCandidate>> {
        let per_page = per_page.to_string();
        let url = self.api_url("/search/issues");
        self.record_request(request_source, lane_id)?;
        let response = self
            .authorized(self.http.get(url))
            .query(&[
                ("q", query),
                ("sort", "updated"),
                ("order", "desc"),
                ("per_page", per_page.as_str()),
            ])
            .send()
            .await?;

        let response = require_success(response).await?;
        let payload = response.json::<SearchResponse>().await?;
        let mut candidates = Vec::new();

        for item in payload.items {
            if !should_include_issue(
                item.pull_request.is_some(),
                item.locked,
                item.assignee.is_some(),
                item.assignees
                    .as_ref()
                    .map(|items| !items.is_empty())
                    .unwrap_or(false),
                &item.labels,
            ) {
                continue;
            }

            let (owner, repo) = parse_repo_api_url(&item.repository_url)?;
            let repo_full_name = format!("{owner}/{repo}");

            let issue = GitHubIssue {
                id: item.id,
                number: item.number,
                title: item.title,
                body: item.body.unwrap_or_default(),
                labels: extract_label_names(&item.labels),
                url: item.html_url,
                repo_full_name,
                repo_name: repo,
                repo_description: String::new(),
                repo_stars: 0,
                created_at: item.created_at,
                updated_at: item.updated_at,
            };
            candidates.push(DiscoveryCandidate::new(
                issue,
                lane_id.to_string(),
                trust_tier,
                profile,
            ));
        }

        Ok(candidates)
    }

    pub async fn fetch_issue(&self, issue_ref: &IssueRef) -> Result<GitHubIssue> {
        let mut repo_cache = HashMap::new();
        let metadata = self
            .repo_metadata(&issue_ref.owner, &issue_ref.repo, &mut repo_cache)
            .await
            .unwrap_or_else(|_| RepoMetadata {
                full_name: issue_ref.repo_full_name(),
                name: issue_ref.repo.clone(),
                description: String::new(),
                stars: 0,
            });

        let url = format!(
            "{}/repos/{}/{}/issues/{}",
            self.api_base_url.trim_end_matches('/'),
            issue_ref.owner,
            issue_ref.repo,
            issue_ref.number
        );
        self.record_request(GitHubRequestSource::DirectIssue, issue_ref.repo_full_name())?;
        let response = self.authorized(self.http.get(url)).send().await?;
        let response = require_success(response).await?;
        let issue = response.json::<IssueResponse>().await?;

        if issue.pull_request.is_some() {
            anyhow::bail!(
                "{} is a pull request, not an issue",
                issue_ref.repo_full_name()
            );
        }

        if !should_include_issue(
            false,
            issue.locked,
            issue.assignee.is_some(),
            issue
                .assignees
                .as_ref()
                .map(|items| !items.is_empty())
                .unwrap_or(false),
            &issue.labels,
        ) {
            anyhow::bail!("issue is locked, assigned, or carries a blocking label");
        }

        Ok(GitHubIssue {
            id: issue.id,
            number: issue.number,
            title: issue.title,
            body: issue.body.unwrap_or_default(),
            labels: extract_label_names(&issue.labels),
            url: issue.html_url,
            repo_full_name: metadata.full_name,
            repo_name: metadata.name,
            repo_description: metadata.description,
            repo_stars: metadata.stars,
            created_at: issue.created_at,
            updated_at: issue.updated_at,
        })
    }

    pub async fn validate_token(&self) -> Result<String> {
        if self.token.trim().is_empty() {
            anyhow::bail!("GitHub token is missing");
        }

        self.record_request(GitHubRequestSource::ValidateToken, "/user")?;
        let response = self
            .authorized(self.http.get(self.api_url("/user")))
            .send()
            .await?;
        let response = require_success(response).await?;
        let value = response.json::<serde_json::Value>().await?;
        Ok(value
            .get("login")
            .and_then(|login| login.as_str())
            .unwrap_or("unknown")
            .to_string())
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.token.trim().is_empty() {
            request
        } else {
            request.bearer_auth(self.token.trim())
        }
    }

    async fn repo_metadata(
        &self,
        owner: &str,
        repo: &str,
        cache: &mut HashMap<String, RepoMetadata>,
    ) -> Result<RepoMetadata> {
        let key = format!("{owner}/{repo}");
        if let Some(metadata) = cache.get(&key) {
            return Ok(metadata.clone());
        }

        let url = self.api_url(&format!("/repos/{owner}/{repo}"));
        self.record_request(GitHubRequestSource::DirectIssue, format!("{owner}/{repo}"))?;
        let response = self.authorized(self.http.get(url)).send().await?;
        let response = require_success(response).await?;
        let repo = response.json::<RepoResponse>().await?;
        let metadata = RepoMetadata {
            full_name: repo.full_name,
            name: repo.name,
            description: repo.description.unwrap_or_default(),
            stars: repo.stargazers_count.unwrap_or_default(),
        };
        cache.insert(key, metadata.clone());
        Ok(metadata)
    }

    fn api_url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.api_base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    fn fallback_global_search_spacing(&self) -> std::time::Duration {
        if self.api_base_url.trim_end_matches('/') == "https://api.github.com" {
            std::time::Duration::from_millis(2_500)
        } else {
            std::time::Duration::ZERO
        }
    }

    fn record_request(&self, source: GitHubRequestSource, detail: impl AsRef<str>) -> Result<()> {
        self.budget
            .record_network_request(source, detail)
            .map_err(Into::into)
    }
}

pub fn build_search_query(label: &str) -> String {
    format!("label:\"{label}\" archived:false is:issue is:open no:assignee")
}

fn primary_trusted_repository_lanes(
    profile: &ProfileConfig,
) -> Result<Vec<(TrustedRepository, RepoTrustTier)>> {
    let mut seen = HashSet::new();
    let mut lanes = Vec::new();

    for repository in overlay_repositories()?
        .into_iter()
        .take(TRUSTED_OVERLAY_REPO_REQUEST_LIMIT)
    {
        push_primary_trusted_repository_lane(
            repository,
            RepoTrustTier::OverlayTrusted,
            &mut seen,
            &mut lanes,
        );
    }

    for repository in profile_trusted_repositories(profile, PROFILE_TRUSTED_REPO_REQUEST_LIMIT)? {
        push_primary_trusted_repository_lane(
            repository,
            RepoTrustTier::ProfileTrusted,
            &mut seen,
            &mut lanes,
        );
    }

    for repository in gfi_repositories(profile, GFI_REPO_REQUEST_LIMIT)? {
        push_primary_trusted_repository_lane(
            repository,
            RepoTrustTier::GfiTrusted,
            &mut seen,
            &mut lanes,
        );
    }

    Ok(lanes)
}

fn push_primary_trusted_repository_lane(
    repository: TrustedRepository,
    trust_tier: RepoTrustTier,
    seen: &mut HashSet<String>,
    lanes: &mut Vec<(TrustedRepository, RepoTrustTier)>,
) {
    if seen.insert(repository.full_name()) {
        lanes.push((repository, trust_tier));
    }
}

#[derive(Debug)]
enum SearchLaneRequest {
    TrustedRepo {
        id: String,
        repository: TrustedRepository,
        trust_tier: RepoTrustTier,
        candidate_limit: usize,
        per_page: usize,
    },
    Global {
        id: String,
        query: String,
        per_page: usize,
    },
}

impl SearchLaneRequest {
    fn trusted(repository: TrustedRepository, trust_tier: RepoTrustTier) -> Self {
        let repo_full_name = repository.full_name();
        let candidate_limit = match trust_tier {
            RepoTrustTier::OverlayTrusted => TRUSTED_OVERLAY_REPO_CANDIDATE_LIMIT,
            RepoTrustTier::ProfileTrusted => PROFILE_TRUSTED_REPO_CANDIDATE_LIMIT,
            RepoTrustTier::GfiTrusted => GFI_REPO_CANDIDATE_LIMIT,
            RepoTrustTier::Global => 0,
        };
        let per_page = match trust_tier {
            RepoTrustTier::ProfileTrusted => PROFILE_TRUSTED_LABEL_PER_PAGE,
            RepoTrustTier::OverlayTrusted | RepoTrustTier::GfiTrusted => TRUSTED_LABEL_PER_PAGE,
            RepoTrustTier::Global => 0,
        };
        Self::TrustedRepo {
            id: format!("{trust_tier}:{repo_full_name}"),
            repository,
            trust_tier,
            candidate_limit,
            per_page,
        }
    }

    fn global(lane: GlobalSearchLane) -> Self {
        Self::Global {
            id: lane.id,
            query: lane.query,
            per_page: GLOBAL_SEARCH_PER_PAGE,
        }
    }

    fn cache_key(&self) -> String {
        match self {
            Self::TrustedRepo { id, .. } | Self::Global { id, .. } => id.clone(),
        }
    }

    fn request_source(&self) -> GitHubRequestSource {
        match self {
            Self::TrustedRepo { trust_tier, .. } => trusted_repo_request_source(*trust_tier),
            Self::Global { .. } => GitHubRequestSource::DiscoveryGlobal,
        }
    }
}

#[derive(Debug)]
struct GlobalSearchLane {
    id: String,
    query: String,
}

struct SearchLaneCacheRequest<'a> {
    paths: &'a IssueFinderPaths,
    refresh: bool,
    query: &'a str,
    per_page: usize,
    lane_id: &'a str,
    request_source: GitHubRequestSource,
    profile: &'a ProfileConfig,
}

struct RepositoryLabelLaneRequest<'a> {
    paths: &'a IssueFinderPaths,
    refresh: bool,
    repository: &'a RepositoryScope,
    label: &'static str,
    stage: &'static str,
    lane_id: &'a str,
    per_page: usize,
    profile: &'a ProfileConfig,
}

struct RepositorySearchLaneRequest<'a> {
    paths: &'a IssueFinderPaths,
    refresh: bool,
    repository: &'a RepositoryScope,
    query: &'a str,
    stage: &'static str,
    lane_id: &'a str,
    per_page: usize,
    profile: &'a ProfileConfig,
}

struct RepositoryRecentLaneRequest<'a> {
    paths: &'a IssueFinderPaths,
    refresh: bool,
    repository: &'a RepositoryScope,
    window: usize,
    lane_id: &'a str,
    profile: &'a ProfileConfig,
}

struct RepositoryLaneOutput {
    candidates: Vec<DiscoveryCandidate>,
    stats: DiscoveryStageStats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FallbackTrustedRequest {
    repository: TrustedRepository,
    label: &'static str,
    trust_tier: RepoTrustTier,
}

fn global_search_lanes(profile: &ProfileConfig) -> Vec<GlobalSearchLane> {
    let mut lanes = vec![
        GlobalSearchLane {
            id: "global:good-first-issue".to_string(),
            query: build_search_query("good first issue"),
        },
        GlobalSearchLane {
            id: "global:good-first-issue-hyphen".to_string(),
            query: build_search_query("good-first-issue"),
        },
    ];

    let terms = crate::scoring::profile_terms(profile)
        .into_iter()
        .filter(|term| term.len() >= 3)
        .take(6)
        .collect::<Vec<_>>();
    for term in terms {
        let term = search_term(&term);
        lanes.push(GlobalSearchLane {
            id: format!("global:beginner:{term}"),
            query: format!("label:beginner archived:false is:issue is:open no:assignee {term}"),
        });
        lanes.push(GlobalSearchLane {
            id: format!("global:easy:{term}"),
            query: format!(
                "label:easy archived:false is:issue is:open no:assignee {term} expected actual"
            ),
        });
        lanes.push(GlobalSearchLane {
            id: format!("global:help-wanted:{term}"),
            query: format!(
                "label:\"help wanted\" archived:false is:issue is:open no:assignee {term} repro"
            ),
        });
    }

    lanes
}

fn fallback_global_search_lanes(profile: &ProfileConfig) -> Vec<GlobalSearchLane> {
    let labels = [
        ("good-first-issue", "label:\"good first issue\""),
        ("good-first-issue-hyphen", "label:\"good-first-issue\""),
        ("help-wanted", "label:\"help wanted\""),
    ];
    let pairs = fallback_query_pairs(profile);
    let recent_filter = fallback_recent_filter();
    let mut lanes = Vec::new();

    for (language, term) in pairs {
        for (label_id, label_query) in labels {
            let term = search_term(&term);
            let query = format!(
                "{label_query} archived:false is:issue is:open no:assignee {recent_filter} language:{language} {term}"
            );
            lanes.push(GlobalSearchLane {
                id: format!("fallback_global:{label_id}:{language}:{term}"),
                query,
            });
        }
    }

    lanes
}

fn fallback_query_pairs(profile: &ProfileConfig) -> Vec<(String, String)> {
    let terms = crate::scoring::profile_terms(profile);
    let has = |needle: &str| terms.iter().any(|term| term == needle);
    let mut pairs = Vec::new();

    if has("kubernetes") || has("docker") || has("ci") || has("infrastructure") {
        push_query_pair(&mut pairs, "Go", "kubernetes");
        push_query_pair(&mut pairs, "Go", "docker");
        push_query_pair(&mut pairs, "Go", "ci");
        push_query_pair(&mut pairs, "TypeScript", "kubernetes");
    } else if has("llm") || has("agent") || has("ai") {
        push_query_pair(&mut pairs, "Python", "mcp");
        push_query_pair(&mut pairs, "Python", "llm");
        push_query_pair(&mut pairs, "TypeScript", "llm");
        push_query_pair(&mut pairs, "Python", "agent");
        push_query_pair(&mut pairs, "TypeScript", "agent");
        push_query_pair(&mut pairs, "Python", "ai");
    } else if has("python") && (has("data") || has("pandas") || has("testing")) {
        push_query_pair(&mut pairs, "Python", "python");
        push_query_pair(&mut pairs, "Python", "pandas");
        push_query_pair(&mut pairs, "Python", "testing");
        push_query_pair(&mut pairs, "Python", "cli");
    } else if (has("rust") && has("go")) || has("backend") || has("compiler") || has("cargo") {
        push_query_pair(&mut pairs, "Rust", "rust");
        push_query_pair(&mut pairs, "Go", "go");
        push_query_pair(&mut pairs, "Rust", "backend");
        push_query_pair(&mut pairs, "Go", "backend");
        push_query_pair(&mut pairs, "Rust", "compiler");
        push_query_pair(&mut pairs, "Rust", "cargo");
    } else if has("frontend") || has("react") || has("ui") || has("browser") {
        push_query_pair(&mut pairs, "TypeScript", "react");
        push_query_pair(&mut pairs, "JavaScript", "react");
        push_query_pair(&mut pairs, "TypeScript", "frontend");
        push_query_pair(&mut pairs, "JavaScript", "browser");
    } else {
        push_query_pair(&mut pairs, "Rust", "rust");
        push_query_pair(&mut pairs, "TypeScript", "typescript");
        push_query_pair(&mut pairs, "Rust", "cli");
        push_query_pair(&mut pairs, "TypeScript", "cli");
    }

    pairs
}

fn push_query_pair(pairs: &mut Vec<(String, String)>, language: &str, term: &str) {
    let pair = (language.to_string(), term.to_string());
    if !pairs.contains(&pair) {
        pairs.push(pair);
    }
}

fn fallback_recent_filter() -> String {
    let floor = (Utc::now() - Duration::days(365))
        .date_naive()
        .format("%Y-%m-%d");
    format!("updated:>={floor}")
}

fn search_term(term: &str) -> String {
    if term.contains(' ') {
        format!("\"{term}\"")
    } else {
        term.to_string()
    }
}

fn repo_scoped_search_query(repository: &RepositoryScope, term: &str) -> String {
    format!(
        "repo:{} is:issue is:open no:assignee archived:false {}",
        repository.full_name(),
        search_term(term)
    )
}

fn repo_scoped_label_lane_id(stage: &str, label: &str) -> String {
    format!("repo_scoped:{stage}:{}", lane_fragment(label))
}

fn lane_fragment(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else if character.is_ascii_whitespace() {
                '_'
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn repository_issue_candidates(
    repository: &RepositoryScope,
    issues: Vec<IssueResponse>,
    lane_id: &str,
    profile: &ProfileConfig,
) -> Vec<DiscoveryCandidate> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for item in issues {
        if !should_include_issue(
            item.pull_request.is_some(),
            item.locked,
            item.assignee.is_some(),
            item.assignees
                .as_ref()
                .map(|items| !items.is_empty())
                .unwrap_or(false),
            &item.labels,
        ) {
            continue;
        }

        let key = format!("{}#{}", repository.full_name(), item.number);
        if !seen.insert(key) {
            continue;
        }

        let issue = GitHubIssue {
            id: item.id,
            number: item.number,
            title: item.title,
            body: item.body.unwrap_or_default(),
            labels: extract_label_names(&item.labels),
            url: item.html_url,
            repo_full_name: repository.full_name(),
            repo_name: repository.repo.clone(),
            repo_description: String::new(),
            repo_stars: 0,
            created_at: item.created_at,
            updated_at: item.updated_at,
        };
        candidates.push(DiscoveryCandidate::new(
            issue,
            lane_id.to_string(),
            RepoTrustTier::Global,
            profile,
        ));
    }
    candidates
}

fn dedupe_repositories(repositories: Vec<TrustedRepository>) -> Vec<TrustedRepository> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for repository in repositories {
        if seen.insert(repository.full_name()) {
            deduped.push(repository);
        }
    }
    deduped
}

fn fallback_trusted_repository_requests(
    profile_repositories: &[TrustedRepository],
    repositories: &[TrustedRepository],
) -> Vec<FallbackTrustedRequest> {
    let profile_repository_keys = profile_repositories
        .iter()
        .map(TrustedRepository::full_name)
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut requests = Vec::new();

    for repository in profile_repositories.iter().take(6) {
        for label in FALLBACK_TRUSTED_LABELS {
            push_fallback_trusted_request(
                repository,
                label,
                RepoTrustTier::ProfileTrusted,
                &mut seen,
                &mut requests,
            );
            if requests.len() == FALLBACK_TRUSTED_REPO_REQUEST_LIMIT {
                return requests;
            }
        }
    }

    for repository in repositories {
        let trust_tier = if profile_repository_keys.contains(&repository.full_name()) {
            RepoTrustTier::ProfileTrusted
        } else {
            RepoTrustTier::GfiTrusted
        };
        push_fallback_trusted_request(
            repository,
            "good first issue",
            trust_tier,
            &mut seen,
            &mut requests,
        );
        if requests.len() == FALLBACK_TRUSTED_REPO_REQUEST_LIMIT {
            break;
        }
    }

    requests
}

fn push_fallback_trusted_request(
    repository: &TrustedRepository,
    label: &'static str,
    trust_tier: RepoTrustTier,
    seen: &mut HashSet<String>,
    requests: &mut Vec<FallbackTrustedRequest>,
) {
    let key = format!("{}:{label}", repository.full_name());
    if seen.insert(key) {
        requests.push(FallbackTrustedRequest {
            repository: repository.clone(),
            label,
            trust_tier,
        });
    }
}

fn cap_candidates_per_repo(
    candidates: Vec<DiscoveryCandidate>,
    per_repo_limit: usize,
) -> Vec<DiscoveryCandidate> {
    let mut counts = HashMap::<String, usize>::new();
    let mut capped = Vec::new();
    for candidate in candidates {
        let count = *counts.get(&candidate.issue.repo_full_name).unwrap_or(&0);
        if count >= per_repo_limit {
            continue;
        }
        counts.insert(candidate.issue.repo_full_name.clone(), count + 1);
        capped.push(candidate);
    }
    capped
}

fn collect_lane_candidates(
    results: Vec<Result<Vec<DiscoveryCandidate>>>,
) -> Result<Vec<DiscoveryCandidate>> {
    let mut candidates = Vec::new();
    let mut rate_limited_count = 0;

    for result in results {
        match result {
            Ok(mut lane_candidates) => candidates.append(&mut lane_candidates),
            Err(error) if is_rate_limit_error(&error) => rate_limited_count += 1,
            Err(_) => {}
        }
    }

    if candidates.is_empty() && rate_limited_count >= DISCOVERY_SEARCH_CONCURRENCY_LIMIT {
        return Err(IssueFinderError::GitHubRateLimited.into());
    }

    Ok(candidates)
}

fn collect_fallback_lane_candidates(
    results: Vec<Result<Vec<DiscoveryCandidate>>>,
) -> Vec<DiscoveryCandidate> {
    let mut candidates = Vec::new();
    for mut lane_candidates in results.into_iter().flatten() {
        candidates.append(&mut lane_candidates);
    }
    candidates
}

fn is_rate_limit_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<IssueFinderError>()
        .is_some_and(|error| matches!(error, IssueFinderError::GitHubRateLimited))
}

fn load_cached_candidates(
    paths: &IssueFinderPaths,
    source: GitHubRequestSource,
    key: &str,
    ttl_minutes: i64,
) -> Result<Option<Vec<DiscoveryCandidate>>> {
    let cache_path = paths.discovery_cache_path(source.as_str(), key);
    if !cache_path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&cache_path)
        .with_context(|| format!("unable to read {}", cache_path.display()))?;
    let Ok(payload) = serde_json::from_str::<DiscoveryCachePayload>(&raw) else {
        return Ok(None);
    };
    if Utc::now() - payload.fetched_at > Duration::minutes(ttl_minutes) {
        return Ok(None);
    }

    Ok(Some(payload.candidates))
}

fn save_cached_candidates(
    paths: &IssueFinderPaths,
    source: GitHubRequestSource,
    key: &str,
    candidates: &[DiscoveryCandidate],
) -> Result<()> {
    let payload = DiscoveryCachePayload {
        fetched_at: Utc::now(),
        candidates: candidates.to_vec(),
    };
    atomic_write(
        paths.discovery_cache_path(source.as_str(), key).as_path(),
        serde_json::to_vec_pretty(&payload)?,
    )?;
    Ok(())
}

fn trusted_repo_request_source(trust_tier: RepoTrustTier) -> GitHubRequestSource {
    match trust_tier {
        RepoTrustTier::OverlayTrusted => GitHubRequestSource::DiscoveryOverlay,
        RepoTrustTier::ProfileTrusted => GitHubRequestSource::DiscoveryProfileTrusted,
        RepoTrustTier::GfiTrusted => GitHubRequestSource::DiscoveryGfiTrusted,
        RepoTrustTier::Global => GitHubRequestSource::DiscoveryGlobal,
    }
}

async fn require_success(response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let body = response.text().await.unwrap_or_default();
    if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
        return Err(IssueFinderError::GitHubRateLimited.into());
    }

    Err(IssueFinderError::GitHubResponse(format!("{status}: {body}")).into())
}

fn parse_repo_api_url(value: &str) -> Result<(String, String)> {
    let url = Url::parse(value)?;
    let segments = url.path_segments().context("repository URL missing path")?;
    let parts = segments.collect::<Vec<_>>();
    let repos_index = parts
        .iter()
        .position(|part| *part == "repos")
        .context("repository URL missing repos segment")?;
    let owner = parts
        .get(repos_index + 1)
        .context("repository URL missing owner")?;
    let repo = parts
        .get(repos_index + 2)
        .context("repository URL missing repo")?;
    Ok((owner.to_string(), repo.to_string()))
}

fn should_include_issue(
    is_pull_request: bool,
    locked: bool,
    has_assignee: bool,
    has_assignees: bool,
    labels: &[GitHubLabel],
) -> bool {
    if is_pull_request || locked || has_assignee || has_assignees {
        return false;
    }

    let label_names = extract_label_names(labels);
    !has_action_blocking_label(&label_names)
}

fn has_beginner_label(labels: &[GitHubLabel]) -> bool {
    let label_names = extract_label_names(labels);
    label_names.iter().any(|label| {
        let normalized = normalize_label(label);
        BEGINNER_LABELS
            .iter()
            .any(|beginner| normalized == normalize_label(beginner))
    })
}

fn extract_label_names(labels: &[GitHubLabel]) -> Vec<String> {
    labels
        .iter()
        .filter_map(|label| match label {
            GitHubLabel::Name(name) => Some(name.clone()),
            GitHubLabel::Object { name } => name.clone(),
        })
        .filter(|name| !name.trim().is_empty())
        .collect()
}

fn has_action_blocking_label(labels: &[String]) -> bool {
    const BLOCKED: [&str; 8] = [
        "blocked",
        "duplicate",
        "invalid",
        "needs info",
        "needs information",
        "question",
        "discussion",
        "wontfix",
    ];

    labels.iter().any(|label| {
        let normalized = normalize_label(label);
        BLOCKED.iter().any(|blocked| {
            normalized == *blocked
                || normalized.ends_with(&format!(" {blocked}"))
                || normalized.contains(&format!("{blocked}:"))
        })
    })
}

fn normalize_label(label: &str) -> String {
    label
        .to_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use tempfile::tempdir;

    use super::{
        build_search_query, collect_lane_candidates, fallback_global_search_lanes,
        fallback_trusted_repository_requests, has_beginner_label, load_cached_candidates,
        primary_trusted_repository_lanes, DiscoveryCachePayload, FallbackTrustedRequest,
        GitHubIssue, GitHubLabel, IssueRef, FALLBACK_DISCOVERY_CACHE_TTL_MINUTES,
        SEARCH_CACHE_TTL_MINUTES,
    };
    use crate::config::ProfileConfig;
    use crate::discovery::{DiscoveryCandidate, RepoTrustTier, TrustedRepository};
    use crate::errors::IssueFinderError;
    use crate::github_budget::GitHubRequestSource;
    use crate::paths::{atomic_write, IssueFinderPaths};

    #[test]
    fn parses_short_issue_reference() {
        let parsed = IssueRef::parse("owner/repo#123").unwrap();
        assert_eq!(parsed.owner, "owner");
        assert_eq!(parsed.repo, "repo");
        assert_eq!(parsed.number, 123);
    }

    #[test]
    fn parses_github_issue_url() {
        let parsed = IssueRef::parse_url("https://github.com/owner/repo/issues/42").unwrap();
        assert_eq!(parsed.repo_full_name(), "owner/repo");
        assert_eq!(parsed.number, 42);
    }

    #[test]
    fn builds_good_first_issue_search_query() {
        assert_eq!(
            build_search_query("good first issue"),
            "label:\"good first issue\" archived:false is:issue is:open no:assignee"
        );
    }

    #[test]
    fn recognizes_beginner_labels_for_trusted_fallback() {
        let labels = vec![GitHubLabel::Object {
            name: Some("beginner-friendly".to_string()),
        }];

        assert!(has_beginner_label(&labels));
    }

    #[test]
    fn ignores_non_beginner_labels_for_trusted_fallback() {
        let labels = vec![GitHubLabel::Name("bug".to_string())];

        assert!(!has_beginner_label(&labels));
    }

    #[test]
    fn primary_lanes_prioritize_profile_trusted_before_auto_gfi() {
        let profile = ProfileConfig {
            tech_stack: vec!["Rust".to_string(), "Go".to_string()],
            keywords: vec!["backend".to_string(), "cargo".to_string()],
        };

        let lanes = primary_trusted_repository_lanes(&profile).unwrap();
        let first_profile_index = lanes
            .iter()
            .position(|(_, trust_tier)| *trust_tier == RepoTrustTier::ProfileTrusted)
            .unwrap();
        let first_gfi_index = lanes
            .iter()
            .position(|(_, trust_tier)| *trust_tier == RepoTrustTier::GfiTrusted)
            .unwrap();
        let profile_lanes = lanes
            .iter()
            .filter(|(_, trust_tier)| *trust_tier == RepoTrustTier::ProfileTrusted)
            .map(|(repository, _)| repository.full_name())
            .collect::<Vec<_>>();

        assert!(first_profile_index < first_gfi_index);
        assert!(profile_lanes.contains(&"rust-lang/rust-clippy".to_string()));
        assert!(profile_lanes.contains(&"tokio-rs/tokio".to_string()));
    }

    #[test]
    fn fallback_trusted_requests_prioritize_manual_bucket_with_request_cap() {
        let profile_repositories = (0..8)
            .map(|index| repository("manual", &format!("repo-{index}")))
            .collect::<Vec<_>>();
        let mut repositories = profile_repositories.clone();
        repositories.extend((0..8).map(|index| repository("auto", &format!("repo-{index}"))));

        let requests = fallback_trusted_repository_requests(&profile_repositories, &repositories);

        assert_eq!(requests.len(), 20);
        assert_eq!(
            requests[0],
            request(
                "manual",
                "repo-0",
                "good first issue",
                RepoTrustTier::ProfileTrusted
            )
        );
        assert_eq!(
            requests[1],
            request(
                "manual",
                "repo-0",
                "good-first-issue",
                RepoTrustTier::ProfileTrusted
            )
        );
        assert_eq!(
            requests[2],
            request(
                "manual",
                "repo-0",
                "help wanted",
                RepoTrustTier::ProfileTrusted
            )
        );
        assert!(requests.contains(&request(
            "manual",
            "repo-5",
            "help wanted",
            RepoTrustTier::ProfileTrusted
        )));
        assert!(requests.contains(&request(
            "manual",
            "repo-6",
            "good first issue",
            RepoTrustTier::ProfileTrusted
        )));
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.repository.full_name() == "auto/repo-0")
                .count(),
            0
        );
    }

    #[test]
    fn fallback_global_lanes_pair_language_with_strong_profile_terms() {
        let rust_backend = ProfileConfig {
            tech_stack: vec!["Rust".to_string(), "Go".to_string()],
            keywords: vec!["backend".to_string(), "cargo".to_string()],
        };
        let lanes = fallback_global_search_lanes(&rust_backend);

        assert!(lanes[0].query.contains("language:Rust rust"));
        assert!(lanes[3].query.contains("language:Go go"));
        assert!(!lanes[0].query.contains("language:Go rust"));

        let devops = ProfileConfig {
            tech_stack: vec!["Go".to_string(), "TypeScript".to_string()],
            keywords: vec!["kubernetes".to_string(), "docker".to_string()],
        };
        let lanes = fallback_global_search_lanes(&devops);

        assert!(lanes[0].query.contains("language:Go kubernetes"));
        assert!(lanes[3].query.contains("language:Go docker"));
    }

    #[test]
    fn collect_lane_candidates_keeps_successes_when_other_lanes_are_rate_limited() {
        let candidate = discovery_candidate();
        let candidates = collect_lane_candidates(vec![
            Ok(vec![candidate.clone()]),
            Err(rate_limit_error()),
            Err(rate_limit_error()),
        ])
        .unwrap();

        assert_eq!(candidates, vec![candidate]);
    }

    #[test]
    fn collect_lane_candidates_fails_when_only_rate_limited_lanes_remain() {
        let error = collect_lane_candidates(vec![Err(rate_limit_error()), Err(rate_limit_error())])
            .unwrap_err();

        assert!(matches!(
            error.downcast_ref::<IssueFinderError>(),
            Some(IssueFinderError::GitHubRateLimited)
        ));
    }

    #[test]
    fn fallback_discovery_cache_has_independent_ttl() {
        let dir = tempdir().unwrap();
        let paths = IssueFinderPaths {
            home: dir.path().to_path_buf(),
            config: dir.path().join("config.toml"),
            cache_dir: dir.path().join("cache"),
            workspaces_dir: dir.path().join("workspaces"),
            inbox_dir: dir.path().join("inbox"),
            reports_dir: dir.path().join("reports"),
        };
        let payload = DiscoveryCachePayload {
            fetched_at: Utc::now() - Duration::minutes(240),
            candidates: vec![discovery_candidate()],
        };
        let key = "same-lane";

        atomic_write(
            &paths.discovery_cache_path(GitHubRequestSource::DiscoveryGlobal.as_str(), key),
            serde_json::to_vec_pretty(&payload).unwrap(),
        )
        .unwrap();
        atomic_write(
            &paths.discovery_cache_path(GitHubRequestSource::DiscoveryFallbackGlobal.as_str(), key),
            serde_json::to_vec_pretty(&payload).unwrap(),
        )
        .unwrap();

        let primary = load_cached_candidates(
            &paths,
            GitHubRequestSource::DiscoveryGlobal,
            key,
            SEARCH_CACHE_TTL_MINUTES,
        )
        .unwrap();
        let fallback = load_cached_candidates(
            &paths,
            GitHubRequestSource::DiscoveryFallbackGlobal,
            key,
            FALLBACK_DISCOVERY_CACHE_TTL_MINUTES,
        )
        .unwrap();

        assert!(primary.is_none());
        assert_eq!(fallback.unwrap().len(), 1);
    }

    fn repository(owner: &str, name: &str) -> TrustedRepository {
        TrustedRepository {
            owner: owner.to_string(),
            name: name.to_string(),
        }
    }

    fn request(
        owner: &str,
        name: &str,
        label: &'static str,
        trust_tier: RepoTrustTier,
    ) -> FallbackTrustedRequest {
        FallbackTrustedRequest {
            repository: repository(owner, name),
            label,
            trust_tier,
        }
    }

    fn discovery_candidate() -> DiscoveryCandidate {
        let profile = ProfileConfig {
            tech_stack: vec!["Rust".to_string()],
            keywords: vec!["cli".to_string()],
        };
        DiscoveryCandidate::new(
            GitHubIssue {
                id: 1,
                number: 1,
                title: "Fix Rust CLI diagnostic".to_string(),
                body: "Steps to reproduce: run the Rust CLI. Expected behavior: useful diagnostic."
                    .to_string(),
                labels: vec!["good first issue".to_string()],
                url: "https://github.com/owner/repo/issues/1".to_string(),
                repo_full_name: "owner/repo".to_string(),
                repo_name: "repo".to_string(),
                repo_description: "Rust CLI developer tools".to_string(),
                repo_stars: 100,
                created_at: Utc::now().to_rfc3339(),
                updated_at: Utc::now().to_rfc3339(),
            },
            "global:test",
            RepoTrustTier::Global,
            &profile,
        )
    }

    fn rate_limit_error() -> anyhow::Error {
        IssueFinderError::GitHubRateLimited.into()
    }
}
