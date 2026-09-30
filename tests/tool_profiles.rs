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

#[test]
fn default_mcp_keeps_catalog_available_and_returns_business_config_errors() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("config.toml"), "github = [").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .arg("mcp")
        .env("ISSUE_FINDER_HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut input = child.stdin.take().unwrap();
        for request in [
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"scout","arguments":{}}}),
            serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"prepare","arguments":{}}}),
        ] {
            writeln!(input, "{request}").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let responses: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 3);
    let names: Vec<_> = responses[0]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["scout", "assess"]);
    let failure = &responses[1]["result"];
    assert_eq!(failure["isError"], true);
    assert!(failure["structuredContent"]["error"]["message"]
        .as_str()
        .unwrap()
        .contains("config"));
    assert_eq!(responses[2]["result"]["isError"], true);
    assert_eq!(
        responses[2]["result"]["structuredContent"]["status"],
        "forbidden_tool"
    );
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
}
