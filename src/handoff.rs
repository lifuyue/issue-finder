use std::path::Path;

use anyhow::Result;
use chrono::{Local, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent_policy::{build_agent_policy, AgentPolicyManifest};
use crate::context_pack::{default_context_pack, write_context_pack, ContextPack};
use crate::evidence_pack::EvidencePack;
use crate::github::GitHubIssue;
use crate::llm_review::LlmReview;
use crate::paths::{atomic_write, sanitize_repo_name, PatchbayPaths};
use crate::prepare_events::PrepareEventLog;
use crate::probe::ProbePack;
use crate::readiness::{assess_readiness, ExecutionReadiness};
use crate::repo_scan::{CandidateFile, ValidationCommand};
use crate::value_scoring::{RecommendationCategory, ScoreBand, ValueAssessment};
use crate::workspace::PreparedWorkspace;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Handoff {
    pub version: u8,
    pub kind: String,
    pub id: String,
    pub created_at: String,
    pub issue: HandoffIssue,
    pub workspace: HandoffWorkspace,
    pub context: HandoffContext,
    #[serde(default = "default_context_pack")]
    pub context_pack: ContextPack,
    #[serde(default)]
    pub agent_policy: AgentPolicyManifest,
    #[serde(default)]
    pub probe_pack: ProbePack,
    #[serde(default)]
    pub readiness: ExecutionReadiness,
    pub value_assessment: ValueAssessment,
    pub evidence_pack: EvidencePack,
    pub instructions: HandoffInstructions,
    pub llm_enhancement: LlmEnhancement,
    pub llm_review: LlmReview,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HandoffIssue {
    pub repo_full_name: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub url: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HandoffWorkspace {
    pub path: String,
    pub default_branch: String,
    pub branch: String,
    pub dirty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HandoffContext {
    pub candidate_files: Vec<CandidateFile>,
    pub validation_commands: Vec<ValidationCommand>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandoffInstructions {
    pub goal: String,
    pub suggested_start: Vec<String>,
    pub constraints: Vec<String>,
    pub expected_output: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmEnhancement {
    pub status: String,
    pub summary: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct WrittenHandoff {
    pub id: String,
    pub dir: String,
    pub handoff_json_path: String,
    pub handoff_md_path: String,
    pub codex_md_path: String,
    pub agent_policy_path: String,
    pub probe_json_path: String,
    pub prepare_events_path: String,
}

impl Handoff {
    pub fn build(issue: &GitHubIssue, workspace: &PreparedWorkspace) -> Self {
        Self::build_with_value(
            issue,
            workspace,
            fallback_assessment(issue),
            EvidencePack::empty(),
            LlmReview::disabled(),
        )
    }

    pub fn build_with_value(
        issue: &GitHubIssue,
        workspace: &PreparedWorkspace,
        value_assessment: ValueAssessment,
        evidence_pack: EvidencePack,
        llm_review: LlmReview,
    ) -> Self {
        let id = handoff_id(issue);
        let mut warnings = workspace.warnings.clone();
        warnings.extend(workspace.scan.warnings.clone());
        warnings.sort();
        warnings.dedup();
        let probe_pack = ProbePack::not_run(workspace.info.path.clone());
        let agent_policy = build_agent_policy(
            &id,
            Path::new(&workspace.info.path),
            None,
            &workspace.scan.validation_commands,
            &probe_pack,
        );
        let readiness = assess_readiness(issue, workspace, &probe_pack);

        Self {
            version: 1,
            kind: "patchbay_handoff".to_string(),
            id,
            created_at: Utc::now().to_rfc3339(),
            issue: HandoffIssue {
                repo_full_name: issue.repo_full_name.clone(),
                number: issue.number,
                title: issue.title.clone(),
                body: issue.body.clone(),
                labels: issue.labels.clone(),
                url: issue.url.clone(),
                updated_at: issue.updated_at.clone(),
            },
            workspace: HandoffWorkspace {
                path: workspace.info.path.clone(),
                default_branch: workspace.info.default_branch.clone(),
                branch: workspace.info.branch.clone(),
                dirty: workspace.info.dirty,
            },
            context: HandoffContext {
                candidate_files: workspace.scan.candidate_files.clone(),
                validation_commands: workspace.scan.validation_commands.clone(),
                warnings,
            },
            context_pack: default_context_pack(),
            agent_policy,
            probe_pack,
            readiness,
            value_assessment,
            evidence_pack,
            instructions: HandoffInstructions::default(),
            llm_enhancement: LlmEnhancement::disabled(),
            llm_review,
        }
    }

    pub fn render_markdown(&self) -> String {
        let suggested_files = if self.context.candidate_files.is_empty() {
            "None detected".to_string()
        } else {
            self.context
                .candidate_files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let validation = if self.context.validation_commands.is_empty() {
            "None detected".to_string()
        } else {
            self.context
                .validation_commands
                .iter()
                .map(|command| command.command.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };

        let mut lines = vec![
            format!(
                "# Handoff: {}#{}",
                self.issue.repo_full_name, self.issue.number
            ),
            String::new(),
            "- JSON payload: ./handoff.json".to_string(),
            "- Agent policy: ./agent-policy.json".to_string(),
            "- Probe pack: ./probe.json".to_string(),
            format!("- Workspace: {}", self.workspace.path),
            format!("- Branch: {}", self.workspace.branch),
            format!("- Suggested files: {suggested_files}"),
            format!("- Suggested validation: {validation}"),
            format!(
                "- Preparation readiness: {} ({})",
                self.readiness.score, self.readiness.band
            ),
            format!("- Probe status: {}", self.probe_pack.status),
            format!(
                "- Category: {}",
                self.value_assessment.recommendation_category
            ),
            format!(
                "- Final rank score: {}",
                self.value_assessment.final_rank_score
            ),
            format!(
                "- Attention score: {} ({})",
                self.value_assessment.attention_score, self.value_assessment.attention_band
            ),
            format!(
                "- Execution score: {} ({})",
                self.value_assessment.execution_score, self.value_assessment.execution_band
            ),
            format!(
                "- Profile fit score: {}",
                self.value_assessment.profile_fit_score
            ),
            format!("- Risk penalty: {}", self.value_assessment.risk_penalty),
            format!(
                "- Risk tags: {}",
                if self.value_assessment.risk_tags.is_empty() {
                    "none".to_string()
                } else {
                    self.value_assessment
                        .risk_tags
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
            String::new(),
            "## Goal".to_string(),
            String::new(),
            self.instructions.goal.clone(),
            String::new(),
        ];

        if !self.readiness.axes.is_empty() {
            lines.extend(["## Preparation Readiness".to_string(), String::new()]);
            for axis in &self.readiness.axes {
                lines.push(format!("- {}: {} - {}", axis.id, axis.score, axis.reason));
            }
            lines.push(String::new());
        }

        if !self.evidence_pack.why_this_has_high_attention.is_empty()
            || !self.evidence_pack.why_this_is_agent_ready.is_empty()
        {
            lines.extend(["## Evidence".to_string(), String::new()]);
            for item in &self.evidence_pack.why_this_has_high_attention {
                lines.push(format!(
                    "- High attention: {} ({})",
                    item.summary,
                    item.source_refs.join(", ")
                ));
            }
            for item in &self.evidence_pack.why_this_is_agent_ready {
                lines.push(format!(
                    "- Agent-ready: {} ({})",
                    item.summary,
                    item.source_refs.join(", ")
                ));
            }
            lines.push(String::new());
        }

        if !self.evidence_pack.risk_factors.is_empty()
            || !self.evidence_pack.missing_evidence.is_empty()
        {
            lines.extend(["## Recommendation Risks".to_string(), String::new()]);
            for item in &self.evidence_pack.risk_factors {
                lines.push(format!(
                    "- {} ({})",
                    item.summary,
                    item.source_refs.join(", ")
                ));
            }
            for item in &self.evidence_pack.missing_evidence {
                lines.push(format!("- Missing evidence: {item}"));
            }
            lines.push(String::new());
        }

        if let Some(summary) = &self.llm_enhancement.summary {
            lines.extend([
                "## LLM Summary".to_string(),
                String::new(),
                summary.clone(),
                String::new(),
            ]);
        }

        if !self.context.warnings.is_empty() {
            lines.push("## Warnings".to_string());
            lines.push(String::new());
            lines.extend(
                self.context
                    .warnings
                    .iter()
                    .map(|warning| format!("- {warning}")),
            );
            lines.push(String::new());
        }

        lines.join("\n")
    }
}

fn fallback_assessment(issue: &GitHubIssue) -> ValueAssessment {
    ValueAssessment {
        final_rank_score: 0,
        attention_score: 0,
        execution_score: 0,
        profile_fit_score: 0,
        risk_penalty: 0,
        recommendation_category: RecommendationCategory::NeedsTriage,
        attention_band: ScoreBand::Low,
        execution_band: ScoreBand::Low,
        signals: Vec::new(),
        risk_tags: Vec::new(),
        missing_evidence: vec![format!(
            "Value assessment was not generated for {}#{}",
            issue.repo_full_name, issue.number
        )],
        explanation: Vec::new(),
    }
}

impl Default for HandoffInstructions {
    fn default() -> Self {
        Self {
            goal: "Investigate and fix the issue with a minimal patch.".to_string(),
            suggested_start: vec![
                "Read the issue body".to_string(),
                "Inspect candidate files".to_string(),
                "Make the smallest targeted change".to_string(),
            ],
            constraints: vec![
                "Keep changes minimal".to_string(),
                "Do not open a PR automatically".to_string(),
                "Do not overwrite unrelated local changes".to_string(),
            ],
            expected_output: vec![
                "Patch in local workspace".to_string(),
                "Validation result".to_string(),
                "PR summary draft".to_string(),
            ],
        }
    }
}

impl LlmEnhancement {
    pub fn disabled() -> Self {
        Self {
            status: "disabled".to_string(),
            summary: None,
            warnings: Vec::new(),
        }
    }

    pub fn failed(warning: impl Into<String>) -> Self {
        Self {
            status: "failed".to_string(),
            summary: None,
            warnings: vec![warning.into()],
        }
    }

    pub fn success(summary: impl Into<String>) -> Self {
        Self {
            status: "success".to_string(),
            summary: Some(summary.into()),
            warnings: Vec::new(),
        }
    }
}

pub fn handoff_id(issue: &GitHubIssue) -> String {
    format!(
        "{}-{}-{}",
        Local::now().format("%Y-%m-%d"),
        sanitize_repo_name(&issue.repo_full_name),
        issue.number
    )
}

pub fn write_handoff(
    paths: &PatchbayPaths,
    handoff: &Handoff,
    issue: &GitHubIssue,
) -> Result<WrittenHandoff> {
    write_handoff_with_events(paths, handoff, issue, None)
}

pub fn write_handoff_with_events(
    paths: &PatchbayPaths,
    handoff: &Handoff,
    issue: &GitHubIssue,
    events: Option<&PrepareEventLog>,
) -> Result<WrittenHandoff> {
    let dir = paths.inbox_item_dir(&handoff.id);
    std::fs::create_dir_all(&dir)?;
    let handoff = handoff.finalized_for_dir(&dir);

    let issue_path = dir.join("issue.json");
    let workspace_path = dir.join("workspace.json");
    let handoff_json_path = dir.join("handoff.json");
    let handoff_md_path = dir.join("handoff.md");
    let agent_policy_path = dir.join("agent-policy.json");
    let probe_json_path = dir.join("probe.json");
    let prepare_events_path = dir.join("prepare-events.jsonl");
    let owned_events = if events.is_none() {
        Some(PrepareEventLog::create(&prepare_events_path)?)
    } else {
        None
    };
    let events = events.or(owned_events.as_ref());
    if let Some(events) = owned_events.as_ref() {
        events.append_prepare_started(issue)?;
    }

    atomic_write(&issue_path, serde_json::to_vec_pretty(issue)?)?;
    atomic_write(
        &workspace_path,
        serde_json::to_vec_pretty(&handoff.workspace)?,
    )?;
    atomic_write(
        &agent_policy_path,
        serde_json::to_vec_pretty(&handoff.agent_policy)?,
    )?;
    if let Some(events) = events {
        events.append(
            "agent_policy_written",
            &[(
                "path",
                Value::String(agent_policy_path.to_string_lossy().to_string()),
            )],
        )?;
    }
    atomic_write(
        &probe_json_path,
        serde_json::to_vec_pretty(&handoff.probe_pack)?,
    )?;
    if let Some(events) = events {
        events.append(
            "probe_written",
            &[(
                "path",
                Value::String(probe_json_path.to_string_lossy().to_string()),
            )],
        )?;
    }
    atomic_write(&handoff_json_path, serde_json::to_vec_pretty(&handoff)?)?;
    atomic_write(&handoff_md_path, handoff.render_markdown())?;
    let written_pack = write_context_pack(&dir, &handoff, issue)?;
    if let Some(events) = events {
        events.append(
            "handoff_written",
            &[(
                "path",
                Value::String(handoff_json_path.to_string_lossy().to_string()),
            )],
        )?;
    }

    Ok(WrittenHandoff {
        id: handoff.id.clone(),
        dir: dir.to_string_lossy().to_string(),
        handoff_json_path: handoff_json_path.to_string_lossy().to_string(),
        handoff_md_path: handoff_md_path.to_string_lossy().to_string(),
        codex_md_path: written_pack.codex_md_path,
        agent_policy_path: agent_policy_path.to_string_lossy().to_string(),
        probe_json_path: probe_json_path.to_string_lossy().to_string(),
        prepare_events_path: prepare_events_path.to_string_lossy().to_string(),
    })
}

impl Handoff {
    fn finalized_for_dir(&self, dir: &Path) -> Self {
        let mut handoff = self.clone();
        handoff.agent_policy = build_agent_policy(
            &handoff.id,
            Path::new(&handoff.workspace.path),
            Some(dir),
            &handoff.context.validation_commands,
            &handoff.probe_pack,
        );
        handoff
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::Handoff;
    use crate::github::GitHubIssue;
    use crate::repo_scan::{CandidateFile, RepoScan, ValidationCommand};
    use crate::workspace::{PreparedWorkspace, WorkspaceInfo};

    #[test]
    fn builds_canonical_handoff_json_shape() {
        let issue = GitHubIssue {
            id: 1,
            number: 123,
            title: "Fix accessible button label".to_string(),
            body: "Body".to_string(),
            labels: vec!["good first issue".to_string()],
            url: "https://github.com/owner/repo/issues/123".to_string(),
            repo_full_name: "owner/repo".to_string(),
            repo_name: "repo".to_string(),
            repo_description: String::new(),
            repo_stars: 0,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        };
        let workspace = PreparedWorkspace {
            info: WorkspaceInfo {
                path: "/tmp/repo".to_string(),
                default_branch: "main".to_string(),
                branch: "patchbay/123-fix-accessible-button-label".to_string(),
                dirty: false,
            },
            scan: RepoScan {
                discovered_files: vec!["src/button.rs".to_string()],
                candidate_files: vec![CandidateFile {
                    path: "src/button.rs".to_string(),
                    reason: "Path matched issue terms".to_string(),
                }],
                validation_commands: vec![ValidationCommand {
                    command: "cargo test".to_string(),
                    reason: "Detected Cargo.toml".to_string(),
                }],
                warnings: Vec::new(),
            },
            warnings: Vec::new(),
        };

        let handoff = Handoff::build(&issue, &workspace);
        assert_eq!(handoff.version, 1);
        assert_eq!(handoff.kind, "patchbay_handoff");
        assert_eq!(handoff.context_pack.version, 1);
        assert_eq!(
            handoff.context_pack.kind,
            "patchbay_progressive_handoff_pack"
        );
        assert_eq!(handoff.context.candidate_files[0].path, "src/button.rs");
        assert!(handoff
            .render_markdown()
            .contains("JSON payload: ./handoff.json"));
    }
}
