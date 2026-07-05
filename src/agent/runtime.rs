use std::future::Future;

use anyhow::Result;
use futures::future::join_all;
use serde_json::{json, Value};

use crate::config::Config;
use crate::paths::IssueFinderPaths;
use crate::tool_runtime::{IssueFinderToolOutput, IssueFinderToolRuntime};
use crate::tool_specs::TOOL_ASSESS;

use super::context::{
    build_thread_context_with_budget, compact_thread_context_now, consume_pending_thread_mailbox,
    AgentContextBudget,
};
use super::llm_client::{
    AgentLlmClient, AgentModelItem, AgentModelResponse, AgentToolCall,
    OpenAiCompatibleAgentLlmClient,
};
use super::model::{
    AgentApprovalRequest, AgentTaskSendRequest, AgentTaskStatus, AgentThreadStatus,
    AgentThreadTurnRequest,
};
use super::protocol::{
    assessment_required_message, final_model_request, finalization_prompt, model_request,
    model_response_content, model_visible_tool_output, normalize_tool_call_arguments,
    requires_assessment_before_final, tool_observation, tool_output_error, user_prompt,
    TaskOrTurnInput,
};
use super::store::{AgentStore, NewAgentThreadItem};
use super::tool_registry::AgentToolRegistry;

const TOOL_RESULT_INLINE_BYTES: usize = 12_000;

pub async fn run_agent_task(
    paths: IssueFinderPaths,
    config: Config,
    task_id: String,
) -> Result<()> {
    let client = OpenAiCompatibleAgentLlmClient::new(config.clone())?;
    run_agent_task_with_client(paths, config, task_id, &client).await
}

async fn run_agent_task_with_client(
    paths: IssueFinderPaths,
    config: Config,
    task_id: String,
    client: &dyn AgentLlmClient,
) -> Result<()> {
    match run_agent_task_inner(paths.clone(), config, task_id.clone(), client).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = mark_task_failed(&paths, &task_id, error.to_string());
            Err(error)
        }
    }
}

pub async fn run_agent_turn(
    paths: IssueFinderPaths,
    config: Config,
    turn_id: String,
) -> Result<()> {
    let client = OpenAiCompatibleAgentLlmClient::new(config.clone())?;
    run_agent_turn_with_client(paths, config, turn_id, &client).await
}

async fn run_agent_turn_with_client(
    paths: IssueFinderPaths,
    config: Config,
    turn_id: String,
    client: &dyn AgentLlmClient,
) -> Result<()> {
    match run_agent_turn_inner(paths.clone(), config, turn_id.clone(), client).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = mark_turn_failed(&paths, &turn_id, error.to_string());
            Err(error)
        }
    }
}

pub async fn approve_agent_approval_request(
    paths: IssueFinderPaths,
    config: Config,
    thread_id: String,
    approval_request_id: String,
) -> Result<AgentApprovalRequest> {
    let approval = {
        let store = AgentStore::open(paths.clone())?;
        let approval = store.get_agent_approval_request(&approval_request_id)?;
        if approval.thread_id != thread_id {
            anyhow::bail!("approval request does not belong to thread {thread_id}");
        }
        if approval.status == "executed" || approval.status == "failed" {
            return Ok(approval);
        }
        if approval.status == "rejected" {
            anyhow::bail!("approval request {} was rejected", approval.id);
        }
        let approved = store.update_agent_approval_status(&approval.id, "approved", None, None)?;
        store.add_thread_item(NewAgentThreadItem {
            thread_id: &approved.thread_id,
            turn_id: approved.turn_id.as_deref(),
            item_type: "approval_approved",
            role: Some("user"),
            content: Some("User approved an approval-gated tool call."),
            tool_name: Some(&approved.tool_name),
            payload: json!({
                "approvalRequestId": approved.id,
                "toolCallId": approved.tool_call_id,
                "toolName": approved.tool_name,
                "arguments": approved.arguments
            }),
        })?;
        store.add_thread_event(
            &approved.thread_id,
            approved.turn_id.as_deref(),
            "approval_approved",
            "User approved an approval-gated tool call.",
            json!({
                "approvalRequestId": approved.id,
                "toolCallId": approved.tool_call_id,
                "toolName": approved.tool_name
            }),
        )?;
        approved
    };

    let runtime = IssueFinderToolRuntime::new(paths.clone(), config);
    let output = runtime
        .execute(crate::tool_runtime::IssueFinderToolInvocation {
            call_id: format!("approved-{}", approval.tool_call_id),
            turn_id: approval.turn_id.clone(),
            tool_name: approval.tool_name.clone(),
            arguments: approval.arguments.clone(),
        })
        .await;
    let output_json = serde_json::to_value(&output)?;
    let output_success = output.success;
    let output_status = output.status.clone();
    let observation = tool_observation(&approval.tool_name, &output_json);

    let store = AgentStore::open(paths)?;
    store.add_thread_item(NewAgentThreadItem {
        thread_id: &approval.thread_id,
        turn_id: approval.turn_id.as_deref(),
        item_type: if output_success {
            "approval_tool_completed"
        } else {
            "approval_tool_failed"
        },
        role: Some("user"),
        content: Some(&observation),
        tool_name: Some(&approval.tool_name),
        payload: json!({
            "approvalRequestId": approval.id,
            "toolCallId": approval.tool_call_id,
            "success": output_success,
            "status": output_status,
            "output": output_json
        }),
    })?;
    store.add_thread_event(
        &approval.thread_id,
        approval.turn_id.as_deref(),
        if output_success {
            "approval_tool_completed"
        } else {
            "approval_tool_failed"
        },
        "Approved agent tool request executed.",
        json!({
            "approvalRequestId": approval.id,
            "toolCallId": approval.tool_call_id,
            "toolName": approval.tool_name,
            "success": output_success,
            "status": output_status
        }),
    )?;
    let mailbox = store.enqueue_mailbox_item(
        &approval.thread_id,
        approval.turn_id.as_deref(),
        "inject_only",
        &format!(
            "Approval request {} executed for {} with status {}.",
            approval.id, approval.tool_name, output_status
        ),
        json!({
            "source": "approval_execution",
            "approvalRequestId": approval.id,
            "toolName": approval.tool_name,
            "status": output_status,
            "success": output_success
        }),
    )?;
    store.add_thread_event(
        &approval.thread_id,
        approval.turn_id.as_deref(),
        "mailbox_item_queued",
        "Queued approved tool execution result for the agent thread.",
        json!({
            "mailboxItemId": mailbox.id,
            "delivery": mailbox.delivery,
            "approvalRequestId": approval.id
        }),
    )?;

    store.update_agent_approval_status(
        &approval.id,
        if output_success { "executed" } else { "failed" },
        Some(output.structured_content),
        (!output_success).then_some(output_status),
    )
}

pub fn reject_agent_approval_request(
    paths: IssueFinderPaths,
    thread_id: String,
    approval_request_id: String,
    reason: Option<String>,
) -> Result<AgentApprovalRequest> {
    let store = AgentStore::open(paths)?;
    let approval = store.get_agent_approval_request(&approval_request_id)?;
    if approval.thread_id != thread_id {
        anyhow::bail!("approval request does not belong to thread {thread_id}");
    }
    if approval.status == "executed" || approval.status == "failed" {
        anyhow::bail!("approval request {} already executed", approval.id);
    }
    let reason = reason
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "rejected by user".to_string());
    let rejected = store.update_agent_approval_status(
        &approval.id,
        "rejected",
        Some(json!({"reason": reason})),
        Some(reason.clone()),
    )?;
    store.add_thread_item(NewAgentThreadItem {
        thread_id: &rejected.thread_id,
        turn_id: rejected.turn_id.as_deref(),
        item_type: "approval_rejected",
        role: Some("user"),
        content: Some(&format!(
            "User rejected an approval-gated tool call: {reason}"
        )),
        tool_name: Some(&rejected.tool_name),
        payload: json!({
            "approvalRequestId": rejected.id,
            "toolCallId": rejected.tool_call_id,
            "toolName": rejected.tool_name,
            "reason": reason
        }),
    })?;
    store.add_thread_event(
        &rejected.thread_id,
        rejected.turn_id.as_deref(),
        "approval_rejected",
        "User rejected an approval-gated tool call.",
        json!({
            "approvalRequestId": rejected.id,
            "toolCallId": rejected.tool_call_id,
            "toolName": rejected.tool_name,
            "reason": reason
        }),
    )?;
    Ok(rejected)
}

async fn run_agent_task_inner(
    paths: IssueFinderPaths,
    config: Config,
    task_id: String,
    client: &dyn AgentLlmClient,
) -> Result<()> {
    let (task_goal, input) = {
        let store = AgentStore::open(paths.clone())?;
        let task = store.get_task(&task_id)?;
        if task.status.is_terminal() {
            return Ok(());
        }
        store.update_task_status(&task_id, AgentTaskStatus::Running, None, None)?;
        store.add_event(
            &task_id,
            "task_started",
            "Agent task started.",
            json!({"goal": task.goal}),
        )?;
        let input = task_input_from_metadata(&task.metadata, &task.goal)?;
        (task.goal, input)
    };

    let registry = AgentToolRegistry::issue_finder_default();
    let mut input_items = vec![AgentModelItem::UserMessage {
        content: user_prompt(&task_goal, &input),
    }];
    let mut last_tool_output: Option<Value> = None;
    let requires_assessment = requires_assessment_before_final(&task_goal);
    let mut assessed_issue = false;

    for turn_index in 0..input.normalized_max_turns() + 2 {
        let response = client
            .sample_turn(model_request(&config, &registry, input_items.clone()))
            .await?;
        persist_task_model_response(&paths, &task_id, turn_index, &response)?;

        let mut tool_calls = response.tool_calls();
        if tool_calls.is_empty() {
            if let Some(final_answer) = response.final_assistant_message() {
                if requires_assessment && !assessed_issue {
                    input_items.push(AgentModelItem::AssistantMessage {
                        content: final_answer,
                    });
                    input_items.push(AgentModelItem::UserMessage {
                        content: assessment_required_message(),
                    });
                    continue;
                }
                complete_task(
                    &paths,
                    &task_id,
                    &task_goal,
                    final_answer,
                    None,
                    last_tool_output,
                )?;
                return Ok(());
            }
            anyhow::bail!("LLM response did not include a tool call or assistant final answer");
        }

        input_items.extend(response.items.into_iter().filter(|item| {
            matches!(
                item,
                AgentModelItem::AssistantMessage { .. } | AgentModelItem::ReasoningSummary { .. }
            )
        }));

        for (call_index, call) in tool_calls.iter_mut().enumerate() {
            normalize_tool_call_arguments(call, TaskOrTurnInput::Task(&input))?;
            let is_assess_call = call.canonical_name() == TOOL_ASSESS;
            let output = execute_task_tool_call(
                &paths, &config, &registry, &task_id, turn_index, call_index, call,
            )
            .await?;
            if is_assess_call && output.success {
                assessed_issue = true;
            }
            let output_json = serde_json::to_value(&output)?;
            last_tool_output = Some(output_json.clone());
            input_items.push(AgentModelItem::ToolCall(call.clone()));
            input_items.push(AgentModelItem::FunctionCallOutput {
                call_id: call.call_id.clone(),
                output: model_visible_tool_output(&output_json),
            });
        }
    }

    input_items.push(AgentModelItem::UserMessage {
        content: finalization_prompt(),
    });
    let final_index = input.normalized_max_turns() + 2;
    let response = client
        .sample_turn(final_model_request(&config, &registry, input_items))
        .await?;
    persist_task_model_response(&paths, &task_id, final_index, &response)?;
    if let Some(final_answer) = response.final_assistant_message() {
        complete_task(
            &paths,
            &task_id,
            &task_goal,
            final_answer,
            None,
            last_tool_output,
        )?;
        return Ok(());
    }

    anyhow::bail!(
        "agent task reached max_turns={} without final answer",
        input.normalized_max_turns()
    )
}

async fn execute_task_tool_call(
    paths: &IssueFinderPaths,
    config: &Config,
    registry: &AgentToolRegistry,
    task_id: &str,
    turn_index: usize,
    call_index: usize,
    call: &AgentToolCall,
) -> Result<IssueFinderToolOutput> {
    let tool_name = call.canonical_name();
    {
        let store = AgentStore::open(paths.clone())?;
        store.start_tool_call_with_id(
            &call.call_id,
            task_id,
            turn_index,
            &tool_name,
            call.arguments.clone(),
        )?;
        store.add_event(
            task_id,
            "tool_call_started",
            &format!("Agent called {tool_name}."),
            json!({
                "toolName": tool_name,
                "toolCallId": call.call_id,
                "turnIndex": turn_index,
                "callIndex": call_index,
                "providerItemId": call.provider_item_id,
                "providerCallIdSource": call.provider_call_id_source
            }),
        )?;
    }

    let runtime = IssueFinderToolRuntime::new(paths.clone(), config.clone());
    let output = registry
        .execute_call(&runtime, paths, Some(task_id.to_string()), call)
        .await;
    let output_json = serde_json::to_value(&output)?;
    let output_success = output.success;
    let output_status = output.status.clone();
    let tool_error = (!output_success).then(|| tool_output_error(&output_json));

    {
        let store = AgentStore::open(paths.clone())?;
        store.finish_tool_call(
            &call.call_id,
            if output_success { "ok" } else { "failed" },
            Some(output_json),
            tool_error,
        )?;
        store.add_event(
            task_id,
            if output_success {
                "tool_call_completed"
            } else {
                "tool_call_failed"
            },
            &format!("Agent tool {tool_name} returned {output_status}."),
            json!({
                "toolName": tool_name,
                "toolCallId": call.call_id,
                "success": output_success,
                "status": output_status,
                "turnIndex": turn_index,
                "callIndex": call_index,
                "providerItemId": call.provider_item_id
            }),
        )?;
    }

    Ok(output)
}

fn persist_task_model_response(
    paths: &IssueFinderPaths,
    task_id: &str,
    turn_index: usize,
    response: &AgentModelResponse,
) -> Result<()> {
    let store = AgentStore::open(paths.clone())?;
    store.add_message(
        task_id,
        "assistant",
        &model_response_content(response),
        json!({
            "turnIndex": turn_index,
            "messageType": "agent_model_response",
            "rawProviderResponse": response.raw_provider_response,
            "usage": response.usage
        }),
    )?;
    Ok(())
}

async fn run_agent_turn_inner(
    paths: IssueFinderPaths,
    config: Config,
    turn_id: String,
    client: &dyn AgentLlmClient,
) -> Result<()> {
    let (thread_id, turn_input, input) = {
        let store = AgentStore::open(paths.clone())?;
        let turn = store.get_turn(&turn_id)?;
        if turn.status.is_terminal() {
            return Ok(());
        }
        let thread = store.get_thread(&turn.thread_id)?;
        if thread.status == AgentThreadStatus::Archived {
            anyhow::bail!("agent thread {} is archived", thread.id);
        }
        store.update_thread_status(&thread.id, AgentThreadStatus::Running)?;
        store.update_turn_status(&turn_id, AgentTaskStatus::Running, None, None)?;
        store.add_thread_event(
            &thread.id,
            Some(&turn_id),
            "turn_started",
            "Agent turn started.",
            json!({"input": turn.input}),
        )?;
        let input = turn_input_from_metadata(&turn.metadata, &turn.input)?;
        (thread.id, turn.input, input)
    };

    let registry = AgentToolRegistry::issue_finder_default();
    let mut input_items = {
        let store = AgentStore::open(paths.clone())?;
        build_thread_context_with_budget(
            &store,
            &thread_id,
            &turn_id,
            &turn_input,
            &input,
            AgentContextBudget::from_config(&config.agent),
        )?
    };

    let mut last_tool_output: Option<Value> = None;
    let requires_assessment = requires_assessment_before_final(&turn_input);
    let mut assessed_issue = {
        let store = AgentStore::open(paths.clone())?;
        thread_has_assessment(&store, &thread_id)?
    };

    for turn_index in 0..input.normalized_max_turns() + 2 {
        append_mailbox_messages(&paths, &thread_id, &turn_id, &mut input_items)?;
        if turn_is_cancelled(&paths, &turn_id)? {
            return Ok(());
        }

        let response = client
            .sample_turn(model_request(&config, &registry, input_items.clone()))
            .await?;
        persist_thread_model_response(&paths, &thread_id, &turn_id, turn_index, &response)?;
        if turn_is_cancelled(&paths, &turn_id)? {
            return Ok(());
        }

        let mut tool_calls = response.tool_calls();
        if tool_calls.is_empty() {
            if let Some(final_answer) = response.final_assistant_message() {
                if requires_assessment && !assessed_issue {
                    input_items.push(AgentModelItem::AssistantMessage {
                        content: final_answer,
                    });
                    input_items.push(AgentModelItem::UserMessage {
                        content: assessment_required_message(),
                    });
                    continue;
                }
                complete_turn(
                    &paths,
                    &thread_id,
                    &turn_id,
                    &turn_input,
                    final_answer,
                    None,
                    last_tool_output,
                )?;
                maintain_thread_context(&paths, &config, &thread_id)?;
                return Ok(());
            }
            anyhow::bail!("LLM response did not include a tool call or assistant final answer");
        }

        input_items.extend(response.items.into_iter().filter(|item| {
            matches!(
                item,
                AgentModelItem::AssistantMessage { .. } | AgentModelItem::ReasoningSummary { .. }
            )
        }));

        for call in tool_calls.iter_mut() {
            normalize_tool_call_arguments(call, TaskOrTurnInput::Turn(&input))?;
        }
        let tool_outputs = execute_thread_tool_calls(
            &paths, &config, &registry, &thread_id, &turn_id, turn_index, tool_calls,
        )
        .await?;

        for (call, output) in tool_outputs {
            if turn_is_cancelled(&paths, &turn_id)? {
                return Ok(());
            }
            let is_assess_call = call.canonical_name() == TOOL_ASSESS;
            if is_assess_call && output.success {
                assessed_issue = true;
            }
            let output_json = serde_json::to_value(&output)?;
            last_tool_output = Some(output_json.clone());
            input_items.push(AgentModelItem::ToolCall(call));
            input_items.push(AgentModelItem::FunctionCallOutput {
                call_id: output.call_id.clone(),
                output: model_visible_tool_output(&output_json),
            });
            append_mailbox_messages(&paths, &thread_id, &turn_id, &mut input_items)?;
        }
    }

    input_items.push(AgentModelItem::UserMessage {
        content: finalization_prompt(),
    });
    let final_index = input.normalized_max_turns() + 2;
    let response = client
        .sample_turn(final_model_request(&config, &registry, input_items))
        .await?;
    persist_thread_model_response(&paths, &thread_id, &turn_id, final_index, &response)?;
    if let Some(final_answer) = response.final_assistant_message() {
        complete_turn(
            &paths,
            &thread_id,
            &turn_id,
            &turn_input,
            final_answer,
            None,
            last_tool_output,
        )?;
        maintain_thread_context(&paths, &config, &thread_id)?;
        return Ok(());
    }

    anyhow::bail!(
        "agent turn reached max_turns={} without final answer",
        input.normalized_max_turns()
    )
}

fn thread_has_assessment(store: &AgentStore, thread_id: &str) -> Result<bool> {
    Ok(store.list_thread_items(thread_id)?.iter().any(|item| {
        item.item_type == "tool_call_completed"
            && item.tool_name.as_deref() == Some(TOOL_ASSESS)
            && item
                .payload
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false)
    }))
}

fn append_mailbox_messages(
    paths: &IssueFinderPaths,
    thread_id: &str,
    turn_id: &str,
    input_items: &mut Vec<AgentModelItem>,
) -> Result<()> {
    let store = AgentStore::open(paths.clone())?;
    let messages = consume_pending_thread_mailbox(&store, thread_id, turn_id)?;
    input_items.extend(messages);
    Ok(())
}

fn turn_is_cancelled(paths: &IssueFinderPaths, turn_id: &str) -> Result<bool> {
    let store = AgentStore::open(paths.clone())?;
    Ok(matches!(
        store.get_turn(turn_id)?.status,
        AgentTaskStatus::Cancelled
    ))
}

fn maintain_thread_context(
    paths: &IssueFinderPaths,
    config: &Config,
    thread_id: &str,
) -> Result<()> {
    let store = AgentStore::open(paths.clone())?;
    let _ = compact_thread_context_now(
        &store,
        thread_id,
        AgentContextBudget::from_config(&config.agent),
    )?;
    Ok(())
}

async fn execute_thread_tool_call(
    paths: &IssueFinderPaths,
    config: &Config,
    registry: &AgentToolRegistry,
    execution: ThreadToolExecution<'_>,
    call: &AgentToolCall,
) -> Result<IssueFinderToolOutput> {
    let started = write_thread_tool_call_started(paths, execution, call)?;
    let output =
        execute_thread_tool_call_runtime(paths, config, registry, execution.turn_id, call).await;
    write_thread_tool_call_finished(paths, execution, call, &started, &output)?;
    Ok(output)
}

async fn execute_thread_tool_calls(
    paths: &IssueFinderPaths,
    config: &Config,
    registry: &AgentToolRegistry,
    thread_id: &str,
    turn_id: &str,
    turn_index: usize,
    calls: Vec<AgentToolCall>,
) -> Result<Vec<(AgentToolCall, IssueFinderToolOutput)>> {
    if calls.len() > 1 && registry.calls_support_parallel(&calls) {
        execute_thread_tool_calls_parallel(
            paths, config, registry, thread_id, turn_id, turn_index, calls,
        )
        .await
    } else {
        let mut outputs = Vec::with_capacity(calls.len());
        for (call_index, call) in calls.into_iter().enumerate() {
            if turn_is_cancelled(paths, turn_id)? {
                return Ok(outputs);
            }
            let output = execute_thread_tool_call(
                paths,
                config,
                registry,
                ThreadToolExecution {
                    thread_id,
                    turn_id,
                    turn_index,
                    call_index,
                },
                &call,
            )
            .await?;
            outputs.push((call, output));
        }
        Ok(outputs)
    }
}

async fn execute_thread_tool_calls_parallel(
    paths: &IssueFinderPaths,
    config: &Config,
    registry: &AgentToolRegistry,
    thread_id: &str,
    turn_id: &str,
    turn_index: usize,
    calls: Vec<AgentToolCall>,
) -> Result<Vec<(AgentToolCall, IssueFinderToolOutput)>> {
    let paths_for_execute = paths.clone();
    let config_for_execute = config.clone();
    let registry_for_execute = registry.clone();
    let turn_id_for_execute = turn_id.to_string();
    execute_thread_tool_calls_parallel_with(
        paths,
        thread_id,
        turn_id,
        turn_index,
        calls,
        move |call| {
            let paths = paths_for_execute.clone();
            let config = config_for_execute.clone();
            let registry = registry_for_execute.clone();
            let turn_id = turn_id_for_execute.clone();
            async move {
                execute_thread_tool_call_runtime(&paths, &config, &registry, &turn_id, &call).await
            }
        },
    )
    .await
}

async fn execute_thread_tool_calls_parallel_with<F, Fut>(
    paths: &IssueFinderPaths,
    thread_id: &str,
    turn_id: &str,
    turn_index: usize,
    calls: Vec<AgentToolCall>,
    execute: F,
) -> Result<Vec<(AgentToolCall, IssueFinderToolOutput)>>
where
    F: Fn(AgentToolCall) -> Fut,
    Fut: Future<Output = IssueFinderToolOutput>,
{
    let mut started = Vec::with_capacity(calls.len());
    for (call_index, call) in calls.iter().enumerate() {
        if turn_is_cancelled(paths, turn_id)? {
            return Ok(Vec::new());
        }
        let execution = ThreadToolExecution {
            thread_id,
            turn_id,
            turn_index,
            call_index,
        };
        let started_item_id = write_thread_tool_call_started(paths, execution, call)?;
        started.push((call_index, call.clone(), started_item_id));
    }

    let futures = started.iter().map(|(_, call, _)| execute(call.clone()));
    let outputs = join_all(futures).await;

    let mut results = Vec::with_capacity(started.len());
    for ((call_index, call, started_item_id), output) in started.into_iter().zip(outputs) {
        let execution = ThreadToolExecution {
            thread_id,
            turn_id,
            turn_index,
            call_index,
        };
        write_thread_tool_call_finished(paths, execution, &call, &started_item_id, &output)?;
        results.push((call, output));
    }
    Ok(results)
}

fn write_thread_tool_call_started(
    paths: &IssueFinderPaths,
    execution: ThreadToolExecution<'_>,
    call: &AgentToolCall,
) -> Result<String> {
    let tool_name = call.canonical_name();
    let store = AgentStore::open(paths.clone())?;
    let item = store.add_thread_item(NewAgentThreadItem {
        thread_id: execution.thread_id,
        turn_id: Some(execution.turn_id),
        item_type: "tool_call_started",
        role: None,
        content: None,
        tool_name: Some(&tool_name),
        payload: json!({
            "toolName": tool_name,
            "toolCallId": call.call_id,
            "arguments": call.arguments,
            "turnIndex": execution.turn_index,
            "callIndex": execution.call_index,
            "providerItemId": call.provider_item_id,
            "providerCallIdSource": call.provider_call_id_source
        }),
    })?;
    store.add_thread_event(
        execution.thread_id,
        Some(execution.turn_id),
        "tool_call_started",
        &format!("Agent called {tool_name}."),
        json!({
            "toolName": tool_name,
            "toolCallId": call.call_id,
            "localItemId": item.id,
            "turnIndex": execution.turn_index,
            "callIndex": execution.call_index,
            "providerItemId": call.provider_item_id,
            "providerCallIdSource": call.provider_call_id_source
        }),
    )?;
    Ok(item.id)
}

async fn execute_thread_tool_call_runtime(
    paths: &IssueFinderPaths,
    config: &Config,
    registry: &AgentToolRegistry,
    turn_id: &str,
    call: &AgentToolCall,
) -> IssueFinderToolOutput {
    let runtime = IssueFinderToolRuntime::new(paths.clone(), config.clone());
    registry
        .execute_call(&runtime, paths, Some(turn_id.to_string()), call)
        .await
}

fn write_thread_tool_call_finished(
    paths: &IssueFinderPaths,
    execution: ThreadToolExecution<'_>,
    call: &AgentToolCall,
    local_started_item_id: &str,
    output: &IssueFinderToolOutput,
) -> Result<()> {
    let tool_name = call.canonical_name();
    let output_json = serde_json::to_value(output)?;
    let output_success = output.success;
    let output_status = output.status.clone();
    let tool_error = (!output_success).then(|| tool_output_error(&output_json));
    let observation = tool_observation(&tool_name, &output_json);
    let approval_request_id =
        record_pending_approval_request(paths, execution, call, &tool_name, &output_json)?;

    let store = AgentStore::open(paths.clone())?;
    let payload = tool_result_thread_payload(
        &store,
        execution,
        call,
        &tool_name,
        local_started_item_id,
        output_success,
        &output_status,
        tool_error.clone(),
        approval_request_id.clone(),
        &output_json,
    )?;
    let item = store.add_thread_item(NewAgentThreadItem {
        thread_id: execution.thread_id,
        turn_id: Some(execution.turn_id),
        item_type: if output_success {
            "tool_call_completed"
        } else {
            "tool_call_failed"
        },
        role: Some("user"),
        content: Some(&observation),
        tool_name: Some(&tool_name),
        payload,
    })?;
    let result_artifact_id = item
        .payload
        .get("resultArtifactId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    store.add_thread_event(
        execution.thread_id,
        Some(execution.turn_id),
        if output_success {
            "tool_call_completed"
        } else {
            "tool_call_failed"
        },
        &format!("Agent tool {tool_name} returned {output_status}."),
        json!({
            "toolName": tool_name,
            "toolCallId": call.call_id,
            "localItemId": item.id,
            "localStartedItemId": local_started_item_id,
            "success": output_success,
            "status": output_status,
            "turnIndex": execution.turn_index,
            "callIndex": execution.call_index,
            "providerItemId": call.provider_item_id,
            "resultArtifactId": result_artifact_id
        }),
    )?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn tool_result_thread_payload(
    store: &AgentStore,
    execution: ThreadToolExecution<'_>,
    call: &AgentToolCall,
    tool_name: &str,
    local_started_item_id: &str,
    output_success: bool,
    output_status: &str,
    tool_error: Option<String>,
    approval_request_id: Option<String>,
    output_json: &Value,
) -> Result<Value> {
    let raw = serde_json::to_vec_pretty(output_json)?;
    let artifact = store.write_artifact(
        "agent_tool_result",
        "application/json",
        &raw,
        json!({
            "threadId": execution.thread_id,
            "turnId": execution.turn_id,
            "toolName": tool_name,
            "toolCallId": call.call_id,
            "providerItemId": call.provider_item_id,
            "success": output_success,
            "status": output_status,
            "byteLen": raw.len()
        }),
    )?;
    let inline_output = raw.len() <= TOOL_RESULT_INLINE_BYTES;
    let mut payload = json!({
        "toolName": tool_name,
        "toolCallId": call.call_id,
        "providerItemId": call.provider_item_id,
        "providerCallIdSource": call.provider_call_id_source,
        "localStartedItemId": local_started_item_id,
        "success": output_success,
        "status": output_status,
        "error": tool_error,
        "approvalRequestId": approval_request_id,
        "resultArtifactId": artifact.id,
        "resultArtifact": {
            "artifactId": artifact.id,
            "contentType": artifact.content_type,
            "sha256": artifact.sha256,
            "byteLen": artifact.byte_len,
            "summary": artifact.summary
        },
        "outputStorage": if inline_output { "inline" } else { "artifact" }
    });
    if inline_output {
        payload["output"] = output_json.clone();
    }
    Ok(payload)
}

fn record_pending_approval_request(
    paths: &IssueFinderPaths,
    execution: ThreadToolExecution<'_>,
    call: &AgentToolCall,
    tool_name: &str,
    output_json: &Value,
) -> Result<Option<String>> {
    if output_json.pointer("/status").and_then(Value::as_str) != Some("pending_approval") {
        return Ok(None);
    }
    let Some(approval_request_id) = output_json
        .pointer("/structured_content/approvalRequired/approvalRequestId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return Ok(None);
    };

    let store = AgentStore::open(paths.clone())?;
    let approval = store.create_agent_approval_request(
        &approval_request_id,
        execution.thread_id,
        Some(execution.turn_id),
        &call.call_id,
        tool_name,
        call.arguments.clone(),
    )?;
    store.add_thread_event(
        execution.thread_id,
        Some(execution.turn_id),
        "approval_required",
        "Agent tool call requires user approval.",
        json!({
            "approvalRequestId": approval.id,
            "toolName": approval.tool_name,
            "toolCallId": approval.tool_call_id
        }),
    )?;
    Ok(Some(approval_request_id))
}

#[derive(Debug, Clone, Copy)]
struct ThreadToolExecution<'a> {
    thread_id: &'a str,
    turn_id: &'a str,
    turn_index: usize,
    call_index: usize,
}

fn persist_thread_model_response(
    paths: &IssueFinderPaths,
    thread_id: &str,
    turn_id: &str,
    turn_index: usize,
    response: &AgentModelResponse,
) -> Result<()> {
    let store = AgentStore::open(paths.clone())?;
    store.add_thread_item(NewAgentThreadItem {
        thread_id,
        turn_id: Some(turn_id),
        item_type: "assistant_model_response",
        role: Some("assistant"),
        content: Some(&model_response_content(response)),
        tool_name: None,
        payload: json!({
            "turnIndex": turn_index,
            "messageType": "agent_model_response",
            "items": response.items,
            "rawProviderResponse": response.raw_provider_response,
            "usage": response.usage
        }),
    })?;
    Ok(())
}

fn task_input_from_metadata(metadata: &Value, goal: &str) -> Result<AgentTaskSendRequest> {
    if let Some(input) = metadata.get("input") {
        let mut parsed = serde_json::from_value::<AgentTaskSendRequest>(input.clone())?;
        if parsed.goal.trim().is_empty() {
            parsed.goal = goal.to_string();
        }
        return Ok(parsed);
    }

    Ok(AgentTaskSendRequest {
        goal: goal.to_string(),
        repo: None,
        limit: Some(10),
        refresh: false,
        max_turns: Some(4),
        run_immediately: true,
    })
}

fn turn_input_from_metadata(metadata: &Value, input: &str) -> Result<AgentThreadTurnRequest> {
    if let Some(raw_input) = metadata.get("input") {
        let mut parsed = serde_json::from_value::<AgentThreadTurnRequest>(raw_input.clone())?;
        if parsed.input.trim().is_empty() {
            parsed.input = input.to_string();
        }
        return Ok(parsed);
    }

    Ok(AgentThreadTurnRequest {
        input: input.to_string(),
        repo: None,
        limit: Some(10),
        refresh: false,
        max_turns: Some(4),
        run_immediately: true,
    })
}

fn complete_task(
    paths: &IssueFinderPaths,
    task_id: &str,
    goal: &str,
    final_answer: String,
    rationale: Option<String>,
    last_tool_output: Option<Value>,
) -> Result<()> {
    let result = json!({
        "kind": "issue_finder_agent_result",
        "version": 1,
        "goal": goal,
        "finalAnswer": final_answer,
        "rationale": rationale,
        "lastToolOutput": last_tool_output
    });
    let store = AgentStore::open(paths.clone())?;
    store.add_message(
        task_id,
        "assistant",
        result["finalAnswer"].as_str().unwrap_or_default(),
        json!({"messageType": "final_answer"}),
    )?;
    store.update_task_status(task_id, AgentTaskStatus::Completed, Some(result), None)?;
    store.add_event(
        task_id,
        "task_completed",
        "Agent task completed.",
        json!({}),
    )?;
    Ok(())
}

fn complete_turn(
    paths: &IssueFinderPaths,
    thread_id: &str,
    turn_id: &str,
    input: &str,
    final_answer: String,
    rationale: Option<String>,
    last_tool_output: Option<Value>,
) -> Result<()> {
    let result = json!({
        "kind": "issue_finder_agent_turn_result",
        "version": 1,
        "input": input,
        "finalAnswer": final_answer,
        "rationale": rationale,
        "lastToolOutput": last_tool_output
    });
    let store = AgentStore::open(paths.clone())?;
    store.add_thread_item(NewAgentThreadItem {
        thread_id,
        turn_id: Some(turn_id),
        item_type: "final_answer",
        role: Some("assistant"),
        content: result["finalAnswer"].as_str(),
        tool_name: None,
        payload: result.clone(),
    })?;
    store.update_turn_status(turn_id, AgentTaskStatus::Completed, Some(result), None)?;
    store.update_thread_status(thread_id, AgentThreadStatus::Active)?;
    store.add_thread_event(
        thread_id,
        Some(turn_id),
        "turn_completed",
        "Agent turn completed.",
        json!({}),
    )?;
    Ok(())
}

fn mark_task_failed(paths: &IssueFinderPaths, task_id: &str, error: String) -> Result<()> {
    let store = AgentStore::open(paths.clone())?;
    store.update_task_status(task_id, AgentTaskStatus::Failed, None, Some(error.clone()))?;
    store.add_event(
        task_id,
        "task_failed",
        "Agent task failed.",
        json!({ "error": error }),
    )?;
    Ok(())
}

fn mark_turn_failed(paths: &IssueFinderPaths, turn_id: &str, error: String) -> Result<()> {
    let store = AgentStore::open(paths.clone())?;
    let turn = store.get_turn(turn_id)?;
    store.update_turn_status(turn_id, AgentTaskStatus::Failed, None, Some(error.clone()))?;
    store.update_thread_status(&turn.thread_id, AgentThreadStatus::Active)?;
    store.add_thread_event(
        &turn.thread_id,
        Some(turn_id),
        "turn_failed",
        "Agent turn failed.",
        json!({ "error": error }),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use anyhow::{anyhow, Result};
    use futures::future::BoxFuture;
    use serde_json::json;
    use tempfile::tempdir;
    use tokio::sync::Barrier;
    use tokio::time::timeout;

    use super::{
        execute_thread_tool_call, execute_thread_tool_calls_parallel_with,
        run_agent_turn_with_client, thread_has_assessment, turn_is_cancelled, ThreadToolExecution,
    };
    use crate::agent::llm_client::{
        AgentLlmClient, AgentModelItem, AgentModelRequest, AgentModelResponse, AgentToolCall,
        AgentWireApi,
    };
    use crate::agent::model::AgentTaskStatus;
    use crate::agent::store::AgentStore;
    use crate::agent::tool_registry::AgentToolRegistry;
    use crate::config::Config;
    use crate::paths::IssueFinderPaths;
    use crate::tool_runtime::{IssueFinderContentItem, IssueFinderToolOutput};

    struct FakeAgentLlmClient {
        responses: Mutex<VecDeque<std::result::Result<AgentModelResponse, String>>>,
        requests: Mutex<Vec<AgentModelRequest>>,
        wire_api: AgentWireApi,
    }

    impl FakeAgentLlmClient {
        fn new(responses: Vec<std::result::Result<AgentModelResponse, String>>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                requests: Mutex::new(Vec::new()),
                wire_api: AgentWireApi::Responses,
            }
        }

        fn requests(&self) -> Vec<AgentModelRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl AgentLlmClient for FakeAgentLlmClient {
        fn sample_turn<'a>(
            &'a self,
            request: AgentModelRequest,
        ) -> BoxFuture<'a, Result<AgentModelResponse>> {
            Box::pin(async move {
                self.requests.lock().unwrap().push(request);
                self.responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| Err("fake LLM response queue exhausted".to_string()))
                    .map_err(|message| anyhow!(message))
            })
        }

        fn wire_api(&self) -> AgentWireApi {
            self.wire_api
        }
    }

    fn tool_call_response(
        call_id: &str,
        name: &str,
        arguments: serde_json::Value,
    ) -> AgentModelResponse {
        AgentModelResponse {
            items: vec![AgentModelItem::ToolCall(AgentToolCall {
                call_id: call_id.to_string(),
                namespace: Some("issue-finder".to_string()),
                name: name.to_string(),
                arguments,
                provider_item_id: Some(format!("fc_{call_id}")),
                provider_call_id_source: Some("fake_responses".to_string()),
            })],
            usage: None,
            raw_provider_response: json!({"fake": "tool_call", "callId": call_id}),
        }
    }

    fn final_response(content: &str) -> AgentModelResponse {
        AgentModelResponse {
            items: vec![AgentModelItem::AssistantMessage {
                content: content.to_string(),
            }],
            usage: None,
            raw_provider_response: json!({"fake": "final"}),
        }
    }

    fn thread_with_turn(
        paths: &IssueFinderPaths,
        input: &str,
        metadata: serde_json::Value,
    ) -> (String, String) {
        let store = AgentStore::open(paths.clone()).unwrap();
        let thread = store
            .create_thread("agent test thread", "agent test", json!({}))
            .unwrap();
        let turn = store.create_turn(&thread.id, input, metadata).unwrap();
        (thread.id, turn.id)
    }

    #[tokio::test]
    async fn runtime_turn_runs_one_tool_call_then_final_answer() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let (thread_id, turn_id) = thread_with_turn(&paths, "check daemon status", json!({}));
        let client = FakeAgentLlmClient::new(vec![
            Ok(tool_call_response(
                "call_status",
                "status",
                json!({"checkAuth": false}),
            )),
            Ok(final_response("Issue Finder is reachable.")),
        ]);

        run_agent_turn_with_client(paths.clone(), Config::default(), turn_id.clone(), &client)
            .await
            .unwrap();

        let detail = AgentStore::open(paths)
            .unwrap()
            .thread_detail(&thread_id)
            .unwrap();
        assert_eq!(detail.turns[0].status, AgentTaskStatus::Completed);
        assert!(detail.items.iter().any(|item| {
            item.item_type == "tool_call_completed"
                && item.payload["toolCallId"] == "call_status"
                && item.tool_name.as_deref() == Some("issue-finder.status")
        }));
        assert!(detail.items.iter().any(|item| {
            item.item_type == "final_answer"
                && item.content.as_deref() == Some("Issue Finder is reachable.")
        }));

        let requests = client.requests();
        assert_eq!(requests.len(), 2);
        assert!(!requests[0].tools.is_empty());
        assert!(requests[1].input_items.iter().any(|item| matches!(
            item,
            AgentModelItem::FunctionCallOutput { call_id, .. } if call_id == "call_status"
        )));
    }

    #[tokio::test]
    async fn runtime_turn_marks_failed_on_provider_error() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let (thread_id, turn_id) = thread_with_turn(&paths, "check daemon status", json!({}));
        let client = FakeAgentLlmClient::new(vec![Err("provider unavailable".to_string())]);

        let error =
            run_agent_turn_with_client(paths.clone(), Config::default(), turn_id.clone(), &client)
                .await
                .unwrap_err();

        assert!(error.to_string().contains("provider unavailable"));
        let detail = AgentStore::open(paths)
            .unwrap()
            .thread_detail(&thread_id)
            .unwrap();
        assert_eq!(detail.turns[0].status, AgentTaskStatus::Failed);
        assert!(detail
            .events
            .iter()
            .any(|event| event.kind == "turn_failed"));
    }

    #[tokio::test]
    async fn runtime_turn_returns_tool_error_to_model_and_can_finish() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let (thread_id, turn_id) = thread_with_turn(&paths, "check daemon status", json!({}));
        let client = FakeAgentLlmClient::new(vec![
            Ok(tool_call_response("call_bad", "not_registered", json!({}))),
            Ok(final_response(
                "No valid tool was available for that request.",
            )),
        ]);

        run_agent_turn_with_client(paths.clone(), Config::default(), turn_id.clone(), &client)
            .await
            .unwrap();

        let requests = client.requests();
        let output = requests[1]
            .input_items
            .iter()
            .find_map(|item| match item {
                AgentModelItem::FunctionCallOutput { call_id, output } if call_id == "call_bad" => {
                    Some(output)
                }
                _ => None,
            })
            .expect("tool failure was returned to the model");
        assert!(output
            .as_str()
            .expect("model-visible output is a string")
            .contains("unknown_tool"));

        let detail = AgentStore::open(paths)
            .unwrap()
            .thread_detail(&thread_id)
            .unwrap();
        assert_eq!(detail.turns[0].status, AgentTaskStatus::Completed);
        assert!(detail
            .items
            .iter()
            .any(|item| item.item_type == "tool_call_failed"));
    }

    #[tokio::test]
    async fn runtime_turn_fails_invalid_tool_call_arguments() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let (thread_id, turn_id) = thread_with_turn(&paths, "check daemon status", json!({}));
        let client = FakeAgentLlmClient::new(vec![Ok(tool_call_response(
            "call_invalid",
            "status",
            json!("not an object"),
        ))]);

        let error =
            run_agent_turn_with_client(paths.clone(), Config::default(), turn_id.clone(), &client)
                .await
                .unwrap_err();

        assert!(error
            .to_string()
            .contains("tool arguments must be a JSON object"));
        let detail = AgentStore::open(paths)
            .unwrap()
            .thread_detail(&thread_id)
            .unwrap();
        assert_eq!(detail.turns[0].status, AgentTaskStatus::Failed);
    }

    #[tokio::test]
    async fn runtime_turn_uses_finalization_request_after_tool_budget_exhaustion() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let (thread_id, turn_id) = thread_with_turn(
            &paths,
            "check daemon status",
            json!({
                "input": {
                    "input": "check daemon status",
                    "maxTurns": 1,
                    "runImmediately": true
                }
            }),
        );
        let client = FakeAgentLlmClient::new(vec![
            Ok(tool_call_response(
                "call_1",
                "status",
                json!({"checkAuth": false}),
            )),
            Ok(tool_call_response(
                "call_2",
                "status",
                json!({"checkAuth": false}),
            )),
            Ok(tool_call_response(
                "call_3",
                "status",
                json!({"checkAuth": false}),
            )),
            Ok(tool_call_response(
                "call_4",
                "status",
                json!({"checkAuth": false}),
            )),
        ]);

        let error =
            run_agent_turn_with_client(paths.clone(), Config::default(), turn_id.clone(), &client)
                .await
                .unwrap_err();

        assert!(error
            .to_string()
            .contains("agent turn reached max_turns=1 without final answer"));
        let requests = client.requests();
        assert_eq!(requests.len(), 4);
        assert!(requests.last().unwrap().tools.is_empty());
        assert!(requests
            .last()
            .unwrap()
            .instructions
            .contains("Do not call tools"));

        let detail = AgentStore::open(paths)
            .unwrap()
            .thread_detail(&thread_id)
            .unwrap();
        assert_eq!(detail.turns[0].status, AgentTaskStatus::Failed);
    }

    #[tokio::test]
    async fn thread_tool_call_lifecycle_keeps_provider_call_ids_and_order() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let store = AgentStore::open(paths.clone()).unwrap();
        let thread = store
            .create_thread("search issues", "search", json!({}))
            .unwrap();
        let turn = store
            .create_turn(&thread.id, "find candidates", json!({}))
            .unwrap();
        drop(store);

        let registry = AgentToolRegistry::issue_finder_default();
        let config = Config::default();
        let mut observed_outputs = Vec::new();
        for (call_index, call_id) in ["call_123", "call_456"].into_iter().enumerate() {
            let call = AgentToolCall {
                call_id: call_id.to_string(),
                namespace: Some("issue-finder".to_string()),
                name: "not_registered".to_string(),
                arguments: json!({}),
                provider_item_id: Some(format!("fc_{call_id}")),
                provider_call_id_source: Some("provider_responses".to_string()),
            };
            let output = execute_thread_tool_call(
                &paths,
                &config,
                &registry,
                ThreadToolExecution {
                    thread_id: &thread.id,
                    turn_id: &turn.id,
                    turn_index: 0,
                    call_index,
                },
                &call,
            )
            .await
            .unwrap();
            observed_outputs.push(output);
        }

        assert_eq!(observed_outputs[0].call_id, "call_123");
        assert_eq!(observed_outputs[0].status, "unknown_tool");
        assert!(!observed_outputs[0].success);
        assert_eq!(observed_outputs[1].call_id, "call_456");

        let store = AgentStore::open(paths).unwrap();
        let detail = store.thread_detail(&thread.id).unwrap();
        let lifecycle_items = detail
            .items
            .iter()
            .filter(|item| {
                matches!(
                    item.item_type.as_str(),
                    "tool_call_started" | "tool_call_failed"
                )
            })
            .map(|item| {
                (
                    item.item_type.as_str(),
                    item.payload["toolCallId"].as_str().unwrap(),
                    item.payload["providerItemId"].as_str().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            lifecycle_items,
            vec![
                ("tool_call_started", "call_123", "fc_call_123"),
                ("tool_call_failed", "call_123", "fc_call_123"),
                ("tool_call_started", "call_456", "fc_call_456"),
                ("tool_call_failed", "call_456", "fc_call_456")
            ]
        );

        let failed_item = detail
            .items
            .iter()
            .find(|item| {
                item.payload["toolCallId"] == "call_123" && item.item_type == "tool_call_failed"
            })
            .unwrap();
        assert_eq!(failed_item.payload["output"]["call_id"], "call_123");
        assert_eq!(failed_item.payload["output"]["status"], "unknown_tool");
        let result_artifact_id = failed_item.payload["resultArtifactId"].as_str().unwrap();
        let result_bytes = store.read_artifact_bytes(result_artifact_id).unwrap();
        let result_json = serde_json::from_slice::<serde_json::Value>(&result_bytes).unwrap();
        assert_eq!(result_json["call_id"], "call_123");
        assert_eq!(result_json["status"], "unknown_tool");

        let lifecycle_events = detail
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind.as_str(),
                    "tool_call_started" | "tool_call_failed"
                )
            })
            .map(|event| {
                (
                    event.kind.as_str(),
                    event.payload["toolCallId"].as_str().unwrap(),
                    event.payload["callIndex"].as_i64().unwrap(),
                    event.payload["providerItemId"].as_str().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            lifecycle_events,
            vec![
                ("tool_call_started", "call_123", 0, "fc_call_123"),
                ("tool_call_failed", "call_123", 0, "fc_call_123"),
                ("tool_call_started", "call_456", 1, "fc_call_456"),
                ("tool_call_failed", "call_456", 1, "fc_call_456")
            ]
        );
        assert!(detail.events.iter().any(|event| {
            event.kind == "tool_call_failed"
                && event.payload["toolCallId"] == "call_123"
                && event.payload["resultArtifactId"] == result_artifact_id
        }));
    }

    #[tokio::test]
    async fn parallel_thread_tool_calls_run_fake_executor_concurrently_and_commit_in_order() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let store = AgentStore::open(paths.clone()).unwrap();
        let thread = store
            .create_thread("parallel checks", "parallel", json!({}))
            .unwrap();
        let turn = store
            .create_turn(&thread.id, "check two things", json!({}))
            .unwrap();
        drop(store);

        let calls = ["call_a", "call_b"]
            .into_iter()
            .map(|call_id| AgentToolCall {
                call_id: call_id.to_string(),
                namespace: Some("issue-finder".to_string()),
                name: "status".to_string(),
                arguments: json!({"checkAuth": false}),
                provider_item_id: Some(format!("fc_{call_id}")),
                provider_call_id_source: Some("fake_responses".to_string()),
            })
            .collect::<Vec<_>>();
        let barrier = Arc::new(Barrier::new(calls.len()));
        let turn_id_for_output = turn.id.clone();

        let results = timeout(
            Duration::from_secs(2),
            execute_thread_tool_calls_parallel_with(&paths, &thread.id, &turn.id, 0, calls, {
                let barrier = barrier.clone();
                move |call| {
                    let barrier = barrier.clone();
                    let turn_id = turn_id_for_output.clone();
                    async move {
                        barrier.wait().await;
                        let tool_name = call.canonical_name();
                        IssueFinderToolOutput {
                            call_id: call.call_id,
                            turn_id: Some(turn_id),
                            tool_name,
                            success: true,
                            status: "ok".to_string(),
                            content_items: vec![IssueFinderContentItem::InputText {
                                text: "fake parallel output".to_string(),
                            }],
                            structured_content: json!({
                                "kind": "issue_finder_tool_output",
                                "tool": "issue-finder.status",
                                "status": "ok",
                                "success": true
                            }),
                        }
                    }
                }
            }),
        )
        .await
        .expect("parallel fake executor should not deadlock")
        .unwrap();

        assert_eq!(
            results
                .iter()
                .map(|(call, output)| (call.call_id.as_str(), output.call_id.as_str()))
                .collect::<Vec<_>>(),
            vec![("call_a", "call_a"), ("call_b", "call_b")]
        );

        let detail = AgentStore::open(paths)
            .unwrap()
            .thread_detail(&thread.id)
            .unwrap();
        let lifecycle = detail
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind.as_str(),
                    "tool_call_started" | "tool_call_completed"
                )
            })
            .map(|event| {
                (
                    event.kind.as_str(),
                    event.payload["toolCallId"].as_str().unwrap(),
                    event.payload["callIndex"].as_i64().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            lifecycle,
            vec![
                ("tool_call_started", "call_a", 0),
                ("tool_call_started", "call_b", 1),
                ("tool_call_completed", "call_a", 0),
                ("tool_call_completed", "call_b", 1)
            ]
        );
    }

    #[tokio::test]
    async fn approval_required_thread_tool_call_creates_request_without_execution() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let store = AgentStore::open(paths.clone()).unwrap();
        let thread = store
            .create_thread("search issues", "search", json!({}))
            .unwrap();
        let turn = store
            .create_turn(&thread.id, "prepare candidate", json!({}))
            .unwrap();
        drop(store);

        let registry = AgentToolRegistry::issue_finder_default();
        let config = Config::default();
        let call = AgentToolCall {
            call_id: "call_prepare".to_string(),
            namespace: Some("issue-finder".to_string()),
            name: "prepare".to_string(),
            arguments: json!({"issue": "owner/repo#1"}),
            provider_item_id: Some("fc_prepare".to_string()),
            provider_call_id_source: Some("provider_responses".to_string()),
        };

        let output = execute_thread_tool_call(
            &paths,
            &config,
            &registry,
            ThreadToolExecution {
                thread_id: &thread.id,
                turn_id: &turn.id,
                turn_index: 0,
                call_index: 0,
            },
            &call,
        )
        .await
        .unwrap();

        assert_eq!(output.status, "pending_approval");
        let detail = AgentStore::open(paths)
            .unwrap()
            .thread_detail(&thread.id)
            .unwrap();
        assert_eq!(detail.approval_requests.len(), 1);
        assert_eq!(
            detail.approval_requests[0].id,
            format!("agent-approval-{}-call_prepare", turn.id)
        );
        assert_eq!(detail.approval_requests[0].status, "pending");
        assert!(detail.events.iter().any(|event| {
            event.kind == "approval_required"
                && event.payload["approvalRequestId"] == detail.approval_requests[0].id
        }));
    }

    #[test]
    fn runtime_observes_cancelled_turn_before_next_step() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let store = AgentStore::open(paths.clone()).unwrap();
        let thread = store
            .create_thread("search issues", "search", json!({}))
            .unwrap();
        let turn = store
            .create_turn(&thread.id, "find candidates", json!({}))
            .unwrap();
        store
            .update_turn_status(
                &turn.id,
                crate::agent::model::AgentTaskStatus::Cancelled,
                None,
                None,
            )
            .unwrap();
        drop(store);

        assert!(turn_is_cancelled(&paths, &turn.id).unwrap());
    }

    #[test]
    fn thread_assessment_gate_detects_prior_successful_assess_call() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let store = AgentStore::open(paths.clone()).unwrap();
        let thread = store
            .create_thread("search issues", "search", json!({}))
            .unwrap();
        let turn = store
            .create_turn(&thread.id, "find candidates", json!({}))
            .unwrap();
        store
            .add_thread_item(crate::agent::store::NewAgentThreadItem {
                thread_id: &thread.id,
                turn_id: Some(&turn.id),
                item_type: "tool_call_completed",
                role: Some("user"),
                content: Some("Observation from issue-finder.assess"),
                tool_name: Some("issue-finder.assess"),
                payload: json!({"success": true}),
            })
            .unwrap();

        assert!(thread_has_assessment(&store, &thread.id).unwrap());
    }

    fn test_paths(home: &Path) -> IssueFinderPaths {
        IssueFinderPaths {
            home: home.to_path_buf(),
            config: home.join("config.toml"),
            cache_dir: home.join("cache"),
            workspaces_dir: home.join("workspaces"),
            inbox_dir: home.join("inbox"),
            reports_dir: home.join("reports"),
        }
    }
}
