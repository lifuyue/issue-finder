pub use crate::scoring::RankedIssue;

use crate::config::ProfileConfig;
use crate::github::GitHubIssue;

pub fn rank_candidates(issues: Vec<GitHubIssue>, profile: &ProfileConfig) -> Vec<RankedIssue> {
    crate::scoring::rank_issues(issues, profile)
}
