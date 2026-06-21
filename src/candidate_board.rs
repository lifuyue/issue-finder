use std::collections::BTreeMap;
use std::fmt;

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dispatch::{
    ApprovalStatus, ApprovalType, DispatchOutcomeKind, DispatchRun, DispatchRunOutcome,
    DispatchRunStatus, DispatchStore, IssueTask, IssueTaskStatus,
};
use crate::inbox::{self, InboxStatus};
use crate::paths::IssueFinderPaths;
use crate::recommendation::{load_events, IssueKey, RecommendationEvent, RecommendationEventType};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CandidateLifecycleStatus {
    Discovered,
    Ranked,
    Prepared,
    ReviewPending,
    PackageReady,
    DispatchRunning,
    OutcomePositive,
    OutcomeNegative,
    Snoozed,
    ReactivationCandidate,
    Archived,
}

impl CandidateLifecycleStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Ranked => "ranked",
            Self::Prepared => "prepared",
            Self::ReviewPending => "review_pending",
            Self::PackageReady => "package_ready",
            Self::DispatchRunning => "dispatch_running",
            Self::OutcomePositive => "outcome_positive",
            Self::OutcomeNegative => "outcome_negative",
            Self::Snoozed => "snoozed",
            Self::ReactivationCandidate => "reactivation_candidate",
            Self::Archived => "archived",
        }
    }
}

impl fmt::Display for CandidateLifecycleStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CandidateDisplayState {
    Visible,
    HiddenSnoozed,
    HiddenArchived,
}

impl CandidateDisplayState {
    pub fn visible(self) -> bool {
        self == Self::Visible
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CandidateTask {
    pub issue_key: IssueKey,
    pub title: Option<String>,
    pub url: Option<String>,
    pub status: CandidateLifecycleStatus,
    pub display: CandidateDisplayState,
    pub priority: Option<i64>,
    pub category: Option<String>,
    pub inbox_id: Option<String>,
    pub issue_task_id: Option<String>,
    pub latest_run_id: Option<String>,
    pub latest_outcome_kind: Option<String>,
    pub pending_approval_ids: Vec<String>,
    pub last_event_at: Option<String>,
    pub last_issue_updated_at: Option<String>,
    pub last_comments_count: Option<u64>,
    pub reasons: Vec<String>,
}

impl CandidateTask {
    pub fn issue_label(&self) -> String {
        self.issue_key.label()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskBoard {
    pub generated_at: String,
    pub items: Vec<CandidateTask>,
}

impl TaskBoard {
    pub fn by_issue(&self, issue_key: &IssueKey) -> Option<&CandidateTask> {
        self.items.iter().find(|item| &item.issue_key == issue_key)
    }

    pub fn active(&self) -> Vec<&CandidateTask> {
        self.items
            .iter()
            .filter(|item| item.display.visible())
            .filter(|item| {
                !matches!(
                    item.status,
                    CandidateLifecycleStatus::OutcomePositive
                        | CandidateLifecycleStatus::OutcomeNegative
                        | CandidateLifecycleStatus::Snoozed
                        | CandidateLifecycleStatus::Archived
                )
            })
            .collect()
    }

    pub fn ready_for_review(&self) -> Vec<&CandidateTask> {
        self.items
            .iter()
            .filter(|item| item.display.visible())
            .filter(|item| item.status == CandidateLifecycleStatus::ReviewPending)
            .collect()
    }

    pub fn ready_for_dispatch(&self) -> Vec<&CandidateTask> {
        self.items
            .iter()
            .filter(|item| item.display.visible())
            .filter(|item| item.status == CandidateLifecycleStatus::PackageReady)
            .collect()
    }

    pub fn reactivation_candidates(&self) -> Vec<&CandidateTask> {
        self.items
            .iter()
            .filter(|item| item.status == CandidateLifecycleStatus::ReactivationCandidate)
            .collect()
    }
}

pub fn load_task_board(paths: &IssueFinderPaths) -> Result<TaskBoard> {
    derive_task_board_at(paths, Utc::now().to_rfc3339())
}

pub fn derive_task_board_at(
    paths: &IssueFinderPaths,
    generated_at: impl Into<String>,
) -> Result<TaskBoard> {
    let mut builders = BTreeMap::<IssueKey, CandidateFacts>::new();
    apply_recommendation_events(&mut builders, load_events(paths)?);
    apply_inbox(&mut builders, inbox::load_index(paths)?.items);
    if paths.dispatch_db_path().exists() {
        let store = DispatchStore::open(paths.clone())?;
        apply_dispatch(&mut builders, &store)?;
    }

    let mut items = builders
        .into_values()
        .map(CandidateFacts::finish)
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        status_rank(left.status)
            .cmp(&status_rank(right.status))
            .then_with(|| right.priority.cmp(&left.priority))
            .then_with(|| {
                right
                    .last_event_at
                    .cmp(&left.last_event_at)
                    .then_with(|| left.issue_label().cmp(&right.issue_label()))
            })
    });

    Ok(TaskBoard {
        generated_at: generated_at.into(),
        items,
    })
}

#[derive(Debug, Clone)]
struct CandidateFacts {
    issue_key: IssueKey,
    title: Option<String>,
    url: Option<String>,
    priority: Option<i64>,
    category: Option<String>,
    inbox_id: Option<String>,
    inbox_status: Option<InboxStatus>,
    issue_task_id: Option<String>,
    issue_task_status: Option<IssueTaskStatus>,
    has_package: bool,
    ranked: bool,
    prepared: bool,
    recommendation_dismissed: bool,
    recommendation_done: bool,
    reactivated_after_snooze: bool,
    pending_approval_ids: Vec<String>,
    latest_run: Option<DispatchRun>,
    latest_outcome: Option<DispatchRunOutcome>,
    latest_run_terminal_status: Option<CandidateLifecycleStatus>,
    last_event_at: Option<String>,
    last_issue_updated_at: Option<String>,
    last_comments_count: Option<u64>,
    snooze_baseline_updated_at: Option<String>,
    snooze_baseline_comments_count: Option<u64>,
    reasons: Vec<String>,
}

impl CandidateFacts {
    fn new(issue_key: IssueKey) -> Self {
        Self {
            issue_key,
            title: None,
            url: None,
            priority: None,
            category: None,
            inbox_id: None,
            inbox_status: None,
            issue_task_id: None,
            issue_task_status: None,
            has_package: false,
            ranked: false,
            prepared: false,
            recommendation_dismissed: false,
            recommendation_done: false,
            reactivated_after_snooze: false,
            pending_approval_ids: Vec::new(),
            latest_run: None,
            latest_outcome: None,
            latest_run_terminal_status: None,
            last_event_at: None,
            last_issue_updated_at: None,
            last_comments_count: None,
            snooze_baseline_updated_at: None,
            snooze_baseline_comments_count: None,
            reasons: Vec::new(),
        }
    }

    fn finish(mut self) -> CandidateTask {
        self.pending_approval_ids.sort();
        self.pending_approval_ids.dedup();
        self.reasons.sort();
        self.reasons.dedup();
        let display = self.display_state();
        let status = self.lifecycle_status(display);
        CandidateTask {
            issue_key: self.issue_key,
            title: self.title,
            url: self.url,
            status,
            display,
            priority: self.priority,
            category: self.category,
            inbox_id: self.inbox_id,
            issue_task_id: self.issue_task_id,
            latest_run_id: self.latest_run.map(|run| run.id),
            latest_outcome_kind: self
                .latest_outcome
                .map(|outcome| outcome.outcome_kind.as_str().to_string()),
            pending_approval_ids: self.pending_approval_ids,
            last_event_at: self.last_event_at,
            last_issue_updated_at: self.last_issue_updated_at,
            last_comments_count: self.last_comments_count,
            reasons: self.reasons,
        }
    }

    fn lifecycle_status(&self, display: CandidateDisplayState) -> CandidateLifecycleStatus {
        if let Some(outcome) = &self.latest_outcome {
            return outcome_status(outcome.outcome_kind);
        }
        if let Some(status) = self.latest_run_terminal_status {
            return status;
        }
        if self.latest_run.as_ref().is_some_and(dispatch_run_is_active) {
            return CandidateLifecycleStatus::DispatchRunning;
        }
        if self.reactivated_after_snooze {
            return CandidateLifecycleStatus::ReactivationCandidate;
        }
        match display {
            CandidateDisplayState::HiddenArchived => return CandidateLifecycleStatus::Archived,
            CandidateDisplayState::HiddenSnoozed => return CandidateLifecycleStatus::Snoozed,
            CandidateDisplayState::Visible => {}
        }
        if self.issue_review_pending() {
            return CandidateLifecycleStatus::ReviewPending;
        }
        if self.has_package || self.issue_task_status == Some(IssueTaskStatus::UserApproved) {
            return CandidateLifecycleStatus::PackageReady;
        }
        if self.prepared {
            return CandidateLifecycleStatus::Prepared;
        }
        if self.ranked {
            return CandidateLifecycleStatus::Ranked;
        }
        CandidateLifecycleStatus::Discovered
    }

    fn display_state(&self) -> CandidateDisplayState {
        if self.reactivated_after_snooze {
            return CandidateDisplayState::Visible;
        }
        if matches!(
            self.inbox_status,
            Some(InboxStatus::Archived | InboxStatus::Done)
        ) || self.recommendation_done
        {
            return CandidateDisplayState::HiddenArchived;
        }
        if self.recommendation_dismissed {
            return CandidateDisplayState::HiddenSnoozed;
        }
        CandidateDisplayState::Visible
    }

    fn issue_review_pending(&self) -> bool {
        self.pending_approval_ids
            .iter()
            .any(|id| id.starts_with("issue_review:"))
    }

    fn touch(&mut self, timestamp: &str) {
        if self
            .last_event_at
            .as_deref()
            .is_none_or(|current| timestamp > current)
        {
            self.last_event_at = Some(timestamp.to_string());
        }
    }

    fn set_issue_facts(&mut self, updated_at: Option<String>, comments_count: Option<u64>) {
        if let Some(updated_at) = updated_at {
            self.last_issue_updated_at = Some(updated_at);
        }
        if let Some(comments_count) = comments_count {
            self.last_comments_count = Some(comments_count);
        }
    }
}

fn apply_recommendation_events(
    builders: &mut BTreeMap<IssueKey, CandidateFacts>,
    mut events: Vec<RecommendationEvent>,
) {
    events.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.event_id.cmp(&right.event_id))
    });

    for event in events {
        let facts = builders
            .entry(event.issue_key.clone())
            .or_insert_with(|| CandidateFacts::new(event.issue_key.clone()));
        facts.touch(&event.timestamp);
        if reactivates_snoozed_issue(facts, &event) {
            facts.reactivated_after_snooze = true;
            facts
                .reasons
                .push("Issue activity changed after snooze feedback".to_string());
        }
        match event.event_type {
            RecommendationEventType::Shown | RecommendationEventType::Read => {
                facts.ranked = true;
                facts.reasons.push(format!(
                    "Recommendation event {}",
                    event_type_name(event.event_type)
                ));
            }
            RecommendationEventType::Prepared => {
                facts.ranked = true;
                facts.prepared = true;
                facts
                    .reasons
                    .push("Recommendation event prepared".to_string());
            }
            RecommendationEventType::Done => {
                facts.recommendation_done = true;
                facts.recommendation_dismissed = false;
                facts.snooze_baseline_updated_at = event
                    .issue_updated_at
                    .clone()
                    .or_else(|| facts.last_issue_updated_at.clone());
                facts.snooze_baseline_comments_count =
                    event.issue_comments_count.or(facts.last_comments_count);
                facts
                    .reasons
                    .push("Recommendation feedback marked done".to_string());
            }
            RecommendationEventType::Dismissed => {
                facts.recommendation_dismissed = true;
                facts.recommendation_done = false;
                facts.snooze_baseline_updated_at = event
                    .issue_updated_at
                    .clone()
                    .or_else(|| facts.last_issue_updated_at.clone());
                facts.snooze_baseline_comments_count =
                    event.issue_comments_count.or(facts.last_comments_count);
                facts
                    .reasons
                    .push("Recommendation feedback dismissed issue".to_string());
            }
            RecommendationEventType::Restored => {
                if facts.recommendation_dismissed || facts.recommendation_done {
                    facts.reactivated_after_snooze = true;
                    facts
                        .reasons
                        .push("Recommendation feedback restored issue".to_string());
                }
                facts.recommendation_dismissed = false;
                facts.recommendation_done = false;
            }
        }
        facts.set_issue_facts(event.issue_updated_at, event.issue_comments_count);
    }
}

fn apply_inbox(builders: &mut BTreeMap<IssueKey, CandidateFacts>, items: Vec<inbox::InboxItem>) {
    for item in items {
        let key = IssueKey::new(item.repo_full_name.clone(), item.issue_number);
        let facts = builders
            .entry(key.clone())
            .or_insert_with(|| CandidateFacts::new(key));
        facts.title = Some(item.title);
        facts.priority = Some(item.score.into());
        facts.inbox_id = Some(item.id.clone());
        facts.inbox_status = Some(item.status.clone());
        facts.touch(&item.created_at);
        match item.status {
            InboxStatus::Ready => {
                facts.prepared = true;
                facts.reasons.push("Inbox item is ready".to_string());
            }
            InboxStatus::PrepareFailed => {
                facts.ranked = true;
                if let Some(reason) = item.failure_reason {
                    facts
                        .reasons
                        .push(format!("Prepare failed before board projection: {reason}"));
                }
            }
            InboxStatus::Archived => {
                facts.reasons.push("Inbox item is archived".to_string());
            }
            InboxStatus::Done => {
                facts.reasons.push("Inbox item is done".to_string());
            }
        }
    }
}

fn apply_dispatch(
    builders: &mut BTreeMap<IssueKey, CandidateFacts>,
    store: &DispatchStore,
) -> Result<()> {
    let review_approvals = store.list_approval_requests_by_type(ApprovalType::IssueReview)?;
    let mut review_approvals_by_task = BTreeMap::<String, Vec<_>>::new();
    for approval in review_approvals {
        if let Some(issue_task_id) = approval_issue_task_id(&approval.details_json) {
            review_approvals_by_task
                .entry(issue_task_id.to_string())
                .or_default()
                .push(approval);
        }
    }

    for issue_task in store.list_issue_tasks()? {
        let key = IssueKey::new(issue_task.repo_full_name.clone(), issue_task.issue_number);
        let facts = builders
            .entry(key.clone())
            .or_insert_with(|| CandidateFacts::new(key));
        apply_issue_task(facts, &issue_task);

        if let Some(approvals) = review_approvals_by_task.get(&issue_task.id) {
            for approval in approvals {
                facts.touch(&approval.created_at);
                if approval.status == ApprovalStatus::Pending {
                    facts
                        .pending_approval_ids
                        .push(format!("issue_review:{}", approval.id));
                    facts
                        .reasons
                        .push("Issue review approval is pending".to_string());
                } else if approval.status == ApprovalStatus::Rejected {
                    facts
                        .reasons
                        .push("Issue review approval was rejected".to_string());
                }
            }
        }

        for run in store.list_dispatch_runs_for_issue_task(&issue_task.id)? {
            facts.touch(&run.created_at);
            if let Some(completed_at) = run.completed_at.as_deref() {
                facts.touch(completed_at);
            }
            if latest_run_is_newer(facts.latest_run.as_ref(), &run) {
                facts.latest_run = Some(run.clone());
                facts.latest_run_terminal_status = terminal_status_for_run(run.status);
            }
            for approval in store.list_approval_requests_for_run(&run.id)? {
                facts.touch(&approval.created_at);
                if approval.status == ApprovalStatus::Pending
                    && approval.approval_type == ApprovalType::Dispatch
                {
                    facts
                        .pending_approval_ids
                        .push(format!("dispatch:{}", approval.id));
                    facts
                        .reasons
                        .push("Dispatch approval is pending".to_string());
                }
            }
            if let Some(outcome) = store.find_dispatch_run_outcome_by_run(&run.id)? {
                facts.touch(&outcome.recorded_at);
                if latest_outcome_is_newer(facts.latest_outcome.as_ref(), &outcome) {
                    facts.latest_outcome = Some(outcome);
                }
            }
        }
    }
    Ok(())
}

fn apply_issue_task(facts: &mut CandidateFacts, issue_task: &IssueTask) {
    facts.title = Some(issue_task.title.clone());
    facts.url = Some(issue_task.url.clone());
    facts.priority = issue_task.priority;
    facts.category = issue_task.category.clone();
    facts.issue_task_id = Some(issue_task.id.clone());
    facts.issue_task_status = Some(issue_task.status);
    facts.has_package = issue_task.current_package_artifact_id.is_some();
    facts.touch(&issue_task.updated_at);
    if issue_task.status != IssueTaskStatus::Discovered || facts.has_package {
        facts.prepared = true;
    }
    facts.reasons.push(format!(
        "Dispatch issue task status is {}",
        issue_task.status
    ));
    if facts.has_package {
        facts
            .reasons
            .push("IssueTaskPackage artifact is ready".to_string());
    }
}

fn reactivates_snoozed_issue(facts: &CandidateFacts, event: &RecommendationEvent) -> bool {
    if !(facts.recommendation_dismissed || facts.recommendation_done) {
        return false;
    }
    if event.event_type == RecommendationEventType::Restored {
        return true;
    }
    let updated = match (
        event.issue_updated_at.as_deref(),
        facts.snooze_baseline_updated_at.as_deref(),
    ) {
        (Some(left), Some(right)) => left > right,
        _ => false,
    };
    let comments_increased = match (
        event.issue_comments_count,
        facts.snooze_baseline_comments_count,
    ) {
        (Some(left), Some(right)) => left > right,
        _ => false,
    };
    updated || comments_increased
}

fn event_type_name(event_type: RecommendationEventType) -> &'static str {
    match event_type {
        RecommendationEventType::Shown => "shown",
        RecommendationEventType::Read => "read",
        RecommendationEventType::Prepared => "prepared",
        RecommendationEventType::Done => "done",
        RecommendationEventType::Dismissed => "dismissed",
        RecommendationEventType::Restored => "restored",
    }
}

fn approval_issue_task_id(details: &Value) -> Option<&str> {
    details.get("issueTaskId").and_then(Value::as_str)
}

fn latest_run_is_newer(current: Option<&DispatchRun>, candidate: &DispatchRun) -> bool {
    current.is_none_or(|current| {
        candidate
            .created_at
            .cmp(&current.created_at)
            .then_with(|| candidate.id.cmp(&current.id))
            .is_gt()
    })
}

fn latest_outcome_is_newer(
    current: Option<&DispatchRunOutcome>,
    candidate: &DispatchRunOutcome,
) -> bool {
    current.is_none_or(|current| {
        candidate
            .recorded_at
            .cmp(&current.recorded_at)
            .then_with(|| candidate.id.cmp(&current.id))
            .is_gt()
    })
}

fn outcome_status(outcome: DispatchOutcomeKind) -> CandidateLifecycleStatus {
    if outcome.is_positive() {
        CandidateLifecycleStatus::OutcomePositive
    } else {
        CandidateLifecycleStatus::OutcomeNegative
    }
}

fn terminal_status_for_run(status: DispatchRunStatus) -> Option<CandidateLifecycleStatus> {
    match status {
        DispatchRunStatus::Completed => Some(CandidateLifecycleStatus::OutcomePositive),
        DispatchRunStatus::Failed | DispatchRunStatus::Canceled => {
            Some(CandidateLifecycleStatus::OutcomeNegative)
        }
        _ => None,
    }
}

fn dispatch_run_is_active(run: &DispatchRun) -> bool {
    matches!(
        run.status,
        DispatchRunStatus::Approved
            | DispatchRunStatus::Queued
            | DispatchRunStatus::Starting
            | DispatchRunStatus::Running
            | DispatchRunStatus::NeedsUser
    )
}

fn status_rank(status: CandidateLifecycleStatus) -> u8 {
    match status {
        CandidateLifecycleStatus::DispatchRunning => 0,
        CandidateLifecycleStatus::ReviewPending => 1,
        CandidateLifecycleStatus::PackageReady => 2,
        CandidateLifecycleStatus::Prepared => 3,
        CandidateLifecycleStatus::ReactivationCandidate => 4,
        CandidateLifecycleStatus::Ranked => 5,
        CandidateLifecycleStatus::Discovered => 6,
        CandidateLifecycleStatus::Snoozed => 7,
        CandidateLifecycleStatus::OutcomeNegative => 8,
        CandidateLifecycleStatus::OutcomePositive => 9,
        CandidateLifecycleStatus::Archived => 10,
    }
}
