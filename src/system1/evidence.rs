//! Bounded GitHub facts used by semantic questions. These contain no ranking conclusions.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::github::GitHubIssue;
use crate::github_enrichment::EnrichedIssue;

pub const BODY_CHAR_LIMIT: usize = 12_000;
pub const COMMENT_CHAR_LIMIT: usize = 4_000;
pub const COMMENTS_CHAR_LIMIT: usize = 24_000;
pub const COMMENT_SAMPLE_LIMIT: usize = 30;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceText {
    pub text: String,
    pub original_chars: usize,
    pub truncated: bool,
    pub source_available: bool,
}

impl EvidenceText {
    pub fn bounded(text: &str, limit: usize) -> Self {
        let original_chars = text.chars().count();
        Self {
            text: text.chars().take(limit).collect(),
            original_chars,
            truncated: original_chars > limit,
            source_available: true,
        }
    }

    pub fn unavailable() -> Self {
        Self {
            text: String::new(),
            original_chars: 0,
            truncated: false,
            source_available: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommentEvidence {
    pub id: Option<u64>,
    pub url: Option<String>,
    pub author: Option<String>,
    pub author_association: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub body: EvidenceText,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommentFetchStatus {
    Complete,
    Sampled,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommentsEvidence {
    pub status: CommentFetchStatus,
    pub total_count: Option<u64>,
    pub selection: String,
    pub sample_limit: usize,
    pub omitted_count: Option<u64>,
    pub bodies_truncated: bool,
    pub chronological_order_complete: bool,
    pub comments: Vec<CommentEvidence>,
}

impl CommentsEvidence {
    pub fn unavailable(total_count: Option<u64>) -> Self {
        Self {
            status: CommentFetchStatus::Unavailable,
            total_count,
            selection: "not_fetched".to_string(),
            sample_limit: COMMENT_SAMPLE_LIMIT,
            omitted_count: total_count,
            bodies_truncated: false,
            chronological_order_complete: false,
            comments: Vec::new(),
        }
    }

    /// Keep the newest comments without selecting on their language or inferred meaning.
    pub fn from_comments(total_count: Option<u64>, mut comments: Vec<CommentEvidence>) -> Self {
        let chronological_order_complete = comments.iter().all(|comment| {
            comment
                .created_at
                .as_deref()
                .is_some_and(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).is_ok())
        });
        if chronological_order_complete {
            comments.sort_by(|a, b| {
                let a_time = a
                    .created_at
                    .as_deref()
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
                let b_time = b
                    .created_at
                    .as_deref()
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
                a_time.cmp(&b_time).then_with(|| a.id.cmp(&b.id))
            });
        }
        let skip = comments.len().saturating_sub(COMMENT_SAMPLE_LIMIT);
        let mut budget = COMMENTS_CHAR_LIMIT;
        let mut selected = Vec::new();
        for mut comment in comments.into_iter().skip(skip).rev() {
            if budget == 0 {
                break;
            }
            let limit = COMMENT_CHAR_LIMIT.min(budget);
            let original_chars = comment.body.original_chars;
            let source_available = comment.body.source_available;
            comment.body = EvidenceText::bounded(&comment.body.text, limit);
            comment.body.original_chars = original_chars;
            comment.body.source_available = source_available;
            comment.body.truncated |= original_chars > comment.body.text.chars().count();
            budget -= comment.body.text.chars().count();
            selected.push(comment);
        }
        selected.reverse();
        let bodies_truncated = selected.iter().any(|comment| comment.body.truncated);
        let omitted_count = total_count.map(|count| count.saturating_sub(selected.len() as u64));
        let complete = total_count == Some(selected.len() as u64)
            && !bodies_truncated
            && selected.iter().all(|comment| comment.body.source_available)
            && chronological_order_complete;
        Self {
            status: if complete {
                CommentFetchStatus::Complete
            } else {
                CommentFetchStatus::Sampled
            },
            total_count,
            selection: if total_count.is_some() {
                "latest_comments"
            } else {
                "first_page_count_unknown"
            }
            .to_string(),
            sample_limit: COMMENT_SAMPLE_LIMIT,
            omitted_count,
            bodies_truncated,
            chronological_order_complete,
            comments: selected,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryEvidence {
    pub description: EvidenceText,
    pub language: Option<String>,
    pub archived: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubStatusEvidence {
    pub issue_state: Option<String>,
    pub assignees: Option<Vec<String>>,
    pub linked_open_pr_count: usize,
    pub linked_closed_pr_count: usize,
    /// A closed PR is not evidence that it was merged. Existing enrichment has no merge facts.
    pub linked_merged_pr_count: Option<usize>,
    pub linked_pr_timeline_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceSnapshot {
    pub schema_version: u32,
    pub issue_id: u64,
    pub issue_url: String,
    pub repo_full_name: String,
    pub issue_updated_at: String,
    pub issue_author: Option<String>,
    pub issue_author_association: String,
    pub title: EvidenceText,
    pub body: EvidenceText,
    pub labels: Vec<String>,
    pub repository: RepositoryEvidence,
    pub comments: CommentsEvidence,
    pub github_status: GitHubStatusEvidence,
    pub user_requirements: EvidenceText,
    pub warnings: Vec<String>,
    pub source_fetched_at: String,
}

#[derive(Debug, Clone, Copy)]
pub enum ContextCategory {
    Task,
    Contribution,
    Preference,
    TaskForm,
}

impl EvidenceSnapshot {
    pub fn from_issue(issue: &GitHubIssue, enriched: &EnrichedIssue) -> Self {
        let repo_available = (enriched.repository.default_branch.is_some()
            || enriched.repository.created_at.is_some()
            || enriched.repository.archived)
            && !enriched
                .warnings
                .iter()
                .any(|w| w.starts_with("Repository metadata enrichment failed"));
        Self {
            schema_version: 1,
            issue_id: issue.id,
            issue_url: enriched.issue.url.clone(),
            repo_full_name: enriched.issue.repo_full_name.clone(),
            issue_updated_at: enriched.issue.updated_at.clone(),
            issue_author: enriched.participants.issue_author.clone(),
            issue_author_association: enriched.issue.author_association.clone(),
            title: EvidenceText::bounded(&enriched.issue.title, 512),
            body: EvidenceText::bounded(&enriched.issue.body, BODY_CHAR_LIMIT),
            labels: enriched
                .issue
                .labels
                .iter()
                .take(100)
                .map(|label| label.chars().take(100).collect())
                .collect(),
            repository: RepositoryEvidence {
                description: EvidenceText::bounded(&enriched.repository.description, 1_000),
                language: enriched.repository.language.clone(),
                archived: repo_available.then_some(enriched.repository.archived),
            },
            comments: CommentsEvidence::unavailable(None),
            github_status: GitHubStatusEvidence {
                issue_state: None,
                assignees: None,
                linked_open_pr_count: enriched.competition.open_pr_refs,
                linked_closed_pr_count: enriched.competition.closed_pr_refs,
                linked_merged_pr_count: None,
                linked_pr_timeline_available:
                    !crate::github_enrichment::competition_timeline_missing(enriched),
            },
            user_requirements: EvidenceText::bounded("", 4_000),
            warnings: Vec::new(),
            source_fetched_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn with_user_requirements(mut self, requirements: &str) -> Self {
        self.user_requirements = EvidenceText::bounded(requirements, 4_000);
        self
    }

    /// Retrieval time is bookkeeping, not a new material version.
    pub fn material_hash(&self) -> String {
        let mut material = self.clone();
        material.source_fetched_at.clear();
        material.warnings.clear();
        let encoded = serde_json::to_vec(&material).expect("evidence serializes to JSON");
        format!("{:x}", Sha256::digest(encoded))
    }

    pub fn context_for(&self, category: ContextCategory) -> Value {
        let task = json!({ "title": self.title, "body": self.body,
            "author": self.issue_author, "author_association": self.issue_author_association });
        let comments = json!({
            "status": self.comments.status,
            "total_count": self.comments.total_count,
            "selection": self.comments.selection,
            "omitted_count": self.comments.omitted_count,
            "bodies_truncated": self.comments.bodies_truncated,
            "chronological_order_complete": self.comments.chronological_order_complete,
            "comments": self.comments.comments.iter().map(|comment| json!({
                "author": comment.author,
                "author_association": comment.author_association,
                "created_at": comment.created_at,
                "updated_at": comment.updated_at,
                "body": comment.body,
            })).collect::<Vec<_>>()
        });
        match category {
            ContextCategory::Task => {
                json!({ "task": task, "repository": self.repository, "clarification_comments": comments })
            }
            ContextCategory::Contribution => json!({ "task": task, "discussion": comments }),
            ContextCategory::Preference => {
                json!({ "explicit_user_requirements": self.user_requirements, "task": task, "repository": self.repository })
            }
            ContextCategory::TaskForm => {
                json!({ "task": task, "labels": self.labels, "clarification_comments": comments })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(id: u64, body: &str) -> CommentEvidence {
        CommentEvidence {
            id: Some(id),
            url: Some(format!("https://github.com/o/r/issues/1#issuecomment-{id}")),
            author: Some(format!("participant-{id}")),
            author_association: "CONTRIBUTOR".to_string(),
            created_at: Some(format!("2026-09-{:02}T00:00:00Z", id)),
            updated_at: Some(format!("2026-09-{:02}T01:00:00Z", id)),
            body: EvidenceText::bounded(body, usize::MAX),
        }
    }

    #[test]
    fn retains_latest_timeline_and_marks_character_budget_omissions() {
        let comments = (1..=10)
            .rev()
            .map(|id| comment(id, &"界".repeat(5_000)))
            .collect();
        let evidence = CommentsEvidence::from_comments(Some(10), comments);
        assert_eq!(evidence.status, CommentFetchStatus::Sampled);
        assert_eq!(evidence.comments.len(), 6);
        assert_eq!(evidence.comments[0].id, Some(5));
        assert_eq!(evidence.comments[5].id, Some(10));
        assert_eq!(evidence.omitted_count, Some(4));
        assert!(evidence.bodies_truncated);
        assert_eq!(
            evidence
                .comments
                .iter()
                .map(|c| c.body.text.chars().count())
                .sum::<usize>(),
            COMMENTS_CHAR_LIMIT
        );
        assert_eq!(evidence.comments[0].body.original_chars, 5_000);
        assert!(evidence.comments[0]
            .url
            .as_deref()
            .unwrap()
            .ends_with("issuecomment-5"));
    }

    #[test]
    fn unavailable_and_unknown_count_are_distinct_from_an_observed_empty_discussion() {
        assert_eq!(
            CommentsEvidence::unavailable(None).status,
            CommentFetchStatus::Unavailable
        );
        assert_eq!(
            CommentsEvidence::from_comments(None, Vec::new()).status,
            CommentFetchStatus::Sampled
        );
        assert_eq!(
            CommentsEvidence::from_comments(Some(0), Vec::new()).status,
            CommentFetchStatus::Complete
        );
        let mut missing_time = comment(1, "I can help.");
        missing_time.created_at = None;
        let partial = CommentsEvidence::from_comments(Some(1), vec![missing_time]);
        assert_eq!(partial.status, CommentFetchStatus::Sampled);
        assert!(!partial.chronological_order_complete);
    }

    #[test]
    fn material_hash_tracks_content_versions_and_requirements_but_not_retrieval_time() {
        let issue = GitHubIssue {
            id: 1,
            number: 1,
            title: "Fix typo".to_string(),
            body: "Change teh to the.".to_string(),
            labels: Vec::new(),
            url: "https://github.com/o/r/issues/1".to_string(),
            repo_full_name: "o/r".to_string(),
            repo_name: "r".to_string(),
            repo_description: String::new(),
            repo_stars: 1,
            created_at: "2026-09-01T00:00:00Z".to_string(),
            updated_at: "2026-09-01T00:00:00Z".to_string(),
        };
        let mut snapshot = EvidenceSnapshot::from_issue(&issue, &EnrichedIssue::from_issue(&issue));
        snapshot.comments =
            CommentsEvidence::from_comments(Some(1), vec![comment(1, "I'll handle this.")]);
        let original = snapshot.material_hash();
        snapshot.source_fetched_at = "2026-10-01T00:00:00Z".to_string();
        assert_eq!(original, snapshot.material_hash());
        snapshot.comments.comments[0].updated_at = Some("2026-10-01T00:00:00Z".to_string());
        assert_ne!(original, snapshot.material_hash());
        let changed = snapshot.material_hash();
        snapshot = snapshot.with_user_requirements("Only Rust code changes");
        assert_ne!(changed, snapshot.material_hash());
        let contribution = snapshot.context_for(ContextCategory::Contribution);
        assert_eq!(
            contribution["discussion"]["comments"][0]["author"],
            "participant-1"
        );
        assert!(contribution["discussion"]["comments"][0]
            .get("id")
            .is_none());
    }
}
