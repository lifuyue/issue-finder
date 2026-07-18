use issue_finder::tool_specs::{list_tool_specs_for_profile, ToolProfile};

#[test]
fn worker_catalog_exposes_exactly_the_two_task_local_tools() {
    let worker = list_tool_specs_for_profile(ToolProfile::Worker);
    let names = worker
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["read_context", "submit_result"]);
    assert!(!names.iter().any(|name| name.contains("dispatch")));
    assert!(!names.iter().any(|name| name.contains("github")));
}

#[test]
fn control_catalog_does_not_expose_worker_result_submission() {
    let control = list_tool_specs_for_profile(ToolProfile::Control);
    assert!(!control
        .tools
        .iter()
        .any(|tool| tool.name == "submit_result"));
}
