use thiserror::Error;

#[derive(Debug, Error)]
pub enum PatchbayError {
    #[error("invalid issue reference; expected owner/repo#123 or a GitHub issue URL")]
    InvalidIssueReference,

    #[error("GitHub issue discovery failed because the Search API is currently rate-limited. Wait a few minutes and retry.")]
    GitHubRateLimited,

    #[error("GitHub returned an unexpected response: {0}")]
    GitHubResponse(String),

    #[error("Patchbay configuration was not found. Run `patchbay init` first.")]
    MissingConfig,

    #[error("inbox item not found: {0}")]
    InboxItemNotFound(String),

    #[error("git command failed: {command}\n{stderr}")]
    GitCommandFailed { command: String, stderr: String },
}

pub type Result<T> = std::result::Result<T, PatchbayError>;
