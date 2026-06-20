use issue_finder::dispatch::{
    AgentCapabilityName, CapabilityStatus, DispatchRuntime, NewAgentCapability, NewAgentProfile,
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
