use anyhow::Result;
use serde_json::json;

use crate::memory::{DispatchMemoryOutcome, MemoryIngestResult, MemoryIngestor, MemoryStore};
use crate::recommendation::IssueKey;

use super::model::{
    ApprovalRequest, ApprovalStatus, DispatchRun, DispatchRunOutcome, MemoryEvent, MemoryEventType,
    NewMemoryEvent,
};
use super::store::DispatchStore;

pub fn record_dispatch_approval_signal(
    store: &DispatchStore,
    run: &DispatchRun,
    approval_request: &ApprovalRequest,
) -> Result<Option<MemoryEvent>> {
    let event_type = match approval_request.status {
        ApprovalStatus::Approved => MemoryEventType::PositiveSignal,
        ApprovalStatus::Rejected | ApprovalStatus::Canceled => MemoryEventType::NegativeSignal,
        ApprovalStatus::Pending => return Ok(None),
    };
    let issue_task = store.get_issue_task(&run.issue_task_id)?;
    let signal = match approval_request.status {
        ApprovalStatus::Approved => "dispatch_approved",
        ApprovalStatus::Rejected => "dispatch_rejected",
        ApprovalStatus::Canceled => "dispatch_canceled",
        ApprovalStatus::Pending => unreachable!(),
    };
    let event = store.append_memory_event(NewMemoryEvent {
        issue_task_id: Some(issue_task.id.clone()),
        event_type,
        source: "dispatch_approval".to_string(),
        payload_json: json!({
            "signal": signal,
            "issueKey": issue_task.issue_key,
            "runId": run.id,
            "runStatus": run.status,
            "agentId": run.agent_id,
            "approvalRequestId": approval_request.id,
            "approvalStatus": approval_request.status
        }),
    })?;
    Ok(Some(event))
}

#[derive(Debug, Clone, PartialEq)]
pub struct DispatchOutcomeMemoryIngest {
    pub dispatch_memory_event: MemoryEvent,
    pub memory_ingest: MemoryIngestResult,
}

pub fn ingest_dispatch_outcome_memory(
    store: &DispatchStore,
    run: &DispatchRun,
    outcome: &DispatchRunOutcome,
) -> Result<DispatchOutcomeMemoryIngest> {
    let issue_task = store.get_issue_task(&run.issue_task_id)?;
    let event_type = if outcome.outcome_kind.is_positive() {
        MemoryEventType::PositiveSignal
    } else {
        MemoryEventType::AgentPerformanceSignal
    };
    let dispatch_memory_event = store.append_memory_event(NewMemoryEvent {
        issue_task_id: Some(issue_task.id.clone()),
        event_type,
        source: "dispatch_outcome".to_string(),
        payload_json: json!({
            "signal": "dispatch_outcome_recorded",
            "issueKey": issue_task.issue_key,
            "runId": run.id,
            "agentId": run.agent_id,
            "outcomeId": outcome.id,
            "outcomeKind": outcome.outcome_kind,
            "failureClass": outcome.failure_class,
            "failureDetail": outcome.failure_detail,
            "taskClass": outcome.task_class,
            "validationOutcome": outcome.validation_outcome,
            "resultArtifactId": outcome.result_artifact_id
        }),
    })?;

    let memory_store = MemoryStore::open(&store.paths())?;
    let memory_ingest =
        MemoryIngestor::new(&memory_store).ingest_dispatch_outcome(&DispatchMemoryOutcome {
            id: outcome.id.clone(),
            issue_key: IssueKey::new(issue_task.repo_full_name, issue_task.issue_number),
            agent_id: run.agent_id.clone(),
            outcome_kind: Some(outcome.outcome_kind.as_str().to_string()),
            task_type: outcome
                .task_class
                .map(|task_class| task_class.as_str().to_string())
                .unwrap_or_else(|| "unknown_task".to_string()),
            succeeded: outcome.outcome_kind.is_positive(),
            failure_class: outcome
                .failure_class
                .map(|failure_class| failure_class.as_str().to_string()),
            failure_reason: outcome.failure_detail.clone(),
            validation_outcome: outcome
                .validation_outcome
                .map(|validation_outcome| validation_outcome.as_str().to_string()),
            validation_paths: Vec::new(),
            artifact_refs: outcome
                .result_artifact_id
                .iter()
                .map(|artifact_id| format!("dispatch_artifact:{artifact_id}"))
                .collect(),
            occurred_at: outcome.recorded_at.clone(),
            metadata: json!({
                "runId": run.id,
                "outcomeId": outcome.id,
                "idempotencyKey": outcome.idempotency_key,
                "metadata": outcome.metadata_json
            }),
        })?;

    Ok(DispatchOutcomeMemoryIngest {
        dispatch_memory_event,
        memory_ingest,
    })
}
