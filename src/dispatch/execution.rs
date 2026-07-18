use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

use super::adapters::{
    AdapterSession, AdapterStartSessionRequest, AdapterTurn, NativeExecutionAdapter,
};
use super::events::{dispatch_run_event, run_thread_event};
use super::failure::execution_failure;
use super::model::{
    AgentArtifact, AgentCapabilityName, ApprovalStatus, CapabilityStatus, DispatchEvent,
    DispatchEventKind, DispatchEventSeverity, DispatchEventSource, DispatchRun, DispatchRunStatus,
    IssueTask, IssueTaskStatus, NewArtifact,
};
use super::native_runtime::{NativeThreadManager, NativeThreadStore, SendTurnRequest};
use super::store::DispatchStore;
use super::task_package::IssueTaskPackage;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DispatchExecutionResult {
    pub run: DispatchRun,
    pub thread: AdapterSession,
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
    let starting_run = match context.run.status {
        DispatchRunStatus::Approved => store.claim_dispatch_run_for_execution(&context.run.id)?,
        DispatchRunStatus::Starting => context.run.clone(),
        status => anyhow::bail!(
            "dispatch run {run_id} cannot be executed or recovered from status {status}"
        ),
    };
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

    let runtime = tokio::runtime::Runtime::new()?;
    let native_store = NativeThreadStore::open(&store.paths())?;
    let manager = runtime.block_on(NativeThreadManager::connect(native_store))?;
    let mut adapter = NativeRuntimeExecutionAdapter { runtime, manager };
    execute_approved_dispatch(store, &mut adapter, run_id)
}

struct NativeRuntimeExecutionAdapter {
    runtime: tokio::runtime::Runtime,
    manager: NativeThreadManager,
}

impl NativeExecutionAdapter for NativeRuntimeExecutionAdapter {
    fn adapter_start_session(
        &mut self,
        request: AdapterStartSessionRequest,
    ) -> Result<AdapterSession> {
        let id = self.runtime.block_on(
            self.manager
                .start_thread(&request.display_name, &request.cwd),
        )?;
        if let Some(goal) = request.goal.as_deref() {
            self.runtime.block_on(self.manager.set_goal(&id, goal))?;
        }
        Ok(AdapterSession {
            native_session_id: id,
            display_name: Some(request.display_name),
            goal: request.goal,
            metadata_json: request.metadata_json,
        })
    }
    fn adapter_resume_session(&mut self, id: &str) -> Result<AdapterSession> {
        self.runtime.block_on(self.manager.resume(id))?;
        self.runtime.block_on(self.manager.reconcile(id))?;
        Ok(runtime_session(id))
    }
    fn adapter_fork_session(&mut self, _: &str) -> Result<AdapterSession> {
        anyhow::bail!("fork requires the native thread mutation flow")
    }
    fn adapter_rename_session(&mut self, id: &str, name: &str) -> Result<AdapterSession> {
        self.runtime.block_on(self.manager.rename(id, name))?;
        Ok(AdapterSession {
            display_name: Some(name.to_string()),
            ..runtime_session(id)
        })
    }
    fn adapter_set_goal(&mut self, id: &str, goal: &str) -> Result<AdapterSession> {
        self.runtime.block_on(self.manager.set_goal(id, goal))?;
        Ok(AdapterSession {
            goal: Some(goal.to_string()),
            ..runtime_session(id)
        })
    }
    fn adapter_set_metadata(&mut self, id: &str, metadata_json: Value) -> Result<AdapterSession> {
        Ok(AdapterSession {
            metadata_json,
            ..runtime_session(id)
        })
    }
    fn adapter_start_turn(
        &mut self,
        id: &str,
        prompt: &str,
        cwd: &str,
        client_id: &str,
    ) -> Result<AdapterTurn> {
        let started = self.runtime.block_on(self.manager.send(SendTurnRequest {
            thread_id: id.to_string(),
            prompt: prompt.to_string(),
            cwd: cwd.to_string(),
            client_user_message_id: client_id.to_string(),
        }))?;
        Ok(AdapterTurn {
            native_turn_id: started.turn_id,
            status: Some("running".to_string()),
        })
    }
    fn adapter_read_transcript(&mut self, _: &str) -> Result<Value> {
        anyhow::bail!("transcript reads use NativeThreadStore")
    }
    fn adapter_archive_session(&mut self, _: &str) -> Result<AdapterSession> {
        anyhow::bail!("archive requires the native thread mutation flow")
    }
    fn adapter_list_sessions(&mut self, _: Option<usize>) -> Result<Vec<AdapterSession>> {
        anyhow::bail!("thread lists use NativeThreadStore")
    }
    fn adapter_search_sessions(
        &mut self,
        _: &str,
        _: Option<usize>,
    ) -> Result<Vec<AdapterSession>> {
        anyhow::bail!("thread search uses NativeThreadStore")
    }
}

fn runtime_session(id: &str) -> AdapterSession {
    AdapterSession {
        native_session_id: id.to_string(),
        display_name: None,
        goal: None,
        metadata_json: Value::Null,
    }
}

struct ExecutionContext {
    run: DispatchRun,
    issue_task: IssueTask,
    package_artifact: AgentArtifact,
    package: IssueTaskPackage,
    codex_md_path: String,
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
    let package: IssueTaskPackage =
        serde_json::from_slice(&store.read_artifact_bytes(&package_artifact.id)?)
            .context("dispatch package artifact is not valid IssueTaskPackage v3")?;
    package
        .validate_for_execution()
        .context("dispatch package artifact is not executable")?;
    let context_artifact_path = if package.source.handoff_artifact_id.trim().is_empty() {
        package_artifact.path.clone()
    } else {
        store
            .get_artifact(&package.source.handoff_artifact_id)?
            .path
    };
    let codex_md_path = std::path::Path::new(&context_artifact_path)
        .parent()
        .context("handoff artifact path has no parent")?
        .join("codex.md")
        .to_string_lossy()
        .to_string();

    let required_capability = if run.selected_thread_id.is_some() {
        AgentCapabilityName::ResumeSession
    } else {
        AgentCapabilityName::StartSession
    };
    ensure_capability(store, &run.agent_id, required_capability)?;
    ensure_capability(store, &run.agent_id, AgentCapabilityName::SetGoal)?;

    Ok(ExecutionContext {
        run,
        issue_task,
        package_artifact,
        package,
        codex_md_path,
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
    let workspace_path = context.package.workspace_policy.workspace.path.clone();
    let session_setup = NativeSessionSetup {
        display_name: &display_name,
        goal: &goal,
        metadata: &metadata,
        cwd: &workspace_path,
    };

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

    let (native_session, thread_event_type) = match starting_run.selected_thread_id.as_deref() {
        Some(thread_id) => {
            resume_session(store, adapter, &starting_run, thread_id, &session_setup)?
        }
        None => start_session(
            store,
            adapter,
            &starting_run,
            &context.issue_task,
            &session_setup,
        )?,
    };

    events.push(store.append_dispatch_event(run_thread_event(
        &starting_run,
        &native_session.native_session_id,
        thread_event_type,
        DispatchEventSource::Adapter,
        Some(native_session.native_session_id.clone()),
        json!({
            "nativeSessionId": native_session.native_session_id,
            "displayName": native_session.display_name,
            "goal": native_session.goal
        }),
    ))?);

    let run = store.set_dispatch_run_thread(&starting_run.id, &native_session.native_session_id)?;
    let prompt = dispatch_turn_prompt(
        &context.issue_task,
        &context.package_artifact,
        &context.codex_md_path,
    );
    let prompt_artifact = existing_or_write_prompt_artifact(
        store,
        &run,
        &context.issue_task,
        &context.package_artifact,
        &prompt,
    )?;
    let client_user_message_id = format!("issue-finder:{}", run.id);
    crate::eval_fault::crash_at("before_turn_start");
    let turn = adapter.adapter_start_turn(
        &native_session.native_session_id,
        &prompt,
        &workspace_path,
        &client_user_message_id,
    )?;
    events.push(store.append_dispatch_event(run_thread_event(
        &run,
        &native_session.native_session_id,
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
    let run_status = dispatch_status_for_turn(&turn);
    let run = store.update_dispatch_run_status(&run.id, run_status, None)?;

    Ok(DispatchExecutionResult {
        run,
        thread: native_session,
        turn,
        prompt_artifact,
        events,
    })
}

fn existing_or_write_prompt_artifact(
    store: &DispatchStore,
    run: &DispatchRun,
    issue_task: &IssueTask,
    package_artifact: &AgentArtifact,
    prompt: &str,
) -> Result<AgentArtifact> {
    let existing = store
        .list_artifacts_for_run(&run.id)?
        .into_iter()
        .filter(|artifact| artifact.kind == "dispatch_prompt")
        .collect::<Vec<_>>();
    if existing.len() > 1 {
        anyhow::bail!(
            "dispatch run {} has multiple prompt artifacts and cannot be recovered safely",
            run.id
        );
    }
    if let Some(artifact) = existing.into_iter().next() {
        if store.read_artifact_bytes(&artifact.id)? != prompt.as_bytes()
            || artifact
                .metadata_json
                .get("packageArtifactId")
                .and_then(Value::as_str)
                != Some(package_artifact.id.as_str())
        {
            anyhow::bail!(
                "dispatch run {} prompt artifact does not match its current task package",
                run.id
            );
        }
        return Ok(artifact);
    }
    store.write_artifact(
        NewArtifact {
            issue_task_id: Some(issue_task.id.clone()),
            run_id: Some(run.id.clone()),
            kind: "dispatch_prompt".to_string(),
            content_type: "text/plain".to_string(),
            metadata_json: json!({
                "templateVersion": 1,
                "packageArtifactId": package_artifact.id
            }),
        },
        prompt.as_bytes(),
    )
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
    setup: &NativeSessionSetup<'_>,
) -> Result<(AdapterSession, DispatchEventKind)>
where
    A: NativeExecutionAdapter,
{
    let native_session = adapter.adapter_start_session(AdapterStartSessionRequest {
        display_name: setup.display_name.to_string(),
        goal: Some(setup.goal.to_string()),
        metadata_json: setup.metadata.clone(),
        cwd: setup.cwd.to_string(),
    })?;
    let _ = (store, run, issue_task);
    Ok((native_session, DispatchEventKind::ThreadStarted))
}

fn resume_session<A>(
    store: &DispatchStore,
    adapter: &mut A,
    run: &DispatchRun,
    thread_id: &str,
    setup: &NativeSessionSetup<'_>,
) -> Result<(AdapterSession, DispatchEventKind)>
where
    A: NativeExecutionAdapter,
{
    let _ = (store, run);
    let mut native_session = adapter.adapter_resume_session(thread_id)?;
    native_session =
        adapter.adapter_rename_session(&native_session.native_session_id, setup.display_name)?;
    native_session = adapter.adapter_set_goal(&native_session.native_session_id, setup.goal)?;
    native_session =
        adapter.adapter_set_metadata(&native_session.native_session_id, setup.metadata.clone())?;
    Ok((native_session, DispatchEventKind::ThreadResumed))
}

struct NativeSessionSetup<'a> {
    display_name: &'a str,
    goal: &'a str,
    metadata: &'a Value,
    cwd: &'a str,
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

fn dispatch_turn_prompt(
    issue_task: &IssueTask,
    package_artifact: &AgentArtifact,
    codex_md_path: &str,
) -> String {
    format!(
        "Issue Finder dispatch for {}: {}\n\
Read the prepared context at {} and the approved task package at {}.\n\
Follow their workspace, safety, validation, interaction, and outcome contracts. Do not push or create a PR.",
        issue_task.issue_key, issue_task.title, codex_md_path, package_artifact.path
    )
}
