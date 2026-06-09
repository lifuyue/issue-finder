use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;

use crate::config::Config;
use crate::config::ProfileConfig;
use crate::discovery::{
    select_enrichment_candidates, sort_candidates, DiscoveryCandidate, DiscoveryDiagnostics,
    DiscoveryOutput, DiscoveryScope, RepositoryScope,
};
use crate::github::{GitHubClient, GitHubIssue};
use crate::github_budget::{GitHubApiBudget, GitHubApiBudgetReport, GitHubRequestSource};
use crate::github_enrichment::{competition_timeline_missing, GitHubEnrichmentClient};
use crate::paths::IssueFinderPaths;
use crate::value_scoring::{assess_issue, RankedValueIssue};

use super::competition_completion::{self, CompetitionCompletionStatus};
use super::events::{record_event_for_issue, RecommendationEventSource, RecommendationEventType};
use super::feed_ranker::{apply_recommendation_assessments, displayable, sort_by_feed};
use super::state::load_state_map;

const ENRICHED_SCOUT_CANDIDATE_LIMIT: usize = 180;
const FALLBACK_ENRICHMENT_CANDIDATE_LIMIT: usize = 80;
const TRUSTED_FALLBACK_ENRICHMENT_CANDIDATE_LIMIT: usize = 40;
const ENRICHMENT_BATCH_SIZE: usize = 25;
const COMPETITION_TIMELINE_CANDIDATE_LIMIT: usize = 20;
const ENRICHMENT_CONCURRENCY_LIMIT: usize = 2;
const COMPETITION_COMPLETION_CONCURRENCY_LIMIT: usize = 2;
const POST_COMPLETION_TRUSTED_REFILL_LIMIT: usize = 32;
const POST_COMPLETION_GLOBAL_REFILL_LIMIT: usize = 32;
const REPO_SCOPED_STAGE_ENRICHMENT_LIMIT: usize = 80;
const REPO_SCOPED_RECENT_WINDOWS: [usize; 3] = [100, 300, 500];
const PRIMARY_RESULTS_PER_REPO_LIMIT: usize = 2;
const COMPETITION_COMPLETED_RESULTS_PER_REPO_LIMIT: usize = 4;
const SCOUT_RESULT_CACHE_TTL_MINUTES: i64 = 360;

#[derive(Debug, Clone, Copy)]
pub struct ScoutOptions {
    pub include_filtered: bool,
    pub record_exposure: bool,
    pub source: RecommendationEventSource,
}

impl ScoutOptions {
    pub fn cli() -> Self {
        Self {
            include_filtered: false,
            record_exposure: true,
            source: RecommendationEventSource::CliScout,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ScoutResult {
    pub ranked: Vec<RankedValueIssue>,
    pub discovery_count: usize,
    pub filtered_count: usize,
    #[serde(flatten)]
    pub diagnostics: DiscoveryDiagnostics,
    pub api_budget: GitHubApiBudgetReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedScoutResult {
    fetched_at: DateTime<Utc>,
    ranked: Vec<RankedValueIssue>,
    discovery_count: usize,
    filtered_count: usize,
    diagnostics: DiscoveryDiagnostics,
}

struct AdditionalRankingRequest<'a> {
    enrichment: &'a GitHubEnrichmentClient,
    ranked: &'a mut Vec<RankedValueIssue>,
    discovery_by_key: &'a mut HashMap<String, DiscoveryCandidate>,
    candidates: Vec<DiscoveryCandidate>,
    refresh: bool,
    display_limit: usize,
    max_budget: usize,
    stop_visible_at: Option<usize>,
    display_mode: DisplayMode,
}

struct ScoutRun {
    ranked: Vec<RankedValueIssue>,
    discovery_count: usize,
    filtered_count: usize,
    diagnostics: DiscoveryDiagnostics,
}

struct RepositoryStageRankingRequest<'a> {
    enrichment: &'a GitHubEnrichmentClient,
    output: DiscoveryOutput,
    diagnostics: &'a mut DiscoveryDiagnostics,
    ranked: &'a mut Vec<RankedValueIssue>,
    discovery_by_key: &'a mut HashMap<String, DiscoveryCandidate>,
    discovered_keys: &'a mut HashSet<String>,
    discovery_count: &'a mut usize,
    limit: usize,
    refresh: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisplayMode {
    Global,
    Repository,
}

impl DisplayMode {
    fn primary_per_repo_limit(self, limit: usize) -> usize {
        match self {
            Self::Global => PRIMARY_RESULTS_PER_REPO_LIMIT,
            Self::Repository => limit.max(1),
        }
    }

    fn completed_per_repo_limit(self, limit: usize) -> usize {
        match self {
            Self::Global => COMPETITION_COMPLETED_RESULTS_PER_REPO_LIMIT,
            Self::Repository => limit.max(1),
        }
    }
}

pub struct RecommendationEngine<'a> {
    paths: &'a IssueFinderPaths,
    config: &'a Config,
}

impl<'a> RecommendationEngine<'a> {
    pub fn new(paths: &'a IssueFinderPaths, config: &'a Config) -> Self {
        Self { paths, config }
    }

    pub async fn scout(
        &self,
        limit: usize,
        refresh: bool,
        options: ScoutOptions,
        scope: DiscoveryScope,
    ) -> Result<ScoutResult> {
        self.paths.ensure_layout()?;
        let api_budget = GitHubApiBudget::from_env();
        let scout_cache_key = scout_result_cache_key(
            &scope,
            &self.config.profile,
            limit,
            options.include_filtered,
        );
        if !refresh && !options.record_exposure {
            if let Some(cached) = load_cached_scout_result(self.paths, &scout_cache_key)? {
                api_budget.record_cache_hit(GitHubRequestSource::ScoutResult);
                return Ok(ScoutResult {
                    ranked: cached.ranked,
                    discovery_count: cached.discovery_count,
                    filtered_count: cached.filtered_count,
                    diagnostics: cached.diagnostics,
                    api_budget: api_budget.report(),
                });
            }
        }

        let github = GitHubClient::with_budget(self.config, api_budget.clone())?;
        let enrichment = GitHubEnrichmentClient::with_budget(self.config, api_budget.clone())?;
        let run = match &scope {
            DiscoveryScope::Global => {
                self.run_global_scout(
                    &github,
                    &enrichment,
                    limit,
                    refresh,
                    options.include_filtered,
                )
                .await?
            }
            DiscoveryScope::Repository { repository } => {
                self.run_repository_scout(
                    &github,
                    &enrichment,
                    repository,
                    limit,
                    refresh,
                    options.include_filtered,
                )
                .await?
            }
        };

        if options.record_exposure {
            self.record_exposure(&run.ranked, options.source, &scope)?;
        } else {
            save_cached_scout_result(
                self.paths,
                &scout_cache_key,
                &CachedScoutResult {
                    fetched_at: Utc::now(),
                    ranked: run.ranked.clone(),
                    discovery_count: run.discovery_count,
                    filtered_count: run.filtered_count,
                    diagnostics: run.diagnostics.clone(),
                },
            )?;
        }

        Ok(ScoutResult {
            ranked: run.ranked,
            discovery_count: run.discovery_count,
            filtered_count: run.filtered_count,
            diagnostics: run.diagnostics,
            api_budget: api_budget.report(),
        })
    }

    async fn run_global_scout(
        &self,
        github: &GitHubClient,
        enrichment: &GitHubEnrichmentClient,
        limit: usize,
        refresh: bool,
        include_filtered: bool,
    ) -> Result<ScoutRun> {
        let mut diagnostics = DiscoveryScope::Global.diagnostics();
        let candidates = github
            .discover_candidates(self.paths, refresh, &self.config.profile)
            .await?;
        let discovery_count = candidates.len();
        let (mut ranked, mut discovery_by_key) = self
            .rank_discovered_candidates(enrichment, candidates, limit, refresh, DisplayMode::Global)
            .await;
        let hard_pass = hard_pass_visible_count(limit);
        let fallback_target = fallback_target_visible_count(limit);
        let completion_prefill_target = completion_prefill_visible_count(limit);

        if display_count(&ranked, limit, false, DisplayMode::Global) < hard_pass {
            let mut fallback_enrichment_budget = FALLBACK_ENRICHMENT_CANDIDATE_LIMIT;
            let trusted_budget =
                TRUSTED_FALLBACK_ENRICHMENT_CANDIDATE_LIMIT.min(fallback_enrichment_budget);
            let fallback = github
                .discover_trusted_fallback_candidates(self.paths, refresh, &self.config.profile)
                .await?;
            let consumed = self
                .rank_additional_candidates(AdditionalRankingRequest {
                    enrichment,
                    ranked: &mut ranked,
                    discovery_by_key: &mut discovery_by_key,
                    candidates: fallback,
                    refresh,
                    display_limit: limit,
                    max_budget: trusted_budget,
                    stop_visible_at: Some(fallback_target.max(completion_prefill_target)),
                    display_mode: DisplayMode::Global,
                })
                .await?;
            fallback_enrichment_budget = fallback_enrichment_budget.saturating_sub(consumed);

            if display_count(&ranked, limit, false, DisplayMode::Global) < hard_pass
                && fallback_enrichment_budget > 0
            {
                let fallback = github
                    .discover_global_fallback_candidates(self.paths, refresh, &self.config.profile)
                    .await?;
                self.rank_additional_candidates(AdditionalRankingRequest {
                    enrichment,
                    ranked: &mut ranked,
                    discovery_by_key: &mut discovery_by_key,
                    candidates: fallback,
                    refresh,
                    display_limit: limit,
                    max_budget: fallback_enrichment_budget,
                    stop_visible_at: Some(hard_pass.max(completion_prefill_target)),
                    display_mode: DisplayMode::Global,
                })
                .await?;
            }
        }

        let mut completion_statuses = self
            .complete_competition_evidence(enrichment, &mut ranked, refresh, limit)
            .await;
        self.apply_feed_ranking(&mut ranked);
        append_discovery_reasons(&mut ranked, &discovery_by_key);
        competition_completion::append_completion_explanations(&mut ranked, &completion_statuses);

        if competition_limited_display_count(&ranked, limit, false, DisplayMode::Global) < hard_pass
        {
            let fallback = github
                .discover_trusted_fallback_candidates(self.paths, refresh, &self.config.profile)
                .await?;
            self.rank_additional_candidates(AdditionalRankingRequest {
                enrichment,
                ranked: &mut ranked,
                discovery_by_key: &mut discovery_by_key,
                candidates: fallback,
                refresh,
                display_limit: limit,
                max_budget: POST_COMPLETION_TRUSTED_REFILL_LIMIT,
                stop_visible_at: Some(completion_prefill_target.max(fallback_target)),
                display_mode: DisplayMode::Global,
            })
            .await?;
            completion_statuses.extend(
                self.complete_competition_evidence(enrichment, &mut ranked, refresh, limit)
                    .await,
            );
            self.apply_feed_ranking(&mut ranked);
            append_discovery_reasons(&mut ranked, &discovery_by_key);
            competition_completion::append_completion_explanations(
                &mut ranked,
                &completion_statuses,
            );

            if competition_limited_display_count(&ranked, limit, false, DisplayMode::Global)
                < hard_pass
            {
                let fallback = github
                    .discover_global_fallback_candidates(self.paths, refresh, &self.config.profile)
                    .await?;
                self.rank_additional_candidates(AdditionalRankingRequest {
                    enrichment,
                    ranked: &mut ranked,
                    discovery_by_key: &mut discovery_by_key,
                    candidates: fallback,
                    refresh,
                    display_limit: limit,
                    max_budget: POST_COMPLETION_GLOBAL_REFILL_LIMIT,
                    stop_visible_at: Some(completion_prefill_target.max(hard_pass)),
                    display_mode: DisplayMode::Global,
                })
                .await?;
                completion_statuses.extend(
                    self.complete_competition_evidence(enrichment, &mut ranked, refresh, limit)
                        .await,
                );
                self.apply_feed_ranking(&mut ranked);
                append_discovery_reasons(&mut ranked, &discovery_by_key);
                competition_completion::append_completion_explanations(
                    &mut ranked,
                    &completion_statuses,
                );
            }
        }

        let filtered_count = ranked
            .iter()
            .filter(|item| !displayable(item, include_filtered))
            .count();
        let visible = competition_completion::select_display_candidates(
            ranked,
            limit,
            include_filtered,
            DisplayMode::Global.completed_per_repo_limit(limit),
        );
        annotate_diagnostics(&mut diagnostics, &discovery_by_key, &visible);

        Ok(ScoutRun {
            ranked: visible,
            discovery_count,
            filtered_count,
            diagnostics,
        })
    }

    async fn run_repository_scout(
        &self,
        github: &GitHubClient,
        enrichment: &GitHubEnrichmentClient,
        repository: &RepositoryScope,
        limit: usize,
        refresh: bool,
        include_filtered: bool,
    ) -> Result<ScoutRun> {
        let mut diagnostics = DiscoveryScope::repository(repository.clone()).diagnostics();
        let mut ranked = Vec::new();
        let mut discovery_by_key = HashMap::new();
        let mut discovered_keys = HashSet::new();
        let mut completion_statuses = HashMap::new();
        let mut discovery_count = 0usize;

        let beginner = github
            .discover_repository_beginner_candidates(
                self.paths,
                refresh,
                repository,
                &self.config.profile,
            )
            .await?;
        self.rank_repository_stage(RepositoryStageRankingRequest {
            enrichment,
            output: beginner,
            diagnostics: &mut diagnostics,
            ranked: &mut ranked,
            discovery_by_key: &mut discovery_by_key,
            discovered_keys: &mut discovered_keys,
            discovery_count: &mut discovery_count,
            limit,
            refresh,
        })
        .await?;
        self.complete_repository_competition(
            enrichment,
            &mut ranked,
            &mut completion_statuses,
            refresh,
            limit,
        )
        .await;

        if display_count(&ranked, limit, false, DisplayMode::Repository) < limit {
            let signals = github
                .discover_repository_signal_candidates(
                    self.paths,
                    refresh,
                    repository,
                    &self.config.profile,
                )
                .await;
            self.rank_repository_stage(RepositoryStageRankingRequest {
                enrichment,
                output: signals,
                diagnostics: &mut diagnostics,
                ranked: &mut ranked,
                discovery_by_key: &mut discovery_by_key,
                discovered_keys: &mut discovered_keys,
                discovery_count: &mut discovery_count,
                limit,
                refresh,
            })
            .await?;
            self.complete_repository_competition(
                enrichment,
                &mut ranked,
                &mut completion_statuses,
                refresh,
                limit,
            )
            .await;
        }

        for window in REPO_SCOPED_RECENT_WINDOWS {
            if display_count(&ranked, limit, false, DisplayMode::Repository) >= limit {
                break;
            }
            match github
                .discover_repository_recent_candidates(
                    self.paths,
                    refresh,
                    repository,
                    &self.config.profile,
                    window,
                )
                .await
            {
                Ok(recent) => {
                    self.rank_repository_stage(RepositoryStageRankingRequest {
                        enrichment,
                        output: recent,
                        diagnostics: &mut diagnostics,
                        ranked: &mut ranked,
                        discovery_by_key: &mut discovery_by_key,
                        discovered_keys: &mut discovered_keys,
                        discovery_count: &mut discovery_count,
                        limit,
                        refresh,
                    })
                    .await?;
                    self.complete_repository_competition(
                        enrichment,
                        &mut ranked,
                        &mut completion_statuses,
                        refresh,
                        limit,
                    )
                    .await;
                }
                Err(error) => {
                    diagnostics
                        .stage_errors
                        .push(format!("repo_scoped:recent_open:{window}: {error}"));
                    break;
                }
            }
        }

        diagnostics.fallback_exhausted =
            display_count(&ranked, limit, include_filtered, DisplayMode::Repository) < limit;
        self.apply_feed_ranking(&mut ranked);
        append_discovery_reasons(&mut ranked, &discovery_by_key);
        competition_completion::append_completion_explanations(&mut ranked, &completion_statuses);

        let filtered_count = ranked
            .iter()
            .filter(|item| !displayable(item, include_filtered))
            .count();
        let visible = competition_completion::select_display_candidates(
            ranked,
            limit,
            include_filtered,
            DisplayMode::Repository.completed_per_repo_limit(limit),
        );
        annotate_diagnostics(&mut diagnostics, &discovery_by_key, &visible);

        Ok(ScoutRun {
            ranked: visible,
            discovery_count,
            filtered_count,
            diagnostics,
        })
    }

    async fn rank_repository_stage(
        &self,
        request: RepositoryStageRankingRequest<'_>,
    ) -> Result<()> {
        let RepositoryStageRankingRequest {
            enrichment,
            output,
            diagnostics,
            ranked,
            discovery_by_key,
            discovered_keys,
            discovery_count,
            limit,
            refresh,
        } = request;
        diagnostics.merge(output.diagnostics);
        let candidates = output
            .candidates
            .into_iter()
            .filter(|candidate| discovered_keys.insert(candidate.key()))
            .collect::<Vec<_>>();
        *discovery_count += candidates.len();

        self.rank_additional_candidates(AdditionalRankingRequest {
            enrichment,
            ranked,
            discovery_by_key,
            candidates,
            refresh,
            display_limit: limit,
            max_budget: REPO_SCOPED_STAGE_ENRICHMENT_LIMIT,
            stop_visible_at: Some(limit),
            display_mode: DisplayMode::Repository,
        })
        .await?;
        Ok(())
    }

    async fn complete_repository_competition(
        &self,
        enrichment: &GitHubEnrichmentClient,
        ranked: &mut [RankedValueIssue],
        completion_statuses: &mut HashMap<String, CompetitionCompletionStatus>,
        refresh: bool,
        limit: usize,
    ) {
        completion_statuses.extend(
            self.complete_competition_evidence(enrichment, ranked, refresh, limit)
                .await,
        );
        self.apply_feed_ranking(ranked);
        competition_completion::append_completion_explanations(ranked, completion_statuses);
    }

    pub async fn daily_candidates(
        &self,
        refresh: bool,
        candidate_limit: usize,
        scope: DiscoveryScope,
    ) -> Result<ScoutResult> {
        self.scout(
            candidate_limit,
            refresh,
            ScoutOptions {
                include_filtered: false,
                record_exposure: false,
                source: RecommendationEventSource::Daily,
            },
            scope,
        )
        .await
    }

    pub async fn assess_issue(
        &self,
        issue: GitHubIssue,
        refresh: bool,
        record_read: bool,
        source: RecommendationEventSource,
    ) -> Result<RankedValueIssue> {
        self.paths.ensure_layout()?;
        let enrichment = GitHubEnrichmentClient::new(self.config)?;
        let ranked = self
            .rank_single_issue(&enrichment, issue, refresh, true)
            .await?;
        if record_read {
            record_event_for_issue(
                self.paths,
                &ranked.issue,
                Some(&ranked.enriched_issue),
                RecommendationEventType::Read,
                source,
                serde_json::json!({}),
            )?;
        }
        Ok(ranked)
    }

    pub fn record_exposure(
        &self,
        ranked: &[RankedValueIssue],
        source: RecommendationEventSource,
        scope: &DiscoveryScope,
    ) -> Result<()> {
        for item in ranked {
            record_event_for_issue(
                self.paths,
                &item.issue,
                Some(&item.enriched_issue),
                RecommendationEventType::Shown,
                source,
                serde_json::json!({
                    "finalFeedScore": item.recommendation.final_feed_score,
                    "baseCategory": item.recommendation.base_category.to_string(),
                    "scope": scope.diagnostics().scope,
                    "repository": scope.diagnostics().repository
                }),
            )?;
        }
        Ok(())
    }

    async fn rank_discovered_candidates(
        &self,
        enrichment: &GitHubEnrichmentClient,
        candidates: Vec<DiscoveryCandidate>,
        limit: usize,
        refresh: bool,
        display_mode: DisplayMode,
    ) -> (Vec<RankedValueIssue>, HashMap<String, DiscoveryCandidate>) {
        let selected = select_enrichment_candidates_for_mode(
            candidates,
            ENRICHED_SCOUT_CANDIDATE_LIMIT,
            display_mode,
        );
        let discovery_by_key = selected
            .iter()
            .map(|candidate| (candidate_key(&candidate.issue), candidate.clone()))
            .collect::<HashMap<_, _>>();
        let mut ranked = Vec::new();

        for (batch_index, batch) in selected.chunks(ENRICHMENT_BATCH_SIZE).enumerate() {
            let ranked_batch = stream::iter(batch.iter().cloned().enumerate().map(
                |(index, candidate)| async move {
                    let absolute_index = batch_index * ENRICHMENT_BATCH_SIZE + index;
                    self.rank_single_issue(
                        enrichment,
                        candidate.issue,
                        refresh,
                        absolute_index < COMPETITION_TIMELINE_CANDIDATE_LIMIT,
                    )
                    .await
                    .ok()
                },
            ))
            .buffer_unordered(ENRICHMENT_CONCURRENCY_LIMIT)
            .filter_map(|item| async move { item })
            .collect::<Vec<_>>()
            .await;

            ranked.extend(ranked_batch);
            self.apply_feed_ranking(&mut ranked);
            append_discovery_reasons(&mut ranked, &discovery_by_key);

            if display_count(&ranked, limit, false, display_mode)
                >= completion_prefill_visible_count(limit)
            {
                break;
            }
        }

        (ranked, discovery_by_key)
    }

    async fn rank_additional_candidates(
        &self,
        request: AdditionalRankingRequest<'_>,
    ) -> Result<usize> {
        let AdditionalRankingRequest {
            enrichment,
            ranked,
            discovery_by_key,
            candidates,
            refresh,
            display_limit,
            max_budget,
            stop_visible_at,
            display_mode,
        } = request;

        if candidates.is_empty() || max_budget == 0 {
            return Ok(0);
        }

        let existing = ranked
            .iter()
            .map(|item| candidate_key(&item.issue))
            .collect::<HashSet<_>>();
        let candidates = candidates
            .into_iter()
            .filter(|candidate| !existing.contains(&candidate.key()))
            .collect::<Vec<_>>();
        let selected = select_enrichment_candidates_for_mode(candidates, max_budget, display_mode);
        if selected.is_empty() {
            return Ok(0);
        }

        for candidate in &selected {
            discovery_by_key.insert(candidate_key(&candidate.issue), candidate.clone());
        }

        let mut consumed = 0;
        for (batch_index, batch) in selected.chunks(ENRICHMENT_BATCH_SIZE).enumerate() {
            let ranked_batch = stream::iter(batch.iter().cloned().enumerate().map(
                |(index, candidate)| async move {
                    let absolute_index = batch_index * ENRICHMENT_BATCH_SIZE + index;
                    self.rank_single_issue(
                        enrichment,
                        candidate.issue,
                        refresh,
                        absolute_index < COMPETITION_TIMELINE_CANDIDATE_LIMIT,
                    )
                    .await
                    .ok()
                },
            ))
            .buffer_unordered(ENRICHMENT_CONCURRENCY_LIMIT)
            .filter_map(|item| async move { item })
            .collect::<Vec<_>>()
            .await;

            consumed += batch.len();
            ranked.extend(ranked_batch);
            self.apply_feed_ranking(ranked);
            append_discovery_reasons(ranked, discovery_by_key);

            if stop_visible_at.is_some_and(|target| {
                display_count(ranked, display_limit, false, display_mode) >= target
            }) {
                break;
            }
        }

        Ok(consumed)
    }

    async fn complete_competition_evidence(
        &self,
        enrichment: &GitHubEnrichmentClient,
        ranked: &mut [RankedValueIssue],
        refresh: bool,
        limit: usize,
    ) -> HashMap<String, CompetitionCompletionStatus> {
        if limit == 0 {
            return HashMap::new();
        }

        self.apply_feed_ranking(ranked);
        let plan = competition_completion::plan_completion(ranked, limit);
        let mut statuses =
            competition_completion::annotate_skipped_by_budget(ranked, &plan.skipped_keys);

        if plan.complete_keys.is_empty() {
            return statuses;
        }

        let requested = plan.complete_keys.into_iter().collect::<HashSet<_>>();
        let requests = ranked
            .iter()
            .filter_map(|item| {
                let key = competition_completion::issue_key(item);
                requested
                    .contains(&key)
                    .then(|| (key, item.issue.clone(), item.enriched_issue.clone()))
            })
            .collect::<Vec<_>>();

        let completed = stream::iter(requests.into_iter().map(
            |(key, issue, current)| async move {
                let enriched = enrichment
                    .complete_competition_timeline(self.paths, &issue, &current, refresh)
                    .await;
                let status = if competition_timeline_missing(&enriched) {
                    CompetitionCompletionStatus::Failed
                } else {
                    CompetitionCompletionStatus::Completed
                };
                (key, enriched, status)
            },
        ))
        .buffer_unordered(COMPETITION_COMPLETION_CONCURRENCY_LIMIT)
        .collect::<Vec<_>>()
        .await;

        let completed_by_key = completed
            .into_iter()
            .map(|(key, enriched, status)| {
                statuses.insert(key.clone(), status);
                (key, enriched)
            })
            .collect::<HashMap<_, _>>();

        for item in ranked {
            let key = competition_completion::issue_key(item);
            let Some(enriched) = completed_by_key.get(&key) else {
                continue;
            };
            item.enriched_issue = enriched.clone();
            item.value_assessment = assess_issue(&item.enriched_issue, &self.config.profile);
            item.score = item.value_assessment.final_rank_score;
            item.explanation = item.value_assessment.explanation.clone();
        }

        statuses
    }

    async fn rank_single_issue(
        &self,
        enrichment: &GitHubEnrichmentClient,
        issue: GitHubIssue,
        refresh: bool,
        include_competition_timeline: bool,
    ) -> Result<RankedValueIssue> {
        let enriched = enrichment
            .enrich_issue_with_options(self.paths, &issue, refresh, include_competition_timeline)
            .await;
        let value_assessment = assess_issue(&enriched, &self.config.profile);
        let mut ranked = RankedValueIssue {
            issue,
            score: value_assessment.final_rank_score,
            value_assessment,
            enriched_issue: enriched,
            explanation: Vec::new(),
            recommendation: Default::default(),
        };
        ranked.explanation = ranked.value_assessment.explanation.clone();
        self.apply_feed_ranking(std::slice::from_mut(&mut ranked));
        Ok(ranked)
    }

    fn apply_feed_ranking(&self, ranked: &mut [RankedValueIssue]) {
        let states = load_state_map(self.paths).unwrap_or_default();
        apply_recommendation_assessments(ranked, &states);
        sort_by_feed(ranked);
    }
}

fn append_discovery_reasons(
    ranked: &mut [RankedValueIssue],
    discovery_by_key: &HashMap<String, DiscoveryCandidate>,
) {
    for item in ranked {
        let key = candidate_key(&item.issue);
        let Some(candidate) = discovery_by_key.get(&key) else {
            continue;
        };
        for reason in candidate.discovery_reasons() {
            if !item.explanation.contains(&reason) {
                item.explanation.push(reason);
            }
        }
    }
}

fn select_enrichment_candidates_for_mode(
    mut candidates: Vec<DiscoveryCandidate>,
    max_budget: usize,
    display_mode: DisplayMode,
) -> Vec<DiscoveryCandidate> {
    if display_mode == DisplayMode::Global {
        return select_enrichment_candidates(candidates, max_budget);
    }

    sort_candidates(&mut candidates);
    candidates.truncate(max_budget);
    candidates
}

fn annotate_diagnostics(
    diagnostics: &mut DiscoveryDiagnostics,
    discovery_by_key: &HashMap<String, DiscoveryCandidate>,
    visible: &[RankedValueIssue],
) {
    let visible_keys = visible
        .iter()
        .map(|item| candidate_key(&item.issue))
        .collect::<HashSet<_>>();
    let mut ranked_keys_by_lane = HashMap::<String, HashSet<String>>::new();
    for (key, candidate) in discovery_by_key {
        for lane in &candidate.source_lanes {
            ranked_keys_by_lane
                .entry(lane.clone())
                .or_default()
                .insert(key.clone());
        }
    }
    diagnostics.mark_ranked_and_visible(&ranked_keys_by_lane, &visible_keys);
}

fn candidate_key(issue: &GitHubIssue) -> String {
    format!("{}#{}", issue.repo_full_name, issue.number)
}

fn display_count(
    ranked: &[RankedValueIssue],
    limit: usize,
    include_filtered: bool,
    display_mode: DisplayMode,
) -> usize {
    if limit == 0 {
        return 0;
    }

    let mut selected = 0;
    let mut repo_counts = HashMap::<&str, usize>::new();
    let per_repo_limit = display_mode.primary_per_repo_limit(limit);

    for item in ranked {
        if !displayable(item, include_filtered) {
            continue;
        }

        let repo = item.issue.repo_full_name.as_str();
        let count = *repo_counts.get(repo).unwrap_or(&0);
        if count < per_repo_limit {
            repo_counts.insert(repo, count + 1);
            selected += 1;
            if selected == limit {
                return selected;
            }
        }
    }

    selected
}

fn hard_pass_visible_count(limit: usize) -> usize {
    ceil_percent(limit, 70)
}

fn fallback_target_visible_count(limit: usize) -> usize {
    ceil_percent(limit, 80)
}

fn completion_prefill_visible_count(limit: usize) -> usize {
    limit.saturating_mul(2).min(limit.saturating_add(15))
}

fn ceil_percent(value: usize, percent: usize) -> usize {
    if value == 0 {
        return 0;
    }
    (value * percent).div_ceil(100)
}

fn competition_limited_display_count(
    ranked: &[RankedValueIssue],
    limit: usize,
    include_filtered: bool,
    display_mode: DisplayMode,
) -> usize {
    competition_completion::select_display_candidates(
        ranked.to_vec(),
        limit,
        include_filtered,
        display_mode.completed_per_repo_limit(limit),
    )
    .len()
}

fn load_cached_scout_result(
    paths: &IssueFinderPaths,
    key: &str,
) -> Result<Option<CachedScoutResult>> {
    let path = paths.scout_result_cache_path(key);
    if !path.exists() {
        return Ok(None);
    }

    let raw =
        fs::read_to_string(&path).with_context(|| format!("unable to read {}", path.display()))?;
    let Ok(payload) = serde_json::from_str::<CachedScoutResult>(&raw) else {
        return Ok(None);
    };
    if Utc::now() - payload.fetched_at > Duration::minutes(SCOUT_RESULT_CACHE_TTL_MINUTES) {
        return Ok(None);
    }
    Ok(Some(payload))
}

fn save_cached_scout_result(
    paths: &IssueFinderPaths,
    key: &str,
    result: &CachedScoutResult,
) -> Result<()> {
    crate::paths::atomic_write(
        &paths.scout_result_cache_path(key),
        serde_json::to_vec_pretty(result)?,
    )
}

fn scout_result_cache_key(
    scope: &DiscoveryScope,
    profile: &ProfileConfig,
    limit: usize,
    include_filtered: bool,
) -> String {
    format!(
        "{}__limit-{limit}__filtered-{include_filtered}__tech-{}__keywords-{}",
        scope.cache_fragment(),
        profile.tech_stack.join("+"),
        profile.keywords.join("+")
    )
}

pub fn select_display_candidates(
    ranked: Vec<RankedValueIssue>,
    limit: usize,
    include_filtered: bool,
) -> Vec<RankedValueIssue> {
    if limit == 0 {
        return Vec::new();
    }

    let mut selected = Vec::new();
    let mut repo_counts = HashMap::<String, usize>::new();

    for item in ranked {
        if !displayable(&item, include_filtered) {
            continue;
        }

        let repo = item.issue.repo_full_name.clone();
        let count = *repo_counts.get(&repo).unwrap_or(&0);
        if count < PRIMARY_RESULTS_PER_REPO_LIMIT {
            repo_counts.insert(repo, count + 1);
            selected.push(item);
            if selected.len() == limit {
                return selected;
            }
        }
    }

    selected
}
