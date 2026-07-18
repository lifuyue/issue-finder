use issue_finder::dispatch::{
    AdapterProbeStatus, AgentCapabilityName, CapabilityStatus, DispatchRuntime, NewAgentCapability,
    NewAgentProfile,
};
use issue_finder::paths::IssueFinderPaths;
use serde_json::json;
use tempfile::tempdir;

#[test]
fn capability_probe_reuses_successful_cache_until_refresh() {
    let dir = tempdir().unwrap();
    let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
    runtime
        .store()
        .create_agent_profile(NewAgentProfile {
            id: Some("fake".to_string()),
            kind: "fake".to_string(),
            display_name: "Fake Agent".to_string(),
            adapter: "fake_adapter".to_string(),
            config_json: json!({}),
            enabled: true,
        })
        .unwrap();
    runtime
        .store()
        .upsert_agent_capability(NewAgentCapability {
            agent_id: "fake".to_string(),
            capability: AgentCapabilityName::StartSession,
            status: CapabilityStatus::Supported,
            details_json: json!({
                "protocol": "fake",
                "method": "session/start"
            }),
        })
        .unwrap();

    let first = runtime.probe_agent("fake", true).unwrap();
    let cached = runtime.probe_agent("fake", false).unwrap();
    let refreshed = runtime.probe_agent("fake", true).unwrap();

    assert_eq!(first.probes.len(), 1);
    assert_eq!(cached.probes[0].id, first.probes[0].id);
    assert_ne!(refreshed.probes[0].id, first.probes[0].id);
    assert!(first.probes[0].expires_at.is_some());
}

#[test]
fn runtime_handshake_failure_does_not_rewrite_unsupported_product_policy() {
    let dir = tempdir().unwrap();
    let runtime = DispatchRuntime::open(test_paths(dir.path())).unwrap();
    runtime
        .store()
        .create_agent_profile(NewAgentProfile {
            id: Some("fake".to_string()),
            kind: "fake".to_string(),
            display_name: "Fake Agent".to_string(),
            adapter: "fake_adapter".to_string(),
            config_json: json!({}),
            enabled: true,
        })
        .unwrap();
    runtime
        .store()
        .upsert_agent_capability(NewAgentCapability {
            agent_id: "fake".to_string(),
            capability: AgentCapabilityName::OpenPr,
            status: CapabilityStatus::Unsupported,
            details_json: json!({
                "reason": "Issue Finder must not create pull requests",
                "startup": { "probe": { "status": "handshake_failed" } }
            }),
        })
        .unwrap();

    let report = runtime.probe_agent("fake", true).unwrap();

    assert_eq!(report.probes[0].status, AdapterProbeStatus::Unsupported);
    assert_eq!(
        report.probes[0].error_code.as_deref(),
        Some("capability_unsupported")
    );
}

#[test]
fn opening_runtime_does_not_rewrite_persisted_capability_audit_state() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let runtime = DispatchRuntime::open(paths.clone()).unwrap();
    runtime
        .store()
        .upsert_agent_capability(NewAgentCapability {
            agent_id: "codex".to_string(),
            capability: AgentCapabilityName::StartSession,
            status: CapabilityStatus::Experimental,
            details_json: json!({
                "protocol": "codex_app_server_json_rpc",
                "method": "thread/start",
                "auditMarker": "persisted-before-unrelated-command"
            }),
        })
        .unwrap();
    drop(runtime);

    let reopened = DispatchRuntime::open(paths).unwrap();
    let capability = reopened
        .store()
        .get_agent_capability("codex", AgentCapabilityName::StartSession)
        .unwrap();

    assert_eq!(
        capability.details_json["auditMarker"],
        "persisted-before-unrelated-command"
    );
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
