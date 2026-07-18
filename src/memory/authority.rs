use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::memory::model::{MemoryHint, MemoryHintScopeType, MemoryRawEvent, MemoryTrustLevel};
use crate::memory::outcome_projection::outcome_feedback_input_from_raw_event;
use crate::memory::store::MemoryStore;

#[derive(Debug, Clone)]
pub(super) struct HintPrediction {
    agent_id: Option<String>,
    task_type: Option<String>,
    pub(super) predicts_success: bool,
}

impl HintPrediction {
    pub(super) fn matches(&self, agent_id: &str, task_type: &str) -> bool {
        self.agent_id
            .as_deref()
            .is_none_or(|value| value == agent_id)
            && self
                .task_type
                .as_deref()
                .is_none_or(|value| value == task_type)
    }
}

pub(super) fn hint_prediction(hint: &MemoryHint) -> Option<HintPrediction> {
    let outcome = first_json_string(
        &hint.policy_json,
        &[
            "prediction",
            "predicts",
            "outcome",
            "expectedOutcome",
            "recommendation",
        ],
    )?
    .to_ascii_lowercase();
    let predicts_success = match outcome.as_str() {
        "success" | "succeeded" | "succeeds" | "prefer" | "preferred" | "positive" => true,
        "failure" | "failed" | "fails" | "avoid" | "negative" => false,
        _ => return None,
    };
    Some(HintPrediction {
        agent_id: first_json_string(&hint.policy_json, &["agentId", "agent_id"]),
        task_type: first_json_string(&hint.policy_json, &["taskType", "task_type"]),
        predicts_success,
    })
}

pub(super) fn dispatch_outcome_for_conflict(
    event: &MemoryRawEvent,
) -> Option<(String, String, bool)> {
    let input = outcome_feedback_input_from_raw_event(event)?;
    let succeeded = match input.outcome_kind.as_str() {
        "success" | "partial" => true,
        "failed" | "blocked" => false,
        _ => return None,
    };
    let task_class = input.task_class_or_unknown().to_string();
    Some((input.agent_id?, task_class, succeeded))
}

pub(super) fn contradicted_by_newer_user_fact(
    store: &MemoryStore,
    hint: &MemoryHint,
) -> Result<bool> {
    let Some(prediction) = hint_prediction(hint) else {
        return Ok(false);
    };
    for event in store.list_raw_events()?.into_iter().filter(|event| {
        event.trust_level == MemoryTrustLevel::UserExplicit
            && event.tombstoned_at.is_none()
            && is_strictly_newer(&event.occurred_at, &hint.created_at)
    }) {
        let Some((agent_id, task_type, succeeded)) = dispatch_outcome_for_conflict(&event) else {
            continue;
        };
        if prediction.matches(&agent_id, &task_type)
            && prediction.predicts_success != succeeded
            && hint_scope_matches_event(hint, &event, &agent_id, &task_type)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn hint_scope_matches_event(
    hint: &MemoryHint,
    event: &MemoryRawEvent,
    agent_id: &str,
    task_type: &str,
) -> bool {
    match hint.scope_type {
        MemoryHintScopeType::Global => true,
        MemoryHintScopeType::Repo => event
            .subject_ref
            .split_once('#')
            .is_some_and(|(repo, _)| repo == hint.scope_ref),
        MemoryHintScopeType::Agent => hint.scope_ref == agent_id,
        MemoryHintScopeType::IssueType => hint.scope_ref == task_type,
        MemoryHintScopeType::Maintainer => false,
    }
}

fn is_strictly_newer(left: &str, right: &str) -> bool {
    let (Ok(left), Ok(right)) = (
        DateTime::parse_from_rfc3339(left),
        DateTime::parse_from_rfc3339(right),
    ) else {
        return false;
    };
    left.with_timezone(&Utc) > right.with_timezone(&Utc)
}

fn first_json_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(ToString::to_string)
    })
}
