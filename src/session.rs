//! Task lifecycle for a coding agent working in its current conversation.
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::github::GitHubIssue;
use crate::paths::IssueFinderPaths;
use crate::recommendation::events::{
    load_events, record_event_for_issue, RecommendationEventSource, RecommendationEventType,
};
use crate::repo_scan::{scan_repository, RepoScan};
pub use crate::workspace::SessionWorkspace;
use crate::workspace::{
    git_root, prepare_session_workspace, repository_matches, session_contribution_root,
    session_git, session_source_common_dir, validate_repository_name,
};

pub const TASK_FILE: &str = ".issue-finder-cli-task.json";
const MAX_TASK_BYTES: u64 = 8_000_000;
const OUTPUT_TAIL_BYTES: u64 = 6_000;
const FEEDBACK_WARNING_PREFIX: &str = "Result saved, but recommendation feedback failed: ";

#[derive(Debug, Clone)]
pub struct PrepareOptions {
    pub checkout: PathBuf,
    pub workspace_root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionTask {
    pub version: u32,
    pub task_id: String,
    pub issue: GitHubIssue,
    /// External issue discussion and assessment are data, never execution instructions.
    pub evidence: Value,
    pub workspace: SessionWorkspace,
    pub repo_scan: RepoScan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedTask {
    pub task_file: String,
    pub task: SessionTask,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChanges {
    pub changed_files: Vec<String>,
    pub staged_files: Vec<String>,
    pub untracked_files: Vec<String>,
    pub diff_stat: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    pub status: String,
    pub task_file: String,
    pub task: SessionTask,
    pub changes: WorkspaceChanges,
    pub last_result: Option<FinishResult>,
}

#[derive(Debug, Clone)]
pub struct FinishOptions {
    pub checks: Vec<Vec<String>>,
    pub check_timeout_seconds: u64,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub argv: Vec<String>,
    pub exit_code: i32,
    pub timed_out: bool,
    pub output_tail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FinishResult {
    pub status: String,
    pub task_id: String,
    pub issue: String,
    pub workspace: SessionWorkspace,
    pub changes: WorkspaceChanges,
    pub checks: Vec<CheckResult>,
    pub summary: Option<String>,
    pub finished_at: String,
    pub result_file: String,
    pub feedback_recorded: bool,
    pub warnings: Vec<String>,
}

pub fn prepare(
    paths: &IssueFinderPaths,
    issue: &GitHubIssue,
    evidence: Value,
    options: &PrepareOptions,
) -> Result<PreparedTask> {
    validate_repository_name(&issue.repo_full_name)?;
    if issue.number == 0 {
        bail!("Issue number must be positive");
    }
    if serde_json::to_vec(&(issue, &evidence))?.len() as u64 > MAX_TASK_BYTES {
        bail!("Issue evidence exceeds the 8 MB context limit");
    }
    let workspace_root = session_contribution_root(&options.workspace_root)?;
    // Keep the same lock files: unlinking them would let another process lock a
    // different inode. OS locks release on drop or process exit, including crashes.
    let _root_lock = operation_lock(&workspace_root.join(".issue-finder-prepare.lock"))?;
    let _source_lock =
        session_source_common_dir(&options.checkout, &workspace_root, &issue.repo_full_name)?
            .map(|common| operation_lock(&common.join("issue-finder-session.lock")))
            .transpose()?;
    let task_id = unique_id();
    let workspace = prepare_session_workspace(
        issue,
        &options.checkout,
        &workspace_root,
        &task_id,
        TASK_FILE,
    )?;
    let workspace_path = Path::new(&workspace.path);
    let task = SessionTask {
        version: 1,
        task_id,
        issue: issue.clone(),
        evidence,
        repo_scan: scan_repository(workspace_path, issue),
        workspace: workspace.clone(),
    };
    let raw = serde_json::to_vec_pretty(&task)?;
    if raw.len() as u64 > MAX_TASK_BYTES {
        bail!("Prepared task exceeds the 8 MB context limit");
    }
    let state_dir = paths.session_task_dir(&task.task_id);
    fs::create_dir_all(&state_dir)?;
    let task_file = workspace_path.join(TASK_FILE);
    write_new(&state_dir.join("task.json"), &raw)?;
    write_new(&task_file, &raw)?;
    let mut warnings = task.repo_scan.warnings.clone();
    if let Err(error) = record_event_for_issue(
        paths,
        issue,
        None,
        RecommendationEventType::Prepared,
        RecommendationEventSource::ToolPrepare,
        json!({"mode": "current_agent", "task_id": task.task_id}),
    ) {
        warnings.push(format!(
            "Task prepared, but recommendation feedback failed: {error}"
        ));
    }
    Ok(PreparedTask {
        task_file: task_file.to_string_lossy().into_owned(),
        task,
        warnings,
    })
}

pub fn task_status(paths: &IssueFinderPaths, workspace: &Path) -> Result<TaskStatus> {
    let workspace = workspace
        .canonicalize()
        .context("unable to locate task workspace")?;
    let task = load_validated_task(paths, &workspace)?;
    let changes = workspace_changes(&workspace, &task)?;
    let last_result = read_result(paths, &task)?;
    let status = last_result
        .as_ref()
        .filter(|result| result.changes.fingerprint == changes.fingerprint)
        .map(|result| result.status.clone())
        .unwrap_or_else(|| "in_progress".to_string());
    Ok(TaskStatus {
        status,
        task_file: workspace.join(TASK_FILE).to_string_lossy().into_owned(),
        task,
        changes,
        last_result,
    })
}

pub fn finish(
    paths: &IssueFinderPaths,
    workspace: &Path,
    options: &FinishOptions,
) -> Result<FinishResult> {
    validate_checks(options)?;
    let workspace = workspace
        .canonicalize()
        .context("unable to locate task workspace")?;
    let task = load_validated_task(paths, &workspace)?;
    let _task_lock = operation_lock(&paths.session_task_dir(&task.task_id).join("operation.lock"))?;
    if load_validated_task(paths, &workspace)? != task {
        bail!("Task identity changed while acquiring its operation lock; retry task_status");
    }
    let previous = read_result(paths, &task)?;
    let before = workspace_changes(&workspace, &task)?;
    if let Some(result) = &previous {
        if result.status == "completed"
            && result.changes.fingerprint == before.fingerprint
            && result
                .checks
                .iter()
                .map(|check| &check.argv)
                .eq(options.checks.iter())
            && result.summary == options.summary
        {
            let mut result = result.clone();
            project_completion_feedback(paths, &task, &mut result)?;
            return Ok(result);
        }
    }
    let mut checks = Vec::new();
    for argv in &options.checks {
        checks.push(run_check(
            paths,
            &task,
            &workspace,
            argv,
            Duration::from_secs(options.check_timeout_seconds),
        )?);
    }
    if load_validated_task(paths, &workspace)? != task {
        bail!("Task JSON changed during validation; no result recorded");
    }
    let changes = workspace_changes(&workspace, &task)?;
    let status = if checks.iter().any(|check| check.exit_code != 0) {
        "validation_failed"
    } else if changes.changed_files.is_empty() {
        "no_changes"
    } else {
        "completed"
    };
    let result_path = paths.session_task_dir(&task.task_id).join("result.json");
    let mut result = FinishResult {
        status: status.to_string(),
        task_id: task.task_id.clone(),
        issue: format!("{}#{}", task.issue.repo_full_name, task.issue.number),
        workspace: task.workspace.clone(),
        changes,
        checks,
        summary: options.summary.clone(),
        finished_at: Utc::now().to_rfc3339(),
        result_file: result_path.to_string_lossy().into_owned(),
        feedback_recorded: previous
            .as_ref()
            .is_some_and(|result| result.feedback_recorded),
        warnings: Vec::new(),
    };
    replace_json(&result_path, &result)?;
    project_completion_feedback(paths, &task, &mut result)?;
    Ok(result)
}

fn project_completion_feedback(
    paths: &IssueFinderPaths,
    task: &SessionTask,
    result: &mut FinishResult,
) -> Result<()> {
    if result.status != "completed" || result.feedback_recorded {
        return Ok(());
    }
    let projection = load_events(paths).and_then(|events| {
        // A process can exit after the append but before recording its success.
        // Reconcile that window by task identity before appending another event.
        if events.iter().any(|event| {
            event.event_type == RecommendationEventType::Done
                && event.issue_key.repo_full_name == task.issue.repo_full_name
                && event.issue_key.issue_number == task.issue.number
                && event.metadata["task_id"].as_str() == Some(&task.task_id)
        }) {
            Ok(())
        } else {
            record_event_for_issue(
                paths,
                &task.issue,
                None,
                RecommendationEventType::Done,
                RecommendationEventSource::FeedbackCommand,
                json!({"mode": "current_agent", "task_id": task.task_id, "result_file": result.result_file}),
            )
        }
    });
    result
        .warnings
        .retain(|warning| !warning.starts_with(FEEDBACK_WARNING_PREFIX));
    match projection {
        Ok(()) => result.feedback_recorded = true,
        Err(error) => result
            .warnings
            .push(format!("{FEEDBACK_WARNING_PREFIX}{error}")),
    }
    replace_json(
        &paths.session_task_dir(&task.task_id).join("result.json"),
        result,
    )
}

fn operation_lock(path: &Path) -> Result<File> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            bail!("Expected a regular operation lock file: {}", path.display());
        }
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("unable to open operation lock {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => bail!(
            "Another Issue Finder operation is using {}; retry when it finishes",
            path.display()
        ),
        Err(TryLockError::Error(error)) => Err(error)
            .with_context(|| format!("unable to lock Issue Finder operation {}", path.display())),
    }
}

fn load_validated_task(paths: &IssueFinderPaths, workspace: &Path) -> Result<SessionTask> {
    let task: SessionTask = read_bounded_json(&workspace.join(TASK_FILE))?;
    validate_repository_name(&task.issue.repo_full_name)?;
    if task.version != 1
        || task.issue.number == 0
        || task.task_id.is_empty()
        || task.task_id.len() > 100
        || !task
            .task_id
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() || ch == '-')
        || task.workspace.repository != task.issue.repo_full_name
        || !(40..=64).contains(&task.workspace.base_commit.len())
        || !task
            .workspace
            .base_commit
            .chars()
            .all(|ch| ch.is_ascii_hexdigit())
        || Path::new(&task.workspace.path) != workspace
        || git_root(workspace).as_deref() != Some(workspace)
    {
        bail!("Task JSON does not match this Git workspace");
    }
    let registry: SessionTask =
        read_bounded_json(&paths.session_task_dir(&task.task_id).join("task.json"))?;
    if registry != task {
        bail!("Task JSON differs from its recorded identity; do not edit it to bypass validation");
    }
    if !repository_matches(workspace, &task.issue.repo_full_name)? {
        bail!("Repository remotes no longer match the prepared task");
    }
    if session_git(workspace, &["branch", "--show-current"])? != task.workspace.branch {
        bail!("Current branch differs from the prepared task branch");
    }
    session_git(
        workspace,
        &[
            "merge-base",
            "--is-ancestor",
            &task.workspace.base_commit,
            "HEAD",
        ],
    )
    .context("Prepared baseline is no longer an ancestor of HEAD")?;
    if !session_git(workspace, &["ls-files", "--", TASK_FILE])?.is_empty()
        || !session_git(
            workspace,
            &[
                "log",
                "--format=%H",
                &format!("{}..HEAD", task.workspace.base_commit),
                "--",
                TASK_FILE,
            ],
        )?
        .is_empty()
    {
        bail!("Task JSON is staged or committed; remove it from the contribution before finishing");
    }
    Ok(task)
}

fn workspace_changes(workspace: &Path, task: &SessionTask) -> Result<WorkspaceChanges> {
    let base = &task.workspace.base_commit;
    let tracked = nul_paths(&session_git(
        workspace,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--name-only",
            "-z",
            base,
            "--",
        ],
    )?);
    let staged = nul_paths(&session_git(
        workspace,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--name-only",
            "-z",
            "--",
        ],
    )?);
    let untracked = nul_paths(&session_git(
        workspace,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?);
    let changed = tracked
        .into_iter()
        .chain(staged.iter().cloned())
        .chain(untracked.iter().cloned())
        .collect::<BTreeSet<_>>();
    let mut digest = Sha256::new();
    digest.update(session_git(workspace, &["rev-parse", "HEAD"])?);
    digest.update(session_git(
        workspace,
        &[
            "diff",
            "--cached",
            "--raw",
            "--no-ext-diff",
            "--no-textconv",
            "--full-index",
            "-z",
            base,
            "--",
        ],
    )?);
    for path in &changed {
        digest.update(path.as_bytes());
        digest.update([0]);
        let path = workspace.join(path);
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                digest.update(metadata.permissions().mode().to_le_bytes());
            }
            if metadata.file_type().is_symlink() {
                digest.update(fs::read_link(path)?.to_string_lossy().as_bytes());
            } else if metadata.is_file() {
                let mut file = File::open(path)?;
                let mut buffer = [0_u8; 65_536];
                loop {
                    let count = file.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    digest.update(&buffer[..count]);
                }
            }
        } else {
            digest.update(b"deleted");
        }
        digest.update([0]);
    }
    Ok(WorkspaceChanges {
        changed_files: changed.into_iter().collect(),
        staged_files: staged.into_iter().collect(),
        untracked_files: untracked.into_iter().collect(),
        diff_stat: session_git(
            workspace,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--stat",
                base,
                "--",
                ".",
                &format!(":(exclude){TASK_FILE}"),
            ],
        )?,
        fingerprint: format!("{:x}", digest.finalize()),
    })
}

fn nul_paths(raw: &str) -> BTreeSet<String> {
    raw.split('\0')
        .filter(|path| !path.is_empty() && *path != TASK_FILE)
        .map(ToOwned::to_owned)
        .collect()
}

fn validate_checks(options: &FinishOptions) -> Result<()> {
    if options.check_timeout_seconds == 0 || options.check_timeout_seconds > 3_600 {
        bail!("check_timeout_seconds must be between 1 and 3600");
    }
    if options
        .summary
        .as_ref()
        .is_some_and(|summary| summary.len() > 32_768)
    {
        bail!("Completion summary must not exceed 32768 bytes");
    }
    if options.checks.is_empty()
        || options.checks.len() > 20
        || options.checks.iter().any(|argv| {
            argv.is_empty()
                || argv[0].is_empty()
                || argv.len() > 100
                || argv
                    .iter()
                    .any(|arg| arg.contains('\0') || arg.len() > 32_768)
        })
    {
        bail!("Provide 1 to 20 nonempty check argv arrays with bounded, NUL-free arguments");
    }
    Ok(())
}

fn run_check(
    paths: &IssueFinderPaths,
    task: &SessionTask,
    workspace: &Path,
    argv: &[String],
    timeout: Duration,
) -> Result<CheckResult> {
    let output_path = paths
        .session_task_dir(&task.task_id)
        .join(format!("check-{}.log", unique_id()));
    let output_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&output_path)?;
    let result = run_check_to_file(workspace, argv, timeout, output_file);
    let cleanup = fs::remove_file(output_path);
    let result = result?;
    cleanup?;
    Ok(result)
}

fn run_check_to_file(
    workspace: &Path,
    argv: &[String],
    timeout: Duration,
    mut output: File,
) -> Result<CheckResult> {
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output.try_clone()?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Ok(CheckResult {
                argv: argv.to_vec(),
                exit_code: 127,
                timed_out: false,
                output_tail: error.to_string(),
            })
        }
    };
    let started = Instant::now();
    let (exit_code, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (status.code().unwrap_or(1), false);
        }
        if started.elapsed() >= timeout {
            #[cfg(unix)]
            // The check has its own process group, so timed-out descendants cannot keep running.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            child.wait()?;
            break (124, true);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let length = output.metadata()?.len();
    output.seek(SeekFrom::Start(length.saturating_sub(OUTPUT_TAIL_BYTES)))?;
    let mut bytes = Vec::new();
    output.take(OUTPUT_TAIL_BYTES).read_to_end(&mut bytes)?;
    Ok(CheckResult {
        argv: argv.to_vec(),
        exit_code,
        timed_out,
        output_tail: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

fn read_result(paths: &IssueFinderPaths, task: &SessionTask) -> Result<Option<FinishResult>> {
    let path = paths.session_task_dir(&task.task_id).join("result.json");
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            let result: FinishResult = read_bounded_json(&path)?;
            if result.task_id != task.task_id || result.workspace != task.workspace {
                bail!("Recorded result does not match this task");
            }
            Ok(Some(result))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn read_bounded_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("unable to read task state {}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_TASK_BYTES {
        bail!(
            "Expected a regular task state file smaller than 8 MB: {}",
            path.display()
        );
    }
    let raw = fs::read(path)?;
    serde_json::from_slice(&raw)
        .with_context(|| format!("invalid task state JSON: {}", path.display()))
}

fn write_new(path: &Path, raw: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(raw)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn replace_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", unique_id()));
    write_new(&temporary, &serde_json::to_vec_pretty(value)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn unique_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{:x}-{:x}-{:x}",
        Utc::now().timestamp_micros(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}
