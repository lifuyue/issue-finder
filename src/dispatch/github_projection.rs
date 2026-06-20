use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::Config;
use crate::github::IssueRef;

use super::model::{
    AgentArtifact, ApprovalRequest, ApprovalStatus, ApprovalType, GitHubInteraction,
    GitHubInteractionStatus, GitHubInteractionType, IssueTask, IssueTaskStatus, NewApprovalRequest,
    NewArtifact, NewGitHubInteraction,
};
use super::store::DispatchStore;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubCommentDraftResult {
    pub issue_task: IssueTask,
    pub interaction: GitHubInteraction,
    pub body_artifact: AgentArtifact,
    pub approval_request: ApprovalRequest,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubApprovalResult {
    pub interaction: GitHubInteraction,
    pub approval_request: ApprovalRequest,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubPostResult {
    pub interaction: GitHubInteraction,
    pub posted_comment: PostedGitHubComment,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PostedGitHubComment {
    pub id: String,
    pub url: String,
}

pub trait GitHubCommentWriter {
    fn post_issue_comment(
        &mut self,
        repo_full_name: &str,
        issue_number: u64,
        body: &str,
    ) -> Result<PostedGitHubComment>;
}

pub struct ReqwestGitHubCommentWriter {
    http: reqwest::blocking::Client,
    token: String,
    api_base_url: String,
}

impl ReqwestGitHubCommentWriter {
    pub fn from_config(config: &Config) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .user_agent("issue-finder")
            .build()?;
        Ok(Self {
            http,
            token: config.resolved_github_token().token,
            api_base_url: std::env::var("ISSUE_FINDER_GITHUB_API_BASE")
                .unwrap_or_else(|_| "https://api.github.com".to_string()),
        })
    }
}

impl GitHubCommentWriter for ReqwestGitHubCommentWriter {
    fn post_issue_comment(
        &mut self,
        repo_full_name: &str,
        issue_number: u64,
        body: &str,
    ) -> Result<PostedGitHubComment> {
        if self.token.trim().is_empty() {
            anyhow::bail!("GitHub token is missing");
        }
        let url = format!(
            "{}/repos/{}/issues/{}/comments",
            self.api_base_url.trim_end_matches('/'),
            repo_full_name,
            issue_number
        );
        let response = self
            .http
            .post(url)
            .bearer_auth(self.token.trim())
            .header("Accept", "application/vnd.github+json")
            .json(&json!({ "body": body }))
            .send()?;
        if !response.status().is_success() {
            anyhow::bail!("GitHub comment post failed: {}", response.status());
        }
        let value = response.json::<Value>()?;
        let id = value
            .get("id")
            .and_then(|id| {
                id.as_str()
                    .map(ToOwned::to_owned)
                    .or_else(|| id.as_u64().map(|id| id.to_string()))
            })
            .context("GitHub comment response missing id")?;
        let url = value
            .get("html_url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok(PostedGitHubComment { id, url })
    }
}

pub fn draft_tracking_comment(
    store: &DispatchStore,
    issue: &str,
    body_override: Option<String>,
) -> Result<GitHubCommentDraftResult> {
    let issue_task = imported_issue_task(store, issue)?;
    let body = body_override
        .unwrap_or_else(|| "I am tracking this issue and preparing a fix attempt.".to_string());
    create_comment_draft(
        store,
        issue_task,
        GitHubInteractionType::TrackingComment,
        body,
        None,
    )
}

pub fn draft_final_comment(
    store: &DispatchStore,
    run_id: &str,
    body_override: Option<String>,
) -> Result<GitHubCommentDraftResult> {
    let run = store.get_dispatch_run(run_id)?;
    let issue_task = store.get_issue_task(&run.issue_task_id)?;
    let body = match body_override {
        Some(body) => body,
        None => derive_final_comment_body(store, &run.result_artifact_id)?,
    };
    create_comment_draft(
        store,
        issue_task,
        GitHubInteractionType::FinalComment,
        body,
        Some(run.id),
    )
}

pub fn approve_github_interaction(
    store: &DispatchStore,
    interaction_id: &str,
) -> Result<GitHubApprovalResult> {
    let approval = pending_github_approval(store, interaction_id)?;
    let approval_request =
        store.resolve_approval_request(&approval.id, ApprovalStatus::Approved)?;
    let interaction = store
        .update_github_interaction_status(interaction_id, GitHubInteractionStatus::Approved)?;
    Ok(GitHubApprovalResult {
        interaction,
        approval_request,
    })
}

pub fn reject_github_interaction(
    store: &DispatchStore,
    interaction_id: &str,
) -> Result<GitHubApprovalResult> {
    let approval = pending_github_approval(store, interaction_id)?;
    let approval_request =
        store.resolve_approval_request(&approval.id, ApprovalStatus::Rejected)?;
    let interaction = store
        .update_github_interaction_status(interaction_id, GitHubInteractionStatus::Rejected)?;
    Ok(GitHubApprovalResult {
        interaction,
        approval_request,
    })
}

pub fn post_github_interaction<W>(
    store: &DispatchStore,
    writer: &mut W,
    interaction_id: &str,
) -> Result<GitHubPostResult>
where
    W: GitHubCommentWriter,
{
    post_ready_github_interaction(
        store,
        writer,
        interaction_id,
        &[GitHubInteractionStatus::Approved],
    )
}

pub fn retry_github_interaction<W>(
    store: &DispatchStore,
    writer: &mut W,
    interaction_id: &str,
) -> Result<GitHubPostResult>
where
    W: GitHubCommentWriter,
{
    let interaction = store.get_github_interaction(interaction_id)?;
    if interaction.status != GitHubInteractionStatus::Failed {
        anyhow::bail!(
            "GitHub interaction {} is {}, not failed",
            interaction.id,
            interaction.status
        );
    }
    store.update_github_interaction_status(interaction_id, GitHubInteractionStatus::Retried)?;
    post_ready_github_interaction(
        store,
        writer,
        interaction_id,
        &[GitHubInteractionStatus::Retried],
    )
}

fn post_ready_github_interaction<W>(
    store: &DispatchStore,
    writer: &mut W,
    interaction_id: &str,
    allowed_statuses: &[GitHubInteractionStatus],
) -> Result<GitHubPostResult>
where
    W: GitHubCommentWriter,
{
    let interaction = store.get_github_interaction(interaction_id)?;
    if !allowed_statuses.contains(&interaction.status) {
        anyhow::bail!(
            "GitHub interaction {} is {}, not approved",
            interaction.id,
            interaction.status
        );
    }
    let issue_task = store.get_issue_task(&interaction.issue_task_id)?;
    let body_artifact_id = interaction
        .body_artifact_id
        .as_deref()
        .context("GitHub interaction has no body artifact")?;
    let body = String::from_utf8(store.read_artifact_bytes(body_artifact_id)?)
        .context("GitHub comment body artifact is not UTF-8")?;
    match writer.post_issue_comment(&issue_task.repo_full_name, issue_task.issue_number, &body) {
        Ok(posted_comment) => {
            let interaction =
                store.mark_github_interaction_posted(&interaction.id, &posted_comment.id)?;
            if interaction.interaction_type == GitHubInteractionType::FinalComment {
                store.update_issue_task_status(&issue_task.id, IssueTaskStatus::GithubPosted)?;
            }
            Ok(GitHubPostResult {
                interaction,
                posted_comment,
            })
        }
        Err(error) => {
            let _ = store.mark_github_interaction_failed(&interaction.id, error.to_string());
            Err(error)
        }
    }
}

pub fn list_github_interactions(
    store: &DispatchStore,
    issue: &str,
) -> Result<Vec<GitHubInteraction>> {
    let issue_task = imported_issue_task(store, issue)?;
    store.list_github_interactions_for_issue_task(&issue_task.id)
}

fn create_comment_draft(
    store: &DispatchStore,
    issue_task: IssueTask,
    interaction_type: GitHubInteractionType,
    body: String,
    run_id: Option<String>,
) -> Result<GitHubCommentDraftResult> {
    let body_artifact = store.write_artifact(
        NewArtifact {
            issue_task_id: Some(issue_task.id.clone()),
            run_id: run_id.clone(),
            kind: "github_comment_body".to_string(),
            content_type: "text/markdown".to_string(),
            metadata_json: json!({
                "interactionType": interaction_type.as_str(),
                "issueKey": issue_task.issue_key
            }),
        },
        projection_comment_body(interaction_type, &body).as_bytes(),
    )?;
    let interaction = store.create_github_interaction(NewGitHubInteraction {
        issue_task_id: issue_task.id.clone(),
        interaction_type,
        body_artifact_id: Some(body_artifact.id.clone()),
        status: GitHubInteractionStatus::Draft,
    })?;
    let approval_request = store.create_approval_request(NewApprovalRequest {
        run_id,
        approval_type: ApprovalType::GithubPost,
        status: ApprovalStatus::Pending,
        prompt: format!(
            "Post {} comment to {}?",
            interaction.interaction_type, issue_task.issue_key
        ),
        details_json: json!({
            "githubInteractionId": interaction.id,
            "issueTaskId": issue_task.id,
            "issueKey": issue_task.issue_key,
            "bodyArtifactId": body_artifact.id
        }),
    })?;
    Ok(GitHubCommentDraftResult {
        issue_task,
        interaction,
        body_artifact,
        approval_request,
    })
}

fn projection_comment_body(interaction_type: GitHubInteractionType, body: &str) -> String {
    if body.contains("<!-- issue-finder:") {
        return body.to_string();
    }
    format!(
        "<!-- issue-finder:{} -->\n{}",
        interaction_type.as_str(),
        body.trim()
    )
}

fn imported_issue_task(store: &DispatchStore, issue: &str) -> Result<IssueTask> {
    let issue_ref = IssueRef::parse(issue)?;
    let issue_key = format!("{}#{}", issue_ref.repo_full_name(), issue_ref.number);
    store
        .find_issue_task_by_key(&issue_key)?
        .with_context(|| format!("issue task {issue_key} has not been imported"))
}

fn derive_final_comment_body(
    store: &DispatchStore,
    result_artifact_id: &Option<String>,
) -> Result<String> {
    let artifact_id = result_artifact_id
        .as_deref()
        .context("dispatch run has no result artifact for final comment")?;
    let bytes = store.read_artifact_bytes(artifact_id)?;
    let value = serde_json::from_slice::<Value>(&bytes)
        .context("fix result artifact must be JSON to derive final comment")?;
    if let Some(reply) = value
        .get("suggestedGitHubReply")
        .or_else(|| value.get("suggested_github_reply"))
        .and_then(Value::as_str)
    {
        return Ok(reply.to_string());
    }
    if let Some(summary) = value.get("summary").and_then(Value::as_str) {
        return Ok(format!(
            "Fix attempt completed.\n\nSummary:\n{summary}\n\nValidation details are recorded in the local Issue Finder result artifact."
        ));
    }
    Ok(
        "Fix attempt completed. Details are recorded in the local Issue Finder result artifact."
            .to_string(),
    )
}

fn pending_github_approval(store: &DispatchStore, interaction_id: &str) -> Result<ApprovalRequest> {
    store
        .list_approval_requests_by_type(ApprovalType::GithubPost)?
        .into_iter()
        .rev()
        .find(|approval| {
            approval.status == ApprovalStatus::Pending
                && approval
                    .details_json
                    .get("githubInteractionId")
                    .and_then(Value::as_str)
                    == Some(interaction_id)
        })
        .with_context(|| {
            format!("GitHub interaction {interaction_id} has no pending GitHub post approval")
        })
}
