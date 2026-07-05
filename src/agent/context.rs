use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use serde_json::json;

use super::llm_client::AgentModelItem;
use super::model::{AgentThreadItem, AgentThreadMailboxItem, AgentThreadTurnRequest};
use super::store::{AgentStore, NewAgentThreadItem};
use super::tool_registry::{AgentToolExposure, AgentToolRegistry};
use crate::tool_specs::{TOOL_PREPARE, TOOL_READ_CONTEXT, TOOL_SCOUT};

const MAX_CONTEXT_ITEMS: usize = 12;
const MAX_CONTEXT_CHARS: usize = 24_000;
const MAX_ITEM_CHARS: usize = 4_000;
const RECENT_ITEMS_AFTER_COMPACTION: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentContextBudget {
    pub max_items: usize,
    pub max_chars: usize,
    pub max_item_chars: usize,
    pub recent_items_after_compaction: usize,
}

impl Default for AgentContextBudget {
    fn default() -> Self {
        Self {
            max_items: MAX_CONTEXT_ITEMS,
            max_chars: MAX_CONTEXT_CHARS,
            max_item_chars: MAX_ITEM_CHARS,
            recent_items_after_compaction: RECENT_ITEMS_AFTER_COMPACTION,
        }
    }
}

impl AgentContextBudget {
    pub fn from_config(config: &crate::config::AgentConfig) -> Self {
        Self {
            max_items: config.context_max_items.max(1),
            max_chars: config.context_max_chars.max(1_000),
            max_item_chars: config.context_max_item_chars.max(200),
            recent_items_after_compaction: config.context_recent_items_after_compaction.max(1),
        }
    }
}

pub fn build_thread_context(
    store: &AgentStore,
    thread_id: &str,
    current_turn_id: &str,
    input_text: &str,
    input: &AgentThreadTurnRequest,
) -> Result<Vec<AgentModelItem>> {
    build_thread_context_with_budget(
        store,
        thread_id,
        current_turn_id,
        input_text,
        input,
        AgentContextBudget::default(),
    )
}

pub fn build_thread_context_with_budget(
    store: &AgentStore,
    thread_id: &str,
    current_turn_id: &str,
    input_text: &str,
    input: &AgentThreadTurnRequest,
    budget: AgentContextBudget,
) -> Result<Vec<AgentModelItem>> {
    let thread = store.get_thread(thread_id)?;
    let items = store
        .list_thread_items(thread_id)?
        .into_iter()
        .filter(|item| item.turn_id.as_deref() != Some(current_turn_id))
        .collect::<Vec<_>>();
    let recovery_items = unresolved_tool_lifecycle_messages(&items);
    let mut context_items = items
        .into_iter()
        .filter(|item| item.item_type != "context_compaction")
        .filter_map(|item| thread_item_message(item, budget.max_item_chars))
        .collect::<Vec<_>>();

    let context_chars = context_items.iter().map(model_item_chars).sum::<usize>();
    if context_items.len() > budget.max_items || context_chars > budget.max_chars {
        context_items = compact_thread_context(store, thread_id, context_items, budget)?;
    }

    let mut model_input = vec![AgentModelItem::UserMessage {
        content: format!("Thread goal: {}", thread.goal),
    }];
    if let Some(discovery) = deferred_tool_discovery_message() {
        model_input.push(discovery);
    }
    model_input.extend(recovery_items);
    model_input.extend(context_items);
    model_input.push(AgentModelItem::UserMessage {
        content: thread_user_prompt(input_text, input),
    });
    Ok(model_input)
}

fn deferred_tool_discovery_message() -> Option<AgentModelItem> {
    let registry = AgentToolRegistry::issue_finder_default();
    let mut summaries = [TOOL_SCOUT, TOOL_PREPARE, TOOL_READ_CONTEXT]
        .into_iter()
        .filter_map(|tool| registry.tool_summary(tool))
        .collect::<Vec<_>>();
    summaries.extend(registry.tool_summaries_for_exposures(&[AgentToolExposure::Deferred], 8));
    summaries
        .extend(registry.tool_summaries_for_exposures(&[AgentToolExposure::ApprovalRequired], 6));
    let mut seen = BTreeSet::new();
    summaries.retain(|summary| seen.insert(summary.clone()));
    if summaries.is_empty() {
        return None;
    }

    Some(AgentModelItem::UserMessage {
        content: format!(
            "Deferred tool discovery: these Issue Finder tools exist but are not directly callable in the current model tool set. Use direct tools first; if one is needed, explain the need or wait for the host/user to expose or approve it.\n{}",
            summaries.join("\n")
        ),
    })
}

pub fn compact_thread_context_now(
    store: &AgentStore,
    thread_id: &str,
    budget: AgentContextBudget,
) -> Result<Option<String>> {
    let items = store.list_thread_items(thread_id)?;
    let mut context_items = items
        .into_iter()
        .filter(|item| item.item_type != "context_compaction")
        .filter_map(|item| thread_item_message(item, budget.max_item_chars))
        .collect::<Vec<_>>();
    let recent_start = context_items
        .len()
        .saturating_sub(budget.recent_items_after_compaction);
    let older = context_items.drain(..recent_start).collect::<Vec<_>>();
    if older.is_empty() {
        return Ok(None);
    }
    add_context_compaction_item(store, thread_id, &older, context_items.len()).map(Some)
}

pub fn consume_pending_thread_mailbox(
    store: &AgentStore,
    thread_id: &str,
    turn_id: &str,
) -> Result<Vec<AgentModelItem>> {
    let pending = store.pending_mailbox_items_for_turn(thread_id, turn_id)?;
    let mut messages = Vec::new();
    for item in pending {
        match item.delivery.as_str() {
            "steer" | "inject_only" | "new_turn" => {
                let consumed = store.consume_mailbox_item(&item.id)?;
                let content = mailbox_prompt(&consumed);
                store.add_thread_item(NewAgentThreadItem {
                    thread_id,
                    turn_id: Some(turn_id),
                    item_type: "mailbox_item_consumed",
                    role: Some("user"),
                    content: Some(&content),
                    tool_name: None,
                    payload: json!({
                        "mailboxItemId": consumed.id,
                        "delivery": consumed.delivery,
                        "input": consumed.input,
                        "status": consumed.status
                    }),
                })?;
                store.add_thread_event(
                    thread_id,
                    Some(turn_id),
                    "mailbox_item_consumed",
                    "Agent runtime consumed pending thread input.",
                    json!({
                        "mailboxItemId": consumed.id,
                        "delivery": consumed.delivery
                    }),
                )?;
                messages.push(AgentModelItem::UserMessage { content });
            }
            "interrupt" => {
                store.reject_mailbox_item(
                    &item.id,
                    "interrupt mailbox items are handled through turn cancellation",
                )?;
            }
            _ => {
                store.reject_mailbox_item(&item.id, "unknown mailbox delivery")?;
            }
        }
    }
    Ok(messages)
}

fn compact_thread_context(
    store: &AgentStore,
    thread_id: &str,
    mut context_items: Vec<AgentModelItem>,
    budget: AgentContextBudget,
) -> Result<Vec<AgentModelItem>> {
    let recent_start = context_items
        .len()
        .saturating_sub(budget.recent_items_after_compaction);
    let older = context_items.drain(..recent_start).collect::<Vec<_>>();
    if older.is_empty() {
        return Ok(context_items);
    }

    let summary = add_context_compaction_item(store, thread_id, &older, context_items.len())?;
    let mut output = vec![AgentModelItem::AssistantMessage { content: summary }];
    output.extend(context_items);
    Ok(output)
}

fn add_context_compaction_item(
    store: &AgentStore,
    thread_id: &str,
    older: &[AgentModelItem],
    retained_recent_item_count: usize,
) -> Result<String> {
    let summary = deterministic_summary(older);
    store.add_thread_item(NewAgentThreadItem {
        thread_id,
        turn_id: None,
        item_type: "context_compaction",
        role: Some("assistant"),
        content: Some(&summary),
        tool_name: None,
        payload: json!({
            "messageType": "context_compaction",
            "strategy": "deterministic_summary",
            "coveredItemCount": older.len(),
            "retainedRecentItemCount": retained_recent_item_count
        }),
    })?;
    Ok(summary)
}

fn deterministic_summary(items: &[AgentModelItem]) -> String {
    let mut lines = vec![format!(
        "Context summary: {} older thread items were compacted. The original items remain in the local thread store.",
        items.len()
    )];
    for (index, item) in items.iter().take(6).enumerate() {
        lines.push(format!("{}: {}", index + 1, compact_item_line(item)));
    }
    if items.len() > 6 {
        lines.push(format!("...{} more compacted items.", items.len() - 6));
    }
    lines.join("\n")
}

fn compact_item_line(item: &AgentModelItem) -> String {
    let line = match item {
        AgentModelItem::SystemMessage { content }
        | AgentModelItem::UserMessage { content }
        | AgentModelItem::AssistantMessage { content }
        | AgentModelItem::ReasoningSummary { content } => content.as_str(),
        AgentModelItem::ToolCall(call) => return format!("tool_call {}", call.canonical_name()),
        AgentModelItem::FunctionCallOutput { call_id, .. } => {
            return format!("function_call_output {call_id}");
        }
    };
    truncate_chars(&line.split_whitespace().collect::<Vec<_>>().join(" "), 220)
}

fn thread_item_message(item: AgentThreadItem, max_item_chars: usize) -> Option<AgentModelItem> {
    match item.item_type.as_str() {
        "user_message"
        | "assistant_model_response"
        | "tool_call_completed"
        | "tool_call_failed"
        | "final_answer"
        | "mailbox_item_consumed" => {
            let role = item.role?;
            let content = item.content?;
            let content = truncate_chars(&content, max_item_chars);
            match role.as_str() {
                "assistant" => Some(AgentModelItem::AssistantMessage { content }),
                "user" => Some(AgentModelItem::UserMessage { content }),
                _ => None,
            }
        }
        _ => None,
    }
}

fn unresolved_tool_lifecycle_messages(items: &[AgentThreadItem]) -> Vec<AgentModelItem> {
    let mut started = BTreeMap::<String, String>::new();
    let mut finished = BTreeMap::<String, String>::new();

    for item in items {
        let Some(call_id) = item
            .payload
            .get("toolCallId")
            .and_then(|value| value.as_str())
        else {
            continue;
        };
        match item.item_type.as_str() {
            "tool_call_started" => {
                started.insert(call_id.to_string(), tool_lifecycle_label(item));
            }
            "tool_call_completed" | "tool_call_failed" => {
                finished.insert(
                    call_id.to_string(),
                    format!("{} {}", item.item_type, tool_lifecycle_label(item)),
                );
            }
            _ => {}
        }
    }

    let finished_ids = finished.keys().cloned().collect::<BTreeSet<_>>();
    let started_ids = started.keys().cloned().collect::<BTreeSet<_>>();
    let missing_results = started_ids
        .difference(&finished_ids)
        .take(6)
        .filter_map(|call_id| {
            started
                .get(call_id)
                .map(|tool| format!("{call_id} ({tool})"))
        })
        .collect::<Vec<_>>();
    let missing_starts = finished_ids
        .difference(&started_ids)
        .take(6)
        .filter_map(|call_id| {
            finished
                .get(call_id)
                .map(|status| format!("{call_id} ({status})"))
        })
        .collect::<Vec<_>>();

    if missing_results.is_empty() && missing_starts.is_empty() {
        return Vec::new();
    }

    let mut lines = vec![
        "Tool lifecycle recovery: previous thread history contains incomplete tool-call bookkeeping."
            .to_string(),
    ];
    if !missing_results.is_empty() {
        lines.push(format!(
            "Started calls without observed results: {}.",
            missing_results.join(", ")
        ));
    }
    if !missing_starts.is_empty() {
        lines.push(format!(
            "Tool results without matching started calls: {}.",
            missing_starts.join(", ")
        ));
    }
    lines.push(
        "Treat these as recovery hints only; continue from explicit observed tool outputs when present."
            .to_string(),
    );

    vec![AgentModelItem::UserMessage {
        content: lines.join("\n"),
    }]
}

fn tool_lifecycle_label(item: &AgentThreadItem) -> String {
    item.tool_name
        .clone()
        .or_else(|| {
            item.payload
                .get("toolName")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "unknown tool".to_string())
}

fn thread_user_prompt(input_text: &str, input: &AgentThreadTurnRequest) -> String {
    let repo = input.repo.as_deref().unwrap_or("null");
    format!(
        "Thread turn input: {input_text}\nDefault discover_candidates arguments: limit={}, repo={}, refresh={}, laneLimit=4.\nChoose the next action.",
        input.normalized_limit(),
        repo,
        input.refresh
    )
}

fn mailbox_prompt(item: &AgentThreadMailboxItem) -> String {
    format!(
        "Additional thread input received while this turn was active (delivery={}): {}\nTreat this as the user's latest steering/context before choosing the next action.",
        item.delivery,
        item.input
    )
}

fn model_item_chars(item: &AgentModelItem) -> usize {
    match item {
        AgentModelItem::SystemMessage { content }
        | AgentModelItem::UserMessage { content }
        | AgentModelItem::AssistantMessage { content }
        | AgentModelItem::ReasoningSummary { content } => content.chars().count(),
        AgentModelItem::ToolCall(call) => call.canonical_name().chars().count(),
        AgentModelItem::FunctionCallOutput { output, .. } => output.to_string().chars().count(),
    }
}

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut truncated = value.chars().take(limit).collect::<String>();
    truncated.push_str("\n...[truncated]");
    truncated
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;
    use tempfile::tempdir;

    use super::{build_thread_context, consume_pending_thread_mailbox};
    use crate::agent::llm_client::AgentModelItem;
    use crate::agent::model::AgentThreadTurnRequest;
    use crate::agent::store::{AgentStore, NewAgentThreadItem};
    use crate::paths::IssueFinderPaths;

    #[test]
    fn context_builder_compacts_large_history_and_keeps_current_input() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let store = AgentStore::open(paths).unwrap();
        let thread = store.create_thread("goal", "goal", json!({})).unwrap();
        let turn = store.create_turn(&thread.id, "latest", json!({})).unwrap();
        for index in 0..20 {
            let content = format!("older message {index} {}", "x".repeat(1200));
            store
                .add_thread_item(NewAgentThreadItem {
                    thread_id: &thread.id,
                    turn_id: None,
                    item_type: "assistant_model_response",
                    role: Some("assistant"),
                    content: Some(&content),
                    tool_name: None,
                    payload: json!({}),
                })
                .unwrap();
        }

        let input = AgentThreadTurnRequest {
            input: "latest".to_string(),
            repo: None,
            limit: Some(3),
            refresh: false,
            max_turns: Some(2),
            run_immediately: true,
        };
        let context = build_thread_context(&store, &thread.id, &turn.id, "latest", &input).unwrap();

        assert!(matches!(
            context.first(),
            Some(AgentModelItem::UserMessage { content }) if content.contains("Thread goal: goal")
        ));
        assert!(context.iter().any(|item| matches!(
            item,
            AgentModelItem::UserMessage { content }
                if content.contains("Deferred tool discovery")
                    && content.contains("issue-finder.scout (deferred)")
                    && content.contains("issue-finder.prepare (approval_required)")
        )));
        assert!(context.iter().any(|item| matches!(
            item,
            AgentModelItem::AssistantMessage { content } if content.contains("Context summary")
        )));
        assert!(matches!(
            context.last(),
            Some(AgentModelItem::UserMessage { content }) if content.contains("Thread turn input: latest")
        ));
        let detail = store.thread_detail(&thread.id).unwrap();
        assert!(detail
            .items
            .iter()
            .any(|item| item.item_type == "context_compaction"));
    }

    #[test]
    fn mailbox_consumption_returns_user_message_and_marks_consumed() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let store = AgentStore::open(paths).unwrap();
        let thread = store.create_thread("goal", "goal", json!({})).unwrap();
        let turn = store.create_turn(&thread.id, "latest", json!({})).unwrap();
        let mailbox = store
            .enqueue_mailbox_item(
                &thread.id,
                Some(&turn.id),
                "steer",
                "look at another candidate",
                json!({}),
            )
            .unwrap();

        let messages = consume_pending_thread_mailbox(&store, &thread.id, &turn.id).unwrap();

        assert_eq!(messages.len(), 1);
        assert!(matches!(
            &messages[0],
            AgentModelItem::UserMessage { content } if content.contains("look at another candidate")
        ));
        let detail = store.thread_detail(&thread.id).unwrap();
        let consumed = detail
            .mailbox_items
            .iter()
            .find(|item| item.id == mailbox.id)
            .unwrap();
        assert_eq!(consumed.status, "consumed");
    }

    #[test]
    fn context_builder_surfaces_unresolved_tool_lifecycle_recovery() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        let store = AgentStore::open(paths).unwrap();
        let thread = store.create_thread("goal", "goal", json!({})).unwrap();
        let old_turn = store.create_turn(&thread.id, "old", json!({})).unwrap();
        let current_turn = store.create_turn(&thread.id, "latest", json!({})).unwrap();
        store
            .add_thread_item(NewAgentThreadItem {
                thread_id: &thread.id,
                turn_id: Some(&old_turn.id),
                item_type: "tool_call_started",
                role: None,
                content: None,
                tool_name: Some("issue-finder.inspect_candidate"),
                payload: json!({
                    "toolCallId": "call_missing_result",
                    "toolName": "issue-finder.inspect_candidate"
                }),
            })
            .unwrap();
        store
            .add_thread_item(NewAgentThreadItem {
                thread_id: &thread.id,
                turn_id: Some(&old_turn.id),
                item_type: "tool_call_completed",
                role: Some("user"),
                content: Some("Observation from issue-finder.assess"),
                tool_name: Some("issue-finder.assess"),
                payload: json!({
                    "toolCallId": "call_missing_start",
                    "toolName": "issue-finder.assess",
                    "success": true
                }),
            })
            .unwrap();
        store
            .add_thread_item(NewAgentThreadItem {
                thread_id: &thread.id,
                turn_id: Some(&current_turn.id),
                item_type: "tool_call_started",
                role: None,
                content: None,
                tool_name: Some("issue-finder.status"),
                payload: json!({
                    "toolCallId": "current_turn_call",
                    "toolName": "issue-finder.status"
                }),
            })
            .unwrap();

        let input = AgentThreadTurnRequest {
            input: "latest".to_string(),
            repo: None,
            limit: Some(3),
            refresh: false,
            max_turns: Some(2),
            run_immediately: true,
        };
        let context =
            build_thread_context(&store, &thread.id, &current_turn.id, "latest", &input).unwrap();
        let recovery = context
            .iter()
            .find_map(|item| match item {
                AgentModelItem::UserMessage { content }
                    if content.contains("Tool lifecycle recovery") =>
                {
                    Some(content.as_str())
                }
                _ => None,
            })
            .expect("unresolved lifecycle recovery message");

        assert!(recovery.contains("call_missing_result"));
        assert!(recovery.contains("call_missing_start"));
        assert!(!recovery.contains("current_turn_call"));
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
