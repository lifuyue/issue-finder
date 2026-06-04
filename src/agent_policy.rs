use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::probe::ProbePack;
use crate::repo_scan::ValidationCommand;

const POLICY_KIND: &str = "patchbay_agent_policy";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentPolicyManifest {
    pub version: u8,
    pub kind: String,
    pub handoff_id: String,
    pub permission_profile: PermissionProfile,
    pub commands: CommandPolicy,
    pub agent_constraints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionProfile {
    pub filesystem: FilesystemPolicy,
    pub network: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilesystemPolicy {
    pub read_roots: Vec<String>,
    pub write_roots: Vec<String>,
    pub protected_roots: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandPolicy {
    pub allowed_low_risk: Vec<AllowedCommand>,
    pub requires_user_approval: Vec<ApprovalCommand>,
    pub forbidden: Vec<ForbiddenCommand>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllowedCommand {
    pub argv: Vec<String>,
    pub cwd: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalCommand {
    pub command: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForbiddenCommand {
    pub pattern: String,
    pub reason: String,
}

impl Default for AgentPolicyManifest {
    fn default() -> Self {
        Self {
            version: 1,
            kind: POLICY_KIND.to_string(),
            handoff_id: String::new(),
            permission_profile: PermissionProfile {
                filesystem: FilesystemPolicy {
                    read_roots: Vec::new(),
                    write_roots: Vec::new(),
                    protected_roots: Vec::new(),
                },
                network: "requires_user_approval".to_string(),
            },
            commands: CommandPolicy {
                allowed_low_risk: Vec::new(),
                requires_user_approval: Vec::new(),
                forbidden: default_forbidden_commands(),
            },
            agent_constraints: default_agent_constraints(),
        }
    }
}

pub fn build_agent_policy(
    handoff_id: &str,
    workspace: &Path,
    inbox_item_dir: Option<&Path>,
    validation_commands: &[ValidationCommand],
    probe_pack: &ProbePack,
) -> AgentPolicyManifest {
    let workspace = absoluteish(workspace);
    let mut read_roots = vec![display_path(&workspace)];
    let mut protected_roots = vec![
        display_path(&workspace.join(".git")),
        display_path(&workspace.join(".agents")),
        display_path(&workspace.join(".codex")),
    ];

    if let Some(inbox_item_dir) = inbox_item_dir {
        let inbox_item_dir = absoluteish(inbox_item_dir);
        read_roots.push(display_path(&inbox_item_dir));
        protected_roots.push(display_path(&inbox_item_dir));
        protected_roots.push(display_path(&inbox_item_dir.join("context")));
    }

    read_roots.sort();
    read_roots.dedup();
    protected_roots.sort();
    protected_roots.dedup();

    AgentPolicyManifest {
        version: 1,
        kind: POLICY_KIND.to_string(),
        handoff_id: handoff_id.to_string(),
        permission_profile: PermissionProfile {
            filesystem: FilesystemPolicy {
                read_roots,
                write_roots: vec![display_path(&workspace)],
                protected_roots,
            },
            network: "requires_user_approval".to_string(),
        },
        commands: CommandPolicy {
            allowed_low_risk: allowed_probe_commands(probe_pack),
            requires_user_approval: approval_commands(validation_commands),
            forbidden: default_forbidden_commands(),
        },
        agent_constraints: default_agent_constraints(),
    }
}

fn allowed_probe_commands(probe_pack: &ProbePack) -> Vec<AllowedCommand> {
    let mut seen = HashSet::new();
    let mut commands = Vec::new();
    for probe in &probe_pack.probes {
        let key = probe.argv.join("\0");
        if probe.argv.is_empty() || !seen.insert(key) {
            continue;
        }
        commands.push(AllowedCommand {
            argv: probe.argv.clone(),
            cwd: probe.cwd.clone(),
            reason: probe_reason(&probe.id).to_string(),
        });
    }
    commands
}

fn approval_commands(validation_commands: &[ValidationCommand]) -> Vec<ApprovalCommand> {
    let mut seen = HashSet::new();
    let mut commands = Vec::new();
    for command in validation_commands {
        if seen.insert(command.command.clone()) {
            commands.push(ApprovalCommand {
                command: command.command.clone(),
                reason: format!(
                    "{}; detected validation may execute repository code",
                    command.reason
                ),
            });
        }
    }
    commands
}

fn probe_reason(id: &str) -> &'static str {
    match id {
        "git_status_porcelain" => "Read workspace dirty state.",
        "git_branch_show_current" => "Read current git branch.",
        "git_ls_files" => "Read tracked file list.",
        "git_remote_get_url_origin" => "Read origin remote URL.",
        "npm_pkg_get_scripts" => "Read package.json scripts without running them.",
        "pnpm_pkg_get_scripts" => "Read package.json scripts without running them.",
        _ => "Read low-risk repository metadata.",
    }
}

fn default_forbidden_commands() -> Vec<ForbiddenCommand> {
    vec![
        ForbiddenCommand {
            pattern: "install dependencies".to_string(),
            reason: "Patchbay does not install dependencies or ask agents to install without user approval."
                .to_string(),
        },
        ForbiddenCommand {
            pattern: "project-defined scripts".to_string(),
            reason: "Patchbay detects scripts but does not run repository scripts during prepare."
                .to_string(),
        },
        ForbiddenCommand {
            pattern: "commit, push, or create pull request".to_string(),
            reason: "Patchbay prepares handoff artifacts only.".to_string(),
        },
        ForbiddenCommand {
            pattern: "modify Patchbay inbox or generated context files".to_string(),
            reason: "Generated handoff artifacts are protected context, not target source."
                .to_string(),
        },
        ForbiddenCommand {
            pattern: "destructive filesystem changes".to_string(),
            reason: "Destructive operations are outside Patchbay's preparation boundary."
                .to_string(),
        },
    ]
}

fn default_agent_constraints() -> Vec<String> {
    vec![
        "Do not modify Patchbay inbox files.".to_string(),
        "Do not modify .git, .agents, .codex, or generated context files.".to_string(),
        "Ask the user before running commands that require network, dependency installation, tests, build, or lint.".to_string(),
    ]
}

fn absoluteish(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::build_agent_policy;
    use crate::probe::{ProbeFacts, ProbePack, ProbeResult};
    use crate::repo_scan::ValidationCommand;

    #[test]
    fn protects_workspace_metadata_and_inbox_roots() {
        let workspace = Path::new("/tmp/workspace");
        let inbox = Path::new("/tmp/patchbay/inbox/item");
        let policy = build_agent_policy(
            "handoff",
            workspace,
            Some(inbox),
            &[ValidationCommand {
                command: "cargo test".to_string(),
                reason: "Detected Cargo.toml".to_string(),
            }],
            &probe_pack(workspace),
        );

        assert!(policy
            .permission_profile
            .filesystem
            .protected_roots
            .contains(&"/tmp/workspace/.git".to_string()));
        assert!(policy
            .permission_profile
            .filesystem
            .protected_roots
            .contains(&"/tmp/workspace/.agents".to_string()));
        assert!(policy
            .permission_profile
            .filesystem
            .protected_roots
            .contains(&"/tmp/workspace/.codex".to_string()));
        assert!(policy
            .permission_profile
            .filesystem
            .protected_roots
            .contains(&"/tmp/patchbay/inbox/item".to_string()));
        assert_eq!(
            policy.commands.requires_user_approval[0].command,
            "cargo test"
        );
        assert!(policy.commands.allowed_low_risk[0]
            .argv
            .starts_with(&["git".to_string(), "status".to_string()]));
    }

    fn probe_pack(workspace: &Path) -> ProbePack {
        ProbePack {
            version: 1,
            kind: "patchbay_probe_pack".to_string(),
            status: "completed".to_string(),
            started_at: "2026-06-04T00:00:00Z".to_string(),
            completed_at: "2026-06-04T00:00:01Z".to_string(),
            workspace: workspace.to_string_lossy().to_string(),
            probes: vec![ProbeResult {
                id: "git_status_porcelain".to_string(),
                argv: vec![
                    "git".to_string(),
                    "status".to_string(),
                    "--porcelain".to_string(),
                ],
                cwd: workspace.to_string_lossy().to_string(),
                exit_code: Some(0),
                duration_ms: 1,
                stdout_excerpt: String::new(),
                stderr_excerpt: String::new(),
                risk: "low".to_string(),
                timed_out: false,
                warnings: Vec::new(),
            }],
            facts: ProbeFacts::default(),
            warnings: Vec::new(),
        }
    }
}
