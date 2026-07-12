use anyhow::{Context, Result};
use serde_json::Value;

use super::model::{DispatchOutcomeKind, DispatchRun, DispatchValidationOutcome, IssueTask};
use super::runtime::DispatchOutcomeRecordRequest;
use super::store::DispatchStore;

pub fn validate_outcome(
    store: &DispatchStore,
    run: &DispatchRun,
    issue_task: &IssueTask,
    request: &DispatchOutcomeRecordRequest,
) -> Result<()> {
    let Some(artifact_id) = request.result_artifact_id.as_deref() else {
        if request.outcome_kind == DispatchOutcomeKind::FixReady {
            anyhow::bail!("fix_ready requires a fix_result artifact");
        }
        return Ok(());
    };
    let artifact = store.get_artifact(artifact_id)?;
    if artifact.run_id.as_deref() != Some(run.id.as_str())
        || artifact.issue_task_id.as_deref() != Some(issue_task.id.as_str())
    {
        anyhow::bail!(
            "result artifact {artifact_id} does not belong to dispatch run {}",
            run.id
        );
    }
    if artifact.kind != "fix_result" || artifact.content_type != "application/json" {
        anyhow::bail!("result artifact {artifact_id} is not a JSON fix_result");
    }
    let value: Value = serde_json::from_slice(&store.read_artifact_bytes(artifact_id)?)
        .context("fix_result artifact is not valid JSON")?;
    let status = value.get("status").and_then(Value::as_str);
    if request.outcome_kind == DispatchOutcomeKind::FixReady {
        if request.validation_outcome != Some(DispatchValidationOutcome::Passed) {
            anyhow::bail!("fix_ready requires validationOutcome=passed");
        }
        if !matches!(status, Some("fix_ready" | "fixed" | "completed")) {
            anyhow::bail!("fix_ready conflicts with fix_result status {status:?}");
        }
    }
    Ok(())
}
