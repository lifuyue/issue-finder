use std::collections::{HashMap, HashSet};
use std::fmt;

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::ProfileConfig;
use crate::github::GitHubIssue;
use crate::scoring::{has_actionable_signal, normalize, profile_terms};

const GFI_REPOSITORIES: &str = include_str!("../data/discovery/good-first-issue-repositories.toml");
const OVERLAY_REPOSITORIES: &str =
    include_str!("../data/discovery/trusted-overlay-repositories.toml");
const PROFILE_TRUSTED_REPOSITORIES: &str =
    include_str!("../data/discovery/profile-trusted-repositories.toml");

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiscoveryScope {
    #[default]
    Global,
    Repository {
        repository: RepositoryScope,
    },
}

impl DiscoveryScope {
    pub fn repository(repository: RepositoryScope) -> Self {
        Self::Repository { repository }
    }

    pub fn cache_fragment(&self) -> String {
        match self {
            Self::Global => "scope-global".to_string(),
            Self::Repository { repository } => {
                format!("scope-repository-{}", repository.sanitized_cache_key())
            }
        }
    }

    pub fn diagnostics(&self) -> DiscoveryDiagnostics {
        match self {
            Self::Global => DiscoveryDiagnostics {
                scope: "global".to_string(),
                repository: None,
                discovery_stages: Vec::new(),
                stage_errors: Vec::new(),
                fallback_exhausted: false,
            },
            Self::Repository { repository } => DiscoveryDiagnostics {
                scope: "repository".to_string(),
                repository: Some(repository.full_name()),
                discovery_stages: Vec::new(),
                stage_errors: Vec::new(),
                fallback_exhausted: false,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct RepositoryScope {
    pub owner: String,
    pub repo: String,
}

impl RepositoryScope {
    pub fn parse(value: &str) -> Result<Self> {
        let value = value.trim();
        if value.starts_with("http://") || value.starts_with("https://") {
            return Self::parse_url(value);
        }

        let (owner, repo) = value
            .split_once('/')
            .ok_or_else(|| anyhow::anyhow!(REPOSITORY_SCOPE_ERROR))?;
        let scope = Self {
            owner: owner.trim().to_string(),
            repo: repo.trim().to_string(),
        };
        scope.validate()?;
        Ok(scope)
    }

    fn parse_url(value: &str) -> Result<Self> {
        let url = Url::parse(value).map_err(|_| anyhow::anyhow!(REPOSITORY_SCOPE_ERROR))?;
        if url.host_str() != Some("github.com") {
            anyhow::bail!(REPOSITORY_SCOPE_ERROR);
        }

        let segments = url
            .path_segments()
            .ok_or_else(|| anyhow::anyhow!(REPOSITORY_SCOPE_ERROR))?
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>();
        if segments.len() != 2 {
            anyhow::bail!(REPOSITORY_SCOPE_ERROR);
        }

        let scope = Self {
            owner: segments[0].to_string(),
            repo: segments[1].trim_end_matches(".git").to_string(),
        };
        scope.validate()?;
        Ok(scope)
    }

    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    pub fn sanitized_cache_key(&self) -> String {
        self.full_name()
            .chars()
            .map(|character| {
                if character == '/' {
                    "__".to_string()
                } else if character.is_ascii_alphanumeric() || character == '_' {
                    character.to_string()
                } else {
                    "_".to_string()
                }
            })
            .collect::<String>()
    }

    fn validate(&self) -> Result<()> {
        if self.owner.trim().is_empty()
            || self.repo.trim().is_empty()
            || self.owner.contains('/')
            || self.repo.contains('/')
            || self.repo.contains('#')
        {
            anyhow::bail!(REPOSITORY_SCOPE_ERROR);
        }
        Ok(())
    }
}

impl fmt::Display for RepositoryScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.full_name())
    }
}

const REPOSITORY_SCOPE_ERROR: &str = "expected owner/repo or https://github.com/owner/repo";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryDiagnostics {
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    pub discovery_stages: Vec<DiscoveryStageStats>,
    pub stage_errors: Vec<String>,
    pub fallback_exhausted: bool,
}

impl DiscoveryDiagnostics {
    pub fn merge(&mut self, mut other: DiscoveryDiagnostics) {
        self.discovery_stages.append(&mut other.discovery_stages);
        self.stage_errors.append(&mut other.stage_errors);
        self.fallback_exhausted |= other.fallback_exhausted;
    }

    pub fn mark_ranked_and_visible(
        &mut self,
        ranked_keys_by_lane: &HashMap<String, HashSet<String>>,
        visible_keys: &HashSet<String>,
    ) {
        for stage in &mut self.discovery_stages {
            let Some(ranked_keys) = ranked_keys_by_lane.get(&stage.lane) else {
                continue;
            };
            stage.ranked = ranked_keys.len();
            stage.visible = ranked_keys.intersection(visible_keys).count();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryStageStats {
    pub stage: String,
    pub lane: String,
    pub requested: usize,
    pub returned: usize,
    pub deduped: usize,
    pub ranked: usize,
    pub visible: usize,
}

impl DiscoveryStageStats {
    pub fn new(
        stage: impl Into<String>,
        lane: impl Into<String>,
        requested: usize,
        returned: usize,
        deduped: usize,
    ) -> Self {
        Self {
            stage: stage.into(),
            lane: lane.into(),
            requested,
            returned,
            deduped,
            ranked: 0,
            visible: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryOutput {
    pub candidates: Vec<DiscoveryCandidate>,
    pub diagnostics: DiscoveryDiagnostics,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoveryCandidate {
    pub issue: GitHubIssue,
    pub source_lanes: Vec<String>,
    pub trust_tier: RepoTrustTier,
    pub matched_labels: Vec<String>,
    pub rough_score: i32,
    pub first_seen_source: String,
}

impl DiscoveryCandidate {
    pub fn new(
        issue: GitHubIssue,
        source_lane: impl Into<String>,
        trust_tier: RepoTrustTier,
        profile: &ProfileConfig,
    ) -> Self {
        let source_lane = source_lane.into();
        let matched_labels = issue.labels.clone();
        let mut candidate = Self {
            issue,
            source_lanes: vec![source_lane.clone()],
            trust_tier,
            matched_labels,
            rough_score: 0,
            first_seen_source: source_lane,
        };
        candidate.recompute_score(profile);
        candidate
    }

    pub fn key(&self) -> String {
        format!("{}#{}", self.issue.repo_full_name, self.issue.number)
    }

    pub fn merge(&mut self, other: DiscoveryCandidate, profile: &ProfileConfig) {
        merge_unique(&mut self.source_lanes, other.source_lanes);
        merge_unique(&mut self.matched_labels, other.matched_labels);
        if other.trust_tier.rank() > self.trust_tier.rank() {
            self.trust_tier = other.trust_tier;
        }
        self.recompute_score(profile);
    }

    pub fn recompute_score(&mut self, profile: &ProfileConfig) {
        self.rough_score = score_discovery_candidate(self, profile);
    }

    pub fn discovery_reasons(&self) -> Vec<String> {
        vec![
            format!(
                "discovery trust `{}` from {}",
                self.trust_tier,
                self.source_lanes.join(", ")
            ),
            format!("discovery rough score {}", self.rough_score),
        ]
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepoTrustTier {
    Global,
    GfiTrusted,
    ProfileTrusted,
    OverlayTrusted,
}

impl RepoTrustTier {
    pub fn rank(self) -> u8 {
        match self {
            Self::Global => 0,
            Self::GfiTrusted => 1,
            Self::ProfileTrusted => 2,
            Self::OverlayTrusted => 3,
        }
    }

    pub fn repo_quota(self) -> usize {
        match self {
            Self::OverlayTrusted => 5,
            Self::ProfileTrusted => 4,
            Self::GfiTrusted => 3,
            Self::Global => 2,
        }
    }
}

impl std::fmt::Display for RepoTrustTier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Global => "global",
            Self::GfiTrusted => "gfi_trusted",
            Self::ProfileTrusted => "profile_trusted",
            Self::OverlayTrusted => "overlay_trusted",
        };
        formatter.write_str(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedRepository {
    pub owner: String,
    pub name: String,
}

impl TrustedRepository {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

#[derive(Debug, Deserialize)]
struct RepositoryList {
    repositories: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ProfileRepositoryList {
    buckets: Vec<ProfileRepositoryBucket>,
}

#[derive(Debug, Deserialize)]
struct ProfileRepositoryBucket {
    profile: String,
    repositories: Vec<String>,
}

pub fn overlay_repositories() -> Result<Vec<TrustedRepository>> {
    parse_repository_list(OVERLAY_REPOSITORIES)
}

pub fn gfi_repositories(profile: &ProfileConfig, limit: usize) -> Result<Vec<TrustedRepository>> {
    let repositories = parse_repository_list(GFI_REPOSITORIES)?;
    Ok(select_gfi_repositories(repositories, profile, limit))
}

pub fn profile_trusted_repositories(
    profile: &ProfileConfig,
    limit: usize,
) -> Result<Vec<TrustedRepository>> {
    let bucket_id = profile_bucket_id(profile);
    let list = toml::from_str::<ProfileRepositoryList>(PROFILE_TRUSTED_REPOSITORIES)
        .context("unable to parse profile trusted repository list")?;
    let mut seen = HashSet::new();
    let mut repositories = Vec::new();
    for bucket in list.buckets {
        if bucket.profile != bucket_id {
            continue;
        }
        for item in bucket.repositories {
            if let Some(repository) = parse_repository(&item) {
                let key = repository.full_name();
                if seen.insert(key) {
                    repositories.push(repository);
                }
            }
        }
    }
    repositories.truncate(limit);
    Ok(repositories)
}

pub fn select_enrichment_candidates(
    mut candidates: Vec<DiscoveryCandidate>,
    max_budget: usize,
) -> Vec<DiscoveryCandidate> {
    if max_budget == 0 {
        return Vec::new();
    }

    sort_candidates(&mut candidates);

    let overlay_target = max_budget * 15 / 100;
    let profile_target = max_budget * 35 / 100;
    let gfi_target = max_budget * 30 / 100;
    let global_target = max_budget.saturating_sub(overlay_target + profile_target + gfi_target);

    let mut selected = Vec::new();
    let mut selected_keys = HashSet::new();
    let mut repo_counts = HashMap::<String, usize>::new();

    let overlay_taken = take_candidates(
        &candidates,
        &mut selected,
        &mut selected_keys,
        &mut repo_counts,
        Some(RepoTrustTier::OverlayTrusted),
        overlay_target,
    );
    let overlay_shortfall = overlay_target.saturating_sub(overlay_taken);

    let profile_taken = take_candidates(
        &candidates,
        &mut selected,
        &mut selected_keys,
        &mut repo_counts,
        Some(RepoTrustTier::ProfileTrusted),
        profile_target + overlay_shortfall,
    );
    let profile_shortfall = (profile_target + overlay_shortfall).saturating_sub(profile_taken);

    take_candidates(
        &candidates,
        &mut selected,
        &mut selected_keys,
        &mut repo_counts,
        Some(RepoTrustTier::GfiTrusted),
        gfi_target + profile_shortfall,
    );

    take_candidates(
        &candidates,
        &mut selected,
        &mut selected_keys,
        &mut repo_counts,
        Some(RepoTrustTier::Global),
        global_target,
    );

    if selected.len() < max_budget {
        let remaining_budget = max_budget - selected.len();
        take_candidates(
            &candidates,
            &mut selected,
            &mut selected_keys,
            &mut repo_counts,
            None,
            remaining_budget,
        );
    }

    sort_candidates(&mut selected);
    selected.truncate(max_budget);
    selected
}

pub fn merge_candidates(
    candidates: Vec<DiscoveryCandidate>,
    profile: &ProfileConfig,
) -> Vec<DiscoveryCandidate> {
    let mut merged = HashMap::<String, DiscoveryCandidate>::new();
    for candidate in candidates {
        let key = candidate.key();
        if let Some(existing) = merged.get_mut(&key) {
            existing.merge(candidate, profile);
        } else {
            merged.insert(key, candidate);
        }
    }
    let mut candidates = merged.into_values().collect::<Vec<_>>();
    sort_candidates(&mut candidates);
    candidates
}

pub fn sort_candidates(candidates: &mut [DiscoveryCandidate]) {
    candidates.sort_by(|left, right| {
        right
            .trust_tier
            .rank()
            .cmp(&left.trust_tier.rank())
            .then_with(|| right.rough_score.cmp(&left.rough_score))
            .then_with(|| right.issue.updated_at.cmp(&left.issue.updated_at))
            .then_with(|| left.issue.repo_full_name.cmp(&right.issue.repo_full_name))
            .then_with(|| left.issue.number.cmp(&right.issue.number))
    });
}

fn take_candidates(
    candidates: &[DiscoveryCandidate],
    selected: &mut Vec<DiscoveryCandidate>,
    selected_keys: &mut HashSet<String>,
    repo_counts: &mut HashMap<String, usize>,
    tier: Option<RepoTrustTier>,
    limit: usize,
) -> usize {
    if limit == 0 {
        return 0;
    }

    let mut taken = 0;
    for candidate in candidates {
        if taken == limit {
            break;
        }
        if tier.is_some_and(|tier| candidate.trust_tier != tier) {
            continue;
        }
        let key = candidate.key();
        if selected_keys.contains(&key) {
            continue;
        }

        let repo = candidate.issue.repo_full_name.clone();
        let count = *repo_counts.get(&repo).unwrap_or(&0);
        if count >= candidate.trust_tier.repo_quota() {
            continue;
        }

        selected_keys.insert(key);
        repo_counts.insert(repo, count + 1);
        selected.push(candidate.clone());
        taken += 1;
    }
    taken
}

fn score_discovery_candidate(candidate: &DiscoveryCandidate, profile: &ProfileConfig) -> i32 {
    let issue = &candidate.issue;
    let terms = profile_terms(profile);
    let title = normalize(&issue.title);
    let repo = normalize(&format!(
        "{} {} {}",
        issue.repo_full_name, issue.repo_name, issue.repo_description
    ));
    let labels = normalize(&issue.labels.join(" "));
    let text = normalize(&format!("{}\n{}", issue.title, issue.body));

    let mut score = match candidate.trust_tier {
        RepoTrustTier::OverlayTrusted => 40,
        RepoTrustTier::ProfileTrusted => 36,
        RepoTrustTier::GfiTrusted => 30,
        RepoTrustTier::Global => 0,
    };

    if labels.contains("good first issue") {
        score += 18;
    }
    if contains_any(
        &labels,
        &["beginner", "beginner friendly", "easy", "starter"],
    ) {
        score += 12;
    }
    if labels.contains("help wanted")
        && (candidate.trust_tier != RepoTrustTier::Global
            || has_actionable_signal(&issue.title, &issue.body))
    {
        score += 6;
    }

    for term in terms {
        if title.contains(&term) {
            score += 18;
        } else if repo.contains(&term) || labels.contains(&term) {
            score += 8;
        }
    }

    if has_actionable_signal(&issue.title, &issue.body) {
        score += 15;
    }
    if issue.body.trim().len() >= 120 {
        score += 6;
    }
    score += freshness_boost(&issue.updated_at);
    score += (((issue.repo_stars + 10) as f64).log10() * 5.0)
        .round()
        .min(12.0) as i32;

    if contains_any(
        &text,
        &[
            "docs only",
            "documentation only",
            "typo",
            "comment wording",
            "copy edit",
            "translate",
            "translation",
        ],
    ) {
        score -= 20;
    }
    if contains_any(
        &text,
        &[
            "bounty",
            "campaign",
            "audit all",
            "find all",
            "list of",
            "write notes",
            "add your project",
        ],
    ) {
        score -= 30;
    }

    score.clamp(0, 200)
}

fn parse_repository_list(raw: &str) -> Result<Vec<TrustedRepository>> {
    let list = toml::from_str::<RepositoryList>(raw).context("unable to parse repository list")?;
    let mut seen = HashSet::new();
    let mut repositories = Vec::new();
    for item in list.repositories {
        if let Some(repository) = parse_repository(&item) {
            let key = repository.full_name();
            if seen.insert(key) {
                repositories.push(repository);
            }
        }
    }
    Ok(repositories)
}

fn parse_repository(value: &str) -> Option<TrustedRepository> {
    let value = value
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("github.com/")
        .trim_end_matches('/');
    let mut parts = value.split('/');
    let owner = parts.next()?.trim();
    let name = parts.next()?.trim();
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(TrustedRepository {
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

fn select_gfi_repositories(
    mut repositories: Vec<TrustedRepository>,
    profile: &ProfileConfig,
    limit: usize,
) -> Vec<TrustedRepository> {
    let terms = profile_terms(profile);
    repositories.sort_by(|left, right| {
        repository_priority(right, &terms)
            .cmp(&repository_priority(left, &terms))
            .then_with(|| left.full_name().cmp(&right.full_name()))
    });

    let high_priority_count = repositories
        .iter()
        .take_while(|repo| repository_priority(repo, &terms) > 0)
        .count();
    if high_priority_count < repositories.len() {
        let rotation_seed = Utc::now().date_naive().num_days_from_ce() as usize;
        let remainder = &mut repositories[high_priority_count..];
        if !remainder.is_empty() {
            let rotate_by = rotation_seed % remainder.len();
            remainder.rotate_left(rotate_by);
        }
    }

    repositories.truncate(limit);
    repositories
}

fn profile_bucket_id(profile: &ProfileConfig) -> &'static str {
    let terms = profile_terms(profile);
    let has = |needle: &str| terms.iter().any(|term| term == needle);

    if has("kubernetes") || has("docker") || has("ci") || has("infrastructure") {
        return "devops_infra";
    }
    if has("ai") || has("llm") || has("agent") {
        return "ai_agent_tools";
    }
    if has("python") && (has("data") || has("pandas") || has("testing")) {
        return "python_data_cli";
    }
    if (has("rust") && has("go")) || has("backend") || has("compiler") || has("cargo") {
        return "rust_backend_systems";
    }
    if has("frontend") || has("react") || has("ui") || has("browser") {
        return "typescript_frontend";
    }
    "default_cli_devtools"
}

fn repository_priority(repository: &TrustedRepository, terms: &[String]) -> i32 {
    let full_name = normalize(&repository.full_name());
    let mut score = 0;
    for term in terms {
        if full_name.contains(term) {
            score += 10;
        }
    }

    if matches!(
        repository.full_name().as_str(),
        "aws/aws-cdk" | "medusajs/medusa" | "astral-sh/uv" | "arduino/arduino-cli"
    ) {
        score += 25;
    }

    score
}

fn freshness_boost(updated_at: &str) -> i32 {
    let Ok(updated_at) = DateTime::parse_from_rfc3339(updated_at) else {
        return 0;
    };
    let age_hours = (Utc::now() - updated_at.with_timezone(&Utc)).num_hours();
    match age_hours {
        value if value <= 24 => 8,
        value if value <= 72 => 7,
        value if value <= 24 * 7 => 6,
        value if value <= 24 * 14 => 3,
        _ => 0,
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn merge_unique(values: &mut Vec<String>, incoming: Vec<String>) {
    let mut seen = values.iter().cloned().collect::<HashSet<_>>();
    for value in incoming {
        if seen.insert(value.clone()) {
            values.push(value);
        }
    }
    values.sort();
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{
        merge_candidates, profile_trusted_repositories, select_enrichment_candidates,
        DiscoveryCandidate, RepoTrustTier,
    };
    use crate::config::ProfileConfig;
    use crate::github::GitHubIssue;

    #[test]
    fn merge_preserves_multiple_discovery_sources() {
        let profile = profile();
        let first = DiscoveryCandidate::new(
            issue("owner/repo", 1, "Fix Rust CLI bug"),
            "gfi:owner/repo",
            RepoTrustTier::GfiTrusted,
            &profile,
        );
        let second = DiscoveryCandidate::new(
            issue("owner/repo", 1, "Fix Rust CLI bug"),
            "global:good-first-issue",
            RepoTrustTier::Global,
            &profile,
        );

        let merged = merge_candidates(vec![first, second], &profile);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].trust_tier, RepoTrustTier::GfiTrusted);
        assert!(merged[0]
            .source_lanes
            .contains(&"gfi:owner/repo".to_string()));
        assert!(merged[0]
            .source_lanes
            .contains(&"global:good-first-issue".to_string()));
    }

    #[test]
    fn enrichment_selection_reserves_gfi_budget_when_no_profile_trusted_candidates_exist() {
        let profile = profile();
        let mut candidates = Vec::new();
        for index in 0..10 {
            candidates.push(DiscoveryCandidate::new(
                issue(&format!("overlay/repo-{index}"), 1, "Fix Rust CLI bug"),
                format!("overlay:{index}"),
                RepoTrustTier::OverlayTrusted,
                &profile,
            ));
        }
        for index in 0..70 {
            candidates.push(DiscoveryCandidate::new(
                issue(&format!("gfi/repo-{index}"), 1, "Fix Rust CLI bug"),
                format!("gfi:{index}"),
                RepoTrustTier::GfiTrusted,
                &profile,
            ));
        }
        for index in 0..70 {
            candidates.push(DiscoveryCandidate::new(
                issue(&format!("global/repo-{index}"), 1, "Fix Rust CLI bug"),
                format!("global:{index}"),
                RepoTrustTier::Global,
                &profile,
            ));
        }

        let selected = select_enrichment_candidates(candidates, 100);
        let gfi_count = selected
            .iter()
            .filter(|candidate| candidate.trust_tier == RepoTrustTier::GfiTrusted)
            .count();
        let global_count = selected
            .iter()
            .filter(|candidate| candidate.trust_tier == RepoTrustTier::Global)
            .count();

        assert!(gfi_count >= 50, "{gfi_count}");
        assert!(global_count <= 20, "{global_count}");
    }

    #[test]
    fn enrichment_selection_prioritizes_profile_trusted_budget() {
        let profile = profile();
        let mut candidates = Vec::new();
        for index in 0..10 {
            candidates.push(DiscoveryCandidate::new(
                issue(&format!("overlay/repo-{index}"), 1, "Fix Rust CLI bug"),
                format!("overlay:{index}"),
                RepoTrustTier::OverlayTrusted,
                &profile,
            ));
        }
        for index in 0..70 {
            candidates.push(DiscoveryCandidate::new(
                issue(&format!("profile/repo-{index}"), 1, "Fix Rust CLI bug"),
                format!("profile:{index}"),
                RepoTrustTier::ProfileTrusted,
                &profile,
            ));
        }
        for index in 0..70 {
            candidates.push(DiscoveryCandidate::new(
                issue(&format!("gfi/repo-{index}"), 1, "Fix Rust CLI bug"),
                format!("gfi:{index}"),
                RepoTrustTier::GfiTrusted,
                &profile,
            ));
        }
        for index in 0..70 {
            candidates.push(DiscoveryCandidate::new(
                issue(&format!("global/repo-{index}"), 1, "Fix Rust CLI bug"),
                format!("global:{index}"),
                RepoTrustTier::Global,
                &profile,
            ));
        }

        let selected = select_enrichment_candidates(candidates, 100);
        let profile_count = selected
            .iter()
            .filter(|candidate| candidate.trust_tier == RepoTrustTier::ProfileTrusted)
            .count();
        let gfi_count = selected
            .iter()
            .filter(|candidate| candidate.trust_tier == RepoTrustTier::GfiTrusted)
            .count();
        let global_count = selected
            .iter()
            .filter(|candidate| candidate.trust_tier == RepoTrustTier::Global)
            .count();

        assert!(profile_count >= 35, "{profile_count}");
        assert!(gfi_count >= 30, "{gfi_count}");
        assert!(global_count <= 20, "{global_count}");
    }

    #[test]
    fn enrichment_selection_applies_repo_diversity_quota() {
        let profile = profile();
        let mut candidates = Vec::new();
        for number in 1..=10 {
            candidates.push(DiscoveryCandidate::new(
                issue("gfi/shared", number, "Fix Rust CLI bug"),
                format!("gfi:{number}"),
                RepoTrustTier::GfiTrusted,
                &profile,
            ));
        }
        for index in 0..10 {
            candidates.push(DiscoveryCandidate::new(
                issue(&format!("gfi/unique-{index}"), 1, "Fix Rust CLI bug"),
                format!("gfi:unique:{index}"),
                RepoTrustTier::GfiTrusted,
                &profile,
            ));
        }

        let selected = select_enrichment_candidates(candidates, 20);
        let shared_count = selected
            .iter()
            .filter(|candidate| candidate.issue.repo_full_name == "gfi/shared")
            .count();

        assert_eq!(shared_count, RepoTrustTier::GfiTrusted.repo_quota());
    }

    #[test]
    fn weak_global_help_wanted_does_not_outrank_gfi_candidate() {
        let profile = profile();
        let global = DiscoveryCandidate::new(
            GitHubIssue {
                labels: vec!["help wanted".to_string()],
                body: "Some help wanted.".to_string(),
                ..issue("global/help", 1, "Small cleanup")
            },
            "global:help-wanted",
            RepoTrustTier::Global,
            &profile,
        );
        let gfi = DiscoveryCandidate::new(
            issue("gfi/actionable", 1, "Fix Rust CLI bug"),
            "gfi:actionable",
            RepoTrustTier::GfiTrusted,
            &profile,
        );

        let selected = select_enrichment_candidates(vec![global, gfi], 2);

        assert_eq!(selected[0].issue.repo_full_name, "gfi/actionable");
    }

    #[test]
    fn profile_trusted_repositories_selects_manual_rust_backend_bucket() {
        let profile = ProfileConfig {
            tech_stack: vec!["Rust".to_string(), "Go".to_string()],
            keywords: vec!["backend".to_string(), "cargo".to_string()],
        };

        let repositories = profile_trusted_repositories(&profile, 3).unwrap();
        let names = repositories
            .iter()
            .map(|repository| repository.full_name())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "rust-analyzer/rust-analyzer".to_string(),
                "rust-lang/rustfmt".to_string(),
                "diesel-rs/diesel".to_string(),
            ]
        );
    }

    #[test]
    fn profile_trusted_repositories_selects_manual_frontend_bucket() {
        let profile = ProfileConfig {
            tech_stack: vec!["TypeScript".to_string(), "React".to_string()],
            keywords: vec!["frontend".to_string(), "component".to_string()],
        };

        let repositories = profile_trusted_repositories(&profile, 2).unwrap();
        let names = repositories
            .iter()
            .map(|repository| repository.full_name())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "rjsf-team/react-jsonschema-form".to_string(),
                "facebook/react-native".to_string(),
            ]
        );
    }

    fn profile() -> ProfileConfig {
        ProfileConfig {
            tech_stack: vec!["Rust".to_string(), "TypeScript".to_string()],
            keywords: vec!["cli".to_string(), "developer-tools".to_string()],
        }
    }

    fn issue(repo_full_name: &str, number: u64, title: &str) -> GitHubIssue {
        let repo_name = repo_full_name.split('/').nth(1).unwrap_or("repo");
        GitHubIssue {
            id: number,
            number,
            title: title.to_string(),
            body: "Expected behavior in src/main.rs. Actual behavior fails under cargo test."
                .to_string(),
            labels: vec!["good first issue".to_string()],
            url: format!("https://github.com/{repo_full_name}/issues/{number}"),
            repo_full_name: repo_full_name.to_string(),
            repo_name: repo_name.to_string(),
            repo_description: "Rust CLI developer tools".to_string(),
            repo_stars: 100,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        }
    }
}
