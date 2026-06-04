use std::fs;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::errors::PatchbayError;
use crate::github::GitHubIssue;
use crate::handoff::WrittenHandoff;
use crate::paths::{atomic_write, PatchbayPaths};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboxIndex {
    pub items: Vec<InboxItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboxItem {
    pub id: String,
    pub repo_full_name: String,
    pub issue_number: u64,
    pub title: String,
    pub score: i32,
    pub status: InboxStatus,
    pub handoff_json_path: String,
    pub handoff_md_path: String,
    #[serde(default)]
    pub codex_md_path: String,
    #[serde(default)]
    pub agent_policy_path: String,
    #[serde(default)]
    pub probe_json_path: String,
    #[serde(default)]
    pub prepare_events_path: String,
    pub created_at: String,
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InboxStatus {
    Ready,
    PrepareFailed,
    Archived,
    Done,
}

impl InboxStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::PrepareFailed => "prepare_failed",
            Self::Archived => "archived",
            Self::Done => "done",
        }
    }
}

pub fn load_index(paths: &PatchbayPaths) -> Result<InboxIndex> {
    let path = paths.inbox_index_path();
    if !path.exists() {
        return Ok(InboxIndex::default());
    }

    let raw =
        fs::read_to_string(&path).with_context(|| format!("unable to read {}", path.display()))?;
    let index = serde_json::from_str::<InboxIndex>(&raw)?;
    Ok(index)
}

pub fn save_index(paths: &PatchbayPaths, index: &InboxIndex) -> Result<()> {
    atomic_write(&paths.inbox_index_path(), serde_json::to_vec_pretty(index)?)?;
    Ok(())
}

pub fn upsert_ready(
    paths: &PatchbayPaths,
    issue: &GitHubIssue,
    score: i32,
    written: &WrittenHandoff,
) -> Result<InboxIndex> {
    let item = InboxItem {
        id: written.id.clone(),
        repo_full_name: issue.repo_full_name.clone(),
        issue_number: issue.number,
        title: issue.title.clone(),
        score,
        status: InboxStatus::Ready,
        handoff_json_path: written.handoff_json_path.clone(),
        handoff_md_path: written.handoff_md_path.clone(),
        codex_md_path: written.codex_md_path.clone(),
        agent_policy_path: written.agent_policy_path.clone(),
        probe_json_path: written.probe_json_path.clone(),
        prepare_events_path: written.prepare_events_path.clone(),
        created_at: Utc::now().to_rfc3339(),
        failure_reason: None,
    };
    upsert_item(paths, item)
}

pub fn upsert_prepare_failed(
    paths: &PatchbayPaths,
    issue: &GitHubIssue,
    score: i32,
    reason: impl Into<String>,
) -> Result<InboxIndex> {
    let id = format!(
        "{}-{}-{}",
        chrono::Local::now().format("%Y-%m-%d"),
        crate::paths::sanitize_repo_name(&issue.repo_full_name),
        issue.number
    );
    let item = InboxItem {
        id,
        repo_full_name: issue.repo_full_name.clone(),
        issue_number: issue.number,
        title: issue.title.clone(),
        score,
        status: InboxStatus::PrepareFailed,
        handoff_json_path: String::new(),
        handoff_md_path: String::new(),
        codex_md_path: String::new(),
        agent_policy_path: String::new(),
        probe_json_path: String::new(),
        prepare_events_path: String::new(),
        created_at: Utc::now().to_rfc3339(),
        failure_reason: Some(reason.into()),
    };
    upsert_item(paths, item)
}

pub fn update_status(paths: &PatchbayPaths, id: &str, status: InboxStatus) -> Result<InboxIndex> {
    let mut index = load_index(paths)?;
    let Some(item) = index.items.iter_mut().find(|item| item.id == id) else {
        return Err(PatchbayError::InboxItemNotFound(id.to_string()).into());
    };
    item.status = status;
    save_index(paths, &index)?;
    Ok(index)
}

pub fn find_item(paths: &PatchbayPaths, id: &str) -> Result<InboxItem> {
    load_index(paths)?
        .items
        .into_iter()
        .find(|item| item.id == id)
        .ok_or_else(|| PatchbayError::InboxItemNotFound(id.to_string()).into())
}

pub fn contains_issue(
    paths: &PatchbayPaths,
    repo_full_name: &str,
    issue_number: u64,
) -> Result<bool> {
    Ok(load_index(paths)?.items.iter().any(|item| {
        item.repo_full_name == repo_full_name
            && item.issue_number == issue_number
            && !matches!(item.status, InboxStatus::Archived)
    }))
}

pub fn render_index(index: &InboxIndex) -> String {
    if index.items.is_empty() {
        return "Inbox is empty".to_string();
    }

    index
        .items
        .iter()
        .map(|item| {
            format!(
                "[{}] {}#{} | score {} | {} | {}",
                item.status.as_str(),
                item.repo_full_name,
                item.issue_number,
                item.score,
                item.id,
                item.title
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn upsert_item(paths: &PatchbayPaths, item: InboxItem) -> Result<InboxIndex> {
    let mut index = load_index(paths)?;
    index.items.retain(|existing| existing.id != item.id);
    index.items.push(item);
    index.items.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.score.cmp(&left.score))
    });
    save_index(paths, &index)?;
    Ok(index)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::tempdir;

    use super::{load_index, update_status, upsert_ready, InboxStatus};
    use crate::github::GitHubIssue;
    use crate::handoff::WrittenHandoff;
    use crate::paths::PatchbayPaths;

    #[test]
    fn upserts_and_updates_inbox_item() {
        let dir = tempdir().unwrap();
        let paths = PatchbayPaths {
            home: dir.path().to_path_buf(),
            config: dir.path().join("config.toml"),
            cache_dir: dir.path().join("cache"),
            workspaces_dir: dir.path().join("workspaces"),
            inbox_dir: dir.path().join("inbox"),
            reports_dir: dir.path().join("reports"),
        };
        paths.ensure_layout().unwrap();
        let issue = GitHubIssue {
            id: 1,
            number: 2,
            title: "Issue".to_string(),
            body: String::new(),
            labels: vec![],
            url: String::new(),
            repo_full_name: "owner/repo".to_string(),
            repo_name: "repo".to_string(),
            repo_description: String::new(),
            repo_stars: 0,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        };
        let written = WrittenHandoff {
            id: "today-owner__repo-2".to_string(),
            dir: String::new(),
            handoff_json_path: "/tmp/handoff.json".to_string(),
            handoff_md_path: "/tmp/handoff.md".to_string(),
            codex_md_path: "/tmp/codex.md".to_string(),
            agent_policy_path: "/tmp/agent-policy.json".to_string(),
            probe_json_path: "/tmp/probe.json".to_string(),
            prepare_events_path: "/tmp/prepare-events.jsonl".to_string(),
        };

        upsert_ready(&paths, &issue, 80, &written).unwrap();
        update_status(&paths, &written.id, InboxStatus::Done).unwrap();
        let index = load_index(&paths).unwrap();
        assert_eq!(index.items[0].status, InboxStatus::Done);
    }
}
