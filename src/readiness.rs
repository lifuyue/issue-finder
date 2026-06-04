use serde::{Deserialize, Serialize};

use crate::github::GitHubIssue;
use crate::probe::ProbePack;
use crate::workspace::PreparedWorkspace;

const READINESS_KIND: &str = "patchbay_execution_readiness";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionReadiness {
    pub version: u8,
    pub kind: String,
    pub score: i32,
    pub band: String,
    pub axes: Vec<ReadinessAxis>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadinessAxis {
    pub id: String,
    pub score: i32,
    pub reason: String,
}

impl Default for ExecutionReadiness {
    fn default() -> Self {
        Self {
            version: 1,
            kind: READINESS_KIND.to_string(),
            score: 0,
            band: "low".to_string(),
            axes: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

pub fn assess_readiness(
    issue: &GitHubIssue,
    workspace: &PreparedWorkspace,
    probe_pack: &ProbePack,
) -> ExecutionReadiness {
    let axes = vec![
        workspace_state_axis(issue, workspace, probe_pack),
        file_locality_axis(workspace),
        validation_detectability_axis(probe_pack),
        setup_clarity_axis(workspace, probe_pack),
        command_safety_axis(probe_pack),
        dependency_complexity_axis(workspace, probe_pack),
        context_completeness_axis(workspace, probe_pack),
    ];
    let score = if axes.is_empty() {
        0
    } else {
        axes.iter().map(|axis| axis.score).sum::<i32>() / axes.len() as i32
    };
    let mut warnings = probe_pack.warnings.clone();
    if workspace.info.dirty {
        warnings.push(
            "Workspace is dirty; coding agent should inspect local changes first.".to_string(),
        );
    }
    warnings.sort();
    warnings.dedup();

    ExecutionReadiness {
        version: 1,
        kind: READINESS_KIND.to_string(),
        score,
        band: readiness_band(score).to_string(),
        axes,
        warnings,
    }
}

fn workspace_state_axis(
    issue: &GitHubIssue,
    workspace: &PreparedWorkspace,
    probe_pack: &ProbePack,
) -> ReadinessAxis {
    if workspace.info.dirty || probe_pack.facts.workspace_dirty {
        return axis(
            "workspace_state",
            40,
            "Workspace has local changes; Patchbay did not overwrite them.",
        );
    }

    let branch_ready = probe_pack
        .facts
        .current_branch
        .as_deref()
        .map(|branch| branch == workspace.info.branch)
        .unwrap_or(true);
    let origin_matches = probe_pack
        .facts
        .origin_url
        .as_deref()
        .map(|origin| origin.contains(&issue.repo_full_name))
        .unwrap_or(true);

    match (branch_ready, origin_matches) {
        (true, true) => axis(
            "workspace_state",
            95,
            "Workspace is clean, branch is prepared, and origin matches the issue repository.",
        ),
        (true, false) => axis(
            "workspace_state",
            70,
            "Workspace is clean and branch is prepared, but origin URL did not match the GitHub repository string.",
        ),
        _ => axis(
            "workspace_state",
            60,
            "Workspace is clean, but probe facts did not confirm the expected Patchbay branch.",
        ),
    }
}

fn file_locality_axis(workspace: &PreparedWorkspace) -> ReadinessAxis {
    let count = workspace.scan.candidate_files.len();
    match count {
        0 => axis(
            "file_locality",
            35,
            "No candidate files were detected from the issue and repository scan.",
        ),
        1..=5 => axis(
            "file_locality",
            90,
            "Candidate files are present and reasonably scoped.",
        ),
        _ => axis(
            "file_locality",
            70,
            "Candidate files were detected, but the set may need narrowing before editing.",
        ),
    }
}

fn validation_detectability_axis(probe_pack: &ProbePack) -> ReadinessAxis {
    if probe_pack.facts.validation_candidates.is_empty() {
        axis(
            "validation_detectability",
            35,
            "No validation candidates were detected without running repository code.",
        )
    } else {
        axis(
            "validation_detectability",
            85,
            "Validation candidates were detected and classified as requiring user approval.",
        )
    }
}

fn setup_clarity_axis(workspace: &PreparedWorkspace, probe_pack: &ProbePack) -> ReadinessAxis {
    let has_docs = workspace.scan.discovered_files.iter().any(|path| {
        path.eq_ignore_ascii_case("README.md")
            || path.eq_ignore_ascii_case("CONTRIBUTING.md")
            || path.ends_with("/CONTRIBUTING.md")
    });
    let has_agent_instructions = !probe_pack.facts.agent_instruction_files.is_empty();
    let has_manifest = !probe_pack.facts.package_managers.is_empty();

    let score = match (has_docs, has_agent_instructions, has_manifest) {
        (true, true, true) => 95,
        (true, _, true) | (_, true, true) => 80,
        (_, _, true) => 65,
        _ => 40,
    };
    axis(
        "setup_clarity",
        score,
        "Setup clarity is based on contribution docs, agent instructions, and package manifests.",
    )
}

fn command_safety_axis(probe_pack: &ProbePack) -> ReadinessAxis {
    let all_validation_requires_approval = probe_pack
        .facts
        .validation_candidates
        .iter()
        .all(|candidate| candidate.approval == "requires_user_approval");
    if all_validation_requires_approval {
        axis(
            "command_safety",
            95,
            "Low-risk probes and validation candidates are separated by approval category.",
        )
    } else {
        axis(
            "command_safety",
            50,
            "At least one validation candidate was not classified as requiring approval.",
        )
    }
}

fn dependency_complexity_axis(
    workspace: &PreparedWorkspace,
    probe_pack: &ProbePack,
) -> ReadinessAxis {
    let manager_count = probe_pack.facts.package_managers.len();
    let lockfiles = workspace
        .scan
        .discovered_files
        .iter()
        .filter(|path| {
            matches!(
                path.as_str(),
                "Cargo.lock"
                    | "package-lock.json"
                    | "pnpm-lock.yaml"
                    | "yarn.lock"
                    | "poetry.lock"
                    | "go.sum"
            )
        })
        .count();

    let (score, reason) = if manager_count == 0 {
        (
            55,
            "No package manager manifest was detected; setup may require manual inspection.",
        )
    } else if manager_count == 1 && lockfiles > 0 {
        (90, "A single package manager and lockfile were detected.")
    } else if manager_count == 1 {
        (
            75,
            "A single package manager was detected, but no lockfile was found.",
        )
    } else {
        (
            55,
            "Multiple package managers were detected; dependency setup may be more complex.",
        )
    };
    axis("dependency_complexity", score, reason)
}

fn context_completeness_axis(
    workspace: &PreparedWorkspace,
    probe_pack: &ProbePack,
) -> ReadinessAxis {
    let mut present = 0;
    if !workspace.scan.discovered_files.is_empty() {
        present += 1;
    }
    if !workspace.scan.candidate_files.is_empty() {
        present += 1;
    }
    if !workspace.scan.validation_commands.is_empty() {
        present += 1;
    }
    if !probe_pack.probes.is_empty() {
        present += 1;
    }
    if !probe_pack.facts.package_managers.is_empty()
        || !probe_pack.facts.agent_instruction_files.is_empty()
    {
        present += 1;
    }

    let score = match present {
        5 => 95,
        4 => 80,
        3 => 65,
        2 => 50,
        _ => 35,
    };
    axis(
        "context_completeness",
        score,
        "Completeness reflects repo scan, candidate files, validation hints, probes, and setup facts.",
    )
}

fn axis(id: &str, score: i32, reason: &str) -> ReadinessAxis {
    ReadinessAxis {
        id: id.to_string(),
        score,
        reason: reason.to_string(),
    }
}

fn readiness_band(score: i32) -> &'static str {
    match score {
        80..=100 => "high",
        50..=79 => "medium",
        _ => "low",
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::assess_readiness;
    use crate::github::GitHubIssue;
    use crate::probe::{ProbeFacts, ProbePack};
    use crate::repo_scan::{CandidateFile, RepoScan, ValidationCommand};
    use crate::workspace::{PreparedWorkspace, WorkspaceInfo};

    #[test]
    fn readiness_scores_clean_scoped_workspace_higher() {
        let issue = issue();
        let workspace = workspace(false);
        let probe_pack = ProbePack {
            version: 1,
            kind: "patchbay_probe_pack".to_string(),
            status: "completed".to_string(),
            started_at: Utc::now().to_rfc3339(),
            completed_at: Utc::now().to_rfc3339(),
            workspace: "/tmp/workspace".to_string(),
            probes: Vec::new(),
            facts: ProbeFacts {
                workspace_dirty: false,
                current_branch: Some("patchbay/1-fix".to_string()),
                origin_url: Some("https://github.com/owner/repo.git".to_string()),
                tracked_file_count: Some(2),
                package_managers: vec!["cargo".to_string()],
                detected_scripts: Vec::new(),
                agent_instruction_files: vec!["AGENTS.md".to_string()],
                validation_candidates: vec![crate::probe::ValidationCandidate {
                    command: "cargo test".to_string(),
                    source: "Detected Cargo.toml".to_string(),
                    approval: "requires_user_approval".to_string(),
                }],
            },
            warnings: Vec::new(),
        };

        let readiness = assess_readiness(&issue, &workspace, &probe_pack);
        assert!(readiness.score >= 70);
        assert!(readiness
            .axes
            .iter()
            .any(|axis| axis.id == "command_safety" && axis.score == 95));
    }

    fn issue() -> GitHubIssue {
        GitHubIssue {
            id: 1,
            number: 1,
            title: "Fix".to_string(),
            body: String::new(),
            labels: Vec::new(),
            url: "https://github.com/owner/repo/issues/1".to_string(),
            repo_full_name: "owner/repo".to_string(),
            repo_name: "repo".to_string(),
            repo_description: String::new(),
            repo_stars: 0,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        }
    }

    fn workspace(dirty: bool) -> PreparedWorkspace {
        PreparedWorkspace {
            info: WorkspaceInfo {
                path: "/tmp/workspace".to_string(),
                default_branch: "main".to_string(),
                branch: "patchbay/1-fix".to_string(),
                dirty,
            },
            scan: RepoScan {
                discovered_files: vec![
                    "Cargo.toml".to_string(),
                    "Cargo.lock".to_string(),
                    "AGENTS.md".to_string(),
                    "src/lib.rs".to_string(),
                ],
                candidate_files: vec![CandidateFile {
                    path: "src/lib.rs".to_string(),
                    reason: "Path matched issue terms".to_string(),
                }],
                validation_commands: vec![ValidationCommand {
                    command: "cargo test".to_string(),
                    reason: "Detected Cargo.toml".to_string(),
                }],
                warnings: Vec::new(),
            },
            warnings: Vec::new(),
        }
    }
}
