use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_TOTAL_REQUEST_BUDGET: usize = 1_200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitHubRequestSource {
    DiscoveryOverlay,
    DiscoveryProfileTrusted,
    DiscoveryGfiTrusted,
    DiscoveryGlobal,
    DiscoveryFallbackTrusted,
    DiscoveryFallbackGlobal,
    DiscoveryRepository,
    EnrichmentRepoMetadata,
    EnrichmentIssueDetails,
    EnrichmentComments,
    EnrichmentTimeline,
    EnrichmentGrowth,
    CompetitionCompletionComments,
    CompetitionCompletionTimeline,
    ScoutResult,
    DirectIssue,
    ValidateToken,
}

impl GitHubRequestSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DiscoveryOverlay => "discovery_overlay",
            Self::DiscoveryProfileTrusted => "discovery_profile_trusted",
            Self::DiscoveryGfiTrusted => "discovery_gfi_trusted",
            Self::DiscoveryGlobal => "discovery_global",
            Self::DiscoveryFallbackTrusted => "discovery_fallback_trusted",
            Self::DiscoveryFallbackGlobal => "discovery_fallback_global",
            Self::DiscoveryRepository => "discovery_repository",
            Self::EnrichmentRepoMetadata => "enrichment_repo_metadata",
            Self::EnrichmentIssueDetails => "enrichment_issue_details",
            Self::EnrichmentComments => "enrichment_comments",
            Self::EnrichmentTimeline => "enrichment_timeline",
            Self::EnrichmentGrowth => "enrichment_growth",
            Self::CompetitionCompletionComments => "competition_completion_comments",
            Self::CompetitionCompletionTimeline => "competition_completion_timeline",
            Self::ScoutResult => "scout_result",
            Self::DirectIssue => "direct_issue",
            Self::ValidateToken => "validate_token",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubApiBudgetReport {
    pub total_budget: Option<usize>,
    pub total_network_requests: usize,
    pub network_requests: BTreeMap<String, usize>,
    pub cache_hits: BTreeMap<String, usize>,
    pub budget_exhausted: BTreeMap<String, usize>,
    pub events: Vec<String>,
}

impl Default for GitHubApiBudgetReport {
    fn default() -> Self {
        Self {
            total_budget: Some(DEFAULT_TOTAL_REQUEST_BUDGET),
            total_network_requests: 0,
            network_requests: BTreeMap::new(),
            cache_hits: BTreeMap::new(),
            budget_exhausted: BTreeMap::new(),
            events: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GitHubApiBudget {
    inner: Arc<Mutex<GitHubApiBudgetReport>>,
}

impl GitHubApiBudget {
    pub fn from_env() -> Self {
        let total_budget = std::env::var("ISSUE_FINDER_GITHUB_API_BUDGET_TOTAL")
            .ok()
            .and_then(|value| parse_total_budget(&value))
            .unwrap_or(Some(DEFAULT_TOTAL_REQUEST_BUDGET));
        Self::with_total_budget(total_budget)
    }

    pub fn with_total_budget(total_budget: Option<usize>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(GitHubApiBudgetReport {
                total_budget,
                ..GitHubApiBudgetReport::default()
            })),
        }
    }

    pub fn record_cache_hit(&self, source: GitHubRequestSource) {
        let mut report = self.inner.lock().expect("GitHub API budget mutex poisoned");
        *report
            .cache_hits
            .entry(source.as_str().to_string())
            .or_insert(0) += 1;
    }

    pub fn record_network_request(
        &self,
        source: GitHubRequestSource,
        detail: impl AsRef<str>,
    ) -> Result<(), GitHubApiBudgetExceeded> {
        let mut report = self.inner.lock().expect("GitHub API budget mutex poisoned");
        if report
            .total_budget
            .is_some_and(|limit| report.total_network_requests >= limit)
        {
            let source_name = source.as_str().to_string();
            *report
                .budget_exhausted
                .entry(source_name.clone())
                .or_insert(0) += 1;
            let detail = detail.as_ref().to_string();
            report
                .events
                .push(format!("budget exhausted before {source_name}: {detail}"));
            return Err(GitHubApiBudgetExceeded {
                source_name,
                detail,
            });
        }

        report.total_network_requests += 1;
        *report
            .network_requests
            .entry(source.as_str().to_string())
            .or_insert(0) += 1;
        Ok(())
    }

    pub fn report(&self) -> GitHubApiBudgetReport {
        self.inner
            .lock()
            .expect("GitHub API budget mutex poisoned")
            .clone()
    }
}

impl Default for GitHubApiBudget {
    fn default() -> Self {
        Self::from_env()
    }
}

#[derive(Debug, Error)]
#[error("GitHub API budget exhausted for {source_name}: {detail}")]
pub struct GitHubApiBudgetExceeded {
    pub source_name: String,
    pub detail: String,
}

fn parse_total_budget(value: &str) -> Option<Option<usize>> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("none") || value.eq_ignore_ascii_case("unlimited") {
        return Some(None);
    }
    value.parse::<usize>().ok().map(Some)
}

#[cfg(test)]
mod tests {
    use super::{GitHubApiBudget, GitHubRequestSource};

    #[test]
    fn budget_exhaustion_is_reported_by_source() {
        let budget = GitHubApiBudget::with_total_budget(Some(1));

        budget
            .record_network_request(GitHubRequestSource::EnrichmentTimeline, "first")
            .unwrap();
        let err = budget
            .record_network_request(GitHubRequestSource::EnrichmentTimeline, "second")
            .unwrap_err();

        assert!(err.to_string().contains("enrichment_timeline"));
        let report = budget.report();
        assert_eq!(report.total_network_requests, 1);
        assert_eq!(report.budget_exhausted["enrichment_timeline"], 1);
    }
}
