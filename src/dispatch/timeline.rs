use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;

use super::model::{
    AgentArtifact, ApprovalRequest, DispatchEvent, DispatchFailure, DispatchRun, GitHubInteraction,
};
use super::store::DispatchStore;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DispatchTimeline {
    pub run: DispatchRun,
    pub items: Vec<TimelineItem>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DispatchTrace {
    pub run: DispatchRun,
    pub events: Vec<DispatchEvent>,
    pub approvals: Vec<ApprovalRequest>,
    pub approval_latencies: Vec<ApprovalLatency>,
    pub artifacts: Vec<AgentArtifact>,
    pub failures: Vec<DispatchFailure>,
    pub github_interactions: Vec<GitHubInteraction>,
    pub timeline: Vec<TimelineItem>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimelineItem {
    pub kind: String,
    pub id: String,
    pub created_at: String,
    pub sequence: Option<i64>,
    pub summary: String,
    pub details_json: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalLatency {
    pub approval_request_id: String,
    pub approval_type: String,
    pub status: String,
    pub created_at: String,
    pub resolved_at: Option<String>,
    pub latency_ms: Option<i64>,
}

pub fn dispatch_timeline(store: &DispatchStore, run_id: &str) -> Result<DispatchTimeline> {
    let run = store.get_dispatch_run(run_id)?;
    let items = build_timeline_items(store, &run)?;
    Ok(DispatchTimeline { run, items })
}

pub fn dispatch_trace(store: &DispatchStore, run_id: &str) -> Result<DispatchTrace> {
    let run = store.get_dispatch_run(run_id)?;
    let approvals = store.list_approval_requests_for_run(run_id)?;
    let events = store.list_dispatch_events_for_run(run_id)?;
    let artifacts = store.list_artifacts_for_run(run_id)?;
    let failures = store.list_dispatch_failures_for_run(run_id)?;
    let github_interactions = store.list_github_interactions_for_issue_task(&run.issue_task_id)?;
    let approval_latencies = approvals.iter().map(approval_latency).collect();
    let timeline = build_timeline_items_for_records(
        &events,
        &approvals,
        &artifacts,
        &failures,
        &github_interactions,
    );
    Ok(DispatchTrace {
        run,
        events,
        approvals,
        approval_latencies,
        artifacts,
        failures,
        github_interactions,
        timeline,
    })
}

fn build_timeline_items(store: &DispatchStore, run: &DispatchRun) -> Result<Vec<TimelineItem>> {
    let events = store.list_dispatch_events_for_run(&run.id)?;
    let approvals = store.list_approval_requests_for_run(&run.id)?;
    let artifacts = store.list_artifacts_for_run(&run.id)?;
    let failures = store.list_dispatch_failures_for_run(&run.id)?;
    let github_interactions = store.list_github_interactions_for_issue_task(&run.issue_task_id)?;
    Ok(build_timeline_items_for_records(
        &events,
        &approvals,
        &artifacts,
        &failures,
        &github_interactions,
    ))
}

fn build_timeline_items_for_records(
    events: &[DispatchEvent],
    approvals: &[ApprovalRequest],
    artifacts: &[AgentArtifact],
    failures: &[DispatchFailure],
    github_interactions: &[GitHubInteraction],
) -> Vec<TimelineItem> {
    let mut items = Vec::new();
    items.extend(events.iter().map(|event| TimelineItem {
        kind: "event".to_string(),
        id: event.id.clone(),
        created_at: event.created_at.clone(),
        sequence: Some(event.sequence),
        summary: event.event_kind.as_str().to_string(),
        details_json: json!({
            "source": event.source,
            "severity": event.severity,
            "subjectType": event.subject_type,
            "subjectId": event.subject_id,
            "nativeEventId": event.native_event_id
        }),
    }));
    items.extend(approvals.iter().map(|approval| {
        let latency = approval_latency(approval);
        TimelineItem {
            kind: "approval".to_string(),
            id: approval.id.clone(),
            created_at: approval.created_at.clone(),
            sequence: None,
            summary: format!("{} {}", approval.approval_type, approval.status),
            details_json: json!({ "latencyMs": latency.latency_ms }),
        }
    }));
    items.extend(artifacts.iter().map(|artifact| TimelineItem {
        kind: "artifact".to_string(),
        id: artifact.id.clone(),
        created_at: artifact.created_at.clone(),
        sequence: None,
        summary: artifact.kind.clone(),
        details_json: json!({
            "contentType": artifact.content_type,
            "sha256": artifact.sha256
        }),
    }));
    items.extend(failures.iter().map(|failure| TimelineItem {
        kind: "failure".to_string(),
        id: failure.id.clone(),
        created_at: failure.created_at.clone(),
        sequence: None,
        summary: format!("{}: {}", failure.code, failure.message),
        details_json: json!({
            "phase": failure.phase,
            "failureClass": failure.failure_class,
            "retryable": failure.retryable
        }),
    }));
    items.extend(github_interactions.iter().map(|interaction| TimelineItem {
        kind: "github_interaction".to_string(),
        id: interaction.id.clone(),
        created_at: interaction.created_at.clone(),
        sequence: None,
        summary: format!("{} {}", interaction.interaction_type, interaction.status),
        details_json: json!({
            "githubCommentId": interaction.github_comment_id,
            "postedAt": interaction.posted_at,
            "error": interaction.error
        }),
    }));
    items.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| {
                left.sequence
                    .unwrap_or(i64::MAX)
                    .cmp(&right.sequence.unwrap_or(i64::MAX))
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    items
}

pub fn approval_latency(approval: &ApprovalRequest) -> ApprovalLatency {
    let latency_ms = approval
        .resolved_at
        .as_deref()
        .and_then(|resolved_at| latency_ms(&approval.created_at, resolved_at));
    ApprovalLatency {
        approval_request_id: approval.id.clone(),
        approval_type: approval.approval_type.as_str().to_string(),
        status: approval.status.as_str().to_string(),
        created_at: approval.created_at.clone(),
        resolved_at: approval.resolved_at.clone(),
        latency_ms,
    }
}

fn latency_ms(created_at: &str, resolved_at: &str) -> Option<i64> {
    let created_at = DateTime::parse_from_rfc3339(created_at)
        .ok()?
        .with_timezone(&Utc);
    let resolved_at = DateTime::parse_from_rfc3339(resolved_at)
        .ok()?
        .with_timezone(&Utc);
    Some((resolved_at - created_at).num_milliseconds())
}
