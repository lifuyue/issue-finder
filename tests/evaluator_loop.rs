use std::process::Command;

use issue_finder::dispatch::projectors::project_terminal_outcome;
use issue_finder::dispatch::{
    ApprovalStatus, CandidateResult, CandidateResultStatus, CriterionEvidence, DispatchRunStatus,
    DispatchRuntime, EvaluationDisposition, IssueTaskStatus, NewAgentProfile, NewDispatchRun,
    NewIssueTask, TaskIdentity, TaskPackage, ValidationEvidence,
};
use issue_finder::memory::MemoryStore;
use issue_finder::paths::IssueFinderPaths;
use serde_json::json;
use tempfile::tempdir;

#[test]
fn evaluator_retries_then_commits_one_truthful_terminal_outcome() {
    let fixture = Fixture::new();
    let mut first = fixture.result(CandidateResultStatus::Success);
    first.success_criteria.clear();
    let retry = fixture.runtime.submit_result(first).unwrap();
    assert_eq!(
        retry.evaluation.report.disposition,
        EvaluationDisposition::Retry
    );
    assert!(retry.terminal.is_none());
    assert_eq!(retry.evaluation.run.status, DispatchRunStatus::Running);

    let accepted = fixture
        .runtime
        .submit_result(fixture.result(CandidateResultStatus::Success))
        .unwrap();
    assert_eq!(
        accepted.evaluation.report.disposition,
        EvaluationDisposition::AcceptedSuccess
    );
    let terminal = accepted.terminal.unwrap();
    assert_eq!(terminal.run.status, DispatchRunStatus::Succeeded);
    assert_eq!(
        fixture
            .runtime
            .store()
            .list_dispatch_run_outcomes()
            .unwrap()
            .len(),
        1
    );

    let memory_before = MemoryStore::open(&fixture.runtime.store().paths())
        .unwrap()
        .list_raw_events()
        .unwrap()
        .len();
    project_terminal_outcome(fixture.runtime.store(), &terminal.outcome).unwrap();
    project_terminal_outcome(fixture.runtime.store(), &terminal.outcome).unwrap();
    let memory_after = MemoryStore::open(&fixture.runtime.store().paths())
        .unwrap()
        .list_raw_events()
        .unwrap()
        .len();
    assert_eq!(memory_after, memory_before);
    assert_eq!(
        fixture
            .runtime
            .store()
            .list_github_interaction_decisions_for_run(&fixture.run_id)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn needs_user_is_resumable_and_never_consumes_terminal_slot() {
    let fixture = Fixture::new();
    for _ in 0..2 {
        let pending = fixture
            .runtime
            .submit_result(fixture.result(CandidateResultStatus::NeedsUser))
            .unwrap();
        assert_eq!(pending.evaluation.run.status, DispatchRunStatus::NeedsUser);
        assert!(pending.terminal.is_none());
        assert!(fixture
            .runtime
            .store()
            .list_dispatch_run_outcomes()
            .unwrap()
            .is_empty());
        fixture
            .runtime
            .store()
            .update_dispatch_run_status(&fixture.run_id, DispatchRunStatus::Running, None)
            .unwrap();
    }
    let accepted = fixture
        .runtime
        .submit_result(fixture.result(CandidateResultStatus::Success))
        .unwrap();
    assert_eq!(
        accepted.terminal.unwrap().run.status,
        DispatchRunStatus::Succeeded
    );
}

struct Fixture {
    _dir: tempfile::TempDir,
    runtime: DispatchRuntime,
    run_id: String,
    task_id: String,
    package_id: String,
    criteria: Vec<String>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        git(&workspace, &["init", "-q"]);
        git(&workspace, &["config", "user.email", "test@example.com"]);
        git(&workspace, &["config", "user.name", "Test"]);
        std::fs::write(workspace.join("tracked.txt"), "before\n").unwrap();
        git(&workspace, &["add", "tracked.txt"]);
        git(&workspace, &["commit", "-qm", "initial"]);
        std::fs::write(workspace.join("tracked.txt"), "after\n").unwrap();

        let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
        runtime
            .store()
            .create_agent_profile(NewAgentProfile {
                id: Some("worker".to_string()),
                kind: "codex".to_string(),
                display_name: "Worker".to_string(),
                adapter: "codex_app_server".to_string(),
                config_json: json!({}),
                enabled: true,
            })
            .unwrap();
        let task = runtime
            .store()
            .upsert_issue_task(NewIssueTask {
                repo_full_name: "owner/repo".to_string(),
                issue_number: 1,
                title: "Fix tracked file".to_string(),
                url: "https://github.com/owner/repo/issues/1".to_string(),
                status: IssueTaskStatus::InProgress,
                priority: None,
                category: None,
            })
            .unwrap();
        let mut package = TaskPackage::new(TaskIdentity {
            repo_full_name: task.repo_full_name.clone(),
            issue_number: task.issue_number,
            title: task.title.clone(),
            url: task.url.clone(),
        });
        package.context_snapshot.snapshot_id = "ctx-test".to_string();
        package.context_snapshot.artifact_id = "snapshot-artifact".to_string();
        package.context_snapshot.entry_artifact_id = "entry-artifact".to_string();
        package.workspace.path = workspace.to_string_lossy().to_string();
        package.workspace.default_branch = "main".to_string();
        package.workspace.branch = "issue-finder/test".to_string();
        let criteria = package.success_criteria.clone();
        let package_artifact = runtime
            .store()
            .write_task_package_artifact(&task.id, &package)
            .unwrap();
        let run = runtime
            .store()
            .create_dispatch_run(NewDispatchRun {
                issue_task_id: task.id.clone(),
                agent_id: "worker".to_string(),
                status: DispatchRunStatus::Running,
                requested_by: "test".to_string(),
                approval_state: ApprovalStatus::Approved,
                selected_thread_id: None,
            })
            .unwrap();
        Self {
            _dir: dir,
            runtime,
            run_id: run.id,
            task_id: task.id,
            package_id: package_artifact.id,
            criteria,
        }
    }

    fn result(&self, status: CandidateResultStatus) -> CandidateResult {
        CandidateResult {
            run_id: self.run_id.clone(),
            issue_task_id: self.task_id.clone(),
            package_id: self.package_id.clone(),
            status,
            summary: "Implemented and validated the scoped change.".to_string(),
            changed_files: vec!["tracked.txt".to_string()],
            reproduction: json!({"attempted":true,"observed":"before"}),
            success_criteria: self
                .criteria
                .iter()
                .map(|criterion| CriterionEvidence {
                    criterion: criterion.clone(),
                    satisfied: true,
                    evidence: vec!["verified by fixture".to_string()],
                })
                .collect(),
            validation: vec![ValidationEvidence {
                command: "cargo test".to_string(),
                passed: true,
                exit_code: Some(0),
                evidence: vec!["exit 0".to_string()],
            }],
            residual_risks: Vec::new(),
            failure_reason: (status == CandidateResultStatus::NeedsUser)
                .then(|| "Need a product decision".to_string()),
            suggested_github_reply: None,
            session_context: json!({}),
        }
    }
}

fn git(workspace: &std::path::Path, args: &[&str]) {
    assert!(Command::new("git")
        .args(args)
        .current_dir(workspace)
        .status()
        .unwrap()
        .success());
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
