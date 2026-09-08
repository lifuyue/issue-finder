use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::errors::IssueFinderError;
use crate::github::GitHubIssue;
use crate::paths::IssueFinderPaths;
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionWorkspace {
    pub path: String,
    pub branch: String,
    pub base_commit: String,
    pub repository: String,
}

/// Prepare only a branch/worktree; the current agent owns implementation and interaction.
pub fn prepare_session_workspace(
    issue: &GitHubIssue,
    checkout: &Path,
    workspace_root: &Path,
    task_id: &str,
    task_filename: &str,
) -> Result<SessionWorkspace> {
    validate_repository_name(&issue.repo_full_name)?;
    let branch = format!("fuyue/issue-{}-{task_id}", issue.number);
    let checkout = checkout
        .canonicalize()
        .with_context(|| format!("unable to locate checkout {}", checkout.display()))?;
    if !checkout.is_dir() {
        bail!("Checkout must be a directory");
    }
    let current = git_root(&checkout);
    let matching = current
        .as_ref()
        .map(|path| repository_matches(path, &issue.repo_full_name))
        .transpose()?
        .unwrap_or(false);
    let source = if matching {
        let current = current.context("missing matching checkout")?;
        let has_task = [task_filename, ".issue-finder-task.json"]
            .iter()
            .any(|name| std::fs::symlink_metadata(current.join(name)).is_ok());
        if !is_dirty(&current)? && !has_task {
            ensure_session_task_absent(&current, task_filename)?;
            let base = session_git(&current, &["rev-parse", "HEAD"])?;
            session_git(&current, &["switch", "-c", &branch])?;
            return Ok(SessionWorkspace {
                path: current.to_string_lossy().into_owned(),
                branch,
                base_commit: base,
                repository: issue.repo_full_name.clone(),
            });
        }
        current
    } else {
        let root = session_contribution_root(workspace_root)?;
        let source = root.join("repositories").join(&issue.repo_full_name);
        reject_symlink_components(&source)?;
        if !source.exists() {
            let parent = source.parent().context("missing cache parent")?;
            std::fs::create_dir_all(parent)?;
            clone_repository(&source, &issue.repo_full_name)?;
            ensure_session_task_absent(&source, task_filename)?;
            let base = session_git(&source, &["rev-parse", "HEAD"])?;
            session_git(&source, &["switch", "-c", &branch])?;
            return Ok(SessionWorkspace {
                path: source.to_string_lossy().into_owned(),
                branch,
                base_commit: base,
                repository: issue.repo_full_name.clone(),
            });
        }
        if git_root(&source).as_ref() != Some(&source)
            || !repository_matches(&source, &issue.repo_full_name)?
        {
            bail!(
                "Existing repository cache is not a checkout of {}",
                issue.repo_full_name
            );
        }
        source
    };

    let root = session_contribution_root(workspace_root)?;
    let destination = root.join("tasks").join(format!(
        "{}-{}-{task_id}",
        issue.repo_full_name.replace('/', "-"),
        issue.number
    ));
    reject_symlink_components(&destination)?;
    if destination.exists() {
        bail!("Task workspace already exists: {}", destination.display());
    }
    // Fetch only from the requested repository, without changing the user's checkout.
    session_git(
        &source,
        &[
            "fetch",
            "--no-tags",
            &format!("https://github.com/{}.git", issue.repo_full_name),
            "HEAD",
        ],
    )?;
    let base = session_git(&source, &["rev-parse", "FETCH_HEAD^{commit}"])?;
    std::fs::create_dir_all(destination.parent().context("missing workspace parent")?)?;
    session_git(
        &source,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &destination.to_string_lossy(),
            &base,
        ],
    )?;
    ensure_session_task_absent(&destination, task_filename)?;
    Ok(SessionWorkspace {
        path: destination.to_string_lossy().into_owned(),
        branch,
        base_commit: base,
        repository: issue.repo_full_name.clone(),
    })
}

pub(crate) fn validate_repository_name(repository: &str) -> Result<()> {
    let parts = repository.split('/').collect::<Vec<_>>();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || part.starts_with('-')
                || !part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        })
    {
        bail!("Expected a GitHub repository in owner/repo form");
    }
    Ok(())
}

pub(crate) fn git_root(path: &Path) -> Option<PathBuf> {
    session_git(path, &["rev-parse", "--show-toplevel"])
        .ok()
        .and_then(|root| Path::new(&root).canonicalize().ok())
}

pub(crate) fn repository_matches(path: &Path, repository: &str) -> Result<bool> {
    for remote in session_git(path, &["remote"])?.lines() {
        let value = session_git(path, &["config", "--get", &format!("remote.{remote}.url")])?;
        let candidate = value
            .strip_prefix("https://github.com/")
            .or_else(|| value.strip_prefix("http://github.com/"))
            .or_else(|| value.strip_prefix("git@github.com:"))
            .or_else(|| value.strip_prefix("ssh://git@github.com/"));
        if candidate.is_some_and(|candidate| {
            candidate
                .trim_end_matches('/')
                .trim_end_matches(".git")
                .eq_ignore_ascii_case(repository)
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn session_git(path: &Path, args: &[&str]) -> Result<String> {
    Ok(run_git_capture(Some(path), args)?
        .trim_end_matches(['\r', '\n'])
        .to_string())
}

pub(crate) fn session_source_common_dir(
    checkout: &Path,
    workspace_root: &Path,
    repository: &str,
) -> Result<Option<PathBuf>> {
    validate_repository_name(repository)?;
    let source = match git_root(checkout) {
        Some(current) if repository_matches(&current, repository)? => Some(current),
        _ => {
            let cache = workspace_root.join("repositories").join(repository);
            reject_symlink_components(&cache)?;
            match git_root(&cache) {
                Some(current) if current == cache && repository_matches(&current, repository)? => {
                    Some(current)
                }
                _ => None,
            }
        }
    };
    source
        .map(|source| {
            PathBuf::from(session_git(
                &source,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )?)
            .canonicalize()
            .context("unable to resolve source Git directory")
        })
        .transpose()
}

fn ensure_session_task_absent(path: &Path, task_filename: &str) -> Result<()> {
    for name in [task_filename, ".issue-finder-task.json"] {
        if std::fs::symlink_metadata(path.join(name)).is_ok()
            || !session_git(path, &["ls-files", "--", name])?.is_empty()
        {
            bail!(
                "Task file already exists or is tracked in {}; resume the existing task",
                path.display()
            );
        }
    }
    Ok(())
}

pub(crate) fn session_contribution_root(path: &Path) -> Result<PathBuf> {
    let expanded = if let Ok(suffix) = path.strip_prefix("~") {
        dirs::home_dir()
            .context("unable to determine home directory")?
            .join(suffix)
    } else {
        path.to_path_buf()
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()?.join(expanded)
    };
    reject_symlink_components(&absolute)?;
    let mut ancestor = absolute.as_path();
    while !ancestor.exists() {
        ancestor = ancestor.parent().context("invalid workspace root")?;
    }
    if git_root(ancestor).is_some() {
        bail!("Workspace root must be outside existing Git repositories");
    }
    std::fs::create_dir_all(&absolute)?;
    Ok(absolute.canonicalize()?)
}

fn reject_symlink_components(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            bail!("Workspace paths must not contain parent traversal");
        }
        current.push(component);
        if let Ok(metadata) = std::fs::symlink_metadata(&current) {
            // Standard macOS /tmp and /var aliases are resolved before validating descendants.
            if metadata.file_type().is_symlink()
                && current != Path::new("/tmp")
                && current != Path::new("/var")
            {
                bail!("Refusing a symlinked workspace path: {}", current.display());
            }
        }
    }
    Ok(())
}

pub fn prepare_workspace(
    paths: &IssueFinderPaths,
    issue: &GitHubIssue,
) -> Result<PreparedWorkspace> {
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

    let target_branch = issue_finder_branch_name(issue);
    let branch = if dirty {
        warnings.push(
            "Workspace has local changes; Issue Finder did not reset or overwrite it".to_string(),
        );
        detect_current_branch(&workspace_path).unwrap_or_else(|error| {
            warnings.push(format!(
                "Unable to detect current workspace branch: {error}"
            ));
            "HEAD".to_string()
        })
    } else {
        checkout_issue_finder_branch(&workspace_path, &default_branch, &target_branch)?;
        target_branch
    };

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

pub fn issue_finder_branch_name(issue: &GitHubIssue) -> String {
    let slug = slugify(&issue.title);
    if slug.is_empty() {
        format!("issue-finder/{}-issue", issue.number)
    } else {
        format!("issue-finder/{}-{slug}", issue.number)
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

fn detect_current_branch(path: &Path) -> Result<String> {
    let branch = run_git_capture(Some(path), &["rev-parse", "--abbrev-ref", "HEAD"])?;
    Ok(branch.trim().to_string())
}

fn checkout_issue_finder_branch(path: &Path, default_branch: &str, branch: &str) -> Result<()> {
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

    Err(IssueFinderError::GitCommandFailed {
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

    Err(IssueFinderError::GitCommandFailed {
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

    use super::issue_finder_branch_name;
    use crate::github::GitHubIssue;

    #[test]
    fn creates_issue_finder_branch_name() {
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
            issue_finder_branch_name(&issue),
            "issue-finder/123-fix-accessible-button-label"
        );
    }
}
