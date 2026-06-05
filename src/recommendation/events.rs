use std::fs::{self, OpenOptions};
use std::io::Write;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::github::{GitHubIssue, IssueRef};
use crate::github_enrichment::EnrichedIssue;
use crate::paths::IssueFinderPaths;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IssueKey {
    pub repo_full_name: String,
    pub issue_number: u64,
}

impl IssueKey {
    pub fn new(repo_full_name: impl Into<String>, issue_number: u64) -> Self {
        Self {
            repo_full_name: repo_full_name.into(),
            issue_number,
        }
    }

    pub fn from_issue(issue: &GitHubIssue) -> Self {
        Self::new(issue.repo_full_name.clone(), issue.number)
    }

    pub fn from_issue_ref(reference: &IssueRef) -> Self {
        Self::new(reference.repo_full_name(), reference.number)
    }

    pub fn label(&self) -> String {
        format!("{}#{}", self.repo_full_name, self.issue_number)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecommendationEvent {
    pub event_id: String,
    pub timestamp: String,
    pub issue_key: IssueKey,
    pub event_type: RecommendationEventType,
    pub source: RecommendationEventSource,
    pub issue_updated_at: Option<String>,
    pub issue_comments_count: Option<u64>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationEventType {
    Shown,
    Read,
    Prepared,
    Done,
    Dismissed,
    Restored,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationEventSource {
    CliScout,
    ToolScout,
    CliAssess,
    ToolAssess,
    CliHandoff,
    CliPrepare,
    ToolPrepare,
    InboxDone,
    InboxArchive,
    FeedbackCommand,
    Daily,
}

pub fn append_event(paths: &IssueFinderPaths, event: &RecommendationEvent) -> Result<()> {
    let path = paths.recommendation_events_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "{}", serde_json::to_string(event)?)?;
    Ok(())
}

pub fn load_events(paths: &IssueFinderPaths) -> Result<Vec<RecommendationEvent>> {
    let path = paths.recommendation_events_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("unable to read {}", path.display()))?;
    let mut events = Vec::new();
    for (index, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event = serde_json::from_str::<RecommendationEvent>(line).with_context(|| {
            format!(
                "unable to parse recommendation event {} in {}",
                index + 1,
                path.display()
            )
        })?;
        events.push(event);
    }
    Ok(events)
}

pub fn record_event_for_issue(
    paths: &IssueFinderPaths,
    issue: &GitHubIssue,
    enriched: Option<&EnrichedIssue>,
    event_type: RecommendationEventType,
    source: RecommendationEventSource,
    metadata: Value,
) -> Result<()> {
    let issue_comments_count = enriched.map(|item| item.issue.comments_count);
    let issue_updated_at = enriched
        .map(|item| item.issue.updated_at.clone())
        .or_else(|| Some(issue.updated_at.clone()));
    record_event_for_key_with_facts(
        paths,
        IssueKey::from_issue(issue),
        issue_updated_at,
        issue_comments_count,
        event_type,
        source,
        metadata,
    )
}

pub fn record_event_for_key(
    paths: &IssueFinderPaths,
    issue_key: IssueKey,
    event_type: RecommendationEventType,
    source: RecommendationEventSource,
) -> Result<()> {
    record_event_for_key_with_facts(paths, issue_key, None, None, event_type, source, json!({}))
}

pub fn record_event_for_key_with_facts(
    paths: &IssueFinderPaths,
    issue_key: IssueKey,
    issue_updated_at: Option<String>,
    issue_comments_count: Option<u64>,
    event_type: RecommendationEventType,
    source: RecommendationEventSource,
    metadata: Value,
) -> Result<()> {
    let now = Utc::now();
    let event = RecommendationEvent {
        event_id: format!(
            "recommendation-event-{}-{}",
            now.timestamp_millis(),
            issue_key.label().replace(['/', '#'], "-")
        ),
        timestamp: now.to_rfc3339(),
        issue_key,
        event_type,
        source,
        issue_updated_at,
        issue_comments_count,
        metadata,
    };
    append_event(paths, &event)
}
