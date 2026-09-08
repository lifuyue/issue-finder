use std::time::Duration;

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

use super::{require_success, GitHubClient, GitHubRequestSource, IssueRef};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueContext {
    pub repo_full_name: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub url: String,
    pub state: String,
    pub locked: bool,
    pub is_pull_request: bool,
    pub assignees: Vec<String>,
    pub total_comments: u64,
    pub comments_page: usize,
    pub comments_per_page: usize,
    pub next_comments_page: Option<usize>,
    pub comments_truncated: bool,
    pub comments: Vec<IssueComment>,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueComment {
    pub id: u64,
    pub url: String,
    pub author: Option<String>,
    pub author_association: String,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Deserialize)]
struct User {
    login: String,
}

#[derive(Deserialize)]
struct IssueResponse {
    number: u64,
    title: String,
    body: Option<String>,
    html_url: String,
    state: String,
    locked: bool,
    pull_request: Option<serde_json::Value>,
    #[serde(default)]
    assignees: Vec<User>,
    assignee: Option<User>,
    comments: u64,
}

#[derive(Deserialize)]
struct CommentResponse {
    id: u64,
    html_url: String,
    user: Option<User>,
    #[serde(default)]
    author_association: String,
    body: Option<String>,
    created_at: String,
    updated_at: String,
}

impl GitHubClient {
    /// Full discussion is paged explicitly so the agent can inspect it before workspace creation.
    pub async fn issue_context(
        &self,
        issue: &IssueRef,
        page: usize,
        per_page: usize,
    ) -> Result<IssueContext> {
        ensure!(
            page > 0 && page <= 100_000,
            "commentsPage must be between 1 and 100000"
        );
        ensure!(
            (1..=100).contains(&per_page),
            "commentsPerPage must be between 1 and 100"
        );
        let path = format!(
            "/repos/{}/{}/issues/{}",
            issue.owner, issue.repo, issue.number
        );
        self.record_request(GitHubRequestSource::DirectIssue, &path)?;
        let response = self
            .authorized(self.http.get(self.api_url(&path)))
            .timeout(Duration::from_secs(15))
            .send()
            .await?;
        let details: IssueResponse = require_success(response).await?.json().await?;
        let canonical = if details.pull_request.is_some() {
            let mut url = url::Url::parse(&details.html_url)?;
            let segments = url
                .path_segments()
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            ensure!(
                segments.len() == 4 && segments[2] == "pull",
                "GitHub pull request identity is inconsistent"
            );
            let path = format!("/{}/{}/issues/{}", segments[0], segments[1], segments[3]);
            url.set_path(&path);
            IssueRef::parse_url(url.as_str())?
        } else {
            IssueRef::parse_url(&details.html_url)?
        };
        ensure!(
            canonical.number == details.number,
            "GitHub issue identity is inconsistent"
        );
        let mut assignees = details
            .assignees
            .into_iter()
            .map(|user| user.login)
            .collect::<Vec<_>>();
        if let Some(user) = details.assignee {
            if !assignees.contains(&user.login) {
                assignees.push(user.login);
            }
        }
        let mut next_page = None;
        let mut comments = Vec::new();
        if details.comments > 0 {
            let path = format!(
                "/repos/{}/{}/issues/{}/comments",
                canonical.owner, canonical.repo, canonical.number
            );
            self.record_request(GitHubRequestSource::EnrichmentComments, &path)?;
            let response = self
                .authorized(self.http.get(self.api_url(&path)))
                .query(&[("page", page), ("per_page", per_page)])
                .timeout(Duration::from_secs(15))
                .send()
                .await?;
            let response = require_success(response).await?;
            let has_next = response
                .headers()
                .get(reqwest::header::LINK)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.contains("rel=\"next\""));
            let data = response.json::<Vec<CommentResponse>>().await?;
            if has_next || (page * per_page) < details.comments as usize {
                next_page = Some(page + 1);
            }
            comments = data
                .into_iter()
                .map(|comment| IssueComment {
                    id: comment.id,
                    url: comment.html_url,
                    author: comment.user.map(|user| user.login),
                    author_association: comment.author_association,
                    body: comment.body.unwrap_or_default(),
                    created_at: comment.created_at,
                    updated_at: comment.updated_at,
                })
                .collect();
        }
        Ok(IssueContext {
            repo_full_name: canonical.repo_full_name(),
            number: details.number,
            title: details.title,
            body: details.body.unwrap_or_default(),
            url: details.html_url,
            state: details.state,
            locked: details.locked,
            is_pull_request: details.pull_request.is_some(),
            assignees,
            total_comments: details.comments,
            comments_page: page,
            comments_per_page: per_page,
            next_comments_page: next_page,
            comments_truncated: page > 1
                || next_page.is_some()
                || comments.len() < details.comments as usize,
            comments,
            fetched_at: chrono::Utc::now().to_rfc3339(),
        })
    }
}
