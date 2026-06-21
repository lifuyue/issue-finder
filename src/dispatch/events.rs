use serde_json::Value;

use super::model::{
    DispatchEventKind, DispatchEventSeverity, DispatchEventSource, DispatchRun,
    DispatchSubjectType, NewDispatchEvent,
};

pub fn dispatch_run_event(
    run: &DispatchRun,
    event_kind: DispatchEventKind,
    source: DispatchEventSource,
    severity: DispatchEventSeverity,
    payload_json: Value,
) -> NewDispatchEvent {
    NewDispatchEvent {
        run_id: Some(run.id.clone()),
        session_link_id: run.selected_session_link_id.clone(),
        issue_task_id: Some(run.issue_task_id.clone()),
        event_kind,
        subject_type: DispatchSubjectType::DispatchRun,
        subject_id: Some(run.id.clone()),
        source,
        severity,
        correlation_id: Some(run.id.clone()),
        causation_id: None,
        native_event_id: None,
        payload_json,
    }
}

pub fn run_session_event(
    run: &DispatchRun,
    session_link_id: &str,
    event_kind: DispatchEventKind,
    source: DispatchEventSource,
    native_event_id: Option<String>,
    payload_json: Value,
) -> NewDispatchEvent {
    NewDispatchEvent {
        run_id: Some(run.id.clone()),
        session_link_id: Some(session_link_id.to_string()),
        issue_task_id: Some(run.issue_task_id.clone()),
        event_kind,
        subject_type: DispatchSubjectType::Session,
        subject_id: Some(session_link_id.to_string()),
        source,
        severity: DispatchEventSeverity::Info,
        correlation_id: Some(run.id.clone()),
        causation_id: None,
        native_event_id,
        payload_json,
    }
}

pub fn session_event(
    session_link_id: &str,
    issue_task_id: Option<String>,
    event_kind: DispatchEventKind,
    source: DispatchEventSource,
    native_event_id: Option<String>,
    payload_json: Value,
) -> NewDispatchEvent {
    NewDispatchEvent {
        run_id: None,
        session_link_id: Some(session_link_id.to_string()),
        issue_task_id,
        event_kind,
        subject_type: DispatchSubjectType::Session,
        subject_id: Some(session_link_id.to_string()),
        source,
        severity: DispatchEventSeverity::Info,
        correlation_id: Some(session_link_id.to_string()),
        causation_id: None,
        native_event_id,
        payload_json,
    }
}
