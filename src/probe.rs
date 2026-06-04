use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::prepare_events::PrepareEventLog;
use crate::repo_scan::{RepoScan, ValidationCommand};

const PROBE_KIND: &str = "patchbay_probe_pack";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_MAX_OUTPUT_BYTES: usize = 16 * 1024;
const DEFAULT_MAX_OUTPUT_LINES: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProbePack {
    pub version: u8,
    pub kind: String,
    pub status: String,
    pub started_at: String,
    pub completed_at: String,
    pub workspace: String,
    pub probes: Vec<ProbeResult>,
    pub facts: ProbeFacts,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProbeResult {
    pub id: String,
    pub argv: Vec<String>,
    pub cwd: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout_excerpt: String,
    pub stderr_excerpt: String,
    pub risk: String,
    pub timed_out: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProbeFacts {
    pub workspace_dirty: bool,
    pub current_branch: Option<String>,
    pub origin_url: Option<String>,
    pub tracked_file_count: Option<usize>,
    pub package_managers: Vec<String>,
    pub detected_scripts: Vec<DetectedScript>,
    pub agent_instruction_files: Vec<String>,
    pub validation_candidates: Vec<ValidationCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DetectedScript {
    pub package_manager: String,
    pub name: String,
    pub command: String,
    pub source: String,
    pub approval: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidationCandidate {
    pub command: String,
    pub source: String,
    pub approval: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeProbeCommand {
    GitStatusPorcelain,
    GitBranchShowCurrent,
    GitLsFiles,
    GitRemoteGetUrlOrigin,
    NpmPkgGetScripts,
    PnpmPkgGetScripts,
}

#[derive(Debug, Clone)]
pub struct SafeProbeRunner {
    timeout: Duration,
    max_output_bytes: usize,
    max_output_lines: usize,
}

impl Default for SafeProbeRunner {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            max_output_lines: DEFAULT_MAX_OUTPUT_LINES,
        }
    }
}

impl ProbePack {
    pub fn not_run(workspace: impl Into<String>) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            version: 1,
            kind: PROBE_KIND.to_string(),
            status: "not_run".to_string(),
            started_at: now.clone(),
            completed_at: now,
            workspace: workspace.into(),
            probes: Vec::new(),
            facts: ProbeFacts::default(),
            warnings: vec!["Safe probes were not run for this handoff builder path.".to_string()],
        }
    }
}

impl Default for ProbePack {
    fn default() -> Self {
        Self::not_run("")
    }
}

impl SafeProbeCommand {
    pub fn id(self) -> &'static str {
        match self {
            Self::GitStatusPorcelain => "git_status_porcelain",
            Self::GitBranchShowCurrent => "git_branch_show_current",
            Self::GitLsFiles => "git_ls_files",
            Self::GitRemoteGetUrlOrigin => "git_remote_get_url_origin",
            Self::NpmPkgGetScripts => "npm_pkg_get_scripts",
            Self::PnpmPkgGetScripts => "pnpm_pkg_get_scripts",
        }
    }

    pub fn argv(self) -> Vec<String> {
        match self {
            Self::GitStatusPorcelain => vec!["git", "status", "--porcelain"],
            Self::GitBranchShowCurrent => vec!["git", "branch", "--show-current"],
            Self::GitLsFiles => vec!["git", "ls-files"],
            Self::GitRemoteGetUrlOrigin => vec!["git", "remote", "get-url", "origin"],
            Self::NpmPkgGetScripts => vec!["npm", "pkg", "get", "scripts", "--json"],
            Self::PnpmPkgGetScripts => vec!["pnpm", "pkg", "get", "scripts", "--json"],
        }
        .into_iter()
        .map(ToString::to_string)
        .collect()
    }

    fn risk(self) -> &'static str {
        "low"
    }
}

impl SafeProbeRunner {
    pub fn run(
        &self,
        workspace: &Path,
        scan: &RepoScan,
        events: Option<&PrepareEventLog>,
    ) -> ProbePack {
        let started_at = Utc::now().to_rfc3339();
        let mut probes = Vec::new();
        for command in selected_probes(workspace) {
            if let Some(events) = events {
                let _ = events.append(
                    "probe_started",
                    &[("probe", Value::String(command.id().to_string()))],
                );
            }
            let result = self.run_one(workspace, command);
            if let Some(events) = events {
                let _ = events.append(
                    "probe_completed",
                    &[
                        ("probe", Value::String(result.id.clone())),
                        (
                            "exit_code",
                            result.exit_code.map(Value::from).unwrap_or(Value::Null),
                        ),
                        ("duration_ms", Value::from(result.duration_ms)),
                    ],
                );
            }
            probes.push(result);
        }

        let mut facts = derive_facts(workspace, scan, &probes);
        facts.detected_scripts = detect_package_scripts(workspace);
        let mut warnings = probes
            .iter()
            .flat_map(|probe| {
                probe
                    .warnings
                    .iter()
                    .map(|warning| format!("{}: {}", probe.id, warning))
            })
            .collect::<Vec<_>>();
        warnings.sort();
        warnings.dedup();
        facts.package_managers.sort();
        facts.package_managers.dedup();
        facts.agent_instruction_files.sort();
        facts.agent_instruction_files.dedup();
        facts
            .detected_scripts
            .sort_by(|left, right| left.name.cmp(&right.name));

        let status = if warnings.is_empty() {
            "completed"
        } else {
            "completed_with_warnings"
        };

        ProbePack {
            version: 1,
            kind: PROBE_KIND.to_string(),
            status: status.to_string(),
            started_at,
            completed_at: Utc::now().to_rfc3339(),
            workspace: workspace.to_string_lossy().to_string(),
            probes,
            facts,
            warnings,
        }
    }

    fn run_one(&self, workspace: &Path, probe: SafeProbeCommand) -> ProbeResult {
        let argv = probe.argv();
        let started = Instant::now();
        let mut warnings = Vec::new();
        let mut command = Command::new(&argv[0]);
        command
            .args(&argv[1..])
            .current_dir(workspace)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                return ProbeResult {
                    id: probe.id().to_string(),
                    argv,
                    cwd: workspace.to_string_lossy().to_string(),
                    exit_code: None,
                    duration_ms: elapsed_ms(started.elapsed()),
                    stdout_excerpt: String::new(),
                    stderr_excerpt: String::new(),
                    risk: probe.risk().to_string(),
                    timed_out: false,
                    warnings: vec![format!("Unable to start probe command: {error}")],
                };
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let max_output_bytes = self.max_output_bytes;
        let stdout_reader =
            stdout.map(|stream| thread::spawn(move || read_limited(stream, max_output_bytes)));
        let stderr_reader =
            stderr.map(|stream| thread::spawn(move || read_limited(stream, max_output_bytes)));

        let mut timed_out = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if started.elapsed() >= self.timeout => {
                    timed_out = true;
                    if let Err(error) = child.kill() {
                        warnings.push(format!("Unable to kill timed-out probe: {error}"));
                    }
                    let _ = child.wait();
                    break None;
                }
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    warnings.push(format!("Unable to wait for probe command: {error}"));
                    break None;
                }
            }
        };

        let stdout = join_limited_output(stdout_reader);
        let stderr = join_limited_output(stderr_reader);
        if stdout.truncated {
            warnings.push(format!(
                "stdout truncated to {} bytes",
                self.max_output_bytes
            ));
        }
        if stderr.truncated {
            warnings.push(format!(
                "stderr truncated to {} bytes",
                self.max_output_bytes
            ));
        }
        if timed_out {
            warnings.push(format!(
                "Probe timed out after {} ms",
                self.timeout.as_millis()
            ));
        }
        let (stdout_excerpt, stdout_line_truncated, stdout_invalid_utf8) =
            excerpt(&stdout.bytes, self.max_output_lines);
        let (stderr_excerpt, stderr_line_truncated, stderr_invalid_utf8) =
            excerpt(&stderr.bytes, self.max_output_lines);
        if stdout_line_truncated {
            warnings.push(format!(
                "stdout truncated to {} lines",
                self.max_output_lines
            ));
        }
        if stderr_line_truncated {
            warnings.push(format!(
                "stderr truncated to {} lines",
                self.max_output_lines
            ));
        }
        if stdout_invalid_utf8 {
            warnings.push("stdout contained invalid UTF-8 and was decoded lossily".to_string());
        }
        if stderr_invalid_utf8 {
            warnings.push("stderr contained invalid UTF-8 and was decoded lossily".to_string());
        }

        ProbeResult {
            id: probe.id().to_string(),
            argv,
            cwd: workspace.to_string_lossy().to_string(),
            exit_code: status.and_then(|status| status.code()),
            duration_ms: elapsed_ms(started.elapsed()),
            stdout_excerpt,
            stderr_excerpt,
            risk: probe.risk().to_string(),
            timed_out,
            warnings,
        }
    }
}

fn selected_probes(workspace: &Path) -> Vec<SafeProbeCommand> {
    let mut probes = vec![
        SafeProbeCommand::GitStatusPorcelain,
        SafeProbeCommand::GitBranchShowCurrent,
        SafeProbeCommand::GitLsFiles,
        SafeProbeCommand::GitRemoteGetUrlOrigin,
    ];
    if workspace.join("package.json").exists() {
        probes.push(SafeProbeCommand::NpmPkgGetScripts);
        if workspace.join("pnpm-lock.yaml").exists()
            || workspace.join("pnpm-workspace.yaml").exists()
        {
            probes.push(SafeProbeCommand::PnpmPkgGetScripts);
        }
    }
    probes
}

fn derive_facts(workspace: &Path, scan: &RepoScan, probes: &[ProbeResult]) -> ProbeFacts {
    let status_stdout = probe_stdout(probes, "git_status_porcelain");
    let branch_stdout = probe_stdout(probes, "git_branch_show_current");
    let remote_stdout = probe_stdout(probes, "git_remote_get_url_origin");
    let ls_files_stdout = probe_stdout(probes, "git_ls_files");

    ProbeFacts {
        workspace_dirty: status_stdout
            .map(|stdout| !stdout.trim().is_empty())
            .unwrap_or(false),
        current_branch: branch_stdout
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        origin_url: remote_stdout
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        tracked_file_count: ls_files_stdout.map(|stdout| {
            stdout
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count()
        }),
        package_managers: detect_package_managers(workspace),
        detected_scripts: Vec::new(),
        agent_instruction_files: detect_agent_instruction_files(scan),
        validation_candidates: validation_candidates(&scan.validation_commands),
    }
}

fn probe_stdout<'a>(probes: &'a [ProbeResult], id: &str) -> Option<&'a str> {
    probes
        .iter()
        .find(|probe| probe.id == id && probe.exit_code == Some(0))
        .map(|probe| probe.stdout_excerpt.as_str())
}

fn validation_candidates(commands: &[ValidationCommand]) -> Vec<ValidationCandidate> {
    commands
        .iter()
        .map(|command| ValidationCandidate {
            command: command.command.clone(),
            source: command.reason.clone(),
            approval: "requires_user_approval".to_string(),
        })
        .collect()
}

fn detect_package_managers(workspace: &Path) -> Vec<String> {
    let mut managers = Vec::new();
    if workspace.join("Cargo.toml").exists() {
        managers.push("cargo".to_string());
    }
    if workspace.join("package.json").exists() {
        managers.push("npm".to_string());
    }
    if workspace.join("pnpm-lock.yaml").exists() || workspace.join("pnpm-workspace.yaml").exists() {
        managers.push("pnpm".to_string());
    }
    if workspace.join("yarn.lock").exists() {
        managers.push("yarn".to_string());
    }
    if workspace.join("pyproject.toml").exists()
        || workspace.join("setup.cfg").exists()
        || workspace.join("setup.py").exists()
    {
        managers.push("python".to_string());
    }
    if workspace.join("go.mod").exists() {
        managers.push("go".to_string());
    }
    managers
}

fn detect_package_scripts(workspace: &Path) -> Vec<DetectedScript> {
    let package_json = workspace.join("package.json");
    let Ok(raw) = fs::read_to_string(&package_json) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let Some(scripts) = value.get("scripts").and_then(Value::as_object) else {
        return Vec::new();
    };

    scripts
        .iter()
        .filter_map(|(name, command)| {
            command.as_str().map(|command| DetectedScript {
                package_manager: "npm".to_string(),
                name: name.clone(),
                command: command.to_string(),
                source: "package.json scripts".to_string(),
                approval: "requires_user_approval".to_string(),
            })
        })
        .collect()
}

fn detect_agent_instruction_files(scan: &RepoScan) -> Vec<String> {
    scan.discovered_files
        .iter()
        .filter(|path| {
            path == &"AGENTS.md"
                || path.ends_with("/AGENTS.md")
                || path.starts_with(".agents/")
                || path.starts_with(".codex/")
        })
        .cloned()
        .collect()
}

#[derive(Debug)]
struct LimitedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

fn read_limited(mut reader: impl Read, max_bytes: usize) -> LimitedOutput {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut buffer = [0u8; 4096];
    while let Ok(read) = reader.read(&mut buffer) {
        if read == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(bytes.len());
        if remaining == 0 {
            truncated = true;
            continue;
        }
        let keep = remaining.min(read);
        bytes.extend_from_slice(&buffer[..keep]);
        if keep < read {
            truncated = true;
        }
    }
    LimitedOutput { bytes, truncated }
}

fn join_limited_output(handle: Option<thread::JoinHandle<LimitedOutput>>) -> LimitedOutput {
    handle
        .and_then(|handle| handle.join().ok())
        .unwrap_or(LimitedOutput {
            bytes: Vec::new(),
            truncated: false,
        })
}

fn excerpt(bytes: &[u8], max_lines: usize) -> (String, bool, bool) {
    let invalid_utf8 = std::str::from_utf8(bytes).is_err();
    let decoded = String::from_utf8_lossy(bytes);
    let mut output = Vec::new();
    let mut truncated = false;
    for (index, line) in decoded.lines().enumerate() {
        if index >= max_lines {
            truncated = true;
            break;
        }
        output.push(line);
    }
    (output.join("\n"), truncated, invalid_utf8)
}

fn elapsed_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{excerpt, SafeProbeCommand};

    #[test]
    fn probe_command_builders_produce_exact_argv_arrays() {
        assert_eq!(
            SafeProbeCommand::GitStatusPorcelain.argv(),
            ["git", "status", "--porcelain"]
        );
        assert_eq!(
            SafeProbeCommand::GitBranchShowCurrent.argv(),
            ["git", "branch", "--show-current"]
        );
        assert_eq!(SafeProbeCommand::GitLsFiles.argv(), ["git", "ls-files"]);
        assert_eq!(
            SafeProbeCommand::GitRemoteGetUrlOrigin.argv(),
            ["git", "remote", "get-url", "origin"]
        );
        assert_eq!(
            SafeProbeCommand::NpmPkgGetScripts.argv(),
            ["npm", "pkg", "get", "scripts", "--json"]
        );
    }

    #[test]
    fn output_excerpt_records_line_truncation() {
        let (excerpt, truncated, invalid_utf8) = excerpt(b"one\ntwo\nthree\n", 2);
        assert_eq!(excerpt, "one\ntwo");
        assert!(truncated);
        assert!(!invalid_utf8);
    }

    #[test]
    fn output_excerpt_records_lossy_utf8() {
        let (excerpt, _, invalid_utf8) = excerpt(&[0xff, b'a'], 2);
        assert!(excerpt.contains('a'));
        assert!(invalid_utf8);
    }
}
