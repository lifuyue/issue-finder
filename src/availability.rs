//! Fresh GitHub facts shared by scout and assess. Missing evidence stays unknown.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AvailabilityDepth {
    Initial,
    Final,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    #[default]
    NotChecked,
    Complete,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceCoverage {
    pub status: CoverageStatus,
    pub pages_fetched: usize,
    pub items_fetched: usize,
    pub has_more: Option<bool>,
    pub queries: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestRelation {
    ExplicitResolution,
    Mention,
    SearchLead,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullRequestEvidence {
    pub repo_full_name: String,
    pub number: u64,
    pub url: Option<String>,
    pub state: Option<String>,
    pub draft: Option<bool>,
    pub merged: Option<bool>,
    pub merged_at: Option<String>,
    pub base_branch: Option<String>,
    pub relation: PullRequestRelation,
    pub sources: Vec<String>,
    pub verified: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AvailabilitySnapshot {
    pub checked_at: String,
    #[serde(default)]
    pub repo_full_name: String,
    pub depth: AvailabilityDepth,
    pub issue_state: Option<String>,
    pub assignees: Option<Vec<String>>,
    pub archived: Option<bool>,
    pub locked: Option<bool>,
    pub default_branch: Option<String>,
    pub linked_coverage: EvidenceCoverage,
    pub search_coverage: EvidenceCoverage,
    pub pull_requests: Vec<PullRequestEvidence>,
    pub uncertainties: Vec<String>,
}

impl AvailabilitySnapshot {
    pub fn new(depth: AvailabilityDepth) -> Self {
        Self {
            checked_at: chrono::Utc::now().to_rfc3339(),
            repo_full_name: String::new(),
            depth,
            issue_state: None,
            assignees: None,
            archived: None,
            locked: None,
            default_branch: None,
            linked_coverage: EvidenceCoverage::default(),
            search_coverage: EvidenceCoverage::default(),
            pull_requests: Vec::new(),
            uncertainties: Vec::new(),
        }
    }

    pub fn is_known_unavailable(&self) -> bool {
        self.issue_state
            .as_deref()
            .is_some_and(|state| state.eq_ignore_ascii_case("closed"))
            || self.archived == Some(true)
            || self.locked == Some(true)
            || self
                .assignees
                .as_ref()
                .is_some_and(|users| !users.is_empty())
    }

    pub fn has_strong_open_competition(&self) -> bool {
        self.pull_requests.iter().any(|pr| {
            pr.verified
                && pr.relation == PullRequestRelation::ExplicitResolution
                && pr.state.as_deref() == Some("open")
                && pr.merged == Some(false)
        })
    }

    /// A merge and an explicit issue reference warrant checking the current code;
    /// they do not prove the reported behavior is fixed.
    pub fn has_merged_resolution_evidence(&self) -> bool {
        self.pull_requests.iter().any(|pr| {
            pr.verified
                && pr.relation == PullRequestRelation::ExplicitResolution
                && pr.merged == Some(true)
                && pr.merged_at.is_some()
                && pr.repo_full_name.eq_ignore_ascii_case(&self.repo_full_name)
                && pr
                    .base_branch
                    .as_ref()
                    .zip(self.default_branch.as_ref())
                    .is_some_and(|(base, default)| base == default)
        })
    }

    pub fn checks_complete(&self) -> bool {
        matches!(self.issue_state.as_deref(), Some("open" | "closed"))
            && self.assignees.is_some()
            && self.archived.is_some()
            && self.locked.is_some()
            && self.linked_coverage.status == CoverageStatus::Complete
            && self
                .pull_requests
                .iter()
                .all(|pr| pr.verified && pr.errors.is_empty())
            && (self.depth == AvailabilityDepth::Initial
                || self.search_coverage.status == CoverageStatus::Complete)
    }

    pub fn fresh_fix_available(&self) -> bool {
        self.checks_complete()
            && !self.is_known_unavailable()
            && !self.has_strong_open_competition()
            && !self.has_merged_resolution_evidence()
            && !self.pull_requests.iter().any(|pr| {
                // Mentions and search hits can be unrelated, but require review;
                // neither proves overlap nor clears it. Branch-specific merges
                // likewise need checking before starting another implementation.
                pr.state.as_deref() == Some("open") || pr.merged == Some(true)
            })
    }
}
