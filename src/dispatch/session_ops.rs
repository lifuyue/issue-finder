use anyhow::Result;
use serde::Serialize;
use serde_json::{json, Value};

use super::adapters::{
    codex_app_server::{CodexAppServerAdapter, CodexAppServerStdioTransport},
    NativeExecutionAdapter,
};
use super::model::{
    AgentArtifact, AgentEvent, AgentSessionLink, AgentSessionStatus, NewAgentEvent,
    NewAgentSessionLink, NewArtifact,
};
use super::store::DispatchStore;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionTranscriptResult {
    pub session: AgentSessionLink,
    pub transcript_artifact: AgentArtifact,
    pub event: AgentEvent,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionMutationResult {
    pub session: AgentSessionLink,
    pub event: AgentEvent,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionsSyncResult {
    pub agent_id: String,
    pub synced: Vec<AgentSessionLink>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionsSyncRequest {
    pub agent_id: String,
    pub search: Option<String>,
    pub limit: Option<usize>,
}

pub fn sync_sessions<A>(
    store: &DispatchStore,
    adapter: &mut A,
    request: SessionsSyncRequest,
) -> Result<SessionsSyncResult>
where
    A: NativeExecutionAdapter,
{
    let native_sessions = match request.search.as_deref() {
        Some(search) if !search.trim().is_empty() => {
            adapter.adapter_search_sessions(search.trim(), request.limit)?
        }
        _ => adapter.adapter_list_sessions(request.limit)?,
    };
    let mut synced = Vec::new();
    for native_session in native_sessions {
        let display_name = native_session
            .display_name
            .clone()
            .unwrap_or_else(|| native_session.native_session_id.clone());
        let session = match store.find_session_link_by_native_id_opt(
            &request.agent_id,
            &native_session.native_session_id,
        )? {
            Some(existing) => {
                store.rename_session_link(&existing.id, &display_name)?;
                store.update_session_link_status(&existing.id, AgentSessionStatus::Idle)?
            }
            None => store.create_session_link(NewAgentSessionLink {
                agent_id: request.agent_id.clone(),
                native_session_id: native_session.native_session_id.clone(),
                issue_task_id: None,
                display_name,
                goal: native_session.goal.clone(),
                status: AgentSessionStatus::Idle,
                metadata_json: native_session.metadata_json.clone(),
            })?,
        };
        store.append_agent_event(NewAgentEvent {
            run_id: None,
            session_link_id: Some(session.id.clone()),
            event_type: "session_synced".to_string(),
            native_event_id: Some(session.native_session_id.clone()),
            payload_json: json!({
                "agentId": request.agent_id.clone(),
                "nativeSessionId": session.native_session_id.clone()
            }),
        })?;
        synced.push(session);
    }

    Ok(SessionsSyncResult {
        agent_id: request.agent_id,
        synced,
    })
}

pub fn read_session_transcript<A>(
    store: &DispatchStore,
    adapter: &mut A,
    session_link_id: &str,
) -> Result<SessionTranscriptResult>
where
    A: NativeExecutionAdapter,
{
    let session = store.get_session_link(session_link_id)?;
    let transcript = adapter.adapter_read_transcript(&session.native_session_id)?;
    let artifact = store.write_artifact(
        NewArtifact {
            issue_task_id: session.issue_task_id.clone(),
            run_id: None,
            kind: "session_transcript".to_string(),
            content_type: "application/json".to_string(),
            metadata_json: json!({
                "sessionLinkId": session.id,
                "nativeSessionId": session.native_session_id
            }),
        },
        serde_json::to_vec_pretty(&transcript)?,
    )?;
    let event = store.append_agent_event(NewAgentEvent {
        run_id: None,
        session_link_id: Some(session.id.clone()),
        event_type: "session_transcript_read".to_string(),
        native_event_id: None,
        payload_json: json!({
            "artifactId": artifact.id,
            "nativeSessionId": session.native_session_id
        }),
    })?;
    let session = store.update_session_link_status(&session.id, AgentSessionStatus::Idle)?;
    Ok(SessionTranscriptResult {
        session,
        transcript_artifact: artifact,
        event,
    })
}

pub fn rename_session<A>(
    store: &DispatchStore,
    adapter: &mut A,
    session_link_id: &str,
    display_name: &str,
) -> Result<SessionMutationResult>
where
    A: NativeExecutionAdapter,
{
    let session = store.get_session_link(session_link_id)?;
    let native_session =
        adapter.adapter_rename_session(&session.native_session_id, display_name)?;
    let display_name = native_session
        .display_name
        .as_deref()
        .unwrap_or(display_name);
    let session = store.rename_session_link(&session.id, display_name)?;
    let event = store.append_agent_event(NewAgentEvent {
        run_id: None,
        session_link_id: Some(session.id.clone()),
        event_type: "session_renamed".to_string(),
        native_event_id: Some(session.native_session_id.clone()),
        payload_json: json!({
            "displayName": session.display_name.clone()
        }),
    })?;
    Ok(SessionMutationResult { session, event })
}

pub fn fork_session<A>(
    store: &DispatchStore,
    adapter: &mut A,
    session_link_id: &str,
) -> Result<SessionMutationResult>
where
    A: NativeExecutionAdapter,
{
    let source_session = store.get_session_link(session_link_id)?;
    let native_session = adapter.adapter_fork_session(&source_session.native_session_id)?;
    let display_name = native_session
        .display_name
        .clone()
        .unwrap_or_else(|| format!("{} (fork)", source_session.display_name));
    let goal = native_session
        .goal
        .clone()
        .or_else(|| source_session.goal.clone());
    let metadata_json = fork_metadata(native_session.metadata_json.clone(), &source_session);

    let session = match store.find_session_link_by_native_id_opt(
        &source_session.agent_id,
        &native_session.native_session_id,
    )? {
        Some(existing) => {
            let session = store.rename_session_link(&existing.id, &display_name)?;
            store.update_session_link_status(&session.id, AgentSessionStatus::Idle)?
        }
        None => store.create_session_link(NewAgentSessionLink {
            agent_id: source_session.agent_id.clone(),
            native_session_id: native_session.native_session_id.clone(),
            issue_task_id: source_session.issue_task_id.clone(),
            display_name,
            goal,
            status: AgentSessionStatus::Idle,
            metadata_json,
        })?,
    };
    let event = store.append_agent_event(NewAgentEvent {
        run_id: None,
        session_link_id: Some(session.id.clone()),
        event_type: "session_forked".to_string(),
        native_event_id: Some(session.native_session_id.clone()),
        payload_json: json!({
            "sourceSessionLinkId": source_session.id,
            "sourceNativeSessionId": source_session.native_session_id,
            "forkedSessionLinkId": session.id,
            "forkedNativeSessionId": session.native_session_id
        }),
    })?;
    Ok(SessionMutationResult { session, event })
}

pub fn archive_session<A>(
    store: &DispatchStore,
    adapter: &mut A,
    session_link_id: &str,
) -> Result<SessionMutationResult>
where
    A: NativeExecutionAdapter,
{
    let session = store.get_session_link(session_link_id)?;
    adapter.adapter_archive_session(&session.native_session_id)?;
    let session = store.update_session_link_status(&session.id, AgentSessionStatus::Archived)?;
    let event = store.append_agent_event(NewAgentEvent {
        run_id: None,
        session_link_id: Some(session.id.clone()),
        event_type: "session_archived".to_string(),
        native_event_id: Some(session.native_session_id.clone()),
        payload_json: json!({
            "nativeSessionId": session.native_session_id.clone()
        }),
    })?;
    Ok(SessionMutationResult { session, event })
}

pub fn read_codex_session_transcript(
    store: &DispatchStore,
    session_link_id: &str,
) -> Result<SessionTranscriptResult> {
    let transport = CodexAppServerStdioTransport::connect()?;
    let mut adapter = CodexAppServerAdapter::new(transport);
    read_session_transcript(store, &mut adapter, session_link_id)
}

pub fn rename_codex_session(
    store: &DispatchStore,
    session_link_id: &str,
    display_name: &str,
) -> Result<SessionMutationResult> {
    let transport = CodexAppServerStdioTransport::connect()?;
    let mut adapter = CodexAppServerAdapter::new(transport);
    rename_session(store, &mut adapter, session_link_id, display_name)
}

pub fn fork_codex_session(
    store: &DispatchStore,
    session_link_id: &str,
) -> Result<SessionMutationResult> {
    let transport = CodexAppServerStdioTransport::connect()?;
    let mut adapter = CodexAppServerAdapter::new(transport);
    fork_session(store, &mut adapter, session_link_id)
}

pub fn archive_codex_session(
    store: &DispatchStore,
    session_link_id: &str,
) -> Result<SessionMutationResult> {
    let transport = CodexAppServerStdioTransport::connect()?;
    let mut adapter = CodexAppServerAdapter::new(transport);
    archive_session(store, &mut adapter, session_link_id)
}

pub fn sync_codex_sessions(
    store: &DispatchStore,
    request: SessionsSyncRequest,
) -> Result<SessionsSyncResult> {
    let transport = CodexAppServerStdioTransport::connect()?;
    let mut adapter = CodexAppServerAdapter::new(transport);
    sync_sessions(store, &mut adapter, request)
}

fn fork_metadata(native_metadata: Value, source_session: &AgentSessionLink) -> Value {
    match native_metadata {
        Value::Object(mut map) => {
            map.insert(
                "forkedFromSessionLinkId".to_string(),
                Value::String(source_session.id.clone()),
            );
            map.insert(
                "forkedFromNativeSessionId".to_string(),
                Value::String(source_session.native_session_id.clone()),
            );
            Value::Object(map)
        }
        Value::Null => json!({
            "forkedFromSessionLinkId": source_session.id.clone(),
            "forkedFromNativeSessionId": source_session.native_session_id.clone()
        }),
        other => json!({
            "nativeMetadata": other,
            "forkedFromSessionLinkId": source_session.id.clone(),
            "forkedFromNativeSessionId": source_session.native_session_id.clone()
        }),
    }
}
