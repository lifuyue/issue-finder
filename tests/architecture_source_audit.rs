use std::path::Path;

#[test]
fn obsolete_agent_loop_paths_are_absent() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for obsolete in [
        "src/dispatch/execution.rs",
        "src/dispatch/task_package.rs",
        "src/dispatch/adapters/mod.rs",
        "src/dispatch/adapters/codex_app_server.rs",
        "src/dispatch/native_runtime/mod.rs",
        "src/dispatch/native_runtime/manager.rs",
        "src/dispatch/native_runtime/store.rs",
        "src/native_runtime_eval.rs",
    ] {
        assert!(
            !root.join(obsolete).exists(),
            "obsolete production path still exists: {obsolete}"
        );
    }
}

#[test]
fn public_tool_catalog_has_no_manual_outcome_or_compatibility_alias() {
    let catalog = issue_finder::tool_specs::list_tool_specs();
    let names = catalog
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert!(!names.contains(&"issue-finder.dispatch_record_outcome"));
    assert!(!names.contains(&"issue-finder.dispatch_propose"));
    assert!(!names.contains(&"issue-finder.submit_result"));
}
