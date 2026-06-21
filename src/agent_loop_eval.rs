use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::dispatch::adapters::{
    AdapterSession, AdapterStartSessionRequest, AdapterTurn, NativeExecutionAdapter,
};
use crate::dispatch::execution::execute_approved_dispatch;
use crate::dispatch::{
    AgentSessionStatus, ApprovalStatus, DispatchOutcomeFailureClass, DispatchOutcomeKind,
    DispatchOutcomeRecordRequest, DispatchProposalRequest, DispatchRunStatus, DispatchTaskClass,
    DispatchValidationOutcome, GitHubCommentWriter, GitHubInteractionStatus, IssueTaskPackage,
    IssueTaskPackageIssue, IssueTaskStatus, NewAgentSessionLink, NewIssueTask, PostedGitHubComment,
};
use crate::github::GitHubIssue;
use crate::github_enrichment::EnrichedIssue;
use crate::memory::{
    MemoryControlPlane, MemoryDecisionHintRequest, MemoryDreamRun, MemoryDreamScope,
    MemoryDreamStatus, MemoryDreamTrigger, MemoryDreamType, MemoryHintScope, MemoryHintScopeType,
    MemoryHintStatus, MemoryHintType, MemoryModelStatus, MemoryRuntimeMode, MemoryStore,
    NewMemoryDream, NewMemoryHint,
};
use crate::paths::{atomic_write, IssueFinderPaths};
use crate::recommendation::feedback::assess_feedback;
use crate::recommendation::{IssueKey, RecommendationIssueState};

const AGENT_LOOP_EVAL_FIXTURES: &str =
    include_str!("../tests/fixtures/agent_loop_eval/samples.json");
static EVAL_HOME_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Deserialize)]
struct AgentLoopFixtureSet {
    samples: Vec<AgentLoopFixtureSample>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentLoopFixtureSample {
    id: String,
    family: String,
    scenario: AgentLoopScenario,
    expected: AgentLoopExpected,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AgentLoopScenario {
    RuntimeFailureDoesNotRewriteIssueQuality,
    PackageMissingOutcomeContractIsInsufficient,
    LifecycleReactivationAfterMaintainerActivity,
    GithubApprovalRetryPolicy,
    SessionResumeUsesSelectedNativeSession,
    MemoryGovernancePreventsProfileDrift,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentLoopExpected {
    behaviors: Vec<String>,
    reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentLoopEvalReport {
    pub kind: String,
    pub metrics: AgentLoopEvalMetrics,
    pub samples: Vec<AgentLoopSampleResult>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentLoopEvalMetrics {
    pub total_samples: usize,
    pub passed_samples: usize,
    pub failed_samples: usize,
    pub families: BTreeMap<String, AgentLoopFamilyMetrics>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentLoopFamilyMetrics {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentLoopSampleResult {
    pub id: String,
    pub family: String,
    pub scenario: String,
    pub passed: bool,
    pub observations: Vec<String>,
    pub failures: Vec<String>,
    pub reasons: Vec<String>,
}

pub fn evaluate_builtin() -> Result<AgentLoopEvalReport> {
    let fixtures = serde_json::from_str::<AgentLoopFixtureSet>(AGENT_LOOP_EVAL_FIXTURES)
        .context("agent loop eval fixture JSON is invalid")?;
    evaluate_fixtures(fixtures)
}

pub fn run_offline_eval(output_dir: &Path) -> Result<AgentLoopEvalReport> {
    let report = evaluate_builtin()?;
    fs::create_dir_all(output_dir)
        .with_context(|| format!("unable to create {}", output_dir.display()))?;
    atomic_write(
        &output_dir.join("metrics.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    atomic_write(
        &output_dir.join("report.md"),
        render_markdown_report(&report),
    )?;
    Ok(report)
}

fn evaluate_fixtures(fixtures: AgentLoopFixtureSet) -> Result<AgentLoopEvalReport> {
    let mut samples = Vec::new();
    for sample in fixtures.samples {
        samples.push(evaluate_sample(sample)?);
    }
    let metrics = metrics_for(&samples);
    Ok(AgentLoopEvalReport {
        kind: "agent_loop_eval_report".to_string(),
        metrics,
        samples,
    })
}

fn evaluate_sample(sample: AgentLoopFixtureSample) -> Result<AgentLoopSampleResult> {
    let observations = match sample.scenario {
        AgentLoopScenario::RuntimeFailureDoesNotRewriteIssueQuality => {
            runtime_failure_observations()?
        }
        AgentLoopScenario::PackageMissingOutcomeContractIsInsufficient => {
            package_insufficiency_observations()
        }
        AgentLoopScenario::LifecycleReactivationAfterMaintainerActivity => {
            lifecycle_reactivation_observations()
        }
        AgentLoopScenario::GithubApprovalRetryPolicy => github_policy_observations()?,
        AgentLoopScenario::SessionResumeUsesSelectedNativeSession => session_resume_observations()?,
        AgentLoopScenario::MemoryGovernancePreventsProfileDrift => {
            memory_governance_observations()?
        }
    };
    let failures = sample
        .expected
        .behaviors
        .iter()
        .filter(|behavior| !observations.iter().any(|observed| observed == *behavior))
        .map(|behavior| format!("missing expected behavior `{behavior}`"))
        .collect::<Vec<_>>();
    Ok(AgentLoopSampleResult {
        id: sample.id,
        family: sample.family,
        scenario: serde_json::to_string(&sample.scenario)
            .unwrap_or_else(|_| "\"unknown\"".to_string())
            .trim_matches('"')
            .to_string(),
        passed: failures.is_empty(),
        observations,
        failures,
        reasons: sample.expected.reasons,
    })
}

fn runtime_failure_observations() -> Result<Vec<String>> {
    let home = EvalHome::new("runtime-failure")?;
    let runtime = crate::dispatch::DispatchRuntime::open(home.paths.clone())?;
    let task = create_packaged_task(&runtime, 101)?;
    let proposal = runtime.propose_dispatch(DispatchProposalRequest {
        issue: "owner/repo#101".to_string(),
        agent_id: "codex".to_string(),
        requested_by: "agent_loop_eval".to_string(),
        selected_session_link_id: None,
        new_session: true,
    })?;
    runtime.resolve_dispatch_approval(&proposal.run.id, ApprovalStatus::Approved)?;
    let recorded = runtime.record_dispatch_outcome(DispatchOutcomeRecordRequest {
        run_id: proposal.run.id.clone(),
        idempotency_key: Some("agent-loop-runtime-failure".to_string()),
        outcome_kind: DispatchOutcomeKind::Failed,
        failure_class: Some(DispatchOutcomeFailureClass::ValidationFailed),
        failure_detail: Some("cargo test still fails".to_string()),
        task_class: Some(DispatchTaskClass::RustCliPanic),
        validation_outcome: Some(DispatchValidationOutcome::Failed),
        result_artifact_id: None,
        metadata_json: json!({ "source": "agent_loop_eval" }),
    })?;
    let memory = runtime
        .store()
        .list_memory_events_for_issue_task(&task.id)?;
    let mut observations = Vec::new();
    if recorded.run.status == DispatchRunStatus::Failed {
        observations.push("run_status_failed".to_string());
    }
    let task_after = runtime.store().get_issue_task(&task.id)?;
    if task_after.status == IssueTaskStatus::Dispatched {
        observations.push("issue_task_status_not_rewritten_to_quality_reject".to_string());
    }
    if memory.iter().any(|event| {
        event.source == "dispatch_outcome"
            && event.payload_json["failureClass"] == "validation_failed"
    }) {
        observations.push("runtime_failure_recorded_as_dispatch_outcome".to_string());
    }
    if recorded.memory_ingest.is_some() {
        observations.push("dispatch_outcome_ingested_as_memory_signal".to_string());
    }
    Ok(observations)
}

fn package_insufficiency_observations() -> Vec<String> {
    let mut package = IssueTaskPackage::new(IssueTaskPackageIssue {
        repo_full_name: "owner/repo".to_string(),
        number: 202,
        title: "Fix parser panic".to_string(),
        url: "https://github.com/owner/repo/issues/202".to_string(),
    });
    package.outcome_contract = Value::Null;
    let mut observations = Vec::new();
    let findings = package_contract_findings(&package);
    if findings
        .iter()
        .any(|finding| finding == "missing_outcome_contract")
    {
        observations.push("package_missing_outcome_contract_detected".to_string());
    }
    if findings
        .iter()
        .any(|finding| finding == "required_fix_result_contract_unavailable")
    {
        observations.push("package_marked_insufficient_for_agent_loop".to_string());
    }
    observations
}

fn lifecycle_reactivation_observations() -> Vec<String> {
    let now = Utc::now();
    let last_feedback = (now - Duration::days(2)).to_rfc3339();
    let issue = GitHubIssue {
        id: 303,
        number: 303,
        title: "Fix parser panic after config reload".to_string(),
        body: "Expected graceful error and regression test.".to_string(),
        labels: vec!["bug".to_string(), "good first issue".to_string()],
        url: "https://github.com/owner/repo/issues/303".to_string(),
        repo_full_name: "owner/repo".to_string(),
        repo_name: "repo".to_string(),
        repo_description: "Rust CLI".to_string(),
        repo_stars: 100,
        created_at: (now - Duration::days(10)).to_rfc3339(),
        updated_at: now.to_rfc3339(),
    };
    let mut enriched = EnrichedIssue::from_issue(&issue);
    enriched.issue.comments_count = 2;
    enriched.activity.maintainer_recent_response = true;
    let state = RecommendationIssueState {
        issue_key: IssueKey::new("owner/repo", 303),
        read_count: 2,
        last_read_at: Some(last_feedback.clone()),
        last_feedback_at: Some(last_feedback),
        last_seen_issue_updated_at: Some((now - Duration::days(4)).to_rfc3339()),
        last_seen_comments_count: Some(1),
        ..RecommendationIssueState::default()
    };
    let feedback = assess_feedback(Some(&state), &enriched);
    let mut observations = Vec::new();
    if feedback.reactivation_boost >= 60 {
        observations.push("reactivation_boost_applied_after_new_activity".to_string());
    }
    if feedback.penalty < 70 {
        observations.push("prior_feedback_penalty_recovered_but_not_erased".to_string());
    }
    if feedback
        .reasons
        .iter()
        .any(|reason| reason.contains("Maintainer activity"))
    {
        observations.push("maintainer_activity_explains_reactivation".to_string());
    }
    observations
}

fn github_policy_observations() -> Result<Vec<String>> {
    let home = EvalHome::new("github-policy")?;
    let runtime = crate::dispatch::DispatchRuntime::open(home.paths.clone())?;
    create_packaged_task(&runtime, 404)?;
    let draft = runtime.draft_github_tracking_comment("owner/repo#404", None)?;
    let mut writer = FakeGitHubWriter::default();
    let blocked = runtime
        .post_github_interaction_with_writer(&mut writer, &draft.interaction.id)
        .unwrap_err();

    let mut observations = Vec::new();
    if blocked.to_string().contains("not approved") && writer.calls.is_empty() {
        observations.push("github_post_blocked_until_approval".to_string());
    }

    runtime.approve_github_interaction(&draft.interaction.id)?;
    writer.failures_remaining = 1;
    let failed = runtime
        .post_github_interaction_with_writer(&mut writer, &draft.interaction.id)
        .unwrap_err();
    let failed_interaction = runtime
        .store()
        .get_github_interaction(&draft.interaction.id)?;
    if failed.to_string().contains("transient GitHub failure")
        && failed_interaction.status == GitHubInteractionStatus::Failed
    {
        observations.push("github_failed_post_is_persisted_for_retry".to_string());
    }
    let posted =
        runtime.retry_github_interaction_with_writer(&mut writer, &draft.interaction.id)?;
    if posted.interaction.status == GitHubInteractionStatus::Posted {
        observations.push("github_retry_posts_after_failure".to_string());
    }

    let rejected = runtime.draft_github_tracking_comment(
        "owner/repo#404",
        Some("second tracking draft".to_string()),
    )?;
    runtime.reject_github_interaction(&rejected.interaction.id)?;
    let calls_before_rejected_post = writer.calls.len();
    let rejected_error = runtime
        .post_github_interaction_with_writer(&mut writer, &rejected.interaction.id)
        .unwrap_err();
    if rejected_error.to_string().contains("not approved")
        && writer.calls.len() == calls_before_rejected_post
    {
        observations.push("github_rejected_draft_cannot_post".to_string());
    }
    Ok(observations)
}

fn session_resume_observations() -> Result<Vec<String>> {
    let home = EvalHome::new("session-resume")?;
    let runtime = crate::dispatch::DispatchRuntime::open(home.paths.clone())?;
    let task = create_packaged_task(&runtime, 505)?;
    let session = runtime.store().create_session_link(NewAgentSessionLink {
        agent_id: "codex".to_string(),
        native_session_id: "native_existing_505".to_string(),
        issue_task_id: Some(task.id.clone()),
        display_name: "old session".to_string(),
        goal: None,
        status: AgentSessionStatus::Idle,
        metadata_json: json!({}),
    })?;
    let proposal = runtime.propose_dispatch(DispatchProposalRequest {
        issue: "owner/repo#505".to_string(),
        agent_id: "codex".to_string(),
        requested_by: "agent_loop_eval".to_string(),
        selected_session_link_id: Some("native_existing_505".to_string()),
        new_session: false,
    })?;
    runtime.resolve_dispatch_approval(&proposal.run.id, ApprovalStatus::Approved)?;
    let mut adapter = FakeNativeAdapter::default();
    let execution = execute_approved_dispatch(runtime.store(), &mut adapter, &proposal.run.id)?;

    let mut observations = Vec::new();
    if proposal.run.selected_session_link_id.as_deref() == Some(session.id.as_str()) {
        observations.push("native_session_selector_resolved_to_local_link".to_string());
    }
    if adapter
        .calls
        .iter()
        .any(|call| call == "resume_session:native_existing_505")
    {
        observations.push("execution_resumed_selected_native_session".to_string());
    }
    if execution.run.status == DispatchRunStatus::Running {
        observations.push("resumed_session_started_agent_turn".to_string());
    }
    if execution.session.id == session.id && execution.session.status == AgentSessionStatus::Active
    {
        observations.push("session_link_remains_continuity_anchor".to_string());
    }
    Ok(observations)
}

fn memory_governance_observations() -> Result<Vec<String>> {
    let home = EvalHome::new("memory-governance")?;
    let store = MemoryStore::open(&home.paths)?;
    seed_memory_dream(&store)?;
    seed_memory_hint(
        &store,
        "candidate-ranking",
        MemoryHintType::Ranking,
        MemoryHintStatus::Candidate,
        5.0,
    )?;
    seed_memory_hint(
        &store,
        "approved-ranking",
        MemoryHintType::Ranking,
        MemoryHintStatus::Approved,
        1.0,
    )?;
    seed_memory_hint(
        &store,
        "profile-candidate",
        MemoryHintType::ProfileCandidate,
        MemoryHintStatus::Approved,
        3.0,
    )?;
    let ranking_hints = MemoryControlPlane::decision_eligible_hints(
        &store,
        &MemoryDecisionHintRequest {
            hint_type: Some(MemoryHintType::Ranking),
            scope: Some(repo_scope("owner/repo")),
            ..MemoryDecisionHintRequest::default()
        },
    )?;
    let profile_hints = MemoryControlPlane::decision_eligible_hints(
        &store,
        &MemoryDecisionHintRequest {
            hint_type: Some(MemoryHintType::ProfileCandidate),
            scope: Some(repo_scope("owner/repo")),
            ..MemoryDecisionHintRequest::default()
        },
    )?;
    let memory_off_hints = MemoryControlPlane::decision_eligible_hints(
        &store,
        &MemoryDecisionHintRequest {
            mode: MemoryRuntimeMode::MemoryOff,
            hint_type: Some(MemoryHintType::Ranking),
            scope: Some(repo_scope("owner/repo")),
            ..MemoryDecisionHintRequest::default()
        },
    )?;
    seed_memory_hint(
        &store,
        "repo-suppressed",
        MemoryHintType::Ranking,
        MemoryHintStatus::Suppressed,
        0.0,
    )?;
    let suppressed_hints = MemoryControlPlane::decision_eligible_hints(
        &store,
        &MemoryDecisionHintRequest {
            hint_type: Some(MemoryHintType::Ranking),
            scope: Some(repo_scope("owner/repo")),
            ..MemoryDecisionHintRequest::default()
        },
    )?;

    let mut observations = Vec::new();
    if ranking_hints.len() == 1 && ranking_hints[0].hint.id == "approved-ranking" {
        observations.push("only_approved_ranking_hint_is_decision_eligible".to_string());
    }
    if profile_hints.len() == 1 && profile_hints[0].hint.id == "profile-candidate" {
        observations.push("profile_candidate_hint_is_separate_from_ranking".to_string());
    }
    if memory_off_hints.is_empty() {
        observations.push("memory_off_blocks_decision_hints".to_string());
    }
    if suppressed_hints.is_empty() {
        observations.push("suppressed_scope_blocks_profile_drift".to_string());
    }
    Ok(observations)
}

fn package_contract_findings(package: &IssueTaskPackage) -> Vec<String> {
    let mut findings = Vec::new();
    if package.outcome_contract.is_null() {
        findings.push("missing_outcome_contract".to_string());
    }
    let required_artifact = package
        .outcome_contract
        .get("requiredArtifact")
        .and_then(Value::as_str);
    let required_fields = package
        .outcome_contract
        .get("requiredFields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let has_summary = required_fields
        .iter()
        .any(|field| field.as_str() == Some("summary"));
    let has_status = required_fields
        .iter()
        .any(|field| field.as_str() == Some("status"));
    if required_artifact != Some("fix_result.json") || !has_summary || !has_status {
        findings.push("required_fix_result_contract_unavailable".to_string());
    }
    findings
}

fn create_packaged_task(
    runtime: &crate::dispatch::DispatchRuntime,
    number: u64,
) -> Result<crate::dispatch::IssueTask> {
    let task = runtime.store().upsert_issue_task(NewIssueTask {
        repo_full_name: "owner/repo".to_string(),
        issue_number: number,
        title: "Fix parser panic".to_string(),
        url: format!("https://github.com/owner/repo/issues/{number}"),
        status: IssueTaskStatus::UserApproved,
        priority: Some(10),
        category: Some("high_value_ready".to_string()),
    })?;
    let package = IssueTaskPackage::new(IssueTaskPackageIssue {
        repo_full_name: "owner/repo".to_string(),
        number,
        title: "Fix parser panic".to_string(),
        url: format!("https://github.com/owner/repo/issues/{number}"),
    });
    runtime
        .store()
        .write_task_package_artifact(&task.id, &package)?;
    runtime.store().get_issue_task(&task.id)
}

fn seed_memory_dream(store: &MemoryStore) -> Result<()> {
    store.insert_dream_run(&MemoryDreamRun {
        id: "agent-loop-dream-run".to_string(),
        trigger: MemoryDreamTrigger::Manual,
        scope: MemoryDreamScope::Global,
        input_activation_run_ids_json: json!([]),
        model_status: MemoryModelStatus::Disabled,
        created_at: Utc::now().to_rfc3339(),
    })?;
    store.insert_dream(&NewMemoryDream {
        id: "agent-loop-dream".to_string(),
        dream_run_id: "agent-loop-dream-run".to_string(),
        dream_type: MemoryDreamType::ProfileAdjustment,
        summary: "agent loop eval seed".to_string(),
        evidence_node_ids_json: json!([]),
        evidence_event_ids_json: json!([]),
        evidence_hint_ids_json: json!([]),
        status: MemoryDreamStatus::Candidate,
        confidence: 0.5,
        version: 1,
        created_at: Utc::now().to_rfc3339(),
        reviewed_at: None,
    })?;
    Ok(())
}

fn seed_memory_hint(
    store: &MemoryStore,
    id: &str,
    hint_type: MemoryHintType,
    status: MemoryHintStatus,
    weight: f64,
) -> Result<()> {
    store.insert_hint(&NewMemoryHint {
        id: id.to_string(),
        dream_id: "agent-loop-dream".to_string(),
        hint_type,
        scope_type: MemoryHintScopeType::Repo,
        scope_ref: "owner/repo".to_string(),
        summary: format!("{id} hint"),
        policy_json: json!({ "source": "agent_loop_eval" }),
        weight,
        status,
        created_at: Utc::now().to_rfc3339(),
        approved_at: status
            .is_active_decision_status()
            .then(|| Utc::now().to_rfc3339()),
        expires_at: None,
    })?;
    Ok(())
}

fn repo_scope(scope_ref: &str) -> MemoryHintScope {
    MemoryHintScope {
        scope_type: MemoryHintScopeType::Repo,
        scope_ref: scope_ref.to_string(),
    }
}

fn metrics_for(samples: &[AgentLoopSampleResult]) -> AgentLoopEvalMetrics {
    let mut families = BTreeMap::<String, AgentLoopFamilyMetrics>::new();
    for sample in samples {
        let entry = families.entry(sample.family.clone()).or_default();
        entry.total += 1;
        if sample.passed {
            entry.passed += 1;
        } else {
            entry.failed += 1;
        }
    }
    AgentLoopEvalMetrics {
        total_samples: samples.len(),
        passed_samples: samples.iter().filter(|sample| sample.passed).count(),
        failed_samples: samples.iter().filter(|sample| !sample.passed).count(),
        families,
    }
}

fn render_markdown_report(report: &AgentLoopEvalReport) -> String {
    let mut lines = vec![
        "# Agent Loop Eval".to_string(),
        String::new(),
        "Generated from deterministic offline fixtures using temp Issue Finder state and fake adapters.".to_string(),
        String::new(),
        format!("- Total samples: {}", report.metrics.total_samples),
        format!("- Passed: {}", report.metrics.passed_samples),
        format!("- Failed: {}", report.metrics.failed_samples),
        String::new(),
        "## Families".to_string(),
        String::new(),
    ];
    for (family, metrics) in &report.metrics.families {
        lines.push(format!(
            "- {family}: {}/{} passed",
            metrics.passed, metrics.total
        ));
    }
    lines.extend([String::new(), "## Samples".to_string(), String::new()]);
    for sample in &report.samples {
        let status = if sample.passed { "pass" } else { "fail" };
        lines.push(format!(
            "- `{}` [{}] {}: {}",
            sample.id,
            status,
            sample.family,
            sample.observations.join(", ")
        ));
        if !sample.failures.is_empty() {
            lines.push(format!("  Failures: {}", sample.failures.join("; ")));
        }
    }
    lines.push(String::new());
    lines.join("\n")
}

struct EvalHome {
    paths: IssueFinderPaths,
}

impl EvalHome {
    fn new(name: &str) -> Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "issue-finder-agent-loop-eval-{name}-{}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default(),
            EVAL_HOME_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let paths = IssueFinderPaths {
            home: root.clone(),
            config: root.join("config.toml"),
            cache_dir: root.join("cache"),
            workspaces_dir: root.join("workspaces"),
            inbox_dir: root.join("inbox"),
            reports_dir: root.join("reports"),
        };
        paths.ensure_layout()?;
        Ok(Self { paths })
    }
}

impl Drop for EvalHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.paths.home);
    }
}

#[derive(Default)]
struct FakeGitHubWriter {
    calls: Vec<String>,
    failures_remaining: usize,
}

impl GitHubCommentWriter for FakeGitHubWriter {
    fn post_issue_comment(
        &mut self,
        repo_full_name: &str,
        issue_number: u64,
        body: &str,
    ) -> Result<PostedGitHubComment> {
        self.calls
            .push(format!("{repo_full_name}#{issue_number}:{body}"));
        if self.failures_remaining > 0 {
            self.failures_remaining -= 1;
            anyhow::bail!("transient GitHub failure");
        }
        Ok(PostedGitHubComment {
            id: format!("comment-{}", self.calls.len()),
            url: format!(
                "https://github.com/{repo_full_name}/issues/{issue_number}#issuecomment-{}",
                self.calls.len()
            ),
        })
    }
}

#[derive(Default)]
struct FakeNativeAdapter {
    calls: Vec<String>,
}

impl NativeExecutionAdapter for FakeNativeAdapter {
    fn adapter_start_session(
        &mut self,
        request: AdapterStartSessionRequest,
    ) -> Result<AdapterSession> {
        self.calls.push("start_session".to_string());
        Ok(AdapterSession {
            native_session_id: "native_started".to_string(),
            display_name: Some(request.display_name),
            goal: request.goal,
            metadata_json: request.metadata_json,
        })
    }

    fn adapter_resume_session(&mut self, native_session_id: &str) -> Result<AdapterSession> {
        self.calls
            .push(format!("resume_session:{native_session_id}"));
        Ok(existing_session(native_session_id))
    }

    fn adapter_fork_session(&mut self, native_session_id: &str) -> Result<AdapterSession> {
        self.calls.push(format!("fork_session:{native_session_id}"));
        Ok(AdapterSession {
            native_session_id: format!("{native_session_id}_fork"),
            display_name: Some(format!("fork of {native_session_id}")),
            goal: Some("Forked goal".to_string()),
            metadata_json: json!({ "source": "fork" }),
        })
    }

    fn adapter_rename_session(
        &mut self,
        native_session_id: &str,
        display_name: &str,
    ) -> Result<AdapterSession> {
        self.calls
            .push(format!("rename_session:{native_session_id}"));
        Ok(AdapterSession {
            display_name: Some(display_name.to_string()),
            ..existing_session(native_session_id)
        })
    }

    fn adapter_set_goal(&mut self, native_session_id: &str, goal: &str) -> Result<AdapterSession> {
        self.calls.push(format!("set_goal:{native_session_id}"));
        Ok(AdapterSession {
            goal: Some(goal.to_string()),
            ..existing_session(native_session_id)
        })
    }

    fn adapter_set_metadata(
        &mut self,
        native_session_id: &str,
        metadata_json: Value,
    ) -> Result<AdapterSession> {
        self.calls.push(format!("set_metadata:{native_session_id}"));
        Ok(AdapterSession {
            metadata_json,
            ..existing_session(native_session_id)
        })
    }

    fn adapter_start_turn(
        &mut self,
        native_session_id: &str,
        _prompt: &str,
    ) -> Result<AdapterTurn> {
        self.calls.push(format!("start_turn:{native_session_id}"));
        Ok(AdapterTurn {
            native_turn_id: "turn-agent-loop-eval".to_string(),
            status: None,
        })
    }

    fn adapter_read_transcript(&mut self, native_session_id: &str) -> Result<Value> {
        self.calls
            .push(format!("read_transcript:{native_session_id}"));
        Ok(json!({ "thread": { "id": native_session_id }, "turns": [], "items": [] }))
    }

    fn adapter_archive_session(&mut self, native_session_id: &str) -> Result<AdapterSession> {
        self.calls
            .push(format!("archive_session:{native_session_id}"));
        Ok(existing_session(native_session_id))
    }

    fn adapter_list_sessions(&mut self, _limit: Option<usize>) -> Result<Vec<AdapterSession>> {
        Ok(Vec::new())
    }

    fn adapter_search_sessions(
        &mut self,
        _search_term: &str,
        _limit: Option<usize>,
    ) -> Result<Vec<AdapterSession>> {
        Ok(Vec::new())
    }
}

fn existing_session(native_session_id: &str) -> AdapterSession {
    AdapterSession {
        native_session_id: native_session_id.to_string(),
        display_name: None,
        goal: None,
        metadata_json: Value::Null,
    }
}
