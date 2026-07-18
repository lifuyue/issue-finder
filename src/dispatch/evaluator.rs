use std::collections::BTreeSet;
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::model::{AgentArtifact, DispatchRun, DispatchRunStatus, NewArtifact};
use super::store::DispatchStore;
use super::task_contract::TaskPackage;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateResultStatus {
    Success,
    Partial,
    Failed,
    NeedsUser,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CandidateResult {
    pub run_id: String,
    pub issue_task_id: String,
    pub package_id: String,
    pub status: CandidateResultStatus,
    pub summary: String,
    pub changed_files: Vec<String>,
    pub reproduction: Value,
    pub success_criteria: Vec<CriterionEvidence>,
    pub validation: Vec<ValidationEvidence>,
    pub residual_risks: Vec<String>,
    pub failure_reason: Option<String>,
    #[serde(rename = "suggestedGitHubReply")]
    pub suggested_github_reply: Option<String>,
    pub session_context: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CriterionEvidence {
    pub criterion: String,
    pub satisfied: bool,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ValidationEvidence {
    pub command: String,
    pub passed: bool,
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationDisposition {
    AcceptedSuccess,
    AcceptedPartial,
    Retry,
    NeedsUser,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationReport {
    pub disposition: EvaluationDisposition,
    pub attempt: u32,
    pub checks: Vec<EvaluationCheck>,
    pub feedback: Vec<String>,
    pub candidate_result_artifact_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EvaluatedCandidate {
    pub run: DispatchRun,
    pub result_artifact: AgentArtifact,
    pub evaluation_artifact: AgentArtifact,
    pub report: EvaluationReport,
}

pub fn evaluate_candidate(
    store: &DispatchStore,
    run: &DispatchRun,
    result: &CandidateResult,
) -> Result<EvaluatedCandidate> {
    if matches!(
        run.status,
        DispatchRunStatus::Succeeded
            | DispatchRunStatus::Partial
            | DispatchRunStatus::Failed
            | DispatchRunStatus::Canceled
    ) {
        anyhow::bail!("run {} is already terminal", run.id);
    }
    if result.run_id != run.id || result.issue_task_id != run.issue_task_id {
        anyhow::bail!("candidate result identity does not match active run");
    }
    let issue = store.get_issue_task(&run.issue_task_id)?;
    let package_id = issue
        .current_package_artifact_id
        .as_deref()
        .context("active task has no package")?;
    if result.package_id != package_id {
        anyhow::bail!("candidate result packageId does not match active package");
    }
    let package_artifact = store.get_artifact(package_id)?;
    let package: TaskPackage = serde_json::from_slice(&store.read_artifact_bytes(package_id)?)
        .context("active TaskPackage is invalid")?;
    package.validate_for_execution()?;

    let attempt = store
        .list_artifacts_for_run(&run.id)?
        .iter()
        .filter(|artifact| artifact.kind == "candidate_result")
        .count() as u32
        + 1;
    let result_bytes = serde_json::to_vec_pretty(result)?;
    let result_artifact = store.write_artifact(
        NewArtifact {
            issue_task_id: Some(issue.id.clone()),
            run_id: Some(run.id.clone()),
            kind: "candidate_result".to_string(),
            content_type: "application/json".to_string(),
            metadata_json: json!({
                "attempt": attempt,
                "packageArtifactId": package_artifact.id,
                "snapshotId": package.context_snapshot.snapshot_id,
                "status": result.status,
            }),
        },
        &result_bytes,
    )?;
    store.set_dispatch_run_attempt(&run.id, attempt)?;
    if result.status == CandidateResultStatus::NeedsUser {
        super::codex_runtime::CodexRuntimeStore::open(&store.paths())?
            .record_candidate_user_request(
                &format!("candidate-needs-user:{}", result_artifact.id),
                run.selected_thread_id.as_deref(),
                run.current_turn_id.as_deref(),
                &json!({
                    "runId":run.id,
                    "summary":result.summary,
                    "failureReason":result.failure_reason,
                    "candidateResultArtifactId":result_artifact.id
                }),
            )?;
    }
    store.update_dispatch_run_status(&run.id, DispatchRunStatus::Evaluating, None)?;

    let mut checks = vec![EvaluationCheck {
        name: "provenance".to_string(),
        passed: true,
        detail: "run, task, package, and immutable snapshot identities match".to_string(),
    }];
    let mut feedback = Vec::new();

    let reported = result
        .changed_files
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let observed = observed_changed_files(&package.workspace.path)?;
    let files_match = reported == observed;
    checks.push(EvaluationCheck {
        name: "changed_files".to_string(),
        passed: files_match,
        detail: format!("reported={reported:?}; observed={observed:?}"),
    });
    if !files_match {
        feedback.push(
            "Report exactly the files currently changed inside the approved workspace.".to_string(),
        );
    }

    let criteria_ok = package.success_criteria.iter().all(|required| {
        result
            .success_criteria
            .iter()
            .any(|item| item.criterion == *required && item.satisfied && !item.evidence.is_empty())
    });
    checks.push(EvaluationCheck {
        name: "success_criteria".to_string(),
        passed: criteria_ok,
        detail: "every package criterion must have satisfied structured evidence".to_string(),
    });
    if !criteria_ok {
        feedback.push(
            "Supply satisfied evidence for every success criterion from the TaskPackage."
                .to_string(),
        );
    }

    let validation_ok = !result.validation.is_empty()
        && result.validation.iter().all(|item| {
            !item.command.trim().is_empty() && item.passed && item.exit_code == Some(0)
        });
    checks.push(EvaluationCheck {
        name: "validation".to_string(),
        passed: validation_ok,
        detail: "at least one validation command must report passed with exitCode 0".to_string(),
    });
    if !validation_ok {
        feedback.push(
            "Run validation and report each command with passed=true and exitCode=0.".to_string(),
        );
    }
    let command_events_ok = observed_command_evidence(store, run, result)?;
    checks.push(EvaluationCheck {
        name: "command_events".to_string(),
        passed: command_events_ok,
        detail: "Codex validation claims match persisted successful command items".to_string(),
    });
    if !command_events_ok {
        feedback.push(
            "Report validation commands that have matching successful Codex command events."
                .to_string(),
        );
    }

    let deterministic_checks_pass = checks.iter().all(|check| check.passed);
    let disposition = match result.status {
        CandidateResultStatus::NeedsUser => EvaluationDisposition::NeedsUser,
        CandidateResultStatus::Failed => EvaluationDisposition::Failed,
        CandidateResultStatus::Success if deterministic_checks_pass => {
            EvaluationDisposition::AcceptedSuccess
        }
        CandidateResultStatus::Partial
            if deterministic_checks_pass && !result.residual_risks.is_empty() =>
        {
            EvaluationDisposition::AcceptedPartial
        }
        CandidateResultStatus::Partial if deterministic_checks_pass => {
            feedback.push("A partial result must identify its residual risks.".to_string());
            EvaluationDisposition::Retry
        }
        CandidateResultStatus::Success | CandidateResultStatus::Partial
            if attempt < package.runtime_policy.max_attempts =>
        {
            EvaluationDisposition::Retry
        }
        CandidateResultStatus::Success | CandidateResultStatus::Partial => {
            feedback.push("The TaskPackage attempt budget is exhausted.".to_string());
            EvaluationDisposition::Failed
        }
    };
    let report = EvaluationReport {
        disposition,
        attempt,
        checks,
        feedback,
        candidate_result_artifact_id: result_artifact.id.clone(),
    };
    let evaluation_artifact = store.write_artifact(
        NewArtifact {
            issue_task_id: Some(issue.id),
            run_id: Some(run.id.clone()),
            kind: "evaluation_report".to_string(),
            content_type: "application/json".to_string(),
            metadata_json: json!({
                "attempt": attempt,
                "candidateResultArtifactId": result_artifact.id,
                "disposition": disposition,
            }),
        },
        &serde_json::to_vec_pretty(&report)?,
    )?;
    let next_status = match disposition {
        EvaluationDisposition::Retry => DispatchRunStatus::Running,
        EvaluationDisposition::NeedsUser => DispatchRunStatus::NeedsUser,
        EvaluationDisposition::AcceptedSuccess
        | EvaluationDisposition::AcceptedPartial
        | EvaluationDisposition::Failed => DispatchRunStatus::Evaluating,
    };
    let run = store.update_dispatch_run_status(&run.id, next_status, None)?;
    Ok(EvaluatedCandidate {
        run,
        result_artifact,
        evaluation_artifact,
        report,
    })
}

fn observed_changed_files(workspace: &str) -> Result<BTreeSet<String>> {
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .current_dir(workspace)
        .output()
        .with_context(|| format!("cannot inspect approved workspace {workspace}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "git status failed in approved workspace: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let mut files = BTreeSet::new();
    for entry in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let text = String::from_utf8_lossy(entry);
        let path = text.get(3..).unwrap_or_default();
        let path = path.rsplit(" -> ").next().unwrap_or(path).trim();
        if !path.is_empty() {
            files.insert(path.to_string());
        }
    }
    Ok(files)
}

fn observed_command_evidence(
    store: &DispatchStore,
    run: &DispatchRun,
    result: &CandidateResult,
) -> Result<bool> {
    let Some(thread_id) = run.selected_thread_id.as_deref() else {
        return Ok(true);
    };
    let items = super::codex_runtime::CodexRuntimeStore::open(&store.paths())?.items(thread_id)?;
    Ok(result.validation.iter().all(|validation| {
        items.iter().any(|item| {
            let command_matches = item.payload.to_string().contains(&validation.command);
            let succeeded = item.payload.get("exitCode").and_then(Value::as_i64) == Some(0)
                || matches!(
                    item.payload.get("status").and_then(Value::as_str),
                    Some("completed" | "succeeded" | "success")
                );
            command_matches && succeeded
        })
    }))
}
