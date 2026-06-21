use anyhow::Result;
use serde_json::json;

use super::model::{
    ApprovalRequest, ApprovalStatus, DispatchRun, IssueTask, MemoryEvent, MemoryEventType,
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

pub fn record_issue_review_signal(
    store: &DispatchStore,
    issue_task: &IssueTask,
    approval_request: &ApprovalRequest,
    reason: Option<String>,
) -> Result<Option<MemoryEvent>> {
    let event_type = match approval_request.status {
        ApprovalStatus::Approved => MemoryEventType::PositiveSignal,
        ApprovalStatus::Rejected | ApprovalStatus::Canceled => MemoryEventType::NegativeSignal,
        ApprovalStatus::Pending => return Ok(None),
    };
    let signal = match approval_request.status {
        ApprovalStatus::Approved => "issue_review_approved",
        ApprovalStatus::Rejected => "issue_review_rejected",
        ApprovalStatus::Canceled => "issue_review_canceled",
        ApprovalStatus::Pending => unreachable!(),
    };
    let event = store.append_memory_event(NewMemoryEvent {
        issue_task_id: Some(issue_task.id.clone()),
        event_type,
        source: "issue_review".to_string(),
        payload_json: json!({
            "signal": signal,
            "issueKey": issue_task.issue_key,
            "approvalRequestId": approval_request.id,
            "approvalStatus": approval_request.status,
            "reason": reason
        }),
    })?;
    Ok(Some(event))
}
