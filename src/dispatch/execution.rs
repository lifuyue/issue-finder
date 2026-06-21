use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

use super::adapters::{
    codex_app_server::{CodexAppServerAdapter, CodexAppServerStdioTransport},
    AdapterSession, AdapterStartSessionRequest, AdapterTurn, NativeExecutionAdapter,
};
use super::events::{dispatch_run_event, run_session_event};
use super::failure::execution_failure;
use super::model::{
    AgentArtifact, AgentCapabilityName, AgentSessionLink, AgentSessionStatus, ApprovalStatus,
    CapabilityStatus, DispatchEvent, DispatchEventKind, DispatchEventSeverity, DispatchEventSource,
    DispatchRun, DispatchRunStatus, IssueTask, IssueTaskStatus, NewAgentSessionLink, NewArtifact,
};
use super::store::DispatchStore;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DispatchExecutionResult {
    pub run: DispatchRun,
    pub session: AgentSessionLink,
    pub turn: AdapterTurn,
    pub prompt_artifact: AgentArtifact,
    pub events: Vec<DispatchEvent>,
}

pub fn execute_approved_dispatch<A>(
    store: &DispatchStore,
    adapter: &mut A,
    run_id: &str,
) -> Result<DispatchExecutionResult>
where
    A: NativeExecutionAdapter,
{
    let context = prepare_execution_context(store, run_id)?;
    let starting_run =
        store.update_dispatch_run_status(&context.run.id, DispatchRunStatus::Starting, None)?;
    match execute_started_dispatch(store, adapter, context, starting_run) {
        Ok(result) => Ok(result),
        Err(error) => {
            let _ = store.update_dispatch_run_status(
                run_id,
                DispatchRunStatus::Failed,
                Some(error.to_string()),
            );
            let _ = store.record_dispatch_failure(execution_failure(run_id, "execute", &error));
            if let Ok(run) = store.get_dispatch_run(run_id) {
                let _ = store.append_dispatch_event(dispatch_run_event(
                    &run,
                    DispatchEventKind::DispatchFailed,
                    DispatchEventSource::Runtime,
                    DispatchEventSeverity::Error,
                    json!({ "error": error.to_string() }),
                ));
            }
            Err(error)
        }
    }
}

pub fn execute_approved_codex_app_server_dispatch(
    store: &DispatchStore,
    run_id: &str,
) -> Result<DispatchExecutionResult> {
    let context = prepare_execution_context(store, run_id)?;
    let agent = store.get_agent_profile(&context.run.agent_id)?;
    if agent.adapter != "codex_app_server" {
        anyhow::bail!(
            "dispatch run {run_id} uses adapter {}, not codex_app_server",
            agent.adapter
        );
    }

    let transport = CodexAppServerStdioTransport::connect()?;
    let mut adapter = CodexAppServerAdapter::new(transport);
    execute_approved_dispatch(store, &mut adapter, run_id)
}

struct ExecutionContext {
    run: DispatchRun,
    issue_task: IssueTask,
    package_artifact: AgentArtifact,
}

fn prepare_execution_context(store: &DispatchStore, run_id: &str) -> Result<ExecutionContext> {
    let run = store.get_dispatch_run(run_id)?;
    if run.approval_state != ApprovalStatus::Approved {
        anyhow::bail!(
            "dispatch run {run_id} is not approved; current approval state is {}",
            run.approval_state
        );
    }
    if matches!(
        run.status,
        DispatchRunStatus::Completed | DispatchRunStatus::Failed | DispatchRunStatus::Canceled
    ) {
        anyhow::bail!("dispatch run {run_id} is already terminal: {}", run.status);
    }

    let issue_task = store.get_issue_task(&run.issue_task_id)?;
    let package_artifact_id = issue_task
        .current_package_artifact_id
        .as_deref()
        .with_context(|| format!("issue task {} has no task package artifact", issue_task.id))?;
    let package_artifact = store.get_artifact(package_artifact_id)?;

    let required_capability = if run.selected_session_link_id.is_some() {
        AgentCapabilityName::ResumeSession
    } else {
        AgentCapabilityName::StartSession
    };
    ensure_capability(store, &run.agent_id, required_capability)?;
    ensure_capability(store, &run.agent_id, AgentCapabilityName::SetGoal)?;
    ensure_capability(store, &run.agent_id, AgentCapabilityName::SetMetadata)?;

    Ok(ExecutionContext {
        run,
        issue_task,
        package_artifact,
    })
}

fn execute_started_dispatch<A>(
    store: &DispatchStore,
    adapter: &mut A,
    context: ExecutionContext,
    starting_run: DispatchRun,
) -> Result<DispatchExecutionResult>
where
    A: NativeExecutionAdapter,
{
    let display_name = deterministic_session_name(&context.issue_task);
    let goal = deterministic_goal(&context.issue_task);
    let metadata = dispatch_metadata(
        &starting_run,
        &context.issue_task,
        &context.package_artifact,
    );

    let mut events = Vec::new();
    events.push(store.append_dispatch_event(dispatch_run_event(
        &starting_run,
        DispatchEventKind::DispatchStarting,
        DispatchEventSource::Runtime,
        DispatchEventSeverity::Info,
        json!({
            "agentId": starting_run.agent_id,
            "issueKey": context.issue_task.issue_key,
            "packageArtifactId": context.package_artifact.id
        }),
    ))?);

    let (session_link, native_session, session_event_type) =
        match starting_run.selected_session_link_id.as_deref() {
            Some(session_link_id) => resume_session(
                store,
                adapter,
                &starting_run,
                session_link_id,
                &display_name,
                &goal,
                metadata.clone(),
            )?,
            None => start_session(
                store,
                adapter,
                &starting_run,
                &context.issue_task,
                &display_name,
                &goal,
                metadata.clone(),
            )?,
        };

    events.push(store.append_dispatch_event(run_session_event(
        &starting_run,
        &session_link.id,
        session_event_type,
        DispatchEventSource::Adapter,
        Some(native_session.native_session_id.clone()),
        json!({
            "nativeSessionId": native_session.native_session_id,
            "displayName": native_session.display_name,
            "goal": native_session.goal
        }),
    ))?);

    let run = store.set_dispatch_run_session(&starting_run.id, &session_link.id)?;
    let prompt = dispatch_turn_prompt(&context.issue_task, &context.package_artifact);
    let prompt_artifact = store.write_artifact(
        NewArtifact {
            issue_task_id: Some(context.issue_task.id.clone()),
            run_id: Some(run.id.clone()),
            kind: "dispatch_prompt".to_string(),
            content_type: "text/plain".to_string(),
            metadata_json: json!({
                "templateVersion": 1,
                "packageArtifactId": context.package_artifact.id
            }),
        },
        prompt.as_bytes(),
    )?;
    let turn = adapter.adapter_start_turn(&session_link.native_session_id, &prompt)?;
    events.push(store.append_dispatch_event(run_session_event(
        &run,
        &session_link.id,
        DispatchEventKind::TurnStarted,
        DispatchEventSource::Adapter,
        Some(turn.native_turn_id.clone()),
        json!({
            "nativeTurnId": turn.native_turn_id,
            "status": turn.status,
            "promptArtifactId": prompt_artifact.id
        }),
    ))?);

    store.update_issue_task_status(&context.issue_task.id, IssueTaskStatus::InProgress)?;
    store.update_session_link_status(&session_link.id, AgentSessionStatus::Active)?;
    let run_status = dispatch_status_for_turn(&turn);
    let run = store.update_dispatch_run_status(&run.id, run_status, None)?;
    let session = store.get_session_link(&session_link.id)?;

    Ok(DispatchExecutionResult {
        run,
        session,
        turn,
        prompt_artifact,
        events,
    })
}

fn dispatch_status_for_turn(turn: &AdapterTurn) -> DispatchRunStatus {
    let Some(status) = turn.status.as_deref() else {
        return DispatchRunStatus::Running;
    };
    let normalized = status.trim().to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "needs_user" | "needs_approval" | "requires_approval" | "waiting_for_approval" => {
            DispatchRunStatus::NeedsUser
        }
        _ => DispatchRunStatus::Running,
    }
}

fn start_session<A>(
    store: &DispatchStore,
    adapter: &mut A,
    run: &DispatchRun,
    issue_task: &IssueTask,
    display_name: &str,
    goal: &str,
    metadata_json: Value,
) -> Result<(AgentSessionLink, AdapterSession, DispatchEventKind)>
where
    A: NativeExecutionAdapter,
{
    let native_session = adapter.adapter_start_session(AdapterStartSessionRequest {
        display_name: display_name.to_string(),
        goal: Some(goal.to_string()),
        metadata_json: metadata_json.clone(),
    })?;
    let session_link = store.create_session_link(NewAgentSessionLink {
        agent_id: run.agent_id.clone(),
        native_session_id: native_session.native_session_id.clone(),
        issue_task_id: Some(issue_task.id.clone()),
        display_name: native_session
            .display_name
            .clone()
            .unwrap_or_else(|| display_name.to_string()),
        goal: native_session
            .goal
            .clone()
            .or_else(|| Some(goal.to_string())),
        status: AgentSessionStatus::Active,
        metadata_json,
    })?;
    Ok((
        session_link,
        native_session,
        DispatchEventKind::SessionStarted,
    ))
}

fn resume_session<A>(
    store: &DispatchStore,
    adapter: &mut A,
    run: &DispatchRun,
    session_link_id: &str,
    display_name: &str,
    goal: &str,
    metadata_json: Value,
) -> Result<(AgentSessionLink, AdapterSession, DispatchEventKind)>
where
    A: NativeExecutionAdapter,
{
    let session_link = store.get_session_link(session_link_id)?;
    if session_link.agent_id != run.agent_id {
        anyhow::bail!(
            "session link {} belongs to agent {}, not {}",
            session_link.id,
            session_link.agent_id,
            run.agent_id
        );
    }

    let mut native_session = adapter.adapter_resume_session(&session_link.native_session_id)?;
    native_session =
        adapter.adapter_rename_session(&native_session.native_session_id, display_name)?;
    native_session = adapter.adapter_set_goal(&native_session.native_session_id, goal)?;
    native_session =
        adapter.adapter_set_metadata(&native_session.native_session_id, metadata_json)?;
    let session_link =
        store.update_session_link_status(&session_link.id, AgentSessionStatus::Active)?;
    Ok((
        session_link,
        native_session,
        DispatchEventKind::SessionResumed,
    ))
}

fn ensure_capability(
    store: &DispatchStore,
    agent_id: &str,
    capability: AgentCapabilityName,
) -> Result<()> {
    let capability_record = store.get_agent_capability(agent_id, capability)?;
    if capability_record.status == CapabilityStatus::Unsupported {
        anyhow::bail!(
            "agent {agent_id} does not support capability {}",
            capability.as_str()
        );
    }
    Ok(())
}

fn deterministic_session_name(issue_task: &IssueTask) -> String {
    let title = issue_task.title.trim();
    let short_title = if title.chars().count() > 72 {
        format!("{}...", title.chars().take(69).collect::<String>())
    } else {
        title.to_string()
    };
    format!("issue-finder: {} - {}", issue_task.issue_key, short_title)
}

fn deterministic_goal(issue_task: &IssueTask) -> String {
    format!(
        "Locate, reproduce if practical, and fix {}",
        issue_task.issue_key
    )
}

fn dispatch_metadata(
    run: &DispatchRun,
    issue_task: &IssueTask,
    package_artifact: &AgentArtifact,
) -> Value {
    json!({
        "source": "issue_finder_dispatch_runtime",
        "runId": run.id,
        "issueTaskId": issue_task.id,
        "issueKey": issue_task.issue_key,
        "packageArtifactId": package_artifact.id,
        "packagePath": package_artifact.path
    })
}

fn dispatch_turn_prompt(issue_task: &IssueTask, package_artifact: &AgentArtifact) -> String {
    format!(
        "You are receiving an Issue Finder task package v3.\n\
Goal: follow the package contract to reproduce when practical, make a scoped fix, validate, and report the result.\n\
Read the package artifact first. Respect workspace_policy, reproduction_contract, change_budget, environment_contract, interaction_policy, session_context, and outcome_contract.\n\
Return fix_result.json with reproduction evidence, success criteria status, changed files, validation run, residual risks, failure reason when applicable, session context, and suggested GitHub reply.\n\n\
Issue: {}\n\
Title: {}\n\
Task package artifact id: {}\n\
Task package path: {}\n",
        issue_task.issue_key, issue_task.title, package_artifact.id, package_artifact.path
    )
}
