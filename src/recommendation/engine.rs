use anyhow::Result;

use crate::config::Config;
use crate::github::{GitHubClient, GitHubIssue};
use crate::github_enrichment::GitHubEnrichmentClient;
use crate::paths::IssueFinderPaths;
use crate::value_scoring::{assess_issue, RankedValueIssue};

use super::candidate_ranker::rank_candidates;
use super::events::{record_event_for_issue, RecommendationEventSource, RecommendationEventType};
use super::feed_ranker::{apply_recommendation_assessments, displayable, sort_by_feed};
use super::state::load_state_map;

const ENRICHED_SCOUT_CANDIDATE_LIMIT: usize = 40;
const COMPETITION_TIMELINE_CANDIDATE_LIMIT: usize = 20;

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

#[derive(Debug, Clone)]
pub struct ScoutResult {
    pub ranked: Vec<RankedValueIssue>,
    pub discovery_count: usize,
    pub filtered_count: usize,
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
    ) -> Result<ScoutResult> {
        self.paths.ensure_layout()?;
        let github = GitHubClient::new(self.config)?;
        let enrichment = GitHubEnrichmentClient::new(self.config)?;
        let issues = github.discover_issues(self.paths, refresh).await?;
        let discovery_count = issues.len();
        let ranked = self
            .rank_discovered_issues(&enrichment, issues, limit, refresh)
            .await;
        let filtered_count = ranked
            .iter()
            .filter(|item| {
                item.recommendation.visibility
                    == super::model::RecommendationVisibility::HiddenFiltered
            })
            .count();
        let visible = ranked
            .into_iter()
            .filter(|item| displayable(item, options.include_filtered))
            .take(limit)
            .collect::<Vec<_>>();

        if options.record_exposure {
            self.record_exposure(&visible, options.source)?;
        }

        Ok(ScoutResult {
            ranked: visible,
            discovery_count,
            filtered_count,
        })
    }

    pub async fn daily_candidates(
        &self,
        refresh: bool,
        candidate_limit: usize,
    ) -> Result<ScoutResult> {
        self.scout(
            candidate_limit,
            refresh,
            ScoutOptions {
                include_filtered: false,
                record_exposure: false,
                source: RecommendationEventSource::Daily,
            },
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
                    "baseCategory": item.recommendation.base_category.to_string()
                }),
            )?;
        }
        Ok(())
    }

    async fn rank_discovered_issues(
        &self,
        enrichment: &GitHubEnrichmentClient,
        issues: Vec<GitHubIssue>,
        limit: usize,
        refresh: bool,
    ) -> Vec<RankedValueIssue> {
        let mut rough = rank_candidates(issues, &self.config.profile);
        rough.truncate(limit.clamp(25, ENRICHED_SCOUT_CANDIDATE_LIMIT));

        let mut ranked = Vec::new();
        for (index, rough_issue) in rough.into_iter().enumerate() {
            if let Ok(item) = self
                .rank_single_issue(
                    enrichment,
                    rough_issue.issue,
                    refresh,
                    index < COMPETITION_TIMELINE_CANDIDATE_LIMIT,
                )
                .await
            {
                ranked.push(item);
            }
        }
        self.apply_feed_ranking(&mut ranked);
        ranked
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
