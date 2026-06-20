use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::evidence_pack::EvidenceItem;
use crate::github::GitHubIssue;
use crate::handoff::Handoff;
use crate::paths::atomic_write;

const PACK_KIND: &str = "issue_finder_progressive_handoff_pack";
const SKILL_NAME: &str = "issue-finder";
const CODEX_ENTRY: &str = "codex.md";
const CONTEXT_DIR: &str = "context";
const SKILL_PATH: &str = ".agents/skills/issue-finder/SKILL.md";
const REFS_PATH: &str = ".agents/skills/issue-finder/refs.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextPack {
    pub version: u8,
    pub kind: String,
    pub disclosure: String,
    pub entrypoint: String,
    pub context_dir: String,
    pub skill: ContextPackSkill,
    pub files: Vec<ContextPackFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextPackSkill {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextPackFile {
    pub id: String,
    pub path: String,
    pub default_visible: bool,
    pub defer_loading: bool,
}

#[derive(Debug, Clone)]
pub struct WrittenContextPack {
    pub codex_md_path: String,
}

#[derive(Debug, Serialize)]
struct SkillRefs {
    version: u8,
    skill: String,
    handoff_id: String,
    default_load: Vec<String>,
    deferred: Vec<DeferredRef>,
}

#[derive(Debug, Serialize)]
struct DeferredRef {
    id: String,
    path: String,
    load_when: String,
}

pub fn default_context_pack() -> ContextPack {
    ContextPack {
        version: 1,
        kind: PACK_KIND.to_string(),
        disclosure: "progressive".to_string(),
        entrypoint: format!("./{CODEX_ENTRY}"),
        context_dir: format!("./{CONTEXT_DIR}"),
        skill: ContextPackSkill {
            name: SKILL_NAME.to_string(),
            path: format!("./{SKILL_PATH}"),
        },
        files: vec![
            context_file("entry", true, false),
            context_file("safety", true, false),
            context_file("probe", true, false),
            context_file("value", false, true),
            context_file("issue", false, true),
            context_file("repo", false, true),
            context_file("validation", false, true),
        ],
    }
}

pub fn write_context_pack(
    dir: &Path,
    handoff: &Handoff,
    issue: &GitHubIssue,
) -> Result<WrittenContextPack> {
    let codex_path = dir.join(CODEX_ENTRY);
    let context_dir = dir.join(CONTEXT_DIR);
    let skill_path = dir.join(SKILL_PATH);
    let refs_path = dir.join(REFS_PATH);

    atomic_write(&codex_path, render_codex_md(dir, handoff, &skill_path)?)?;
    atomic_write(&context_dir.join("entry.md"), render_entry_md(handoff))?;
    atomic_write(&context_dir.join("value.md"), render_value_md(handoff))?;
    atomic_write(
        &context_dir.join("issue.md"),
        render_issue_md(handoff, issue),
    )?;
    atomic_write(&context_dir.join("repo.md"), render_repo_md(handoff, issue))?;
    atomic_write(
        &context_dir.join("validation.md"),
        render_validation_md(handoff),
    )?;
    atomic_write(&context_dir.join("safety.md"), render_safety_md(handoff))?;
    atomic_write(&context_dir.join("probe.md"), render_probe_md(handoff))?;
    atomic_write(&skill_path, render_skill_md())?;
    atomic_write(&refs_path, serde_json::to_vec_pretty(&skill_refs(handoff))?)?;

    Ok(WrittenContextPack {
        codex_md_path: display_path(&codex_path)?,
    })
}

fn context_file(id: &str, default_visible: bool, defer_loading: bool) -> ContextPackFile {
    ContextPackFile {
        id: id.to_string(),
        path: format!("./{CONTEXT_DIR}/{id}.md"),
        default_visible,
        defer_loading,
    }
}

fn render_codex_md(dir: &Path, handoff: &Handoff, skill_path: &Path) -> Result<String> {
    let dir = display_path(dir)?;
    let skill_path = display_path(skill_path)?;
    let entry_path = display_path(&PathBuf::from(&dir).join("context/entry.md"))?;
    let safety_path = display_path(&PathBuf::from(&dir).join("context/safety.md"))?;
    let probe_path = display_path(&PathBuf::from(&dir).join("context/probe.md"))?;
    let handoff_json_path = display_path(&PathBuf::from(&dir).join("handoff.json"))?;
    let handoff_md_path = display_path(&PathBuf::from(&dir).join("handoff.md"))?;
    let agent_policy_path = display_path(&PathBuf::from(&dir).join("agent-policy.json"))?;
    let probe_json_path = display_path(&PathBuf::from(&dir).join("probe.json"))?;

    Ok(vec![
        "# Issue Finder Codex Entry".to_string(),
        String::new(),
        format!(
            "- Issue: {}#{} - {}",
            handoff.issue.repo_full_name,
            handoff.issue.number,
            single_line(&handoff.issue.title)
        ),
        format!("- URL: {}", handoff.issue.url),
        format!("- Workspace: {}", handoff.workspace.path),
        format!("- Branch: {}", handoff.workspace.branch),
        format!(
            "- Category: {} | rank {} | attention {} | execution {} | risk {}",
            handoff.value_assessment.recommendation_category,
            handoff.value_assessment.final_rank_score,
            handoff.value_assessment.attention_score,
            handoff.value_assessment.execution_score,
            handoff.value_assessment.risk_penalty
        ),
        format!(
            "- Feed score: {} | freshness +{} | feedback -{} | quality -{} | reactivation +{} | visibility {}",
            handoff.recommendation.final_feed_score,
            handoff.recommendation.freshness_boost,
            handoff.recommendation.feedback_penalty,
            handoff.recommendation.quality_penalty,
            handoff.recommendation.reactivation_boost,
            handoff.recommendation.visibility
        ),
        format!("- Handoff pack: {dir}"),
        format!("- Handoff JSON: {handoff_json_path}"),
        format!("- Handoff Markdown: {handoff_md_path}"),
        format!("- Agent policy: {agent_policy_path}"),
        format!("- Probe pack: {probe_json_path}"),
        format!("- Skill: {skill_path}"),
        String::new(),
        "Use the local skill at:".to_string(),
        skill_path,
        String::new(),
        "Start with:".to_string(),
        entry_path,
        safety_path,
        probe_path,
        String::new(),
        "Do not read every context file at once. Load value, issue, repo, and validation context only when the work reaches that phase.".to_string(),
    ]
    .join("\n"))
}

fn render_entry_md(handoff: &Handoff) -> String {
    let mut lines = vec![
        "# Entry".to_string(),
        String::new(),
        "## Task".to_string(),
        String::new(),
        handoff.instructions.goal.clone(),
        String::new(),
        format!(
            "- Issue: {}#{} - {}",
            handoff.issue.repo_full_name,
            handoff.issue.number,
            single_line(&handoff.issue.title)
        ),
        format!("- URL: {}", handoff.issue.url),
        String::new(),
        "## Why This Was Selected".to_string(),
        String::new(),
        format!(
            "- Category: {}",
            handoff.value_assessment.recommendation_category
        ),
        format!(
            "- Final rank score: {}",
            handoff.value_assessment.final_rank_score
        ),
        format!(
            "- Attention score: {} ({})",
            handoff.value_assessment.attention_score, handoff.value_assessment.attention_band
        ),
        format!(
            "- Execution score: {} ({})",
            handoff.value_assessment.execution_score, handoff.value_assessment.execution_band
        ),
        format!("- Risk penalty: {}", handoff.value_assessment.risk_penalty),
    ];
    push_recommendation_summary(&mut lines, handoff);

    if let Some(summary) = first_nonempty(&handoff.value_assessment.explanation) {
        lines.push(format!("- Summary: {}", single_line(summary)));
    } else if let Some(item) = handoff.evidence_pack.why_this_has_high_attention.first() {
        lines.push(format!("- Summary: {}", single_line(&item.summary)));
    }

    lines.extend([
        String::new(),
        "## Workspace".to_string(),
        String::new(),
        format!("- Path: {}", handoff.workspace.path),
        format!("- Branch: {}", handoff.workspace.branch),
        format!("- Dirty: {}", handoff.workspace.dirty),
        "- Candidate files:".to_string(),
    ]);
    push_candidate_files(&mut lines, handoff, 5);

    lines.extend([
        String::new(),
        "## Next Reads".to_string(),
        String::new(),
        "- Read context/repo.md before planning code changes.".to_string(),
        "- Read context/probe.md before deciding which commands to run.".to_string(),
        "- Read context/issue.md when you need the full issue body.".to_string(),
        "- Read context/value.md only when reviewing priority or explaining why this is worth doing."
            .to_string(),
        "- Read context/validation.md before running validation.".to_string(),
        String::new(),
        "## Safety".to_string(),
        String::new(),
        "- Do not treat Issue Finder-generated files as target repository source.".to_string(),
        "- Respect agent-policy.json for allowed, approval-required, and forbidden actions."
            .to_string(),
        "- Do not install dependencies, commit, push, or create a PR unless the user explicitly asks.".to_string(),
        "- If the workspace is dirty, explain the risk before editing.".to_string(),
    ]);

    lines.join("\n")
}

fn render_value_md(handoff: &Handoff) -> String {
    let mut lines = vec![
        "# Recommendation Assessment".to_string(),
        String::new(),
        "## Assessment".to_string(),
        String::new(),
        format!(
            "- Category: {}",
            handoff.value_assessment.recommendation_category
        ),
        format!(
            "- Final rank score: {}",
            handoff.value_assessment.final_rank_score
        ),
        format!(
            "- Attention score: {} ({})",
            handoff.value_assessment.attention_score, handoff.value_assessment.attention_band
        ),
        format!(
            "- Execution score: {} ({})",
            handoff.value_assessment.execution_score, handoff.value_assessment.execution_band
        ),
        format!(
            "- Profile fit score: {}",
            handoff.value_assessment.profile_fit_score
        ),
        format!("- Risk penalty: {}", handoff.value_assessment.risk_penalty),
        String::new(),
        "## Feed Recommendation".to_string(),
        String::new(),
        format!("- Feed score: {}", handoff.recommendation.final_feed_score),
        format!(
            "- Freshness boost: +{}",
            handoff.recommendation.freshness_boost
        ),
        format!(
            "- Feedback penalty: -{}",
            handoff.recommendation.feedback_penalty
        ),
        format!(
            "- Quality penalty: -{}",
            handoff.recommendation.quality_penalty
        ),
        format!(
            "- Reactivation boost: +{}",
            handoff.recommendation.reactivation_boost
        ),
        format!("- Visibility: {}", handoff.recommendation.visibility),
        String::new(),
        "## Feed Reasons".to_string(),
        String::new(),
    ];
    push_string_list(
        &mut lines,
        &handoff.recommendation.reasons,
        "No feed recommendation reasons were recorded.",
    );

    lines.extend([
        String::new(),
        "## Memory Context".to_string(),
        String::new(),
    ]);
    let memory_refs = handoff
        .memory_context
        .approved_hints
        .iter()
        .map(|hint| format!("{} ({})", hint.id, hint.hint_type))
        .collect::<Vec<_>>();
    push_string_list(
        &mut lines,
        &memory_refs,
        "No approved memory hints were attached.",
    );

    lines.extend([
        String::new(),
        "## Value Explanation".to_string(),
        String::new(),
    ]);
    push_string_list(
        &mut lines,
        &handoff.value_assessment.explanation,
        "No value explanation was generated.",
    );

    lines.extend([
        String::new(),
        "## Attention Signals".to_string(),
        String::new(),
    ]);
    push_signal_axis(
        &mut lines,
        handoff,
        crate::value_signals::SignalAxis::Attention,
    );

    lines.extend([
        String::new(),
        "## Execution Signals".to_string(),
        String::new(),
    ]);
    push_signal_axis(
        &mut lines,
        handoff,
        crate::value_signals::SignalAxis::Execution,
    );

    lines.extend([
        String::new(),
        "## Profile Fit Signals".to_string(),
        String::new(),
    ]);
    push_signal_axis(
        &mut lines,
        handoff,
        crate::value_signals::SignalAxis::ProfileFit,
    );

    lines.extend([String::new(), "## All Signals".to_string(), String::new()]);
    if handoff.value_assessment.signals.is_empty() {
        lines.push("- No recommendation signals were generated.".to_string());
    } else {
        for signal in &handoff.value_assessment.signals {
            lines.push(format!(
                "- {:?} ({:?}, delta {}): {}{}",
                signal.kind,
                signal.axis,
                signal.score_delta,
                single_line(&signal.summary),
                refs_suffix(&signal.evidence_refs)
            ));
        }
    }

    lines.extend([String::new(), "## Risks".to_string(), String::new()]);
    if handoff.value_assessment.risk_tags.is_empty() {
        lines.push("- No risk tags were identified.".to_string());
    } else {
        for tag in &handoff.value_assessment.risk_tags {
            lines.push(format!("- {tag}"));
        }
    }

    lines.extend([
        String::new(),
        "## Missing Evidence".to_string(),
        String::new(),
    ]);
    push_string_list(
        &mut lines,
        &handoff.value_assessment.missing_evidence,
        "No missing evidence was recorded.",
    );

    lines.extend([
        String::new(),
        "## Evidence Pack: High Attention".to_string(),
        String::new(),
    ]);
    push_evidence_items(
        &mut lines,
        &handoff.evidence_pack.why_this_has_high_attention,
        "No high-attention evidence was recorded.",
    );

    lines.extend([
        String::new(),
        "## Evidence Pack: Agent Ready".to_string(),
        String::new(),
    ]);
    push_evidence_items(
        &mut lines,
        &handoff.evidence_pack.why_this_is_agent_ready,
        "No agent-ready evidence was recorded.",
    );

    lines.extend([
        String::new(),
        "## Evidence Pack: Risk Factors".to_string(),
        String::new(),
    ]);
    push_evidence_items(
        &mut lines,
        &handoff.evidence_pack.risk_factors,
        "No evidence-pack risk factors were recorded.",
    );

    lines.join("\n")
}

fn render_issue_md(handoff: &Handoff, issue: &GitHubIssue) -> String {
    let mut lines = vec![
        "# Issue".to_string(),
        String::new(),
        format!("- Repo: {}", handoff.issue.repo_full_name),
        format!("- Number: {}", handoff.issue.number),
        format!("- Title: {}", single_line(&handoff.issue.title)),
        format!("- URL: {}", handoff.issue.url),
        format!(
            "- Labels: {}",
            if handoff.issue.labels.is_empty() {
                "none".to_string()
            } else {
                handoff.issue.labels.join(", ")
            }
        ),
        format!("- Updated at: {}", handoff.issue.updated_at),
        String::new(),
        "## Body".to_string(),
        String::new(),
    ];

    if handoff.issue.body.trim().is_empty() {
        lines.push("_No issue body was provided._".to_string());
    } else {
        lines.push(handoff.issue.body.clone());
    }

    lines.extend([
        String::new(),
        "## Enrichment Summary".to_string(),
        String::new(),
        format!("- Repository description: {}", issue.repo_description),
        format!("- Stars: {}", issue.repo_stars),
        format!(
            "- LLM enhancement status: {}",
            handoff.llm_enhancement.status
        ),
        format!("- LLM review status: {}", handoff.llm_review.status),
    ]);
    if let Some(summary) = &handoff.llm_enhancement.summary {
        lines.push(format!("- LLM summary: {}", single_line(summary)));
    }
    if let Some(summary) = &handoff.llm_review.review_summary {
        lines.push(format!("- LLM review: {}", single_line(summary)));
    }

    lines.join("\n")
}

fn render_repo_md(handoff: &Handoff, issue: &GitHubIssue) -> String {
    let mut lines = vec![
        "# Repository".to_string(),
        String::new(),
        "## Workspace".to_string(),
        String::new(),
        format!("- Path: {}", handoff.workspace.path),
        format!("- Default branch: {}", handoff.workspace.default_branch),
        format!("- Issue Finder branch: {}", handoff.workspace.branch),
        format!("- Dirty: {}", handoff.workspace.dirty),
        String::new(),
        "## Repository Context".to_string(),
        String::new(),
        format!("- Full name: {}", handoff.issue.repo_full_name),
        format!("- Name: {}", issue.repo_name),
        format!("- Description: {}", issue.repo_description),
        format!("- Stars: {}", issue.repo_stars),
        String::new(),
        "## Candidate Files".to_string(),
        String::new(),
    ];
    push_candidate_files(&mut lines, handoff, usize::MAX);

    lines.extend([String::new(), "## Scan Warnings".to_string(), String::new()]);
    push_string_list(
        &mut lines,
        &handoff.context.warnings,
        "No scan warnings were recorded.",
    );

    lines.join("\n")
}

fn render_validation_md(handoff: &Handoff) -> String {
    let mut lines = vec![
        "# Validation".to_string(),
        String::new(),
        "Issue Finder suggests validation commands only. It does not run them automatically."
            .to_string(),
        "Detected validation commands require user approval because they may execute repository code."
            .to_string(),
        String::new(),
        "## Commands".to_string(),
        String::new(),
    ];

    if handoff.context.validation_commands.is_empty() {
        lines.push(
            "- No validation commands were detected. Start with the repository's own README, package manifest, or existing test conventions before inventing a broader validation flow."
                .to_string(),
        );
    } else {
        for command in &handoff.context.validation_commands {
            lines.push(format!(
                "- `{}`: {}",
                command.command,
                single_line(&command.reason)
            ));
        }
    }

    lines.join("\n")
}

fn render_probe_md(handoff: &Handoff) -> String {
    let facts = &handoff.probe_pack.facts;
    let mut lines = vec![
        "# Probe".to_string(),
        String::new(),
        format!("- Status: {}", handoff.probe_pack.status),
        format!("- Workspace: {}", handoff.probe_pack.workspace),
        format!(
            "- Started: {} | completed: {}",
            handoff.probe_pack.started_at, handoff.probe_pack.completed_at
        ),
        format!("- Workspace dirty: {}", facts.workspace_dirty),
        format!(
            "- Current branch: {}",
            facts.current_branch.as_deref().unwrap_or("unknown")
        ),
        format!(
            "- Origin URL: {}",
            facts.origin_url.as_deref().unwrap_or("unknown")
        ),
        format!(
            "- Tracked files: {}",
            facts
                .tracked_file_count
                .map(|count| count.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ),
        format!(
            "- Package managers: {}",
            if facts.package_managers.is_empty() {
                "none detected".to_string()
            } else {
                facts.package_managers.join(", ")
            }
        ),
        String::new(),
        "## Probes Run".to_string(),
        String::new(),
    ];

    if handoff.probe_pack.probes.is_empty() {
        lines.push("- No safe probes were run.".to_string());
    } else {
        for probe in &handoff.probe_pack.probes {
            lines.push(format!(
                "- `{}`: exit {:?}, {} ms, risk {}, warnings {}",
                probe.argv.join(" "),
                probe.exit_code,
                probe.duration_ms,
                probe.risk,
                if probe.warnings.is_empty() {
                    "none".to_string()
                } else {
                    probe.warnings.join("; ")
                }
            ));
        }
    }

    lines.extend([
        String::new(),
        "## Agent Instruction Files".to_string(),
        String::new(),
    ]);
    push_string_list(
        &mut lines,
        &facts.agent_instruction_files,
        "No agent instruction files were detected.",
    );

    lines.extend([
        String::new(),
        "## Detected Scripts".to_string(),
        String::new(),
    ]);
    if facts.detected_scripts.is_empty() {
        lines.push("- No package scripts were detected.".to_string());
    } else {
        for script in &facts.detected_scripts {
            lines.push(format!(
                "- `{}`: {} ({})",
                script.name,
                single_line(&script.command),
                script.approval
            ));
        }
    }

    lines.extend([
        String::new(),
        "## Validation Candidates".to_string(),
        String::new(),
    ]);
    if facts.validation_candidates.is_empty() {
        lines.push("- No validation candidates were detected.".to_string());
    } else {
        for candidate in &facts.validation_candidates {
            lines.push(format!(
                "- `{}`: {} ({})",
                candidate.command,
                single_line(&candidate.source),
                candidate.approval
            ));
        }
    }

    lines.extend([
        String::new(),
        "## Commands Not Run".to_string(),
        String::new(),
        "- Issue Finder did not run tests, lint, build, install, project-defined scripts, or commands inferred from issue text.".to_string(),
        "- Run validation only after checking agent-policy.json and getting user approval when required.".to_string(),
        String::new(),
        "## Warnings".to_string(),
        String::new(),
    ]);
    push_string_list(
        &mut lines,
        &handoff.probe_pack.warnings,
        "No probe warnings were recorded.",
    );

    lines.join("\n")
}

fn render_safety_md(handoff: &Handoff) -> String {
    [
        "# Safety".to_string(),
        String::new(),
        "- Issue Finder prepares local workspaces and handoff artifacts only.".to_string(),
        "- Issue Finder does not install dependencies, commit, push, or create PRs.".to_string(),
        "- Agent policy manifest: ../agent-policy.json".to_string(),
        "- Probe pack: ../probe.json".to_string(),
        "- Do not treat Issue Finder-generated files under the inbox as target repository source files.".to_string(),
        format!("- Target workspace: {}", handoff.workspace.path),
        format!("- Target branch: {}", handoff.workspace.branch),
        format!("- Workspace dirty: {}", handoff.workspace.dirty),
        "- If the workspace is dirty, inspect and explain the risk before modifying files.".to_string(),
        "- If validation needs network access, long-running commands, dependency installation, or destructive operations, explain the tradeoff and get user confirmation first.".to_string(),
    ]
    .join("\n")
}

fn render_skill_md() -> String {
    vec![
        "# issue-finder".to_string(),
        String::new(),
        "Use this skill when the user provides a Issue Finder handoff directory, codex.md, or inbox item.".to_string(),
        String::new(),
        "1. Read context/entry.md and context/safety.md first.".to_string(),
        "2. Do not read every context file at once.".to_string(),
        "3. Read context/probe.md before deciding which commands to run.".to_string(),
        "4. Read context/value.md only when assessing why the issue is worth doing or explaining priority.".to_string(),
        "5. Read context/issue.md when you need the original issue body and issue metadata.".to_string(),
        "6. Read context/repo.md before planning code changes.".to_string(),
        "7. Read context/validation.md before running validation.".to_string(),
        "8. Keep Issue Finder and coding-agent responsibilities separate: Issue Finder prepares evidence and local handoff files; the coding agent performs user-directed code work in the target workspace.".to_string(),
        String::new(),
        "Issue Finder-generated inbox files are context, not target repository source files.".to_string(),
    ]
    .join("\n")
}

fn skill_refs(handoff: &Handoff) -> SkillRefs {
    SkillRefs {
        version: 1,
        skill: SKILL_NAME.to_string(),
        handoff_id: handoff.id.clone(),
        default_load: vec![
            "context/entry.md".to_string(),
            "context/safety.md".to_string(),
            "context/probe.md".to_string(),
        ],
        deferred: vec![
            DeferredRef {
                id: "value".to_string(),
                path: "context/value.md".to_string(),
                load_when: "Assessing why this issue is worth doing".to_string(),
            },
            DeferredRef {
                id: "issue".to_string(),
                path: "context/issue.md".to_string(),
                load_when: "Reading the original issue context".to_string(),
            },
            DeferredRef {
                id: "repo".to_string(),
                path: "context/repo.md".to_string(),
                load_when: "Planning code changes".to_string(),
            },
            DeferredRef {
                id: "validation".to_string(),
                path: "context/validation.md".to_string(),
                load_when: "Choosing or running validation".to_string(),
            },
        ],
    }
}

fn push_candidate_files(lines: &mut Vec<String>, handoff: &Handoff, limit: usize) {
    if handoff.context.candidate_files.is_empty() {
        lines.push("- None detected.".to_string());
        return;
    }

    for file in handoff.context.candidate_files.iter().take(limit) {
        lines.push(format!("- {}: {}", file.path, single_line(&file.reason)));
    }
    if handoff.context.candidate_files.len() > limit {
        lines.push(format!(
            "- {} more candidate file(s) are listed in context/repo.md.",
            handoff.context.candidate_files.len() - limit
        ));
    }
}

fn push_recommendation_summary(lines: &mut Vec<String>, handoff: &Handoff) {
    lines.extend([
        format!("- Feed score: {}", handoff.recommendation.final_feed_score),
        format!(
            "- Feed freshness: +{}",
            handoff.recommendation.freshness_boost
        ),
        format!(
            "- Feed feedback penalty: -{}",
            handoff.recommendation.feedback_penalty
        ),
        format!(
            "- Feed quality penalty: -{}",
            handoff.recommendation.quality_penalty
        ),
        format!(
            "- Feed reactivation: +{}",
            handoff.recommendation.reactivation_boost
        ),
        format!("- Feed visibility: {}", handoff.recommendation.visibility),
    ]);
}

fn push_string_list(lines: &mut Vec<String>, values: &[String], fallback: &str) {
    if values.is_empty() {
        lines.push(format!("- {fallback}"));
    } else {
        for value in values {
            lines.push(format!("- {}", single_line(value)));
        }
    }
}

fn push_evidence_items(lines: &mut Vec<String>, values: &[EvidenceItem], fallback: &str) {
    if values.is_empty() {
        lines.push(format!("- {fallback}"));
    } else {
        for item in values {
            lines.push(format!(
                "- {}{}",
                single_line(&item.summary),
                refs_suffix(&item.source_refs)
            ));
        }
    }
}

fn push_signal_axis(
    lines: &mut Vec<String>,
    handoff: &Handoff,
    axis: crate::value_signals::SignalAxis,
) {
    let mut matched = handoff
        .value_assessment
        .signals
        .iter()
        .filter(|signal| signal.axis == axis)
        .peekable();
    if matched.peek().is_none() {
        lines.push("- None recorded.".to_string());
    } else {
        for signal in matched {
            lines.push(format!(
                "- {:?} (delta {}): {}{}",
                signal.kind,
                signal.score_delta,
                single_line(&signal.summary),
                refs_suffix(&signal.evidence_refs)
            ));
        }
    }
}

fn refs_suffix(refs: &[String]) -> String {
    if refs.is_empty() {
        String::new()
    } else {
        format!(" (refs: {})", refs.join(", "))
    }
}

fn first_nonempty(values: &[String]) -> Option<&String> {
    values.iter().find(|value| !value.trim().is_empty())
}

fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn display_path(path: &Path) -> Result<String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(path.to_string_lossy().to_string())
}
