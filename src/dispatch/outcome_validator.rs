use anyhow::{Context, Result};
use serde_json::Value;

use super::model::{DispatchOutcomeKind, DispatchRun, DispatchValidationOutcome, IssueTask};
use super::runtime::DispatchOutcomeRecordRequest;
use super::store::DispatchStore;

const REQUIRED_RESULT_FIELDS: &[&str] = &[
    "status",
    "summary",
    "changedFiles",
    "reproduction",
    "successCriteria",
    "validation",
    "residualRisks",
    "failureReason",
    "suggestedGitHubReply",
    "sessionContext",
];

pub(crate) fn validate_outcome(
    store: &DispatchStore,
    run: &DispatchRun,
    issue_task: &IssueTask,
    request: &DispatchOutcomeRecordRequest,
) -> Result<()> {
    let requires_result = matches!(
        request.outcome_kind,
        DispatchOutcomeKind::Success | DispatchOutcomeKind::Partial
    );
    let Some(artifact_id) = request.result_artifact_id.as_deref() else {
        if requires_result {
            anyhow::bail!("{} requires a result artifact", request.outcome_kind);
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
    if artifact.kind != "candidate_result" || artifact.content_type != "application/json" {
        anyhow::bail!("result artifact {artifact_id} is not a JSON candidate_result");
    }

    let value: Value = serde_json::from_slice(&store.read_artifact_bytes(artifact_id)?)
        .context("candidate_result artifact is not valid JSON")?;
    let object = value
        .as_object()
        .context("candidate_result artifact must be a JSON object")?;
    for field in REQUIRED_RESULT_FIELDS {
        if !object.contains_key(*field) {
            anyhow::bail!("candidate_result artifact is missing required field {field}");
        }
    }

    let status = object.get("status").and_then(Value::as_str);
    let expected_status = match request.outcome_kind {
        DispatchOutcomeKind::Success => "success",
        DispatchOutcomeKind::Partial => "partial",
        DispatchOutcomeKind::Failed => "failed",
        DispatchOutcomeKind::Canceled => {
            anyhow::bail!("canceled outcomes do not accept candidate result artifacts")
        }
    };
    if status != Some(expected_status) {
        anyhow::bail!(
            "{} outcome conflicts with result status {status:?}",
            request.outcome_kind
        );
    }
    if requires_result && request.validation_outcome != Some(DispatchValidationOutcome::Passed) {
        anyhow::bail!("{} requires validationOutcome=passed", request.outcome_kind);
    }
    Ok(())
}
