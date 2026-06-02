use std::path::Path;
use std::process::Command;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::errors::PatchbayError;
use crate::github::GitHubIssue;
use crate::paths::PatchbayPaths;
use crate::repo_scan::{scan_repository, RepoScan};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceInfo {
    pub path: String,
    pub default_branch: String,
    pub branch: String,
    pub dirty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedWorkspace {
    pub info: WorkspaceInfo,
    pub scan: RepoScan,
    pub warnings: Vec<String>,
}

pub fn prepare_workspace(paths: &PatchbayPaths, issue: &GitHubIssue) -> Result<PreparedWorkspace> {
    let workspace_path = paths.workspace_path_for(&issue.repo_full_name);
    let mut warnings = Vec::new();

    if !workspace_path.exists() {
        clone_repository(&workspace_path, &issue.repo_full_name)?;
    } else {
        fetch_repository(&workspace_path)?;
    }

    let default_branch = detect_default_branch(&workspace_path).unwrap_or_else(|error| {
        warnings.push(format!("Unable to detect default branch: {error}"));
        "main".to_string()
    });
    let dirty = is_dirty(&workspace_path).unwrap_or_else(|error| {
        warnings.push(format!("Unable to detect workspace dirty state: {error}"));
        true
    });

    let branch = patchbay_branch_name(issue);
    if dirty {
        warnings.push(
            "Workspace has local changes; Patchbay did not reset or overwrite it".to_string(),
        );
    } else {
        checkout_patchbay_branch(&workspace_path, &default_branch, &branch)?;
    }

    let scan = scan_repository(&workspace_path, issue);
    warnings.extend(scan.warnings.clone());

    Ok(PreparedWorkspace {
        info: WorkspaceInfo {
            path: workspace_path.to_string_lossy().to_string(),
            default_branch,
            branch,
            dirty,
        },
        scan,
        warnings,
    })
}

pub fn patchbay_branch_name(issue: &GitHubIssue) -> String {
    let slug = slugify(&issue.title);
    if slug.is_empty() {
        format!("patchbay/{}-issue", issue.number)
    } else {
        format!("patchbay/{}-{slug}", issue.number)
    }
}

pub fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn clone_repository(path: &Path, repo_full_name: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let repo_url = format!("https://github.com/{repo_full_name}.git");
    let path_arg = path.to_string_lossy().to_string();
    run_git(None, &["clone", &repo_url, &path_arg])?;
    Ok(())
}

fn fetch_repository(path: &Path) -> Result<()> {
    run_git(Some(path), &["fetch", "origin"])?;
    Ok(())
}

fn detect_default_branch(path: &Path) -> Result<String> {
    let output = run_git_capture(Some(path), &["symbolic-ref", "refs/remotes/origin/HEAD"])?;
    let trimmed = output.trim();
    if let Some(branch) = trimmed.rsplit('/').next() {
        if !branch.is_empty() {
            return Ok(branch.to_string());
        }
    }

    for branch in ["main", "master"] {
        let remote_branch = format!("origin/{branch}");
        if run_git(Some(path), &["rev-parse", "--verify", &remote_branch]).is_ok() {
            return Ok(branch.to_string());
        }
    }

    Ok("main".to_string())
}

fn is_dirty(path: &Path) -> Result<bool> {
    Ok(!run_git_capture(Some(path), &["status", "--porcelain"])?
        .trim()
        .is_empty())
}

fn checkout_patchbay_branch(path: &Path, default_branch: &str, branch: &str) -> Result<()> {
    let local_branch_ref = format!("refs/heads/{branch}");
    if run_git(Some(path), &["rev-parse", "--verify", &local_branch_ref]).is_ok() {
        run_git(Some(path), &["checkout", branch])?;
    } else {
        let remote_default = format!("origin/{default_branch}");
        if run_git(Some(path), &["checkout", "-b", branch, &remote_default]).is_err() {
            run_git(Some(path), &["checkout", default_branch])?;
            run_git(Some(path), &["checkout", "-b", branch])?;
        }
    }
    Ok(())
}

fn run_git(cwd: Option<&Path>, args: &[&str]) -> Result<()> {
    let output = git_command(cwd, args).output()?;
    if output.status.success() {
        return Ok(());
    }

    Err(PatchbayError::GitCommandFailed {
        command: format!("git {}", args.join(" ")),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    }
    .into())
}

fn run_git_capture(cwd: Option<&Path>, args: &[&str]) -> Result<String> {
    let output = git_command(cwd, args).output()?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }

    Err(PatchbayError::GitCommandFailed {
        command: format!("git {}", args.join(" ")),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    }
    .into())
}

fn git_command(cwd: Option<&Path>, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command.args(args);
    command
}

fn slugify(input: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for ch in input.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }

        if slug.len() >= 48 {
            break;
        }
    }

    slug.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::patchbay_branch_name;
    use crate::github::GitHubIssue;

    #[test]
    fn creates_patchbay_branch_name() {
        let issue = GitHubIssue {
            id: 1,
            number: 123,
            title: "Fix accessible button label!".to_string(),
            body: String::new(),
            labels: vec![],
            url: "https://github.com/owner/repo/issues/123".to_string(),
            repo_full_name: "owner/repo".to_string(),
            repo_name: "repo".to_string(),
            repo_description: String::new(),
            repo_stars: 0,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        };

        assert_eq!(
            patchbay_branch_name(&issue),
            "patchbay/123-fix-accessible-button-label"
        );
    }
}
