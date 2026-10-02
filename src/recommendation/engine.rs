use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;

use crate::config::Config;
use crate::config::ProfileConfig;
use crate::discovery::{
    select_enrichment_candidates, sort_candidates, DiscoveryCandidate, DiscoveryDiagnostics,
    DiscoveryOutput, DiscoveryScope, RepositoryScope, SearchOptions,
};
use crate::github::{GitHubClient, GitHubIssue};
use crate::github_budget::{GitHubApiBudget, GitHubApiBudgetReport, GitHubRequestSource};
use crate::github_enrichment::{
    canonicalize_enriched_issue_repo, competition_timeline_missing, EnrichedIssue,
    GitHubEnrichmentClient,
};
use crate::memory::apply_ranking_hints_to_ranked;
use crate::paths::IssueFinderPaths;
use crate::value_scoring::{assess_issue, RankedValueIssue};

use super::competition_completion::{self, CompetitionCompletionStatus};
use super::events::{record_event_for_issue, RecommendationEventSource, RecommendationEventType};
use super::feed_ranker::{apply_recommendation_assessments, displayable, sort_by_feed};
use super::state::{load_state_map_with_policy, FeedbackPolicy};

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
    feedback_policy: FeedbackPolicy,
    decision_provider: Option<std::sync::Arc<dyn crate::decision::contract::Provider>>,
}

impl<'a> RecommendationEngine<'a> {
    pub fn new(paths: &'a IssueFinderPaths, config: &'a Config) -> Self {
        Self {
            paths,
            config,
            feedback_policy: FeedbackPolicy::LegacyLifecycle,
            decision_provider: None,
        }
    }

    pub fn for_codex(paths: &'a IssueFinderPaths, config: &'a Config) -> Self {
        Self {
            paths,
            config,
            feedback_policy: FeedbackPolicy::CodexExposure,
            decision_provider: None,
        }
    }

    /// Same material engineering and policy for any provider, including offline replay fakes.
    pub fn with_decision_provider(
        paths: &'a IssueFinderPaths,
        config: &'a Config,
        provider: std::sync::Arc<dyn crate::decision::contract::Provider>,
    ) -> Self {
        let mut engine = Self::for_codex(paths, config);
        engine.decision_provider = Some(provider);
        engine
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
            self.feedback_policy,
        );
        // Session screening must revalidate material versions; only per-material
        // judgments are cached. A cached final list can hide newly eligible items.
        if !refresh
            && !options.record_exposure
            && self.feedback_policy != FeedbackPolicy::CodexExposure
        {
            if let Some(cached) = load_cached_scout_result(self.paths, &scout_cache_key)? {
                api_budget.record_cache_hit(GitHubRequestSource::ScoutResult);
                let mut ranked = cached.ranked;
                canonicalize_ranked_issues(&mut ranked);
                if self.feedback_policy == FeedbackPolicy::CodexExposure {
                    self.apply_feed_ranking(&mut ranked);
                } else {
                    let _ = apply_ranking_hints_to_ranked(self.paths, &mut ranked);
                    sort_by_feed(&mut ranked);
                }
                return Ok(ScoutResult {
                    ranked,
                    discovery_count: cached.discovery_count,
                    filtered_count: cached.filtered_count,
                    diagnostics: cached.diagnostics,
                    api_budget: api_budget.report(),
                });
            }
        }

        let github = GitHubClient::with_budget(self.config, api_budget.clone())?;
        let enrichment = GitHubEnrichmentClient::with_budget(self.config, api_budget.clone())?;
        let mut run = match &scope {
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
        } else if self.feedback_policy != FeedbackPolicy::CodexExposure {
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
        if self.feedback_policy != FeedbackPolicy::CodexExposure {
            let _ = apply_ranking_hints_to_ranked(self.paths, &mut run.ranked);
        }
        sort_by_feed(&mut run.ranked);

        Ok(ScoutResult {
            ranked: run.ranked,
            discovery_count: run.discovery_count,
            filtered_count: run.filtered_count,
            diagnostics: run.diagnostics,
            api_budget: api_budget.report(),
        })
    }

    /// Search the agent's chosen candidate pool, then use the same assessments,
    /// feedback, evidence completion and display policy as the regular scout.
    pub async fn scout_search(
        &self,
        limit: usize,
        refresh: bool,
        options: ScoutOptions,
        scope: DiscoveryScope,
        search: &SearchOptions,
    ) -> Result<ScoutResult> {
        search.validate(&scope)?;
        if !(1..=100).contains(&limit) {
            anyhow::bail!("search result limit must be between 1 and 100");
        }
        self.paths.ensure_layout()?;
        let configured_budget = GitHubApiBudget::from_env().report().total_budget;
        let api_budget = GitHubApiBudget::with_total_budget(Some(
            configured_budget.map_or(search.api_budget, |limit| limit.min(search.api_budget)),
        ));
        let github = GitHubClient::with_budget(self.config, api_budget.clone())?;
        let enrichment = GitHubEnrichmentClient::with_budget(self.config, api_budget.clone())?;
        let output = github
            .search_candidates(self.paths, refresh, &self.config.profile, &scope, search)
            .await?;
        let mut diagnostics = output.diagnostics;
        let discovery_count = output.candidates.len();
        let display_mode = match &scope {
            DiscoveryScope::Global => DisplayMode::Global,
            DiscoveryScope::Repository { .. } => DisplayMode::Repository,
        };
        let (mut ranked, mut discovery_by_key) = self
            .rank_discovered_candidates(
                &enrichment,
                output.candidates,
                limit,
                refresh,
                display_mode,
            )
            .await;
        let completion_statuses = self
            .complete_competition_evidence(&enrichment, &mut ranked, refresh, limit)
            .await;
        self.apply_decision(&enrichment, &mut ranked, refresh, &mut diagnostics)
            .await;
        self.recheck_shortlist(
            &enrichment,
            &mut ranked,
            limit,
            options.include_filtered,
            display_mode,
            &mut diagnostics,
        )
        .await;
        self.apply_final_feed_ranking(&mut ranked, &mut diagnostics)?;
        append_discovery_reasons(&mut ranked, &mut discovery_by_key);
        competition_completion::append_completion_explanations(&mut ranked, &completion_statuses);
        if let Some(details) = &mut diagnostics.search {
            details.assessed_count = ranked.len();
            details.unassessed_count = discovery_count.saturating_sub(ranked.len());
            details.enrichment_limit = ENRICHED_SCOUT_CANDIDATE_LIMIT;
            details.evidence_incomplete = details.unassessed_count > 0
                || ranked.iter().any(|item| {
                    !item.enriched_issue.warnings.is_empty()
                        || competition_timeline_missing(&item.enriched_issue)
                });
        }
        let assessed_keys = ranked
            .iter()
            .map(|item| candidate_key(&item.issue))
            .collect::<HashSet<_>>();
        discovery_by_key.retain(|key, _| assessed_keys.contains(key));
        let filtered_count = ranked
            .iter()
            .filter(|item| !displayable(item, options.include_filtered))
            .count();
        let ranked = competition_completion::select_display_candidates(
            ranked,
            limit,
            options.include_filtered,
            display_mode.completed_per_repo_limit(limit),
        );
        annotate_diagnostics(&mut diagnostics, &discovery_by_key, &ranked);
        let report = api_budget.report();
        if !report.budget_exhausted.is_empty() {
            diagnostics.stage_errors.push(
                "GitHub API request budget exhausted; candidate evidence may be incomplete. Refine the query or increase apiBudget for a subsequent call.".to_string(),
            );
        }
        if options.record_exposure {
            self.record_exposure(&ranked, options.source, &scope)?;
        }
        Ok(ScoutResult {
            ranked,
            discovery_count,
            filtered_count,
            diagnostics,
            api_budget: report,
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
                .discover_trusted_fallback_candidates_with_factual_order(
                    self.paths,
                    refresh,
                    &self.config.profile,
                    self.feedback_policy == FeedbackPolicy::CodexExposure,
                )
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
                    .discover_global_fallback_candidates_with_factual_order(
                        self.paths,
                        refresh,
                        &self.config.profile,
                        self.feedback_policy == FeedbackPolicy::CodexExposure,
                    )
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
        append_discovery_reasons(&mut ranked, &mut discovery_by_key);
        competition_completion::append_completion_explanations(&mut ranked, &completion_statuses);

        if competition_limited_display_count(&ranked, limit, false, DisplayMode::Global) < hard_pass
        {
            let fallback = github
                .discover_trusted_fallback_candidates_with_factual_order(
                    self.paths,
                    refresh,
                    &self.config.profile,
                    self.feedback_policy == FeedbackPolicy::CodexExposure,
                )
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
            append_discovery_reasons(&mut ranked, &mut discovery_by_key);
            competition_completion::append_completion_explanations(
                &mut ranked,
                &completion_statuses,
            );

            if competition_limited_display_count(&ranked, limit, false, DisplayMode::Global)
                < hard_pass
            {
                let fallback = github
                    .discover_global_fallback_candidates_with_factual_order(
                        self.paths,
                        refresh,
                        &self.config.profile,
                        self.feedback_policy == FeedbackPolicy::CodexExposure,
                    )
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
                append_discovery_reasons(&mut ranked, &mut discovery_by_key);
                competition_completion::append_completion_explanations(
                    &mut ranked,
                    &completion_statuses,
                );
            }
        }

        self.apply_decision(enrichment, &mut ranked, refresh, &mut diagnostics)
            .await;
        self.recheck_shortlist(
            enrichment,
            &mut ranked,
            limit,
            include_filtered,
            DisplayMode::Global,
            &mut diagnostics,
        )
        .await;
        self.apply_final_feed_ranking(&mut ranked, &mut diagnostics)?;
        append_discovery_reasons(&mut ranked, &mut discovery_by_key);
        competition_completion::append_completion_explanations(&mut ranked, &completion_statuses);
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
        self.apply_decision(enrichment, &mut ranked, refresh, &mut diagnostics)
            .await;
        self.recheck_shortlist(
            enrichment,
            &mut ranked,
            limit,
            include_filtered,
            DisplayMode::Repository,
            &mut diagnostics,
        )
        .await;
        self.apply_final_feed_ranking(&mut ranked, &mut diagnostics)?;
        append_discovery_reasons(&mut ranked, &mut discovery_by_key);
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
        let mut ranked = self
            .rank_single_issue(&enrichment, issue, refresh, true)
            .await?;
        if self.feedback_policy == FeedbackPolicy::CodexExposure {
            ranked.enriched_issue.availability = Some(
                enrichment
                    .availability(
                        self.paths,
                        &ranked.issue,
                        crate::availability::AvailabilityDepth::Final,
                    )
                    .await,
            );
        }
        if let Some(snapshot) = &mut ranked.enriched_issue.decision {
            snapshot.status = crate::decision::JudgmentStatus::NotEvaluated;
            snapshot.error = Some("assess fetched current GitHub evidence; any prior scout judgment applies only to its saved material and has not been revalidated here".to_string());
            ranked.value_assessment = assess_issue(&ranked.enriched_issue, &self.config.profile);
            self.apply_feed_ranking(std::slice::from_mut(&mut ranked));
        }
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
        let selected = self.select_enrichment_candidates(
            candidates,
            ENRICHED_SCOUT_CANDIDATE_LIMIT,
            display_mode,
        );
        let mut discovery_by_key = selected
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
            append_discovery_reasons(&mut ranked, &mut discovery_by_key);

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
        let selected = self.select_enrichment_candidates(candidates, max_budget, display_mode);
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
            if self.feedback_policy == FeedbackPolicy::CodexExposure {
                crate::decision::factual_competition_only(&mut item.enriched_issue);
            }
            canonicalize_ranked_issue(item);
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
        let availability = if self.feedback_policy == FeedbackPolicy::CodexExposure {
            Some(
                enrichment
                    .availability(
                        self.paths,
                        &issue,
                        crate::availability::AvailabilityDepth::Initial,
                    )
                    .await,
            )
        } else {
            None
        };
        let excluded = availability
            .as_ref()
            .is_some_and(availability_blocks_new_work);
        let mut enriched = if excluded {
            EnrichedIssue::from_issue(&issue)
        } else {
            enrichment
                .enrich_issue_with_options(
                    self.paths,
                    &issue,
                    refresh,
                    include_competition_timeline,
                )
                .await
        };
        enriched.availability = availability;
        canonicalize_enriched_issue_repo(&mut enriched);
        if self.feedback_policy == FeedbackPolicy::CodexExposure {
            // Tag before the first assessment, so legacy semantics cannot hide
            // candidates before they reach the bounded model pool.
            enriched.decision = Some(crate::decision::JudgmentSnapshot::pending(
                "Semantic screening has not run for this material",
            ));
            crate::decision::factual_competition_only(&mut enriched);
        }
        let value_assessment = assess_issue(&enriched, &self.config.profile);
        let mut ranked = RankedValueIssue {
            issue,
            score: value_assessment.final_rank_score,
            value_assessment,
            enriched_issue: enriched,
            explanation: Vec::new(),
            recommendation: Default::default(),
        };
        canonicalize_ranked_issue(&mut ranked);
        ranked.explanation = ranked.value_assessment.explanation.clone();
        self.apply_feed_ranking(std::slice::from_mut(&mut ranked));
        Ok(ranked)
    }

    fn apply_feed_ranking(&self, ranked: &mut [RankedValueIssue]) {
        let states =
            load_state_map_with_policy(self.paths, self.feedback_policy).unwrap_or_default();
        apply_recommendation_assessments(ranked, &states);
        // Historical issue-type hints classify text with keywords. They belong
        // to the legacy workflow, not the semantic screening policy.
        if self.feedback_policy != FeedbackPolicy::CodexExposure {
            let _ = apply_ranking_hints_to_ranked(self.paths, ranked);
        }
        sort_by_feed(ranked);
    }

    fn apply_final_feed_ranking(
        &self,
        ranked: &mut [RankedValueIssue],
        diagnostics: &mut DiscoveryDiagnostics,
    ) -> Result<()> {
        if self.feedback_policy != FeedbackPolicy::CodexExposure {
            self.apply_feed_ranking(ranked);
            return Ok(());
        }
        let states = load_state_map_with_policy(self.paths, self.feedback_policy)?;
        apply_recommendation_assessments(ranked, &states);
        sort_by_feed(ranked);
        let replay = crate::decision::replay::ScoutReplay::capture(
            &self.config.profile,
            ranked,
            &states,
            self.feedback_policy,
        )?;
        diagnostics.decision_replay_path = Some(replay.save(self.paths)?.display().to_string());
        Ok(())
    }

    /// Refresh provisional recommendations, then backfill from the already
    /// bounded pool. Rechecking never adds discovery pages or model calls.
    async fn recheck_shortlist(
        &self,
        enrichment: &GitHubEnrichmentClient,
        ranked: &mut [RankedValueIssue],
        limit: usize,
        include_filtered: bool,
        mode: DisplayMode,
        diagnostics: &mut DiscoveryDiagnostics,
    ) {
        if self.feedback_policy != FeedbackPolicy::CodexExposure {
            return;
        }
        let mut checked = HashSet::new();
        loop {
            self.apply_feed_ranking(ranked);
            let provisional = competition_completion::select_display_candidates(
                ranked.to_vec(),
                limit,
                include_filtered,
                mode.completed_per_repo_limit(limit),
            );
            let next = provisional
                .iter()
                .find(|item| !checked.contains(&candidate_key(&item.issue)));
            let Some(next) = next else {
                break;
            };
            let key = candidate_key(&next.issue);
            let facts = enrichment
                .availability(
                    self.paths,
                    &next.issue,
                    crate::availability::AvailabilityDepth::Final,
                )
                .await;
            if !facts.checks_complete() {
                diagnostics.stage_errors.push(format!("Availability: {key} has incomplete current evidence; absence of a competing PR or existing fix is not established"));
            }
            checked.insert(key.clone());
            if let Some(item) = ranked
                .iter_mut()
                .find(|item| candidate_key(&item.issue) == key)
            {
                item.enriched_issue.availability = Some(facts);
                item.value_assessment = assess_issue(&item.enriched_issue, &self.config.profile);
            }
        }
    }

    fn select_enrichment_candidates(
        &self,
        mut candidates: Vec<DiscoveryCandidate>,
        budget: usize,
        mode: DisplayMode,
    ) -> Vec<DiscoveryCandidate> {
        if self.feedback_policy == FeedbackPolicy::CodexExposure {
            // Discovery source quotas are product policy; textual rough scores
            // are not allowed to consume the semantic call budget first.
            for candidate in &mut candidates {
                candidate.rough_score =
                    candidate.issue.repo_stars.checked_ilog10().unwrap_or(0) as i32;
            }
        }
        select_enrichment_candidates_for_mode(candidates, budget, mode)
    }

    async fn apply_decision(
        &self,
        enrichment: &GitHubEnrichmentClient,
        ranked: &mut [RankedValueIssue],
        refresh: bool,
        diagnostics: &mut DiscoveryDiagnostics,
    ) {
        use crate::decision::{self, JudgmentSnapshot, JudgmentStatus};
        if self.feedback_policy != FeedbackPolicy::CodexExposure || ranked.is_empty() {
            return;
        }
        if let Err(error) = self.config.decision.validate() {
            diagnostics.stage_errors.push(error.to_string());
            for item in ranked.iter_mut() {
                let mut snapshot = JudgmentSnapshot::pending(&error.to_string());
                snapshot.status = JudgmentStatus::Failed;
                item.enriched_issue.decision = Some(snapshot);
                item.value_assessment = assess_issue(&item.enriched_issue, &self.config.profile);
            }
            return;
        }
        let eligible = ranked
            .iter()
            .enumerate()
            .filter_map(|(index, item)| (!facts_block_new_work(item)).then_some(index))
            .collect::<Vec<_>>();
        let budget = self.config.decision.candidate_budget.min(eligible.len());
        // Include a deterministic spread beyond the top segment. No legacy
        // semantic tags, counters or text keywords enter pool selection.
        let indices = semantic_pool_indices(eligible.len(), budget)
            .into_iter()
            .map(|index| eligible[index])
            .collect::<Vec<_>>();
        for item in ranked.iter_mut() {
            let mut snapshot = JudgmentSnapshot::pending(
                "Decision model candidate budget did not cover this item; semantic state is unknown",
            );
            if facts_block_new_work(item) {
                snapshot.status = JudgmentStatus::SkippedFacts;
                snapshot.error = Some("Current GitHub facts exclude this candidate from default new-fix recommendations; semantic screening was not needed".to_string());
            } else {
                snapshot.status = JudgmentStatus::SkippedBudget;
            }
            item.enriched_issue.decision = Some(snapshot);
        }
        let owned = if self.decision_provider.is_none() && budget > 0 {
            Some(crate::decision::provider::ConfiguredProvider::new(
                &self.config.decision,
            ))
        } else {
            None
        };
        let provider: Option<&dyn crate::decision::contract::Provider> =
            self.decision_provider.as_deref().or_else(|| {
                owned
                    .as_ref()
                    .and_then(|p| p.as_ref().ok())
                    .map(|p| p as &dyn crate::decision::contract::Provider)
            });
        let initialization_error = owned
            .as_ref()
            .and_then(|p| p.as_ref().err())
            .map(ToString::to_string);
        let queued_at = std::time::Instant::now();
        let active = std::sync::atomic::AtomicUsize::new(0);
        let peak = std::sync::atomic::AtomicUsize::new(0);
        // Material gathering and the complete model turn share one issue slot.
        // The provider deadline starts only after this future obtains its slot.
        let jobs = indices
            .into_iter()
            .map(|index| (index, ranked[index].clone()))
            .collect::<Vec<_>>();
        let results = stream::iter(jobs.into_iter().map(|(index, item)| {
            let initialization_error = initialization_error.as_deref();
            let active = &active;
            let peak = &peak;
            async move {
                let queue_wait_ms = queued_at.elapsed().as_millis() as u64;
                let started = std::time::Instant::now();
                let in_flight = active.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                peak.fetch_max(in_flight, std::sync::atomic::Ordering::SeqCst);
                let snapshot = if let Some(reason) = initialization_error {
                    let mut snapshot = JudgmentSnapshot::pending(reason);
                    snapshot.status = JudgmentStatus::Failed;
                    snapshot
                } else {
                    let result = async {
                        let evidence = enrichment
                            .decision_evidence(
                                self.paths,
                                &item.issue,
                                &item.enriched_issue,
                                refresh,
                            )
                            .await?
                            .with_user_requirements(&self.config.decision.task_preferences);
                        decision::judge(
                            self.paths,
                            provider.context("Decision model provider unavailable")?,
                            evidence,
                            &self.config.profile,
                            refresh,
                        )
                        .await
                    }
                    .await;
                    match result {
                        Ok(snapshot) => snapshot,
                        Err(error) => {
                            let mut snapshot = JudgmentSnapshot::pending(&error.to_string());
                            snapshot.status = JudgmentStatus::Failed;
                            snapshot
                        }
                    }
                };
                active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                let timing = decision::TaskTiming {
                    candidate: candidate_key(&item.issue),
                    queue_wait_ms,
                    execution_ms: started.elapsed().as_millis() as u64,
                    model_duration_ms: if snapshot.cache_hit {
                        None
                    } else {
                        snapshot
                            .response
                            .as_ref()
                            .and_then(|response| response.metadata.duration_ms)
                    },
                    cache_hit: snapshot.cache_hit,
                };
                (index, snapshot, timing)
            }
        }))
        .buffer_unordered(self.config.decision.concurrency)
        .collect::<Vec<_>>()
        .await;
        let mut results = results;
        results.sort_by_key(|(index, _, _)| *index);
        let mut timings = Vec::new();
        let mut failures = 0;
        for (index, mut snapshot, timing) in results {
            let item = &mut ranked[index];
            if snapshot.status == JudgmentStatus::Failed {
                failures += 1;
                diagnostics.stage_errors.push(format!(
                    "Decision model {}: {}",
                    candidate_key(&item.issue),
                    snapshot.error.as_deref().unwrap_or("provider failed")
                ));
            }
            timings.push(timing);
            if let Some(evidence) = &snapshot.evidence {
                item.enriched_issue.source_fetched_at = evidence.source_fetched_at.clone();
            }
            if snapshot.status == JudgmentStatus::Pending {
                snapshot.status = JudgmentStatus::Failed;
            }
            item.enriched_issue.decision = Some(snapshot);
        }
        diagnostics.decision_execution = Some(decision::ExecutionReport {
            concurrency: self.config.decision.concurrency,
            peak_in_flight: peak.load(std::sync::atomic::Ordering::SeqCst),
            tasks: timings,
        });
        // Refresh every assessment, including budget-skipped entries, so saved
        // inputs replay the same uncertainty notes and frozen ranking clock.
        for item in ranked.iter_mut() {
            decision::factual_competition_only(&mut item.enriched_issue);
            item.value_assessment = assess_issue(&item.enriched_issue, &self.config.profile);
        }
        if let Some(Ok(provider)) = owned.as_ref() {
            provider.close().await;
        }
        if failures > 0 {
            diagnostics.stage_errors.push(format!("Decision model: {failures} candidates failed semantic screening; failures are isolated and no keyword fallback was used. {}", initialization_error.unwrap_or_default()));
        }
        if budget < eligible.len() {
            diagnostics.stage_errors.push(format!("Decision model: {} candidates were outside the {budget}-candidate budget and remain explicitly unassessed", eligible.len() - budget));
        }
        let partial = ranked
            .iter()
            .filter(|item| {
                item.enriched_issue
                    .decision
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.status == JudgmentStatus::Partial)
            })
            .count();
        if partial > 0 {
            diagnostics.stage_errors.push(format!(
                "Decision model: {partial} candidates have unanswered semantic questions"
            ));
        }
    }
}

fn availability_blocks_new_work(facts: &crate::availability::AvailabilitySnapshot) -> bool {
    facts.is_known_unavailable()
        || facts.has_strong_open_competition()
        || facts.has_merged_resolution_evidence()
}

fn facts_block_new_work(item: &RankedValueIssue) -> bool {
    item.enriched_issue
        .availability
        .as_ref()
        .is_some_and(availability_blocks_new_work)
}

fn semantic_pool_indices(pool: usize, budget: usize) -> Vec<usize> {
    let budget = budget.min(pool);
    if budget == 0 {
        return Vec::new();
    }
    if budget == pool {
        return (0..pool).collect();
    }
    let leading = (budget * 3 / 4).max(1);
    let mut selected = (0..leading).collect::<Vec<_>>();
    let rest = budget - leading;
    for index in 0..rest {
        selected.push(leading + (index + 1) * (pool - leading) / (rest + 1));
    }
    selected
}

fn append_discovery_reasons(
    ranked: &mut [RankedValueIssue],
    discovery_by_key: &mut HashMap<String, DiscoveryCandidate>,
) {
    canonicalize_discovery_keys(ranked, discovery_by_key);
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

fn canonicalize_discovery_keys(
    ranked: &[RankedValueIssue],
    discovery_by_key: &mut HashMap<String, DiscoveryCandidate>,
) {
    let additions = ranked
        .iter()
        .filter_map(|item| {
            let canonical_key = candidate_key(&item.issue);
            if discovery_by_key.contains_key(&canonical_key) {
                return None;
            }
            let (_, candidate) = discovery_by_key
                .iter()
                .find(|(_, candidate)| matches_canonicalized_candidate(candidate, item))?;
            let mut candidate = candidate.clone();
            candidate.issue = item.issue.clone();
            Some((canonical_key, candidate))
        })
        .collect::<Vec<_>>();

    discovery_by_key.extend(additions);
}

fn matches_canonicalized_candidate(
    candidate: &DiscoveryCandidate,
    item: &RankedValueIssue,
) -> bool {
    candidate.issue.number == item.issue.number
        && (candidate.issue.url == item.issue.url
            || (candidate.issue.repo_name == item.issue.repo_name
                && candidate.issue.title == item.issue.title))
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

fn canonicalize_ranked_issue(item: &mut RankedValueIssue) {
    canonicalize_github_issue_from_enriched(&mut item.issue, &item.enriched_issue);
}

fn canonicalize_ranked_issues(ranked: &mut [RankedValueIssue]) {
    for item in ranked {
        canonicalize_ranked_issue(item);
    }
}

fn canonicalize_github_issue_from_enriched(issue: &mut GitHubIssue, enriched: &EnrichedIssue) {
    let canonical = enriched.issue.repo_full_name.trim();
    if canonical.is_empty() || issue.repo_full_name == canonical {
        return;
    }

    let repo_name = canonical
        .split('/')
        .nth(1)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| issue.repo_name.clone());

    issue.repo_full_name = canonical.to_string();
    issue.repo_name = repo_name;
    issue.url = enriched.issue.url.clone();
    issue.repo_description = enriched.repository.description.clone();
    issue.repo_stars = enriched.repository.stars;
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
    feedback_policy: FeedbackPolicy,
) -> String {
    let identity = serde_json::to_vec(&(scope, profile, limit, include_filtered, feedback_policy))
        .expect("scout cache identity contains only serializable values");
    format!("scout-v2-{:x}", Sha256::digest(identity))
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
