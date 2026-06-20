use issue_finder::dispatch::policy::{classify_action, ensure_capability_preconditions};
use issue_finder::dispatch::{
    AgentCapabilityName, ApprovalType, CapabilityStatus, DispatchRuntime, NewAgentCapability,
    PolicyAction, PolicyRequirement,
};
use issue_finder::paths::IssueFinderPaths;
use serde_json::json;
use tempfile::tempdir;

#[test]
fn dispatch_policy_classifies_approval_matrix_and_forbidden_actions() {
    let start = classify_action(PolicyAction::StartDispatch);
    assert_eq!(start.requirement, PolicyRequirement::RequiresApproval);
    assert_eq!(start.approval_type, Some(ApprovalType::Dispatch));
    assert!(start
        .required_capabilities
        .contains(&AgentCapabilityName::StartSession));

    let read = classify_action(PolicyAction::ReadSessionTranscript);
    assert_eq!(read.requirement, PolicyRequirement::Allowed);
    assert_eq!(read.approval_type, None);

    let open_pr = classify_action(PolicyAction::OpenPr);
    assert_eq!(open_pr.requirement, PolicyRequirement::Forbidden);
    assert_eq!(open_pr.approval_type, Some(ApprovalType::OpenPr));
}

#[test]
fn dispatch_policy_enforces_capability_preconditions() {
    let dir = tempdir().unwrap();
    let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
    runtime
        .store()
        .upsert_agent_capability(NewAgentCapability {
            agent_id: "codex".to_string(),
            capability: AgentCapabilityName::StartSession,
            status: CapabilityStatus::Unsupported,
            details_json: json!({ "reason": "test" }),
        })
        .unwrap();

    let decision = classify_action(PolicyAction::StartDispatch);
    let error = ensure_capability_preconditions(runtime.store(), "codex", &decision).unwrap_err();
    assert!(error
        .to_string()
        .contains("does not support capability start_session"));
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
