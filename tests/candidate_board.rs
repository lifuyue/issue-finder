use chrono::Utc;
use issue_finder::candidate_board::{
    derive_task_board_at, CandidateDisplayState, CandidateLifecycleStatus,
};
use issue_finder::dispatch::{
    ApprovalStatus, ApprovalType, DispatchOutcomeKind, DispatchRunStatus, DispatchStore,
    IssueTaskPackage, IssueTaskPackageIssue, IssueTaskStatus, NewAgentProfile, NewApprovalRequest,
    NewDispatchRun, NewDispatchRunOutcome, NewIssueTask,
};
use issue_finder::inbox::{save_index, InboxIndex, InboxItem, InboxStatus};
use issue_finder::paths::IssueFinderPaths;
use issue_finder::recommendation::{
    append_event, IssueKey, RecommendationEvent, RecommendationEventSource, RecommendationEventType,
};
use serde_json::json;
use tempfile::tempdir;

#[test]
fn candidate_board_projects_review_package_and_running_lifecycle() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let store = DispatchStore::open(paths.clone()).unwrap();
    seed_agent(&store);

    let review_task = store
        .upsert_issue_task(NewIssueTask {
            repo_full_name: "owner/review".to_string(),
            issue_number: 1,
            title: "Review task".to_string(),
            url: "https://github.com/owner/review/issues/1".to_string(),
            status: IssueTaskStatus::LlmConfirmed,
            priority: Some(80),
            category: Some("high_value_ready".to_string()),
        })
        .unwrap();
    store
        .create_approval_request(NewApprovalRequest {
            run_id: None,
            approval_type: ApprovalType::IssueReview,
            status: ApprovalStatus::Pending,
            prompt: "Approve review task?".to_string(),
            details_json: json!({ "issueTaskId": review_task.id }),
        })
        .unwrap();

    let packaged_task = store
        .upsert_issue_task(NewIssueTask {
            repo_full_name: "owner/packaged".to_string(),
            issue_number: 2,
            title: "Packaged task".to_string(),
            url: "https://github.com/owner/packaged/issues/2".to_string(),
            status: IssueTaskStatus::UserApproved,
            priority: Some(70),
            category: Some("high_value_ready".to_string()),
        })
        .unwrap();
    store
        .write_task_package_artifact(&packaged_task.id, &package("owner/packaged", 2))
        .unwrap();

    let running_task = store
        .upsert_issue_task(NewIssueTask {
            repo_full_name: "owner/running".to_string(),
            issue_number: 3,
            title: "Running task".to_string(),
            url: "https://github.com/owner/running/issues/3".to_string(),
            status: IssueTaskStatus::InProgress,
            priority: Some(90),
            category: Some("high_value_ready".to_string()),
        })
        .unwrap();
    store
        .write_task_package_artifact(&running_task.id, &package("owner/running", 3))
        .unwrap();
    store
        .create_dispatch_run(NewDispatchRun {
            issue_task_id: running_task.id,
            agent_id: "codex".to_string(),
            status: DispatchRunStatus::Running,
            requested_by: "test".to_string(),
            approval_state: ApprovalStatus::Approved,
            selected_session_link_id: None,
        })
        .unwrap();

    let board = derive_task_board_at(&paths, "2026-06-21T00:00:00Z").unwrap();

    assert_eq!(
        board
            .by_issue(&IssueKey::new("owner/review", 1))
            .unwrap()
            .status,
        CandidateLifecycleStatus::ReviewPending
    );
    assert_eq!(
        board.ready_for_review()[0].issue_key,
        IssueKey::new("owner/review", 1)
    );
    assert_eq!(
        board
            .by_issue(&IssueKey::new("owner/packaged", 2))
            .unwrap()
            .status,
        CandidateLifecycleStatus::PackageReady
    );
    assert_eq!(
        board.ready_for_dispatch()[0].issue_key,
        IssueKey::new("owner/packaged", 2)
    );
    assert_eq!(
        board
            .by_issue(&IssueKey::new("owner/running", 3))
            .unwrap()
            .status,
        CandidateLifecycleStatus::DispatchRunning
    );
}

#[test]
fn dispatch_terminal_outcome_wins_over_inbox_done_and_archive_display() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let key = IssueKey::new("owner/done", 4);
    append_event(
        &paths,
        &event(
            &key,
            RecommendationEventType::Dismissed,
            "2026-06-21T00:00:00Z",
            None,
            None,
        ),
    )
    .unwrap();
    save_index(
        &paths,
        &InboxIndex {
            items: vec![inbox_item(
                "done-inbox",
                "owner/done",
                4,
                "Done task",
                InboxStatus::Done,
            )],
        },
    )
    .unwrap();

    let store = DispatchStore::open(paths.clone()).unwrap();
    seed_agent(&store);
    let task = store
        .upsert_issue_task(NewIssueTask {
            repo_full_name: "owner/done".to_string(),
            issue_number: 4,
            title: "Done task".to_string(),
            url: "https://github.com/owner/done/issues/4".to_string(),
            status: IssueTaskStatus::InProgress,
            priority: Some(60),
            category: Some("high_value_ready".to_string()),
        })
        .unwrap();
    let run = store
        .create_dispatch_run(NewDispatchRun {
            issue_task_id: task.id,
            agent_id: "codex".to_string(),
            status: DispatchRunStatus::Running,
            requested_by: "test".to_string(),
            approval_state: ApprovalStatus::Approved,
            selected_session_link_id: None,
        })
        .unwrap();
    store
        .record_dispatch_run_outcome(NewDispatchRunOutcome {
            run_id: run.id,
            idempotency_key: "done-positive-outcome".to_string(),
            outcome_kind: DispatchOutcomeKind::FixReady,
            failure_class: None,
            failure_detail: None,
            task_class: None,
            validation_outcome: None,
            result_artifact_id: None,
            metadata_json: json!({ "source": "test" }),
        })
        .unwrap();

    let board = derive_task_board_at(&paths, "2026-06-21T00:00:00Z").unwrap();
    let item = board.by_issue(&key).unwrap();

    assert_eq!(item.status, CandidateLifecycleStatus::OutcomePositive);
    assert_eq!(item.display, CandidateDisplayState::HiddenArchived);
    assert_eq!(item.latest_outcome_kind.as_deref(), Some("fix_ready"));
}

#[test]
fn snoozed_issue_reactivates_when_later_issue_facts_change() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    paths.ensure_layout().unwrap();
    let snoozed = IssueKey::new("owner/snoozed", 5);
    let reactivated = IssueKey::new("owner/reactivated", 6);

    append_event(
        &paths,
        &event(
            &snoozed,
            RecommendationEventType::Dismissed,
            "2026-06-21T00:00:00Z",
            Some("2026-06-20T00:00:00Z"),
            Some(1),
        ),
    )
    .unwrap();
    append_event(
        &paths,
        &event(
            &reactivated,
            RecommendationEventType::Dismissed,
            "2026-06-21T00:00:00Z",
            Some("2026-06-20T00:00:00Z"),
            Some(1),
        ),
    )
    .unwrap();
    append_event(
        &paths,
        &event(
            &reactivated,
            RecommendationEventType::Shown,
            "2026-06-21T01:00:00Z",
            Some("2026-06-21T00:30:00Z"),
            Some(2),
        ),
    )
    .unwrap();

    let board = derive_task_board_at(&paths, "2026-06-21T02:00:00Z").unwrap();

    let snoozed_item = board.by_issue(&snoozed).unwrap();
    assert_eq!(snoozed_item.status, CandidateLifecycleStatus::Snoozed);
    assert_eq!(snoozed_item.display, CandidateDisplayState::HiddenSnoozed);

    let reactivated_item = board.by_issue(&reactivated).unwrap();
    assert_eq!(
        reactivated_item.status,
        CandidateLifecycleStatus::ReactivationCandidate
    );
    assert_eq!(reactivated_item.display, CandidateDisplayState::Visible);
    assert_eq!(
        board.reactivation_candidates()[0].issue_key,
        IssueKey::new("owner/reactivated", 6)
    );
}

fn event(
    issue_key: &IssueKey,
    event_type: RecommendationEventType,
    timestamp: &str,
    issue_updated_at: Option<&str>,
    issue_comments_count: Option<u64>,
) -> RecommendationEvent {
    RecommendationEvent {
        event_id: format!(
            "test-{}-{}",
            timestamp.replace([':', '-'], ""),
            issue_key.label().replace(['/', '#'], "-")
        ),
        timestamp: timestamp.to_string(),
        issue_key: issue_key.clone(),
        event_type,
        source: RecommendationEventSource::FeedbackCommand,
        issue_updated_at: issue_updated_at.map(ToString::to_string),
        issue_comments_count,
        metadata: json!({}),
    }
}

fn inbox_item(
    id: &str,
    repo_full_name: &str,
    issue_number: u64,
    title: &str,
    status: InboxStatus,
) -> InboxItem {
    InboxItem {
        id: id.to_string(),
        repo_full_name: repo_full_name.to_string(),
        issue_number,
        title: title.to_string(),
        score: 50,
        status,
        handoff_json_path: String::new(),
        handoff_md_path: String::new(),
        codex_md_path: String::new(),
        agent_policy_path: String::new(),
        probe_json_path: String::new(),
        prepare_events_path: String::new(),
        created_at: Utc::now().to_rfc3339(),
        failure_reason: None,
    }
}

fn package(repo_full_name: &str, number: u64) -> IssueTaskPackage {
    IssueTaskPackage::new(IssueTaskPackageIssue {
        repo_full_name: repo_full_name.to_string(),
        number,
        title: "Task".to_string(),
        url: format!("https://github.com/{repo_full_name}/issues/{number}"),
    })
}

fn seed_agent(store: &DispatchStore) {
    store
        .create_agent_profile(NewAgentProfile {
            id: Some("codex".to_string()),
            kind: "codex".to_string(),
            display_name: "Codex".to_string(),
            adapter: "codex_app_server".to_string(),
            config_json: json!({}),
            enabled: true,
        })
        .unwrap();
}

fn test_paths(root: &std::path::Path) -> IssueFinderPaths {
    IssueFinderPaths {
        home: root.to_path_buf(),
        config: root.join("config.toml"),
        cache_dir: root.join("cache"),
        workspaces_dir: root.join("workspaces"),
        inbox_dir: root.join("inbox"),
        reports_dir: root.join("reports"),
    }
}
