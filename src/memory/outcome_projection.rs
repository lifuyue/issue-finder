use std::collections::BTreeSet;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::dispatch::{
    DispatchRun, DispatchRunOutcome, DispatchStore, DispatchTaskClass, IssueTask,
};
use crate::memory::ingest::{DispatchMemoryOutcome, MemoryIngestor};
use crate::memory::model::{
    MemoryHintScopeType, MemoryHintType, MemoryRawEvent, MemoryRawEventType,
};
use crate::memory::store::MemoryStore;
use crate::paths::IssueFinderPaths;
use crate::recommendation::IssueKey;

const MAX_RANKING_BOOST: i32 = 60;
const MAX_RANKING_PENALTY: i32 = -80;
const UNKNOWN_TASK: &str = "unknown_task";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomePriorKind {
    IssueQuality,
    ExecutionFriction,
    AgentSuitability,
}

impl OutcomePriorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IssueQuality => "issue_quality",
            Self::ExecutionFriction => "execution_friction",
            Self::AgentSuitability => "agent_suitability",
        }
    }

    pub fn policy_kind(self) -> &'static str {
        match self {
            Self::IssueQuality => "issue_quality_prior",
            Self::ExecutionFriction => "execution_friction_prior",
            Self::AgentSuitability => "agent_suitability_prior",
        }
    }

    pub fn hint_type(self) -> MemoryHintType {
        match self {
            Self::IssueQuality | Self::ExecutionFriction => MemoryHintType::Ranking,
            Self::AgentSuitability => MemoryHintType::Dispatch,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutcomeFeedbackInput {
    pub id: String,
    #[serde(default)]
    pub approved: bool,
    pub issue_key: IssueKey,
    pub repo_scope: Option<String>,
    pub agent_id: Option<String>,
    pub outcome_kind: String,
    pub failure_class: Option<String>,
    pub task_class: Option<String>,
    pub validation_outcome: Option<String>,
}

impl OutcomeFeedbackInput {
    pub fn task_class_or_unknown(&self) -> &str {
        self.task_class.as_deref().unwrap_or(UNKNOWN_TASK)
    }

    fn is_controlled_non_signal(&self) -> bool {
        matches!(self.outcome_kind.as_str(), "needs_user" | "canceled")
            || matches!(
                self.failure_class.as_deref(),
                Some("policy_blocked" | "user_canceled")
            )
    }

    pub fn is_runtime_failure(&self) -> bool {
        matches!(
            self.failure_class.as_deref(),
            Some(
                "dependency_unavailable"
                    | "workspace_unavailable"
                    | "external_service_error"
                    | "agent_runtime_error"
            )
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutcomeProjection {
    pub prior_kind: OutcomePriorKind,
    pub hint_type: MemoryHintType,
    pub scope_type: MemoryHintScopeType,
    pub scope_ref: String,
    pub summary: String,
    pub policy_json: Value,
    pub weight: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutcomeFeedbackSync {
    pub projected_outcomes: usize,
    pub raw_event_ids: Vec<String>,
    pub node_ids: Vec<String>,
}

pub fn project_outcome(input: &OutcomeFeedbackInput) -> Vec<OutcomeProjection> {
    if input.is_controlled_non_signal() {
        return Vec::new();
    }

    let mut projections = Vec::new();
    if let Some(weight) = issue_quality_weight(input) {
        projections.extend(ranking_projections(
            input,
            OutcomePriorKind::IssueQuality,
            weight,
        ));
    }
    if let Some(weight) = execution_friction_weight(input) {
        projections.extend(ranking_projections(
            input,
            OutcomePriorKind::ExecutionFriction,
            weight,
        ));
    }
    if let Some(weight) = agent_suitability_weight(input) {
        if let Some(agent_id) = input.agent_id.as_deref() {
            projections.push(projection(
                input,
                OutcomePriorKind::AgentSuitability,
                MemoryHintScopeType::Agent,
                agent_id.to_string(),
                weight,
            ));
        }
    }
    projections
}

pub fn project_raw_dispatch_outcome(event: &MemoryRawEvent) -> Option<Vec<OutcomeProjection>> {
    let input = outcome_feedback_input_from_raw_event(event)?;
    Some(project_outcome(&input))
}

pub fn ranking_adjustment_for_candidate(
    outcomes: &[OutcomeFeedbackInput],
    repo_full_name: &str,
    task_class: &str,
) -> i32 {
    let raw = outcomes
        .iter()
        .filter(|outcome| outcome.approved)
        .flat_map(project_outcome)
        .filter(|projection| projection.hint_type == MemoryHintType::Ranking)
        .filter(|projection| projection_matches_candidate(projection, repo_full_name, task_class))
        .map(|projection| (projection.weight * 100.0).round() as i32)
        .sum::<i32>();
    raw.clamp(MAX_RANKING_PENALTY, MAX_RANKING_BOOST)
}

pub fn sync_dispatch_outcome_feedback(paths: &IssueFinderPaths) -> Result<OutcomeFeedbackSync> {
    let dispatch_store = DispatchStore::open(paths.clone())?;
    let memory_store = MemoryStore::open(paths)?;
    let ingestor = MemoryIngestor::new(&memory_store);
    let mut raw_event_ids = BTreeSet::new();
    let mut node_ids = BTreeSet::new();
    let mut projected_outcomes = 0;

    for outcome in dispatch_store.list_dispatch_run_outcomes()? {
        let run = dispatch_store.get_dispatch_run(&outcome.run_id)?;
        let issue_task = dispatch_store.get_issue_task(&run.issue_task_id)?;
        let memory_outcome = dispatch_memory_outcome(&issue_task, &run, &outcome);
        let ingest = ingestor
            .ingest_dispatch_outcome(&memory_outcome)
            .with_context(|| format!("projecting dispatch outcome {}", outcome.id))?;
        raw_event_ids.extend(ingest.raw_event_ids);
        node_ids.extend(ingest.node_ids);
        projected_outcomes += 1;
    }

    Ok(OutcomeFeedbackSync {
        projected_outcomes,
        raw_event_ids: raw_event_ids.into_iter().collect(),
        node_ids: node_ids.into_iter().collect(),
    })
}

pub fn dispatch_memory_outcome(
    issue_task: &IssueTask,
    run: &DispatchRun,
    outcome: &DispatchRunOutcome,
) -> DispatchMemoryOutcome {
    DispatchMemoryOutcome {
        id: outcome.id.clone(),
        issue_key: IssueKey::new(issue_task.repo_full_name.clone(), issue_task.issue_number),
        agent_id: run.agent_id.clone(),
        outcome_kind: Some(outcome.outcome_kind.as_str().to_string()),
        task_type: outcome
            .task_class
            .map(|task_class| task_class.as_str().to_string())
            .unwrap_or_else(|| DispatchTaskClass::UnknownTask.as_str().to_string()),
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
    }
}

pub fn outcome_feedback_input_from_raw_event(
    event: &MemoryRawEvent,
) -> Option<OutcomeFeedbackInput> {
    let succeeded = match event.event_type {
        MemoryRawEventType::DispatchSuccess | MemoryRawEventType::ValidationPass => true,
        MemoryRawEventType::DispatchFailure | MemoryRawEventType::ValidationFail => false,
        _ => return None,
    };
    let issue_label =
        json_string(&event.payload_json, "issue").unwrap_or_else(|| event.subject_ref.clone());
    let issue_key = parse_issue_key(&issue_label)?;
    let outcome_kind = json_string(&event.payload_json, "outcomeKind").unwrap_or_else(|| {
        if succeeded {
            "fix_ready".to_string()
        } else {
            "failed".to_string()
        }
    });

    Some(OutcomeFeedbackInput {
        id: json_string(&event.payload_json, "outcomeId").unwrap_or_else(|| event.id.clone()),
        approved: false,
        repo_scope: Some(issue_key.repo_full_name.clone()),
        issue_key,
        agent_id: json_string(&event.payload_json, "agentId"),
        outcome_kind,
        failure_class: json_string(&event.payload_json, "failureClass"),
        task_class: json_string(&event.payload_json, "taskType")
            .filter(|task| !task.trim().is_empty()),
        validation_outcome: json_string(&event.payload_json, "validationOutcome"),
    })
}

fn issue_quality_weight(input: &OutcomeFeedbackInput) -> Option<f64> {
    match input.outcome_kind.as_str() {
        "fix_ready" => Some(0.35),
        "completed_no_change" => Some(0.20),
        "failed" | "blocked" => match input.failure_class.as_deref() {
            Some("reproduction_failed") => Some(-0.35),
            Some("context_insufficient") => Some(-0.25),
            _ => None,
        },
        _ => None,
    }
}

fn execution_friction_weight(input: &OutcomeFeedbackInput) -> Option<f64> {
    match input.failure_class.as_deref() {
        Some("validation_failed") => Some(-0.45),
        Some("dependency_unavailable" | "workspace_unavailable") => Some(-0.25),
        Some("external_service_error") => Some(-0.20),
        _ => None,
    }
}

fn agent_suitability_weight(input: &OutcomeFeedbackInput) -> Option<f64> {
    match input.outcome_kind.as_str() {
        "fix_ready" => Some(0.25),
        "completed_no_change" => Some(0.10),
        "failed" | "blocked" => match input.failure_class.as_deref() {
            Some("validation_failed") => Some(-0.25),
            Some("agent_runtime_error") => Some(-0.35),
            _ => None,
        },
        _ => None,
    }
}

fn ranking_projections(
    input: &OutcomeFeedbackInput,
    prior_kind: OutcomePriorKind,
    weight: f64,
) -> Vec<OutcomeProjection> {
    let scopes = ranking_scopes(input);
    if scopes.is_empty() {
        return Vec::new();
    }
    let scoped_weight = weight / scopes.len() as f64;
    scopes
        .into_iter()
        .map(|(scope_type, scope_ref)| {
            projection(input, prior_kind, scope_type, scope_ref, scoped_weight)
        })
        .collect()
}

fn ranking_scopes(input: &OutcomeFeedbackInput) -> Vec<(MemoryHintScopeType, String)> {
    let mut scopes = Vec::new();
    if let Some(repo) = input.repo_scope.as_deref() {
        scopes.push((MemoryHintScopeType::Repo, repo.to_string()));
    }
    if let Some(task_class) = input.task_class.as_deref() {
        if task_class != UNKNOWN_TASK {
            scopes.push((MemoryHintScopeType::IssueType, task_class.to_string()));
        }
    }
    scopes
}

fn projection(
    input: &OutcomeFeedbackInput,
    prior_kind: OutcomePriorKind,
    scope_type: MemoryHintScopeType,
    scope_ref: String,
    weight: f64,
) -> OutcomeProjection {
    let runtime_failure = input.is_runtime_failure();
    OutcomeProjection {
        prior_kind,
        hint_type: prior_kind.hint_type(),
        scope_type,
        scope_ref: scope_ref.clone(),
        summary: projection_summary(prior_kind, &scope_ref, weight),
        policy_json: json!({
            "kind": prior_kind.policy_kind(),
            "priorKind": prior_kind.as_str(),
            "scope": scope_ref,
            "outcomeId": input.id,
            "outcomeKind": input.outcome_kind,
            "failureClass": input.failure_class,
            "taskClass": input.task_class,
            "agentId": input.agent_id,
            "validationOutcome": input.validation_outcome,
            "runtimeFailure": runtime_failure,
            "contributionOutcome": !runtime_failure,
            "projectorVersion": 1
        }),
        weight,
    }
}

fn projection_summary(prior_kind: OutcomePriorKind, scope_ref: &str, weight: f64) -> String {
    let direction = if weight >= 0.0 { "boost" } else { "cooldown" };
    match prior_kind {
        OutcomePriorKind::IssueQuality => {
            format!("Use issue-quality outcome evidence for `{scope_ref}` as a `{direction}` ranking prior.")
        }
        OutcomePriorKind::ExecutionFriction => {
            format!("Use execution-friction outcome evidence for `{scope_ref}` as a `{direction}` ranking prior.")
        }
        OutcomePriorKind::AgentSuitability => {
            format!("Use agent-suitability outcome evidence for `{scope_ref}` as a `{direction}` dispatch prior.")
        }
    }
}

fn projection_matches_candidate(
    projection: &OutcomeProjection,
    repo_full_name: &str,
    task_class: &str,
) -> bool {
    match projection.scope_type {
        MemoryHintScopeType::Repo => projection.scope_ref == repo_full_name,
        MemoryHintScopeType::IssueType => projection.scope_ref == task_class,
        MemoryHintScopeType::Global
        | MemoryHintScopeType::Agent
        | MemoryHintScopeType::Maintainer => false,
    }
}

fn parse_issue_key(value: &str) -> Option<IssueKey> {
    let (repo, number) = value.rsplit_once('#')?;
    let number = number.parse::<u64>().ok()?;
    Some(IssueKey::new(repo.to_string(), number))
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}
