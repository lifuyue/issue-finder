use anyhow::Result;
use issue_finder::dispatch::adapters::codex_app_server::{
    codex_app_server_startup_metadata, dispatch_prompt, CodexAppServerAdapter,
    CodexAppServerConnectionMode, CodexAppServerStdioTransport, CodexAppServerTransport,
    CodexStartSessionRequest,
};
use issue_finder::dispatch::AgentCapabilityName;
use serde_json::{json, Value};
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use tempfile::tempdir;

#[test]
fn codex_adapter_declares_native_thread_capability_mapping_without_pr_creation() {
    let mappings = CodexAppServerAdapter::<FakeTransport>::capability_mappings();

    assert!(mappings.iter().any(|mapping| {
        mapping.capability == AgentCapabilityName::StartSession && mapping.method == "thread/start"
    }));
    assert!(mappings.iter().any(|mapping| {
        mapping.capability == AgentCapabilityName::ResumeSession
            && mapping.method == "thread/resume"
    }));
    assert!(mappings.iter().any(|mapping| {
        mapping.capability == AgentCapabilityName::ForkSession && mapping.method == "thread/fork"
    }));
    assert!(mappings.iter().any(|mapping| {
        mapping.capability == AgentCapabilityName::RenameSession
            && mapping.method == "thread/name/set"
    }));
    assert!(mappings.iter().any(|mapping| {
        mapping.capability == AgentCapabilityName::ReadTranscript && mapping.method == "thread/read"
    }));
    assert!(mappings.iter().any(|mapping| {
        mapping.capability == AgentCapabilityName::ArchiveSession
            && mapping.method == "thread/archive"
    }));
    assert!(!mappings
        .iter()
        .any(|mapping| mapping.capability == AgentCapabilityName::OpenPr));
}

#[test]
fn codex_adapter_session_operations_call_json_rpc_thread_methods() {
    let mut adapter = CodexAppServerAdapter::new(FakeTransport::default());

    let started = adapter
        .start_session(CodexStartSessionRequest {
            name: Some("issue-finder: owner/repo#123".to_string()),
            goal: Some("Fix owner/repo#123".to_string()),
            metadata: json!({ "issueKey": "owner/repo#123" }),
        })
        .unwrap();
    assert_eq!(started.thread_id, "thread_started");
    assert_eq!(
        started.name.as_deref(),
        Some("issue-finder: owner/repo#123")
    );

    let resumed = adapter.resume_session("thread_existing").unwrap();
    assert_eq!(resumed.thread_id, "thread_existing");

    let forked = adapter.fork_session("thread_existing").unwrap();
    assert_eq!(forked.thread_id, "thread_forked");

    let renamed = adapter
        .rename_session("thread_existing", "issue-finder: renamed")
        .unwrap();
    assert_eq!(renamed.name.as_deref(), Some("issue-finder: renamed"));

    let archived = adapter.archive_session("thread_existing").unwrap();
    assert_eq!(archived.metadata["archived"], true);

    let turn = adapter
        .start_turn("thread_existing", dispatch_prompt())
        .unwrap();
    assert_eq!(turn.turn_id, "turn_started");

    let transport = adapter.into_transport();
    assert_eq!(
        transport.methods(),
        vec![
            "thread/start",
            "thread/name/set",
            "thread/goal/set",
            "thread/metadata/update",
            "thread/resume",
            "thread/fork",
            "thread/name/set",
            "thread/archive",
            "turn/start"
        ]
    );
    assert_eq!(transport.calls[0].1["threadSource"], "issue_finder");
    assert_eq!(
        transport.calls[3].1["metadata"]["issueKey"],
        "owner/repo#123"
    );
    assert!(transport.calls[8].1["input"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Issue Finder task package"));
}

#[test]
fn codex_adapter_reads_transcript_from_thread_turn_and_item_methods() {
    let mut adapter = CodexAppServerAdapter::new(FakeTransport::default());

    let transcript = adapter.read_transcript("thread_existing").unwrap();

    assert_eq!(transcript.thread.thread_id, "thread_existing");
    assert_eq!(transcript.turns.len(), 2);
    assert_eq!(transcript.items.len(), 2);
    assert_eq!(transcript.items[0].turn_id.as_deref(), Some("turn_1"));
    assert_eq!(transcript.items[0].text.as_deref(), Some("first response"));
    assert_eq!(transcript.items[1].turn_id.as_deref(), Some("turn_2"));

    let transport = adapter.into_transport();
    assert_eq!(
        transport.methods(),
        vec![
            "thread/read",
            "thread/turns/list",
            "thread/turns/items/list",
            "thread/turns/items/list"
        ]
    );
    assert_eq!(transport.calls[2].1["turnId"], "turn_1");
    assert_eq!(transport.calls[3].1["turnId"], "turn_2");
}

#[cfg(unix)]
#[test]
fn codex_stdio_transport_starts_daemon_and_uses_proxy_by_default() {
    let dir = tempdir().unwrap();
    let log_path = dir.path().join("calls.log");
    let script_path = dir.path().join("fake-codex");
    fs::write(
        &script_path,
        format!(
            r#"#!/bin/sh
echo "$@" >> "{log}"
if [ "$1" = "app-server" ] && [ "$2" = "daemon" ] && [ "$3" = "start" ]; then
  exit 0
fi
if [ "$1" = "app-server" ] && [ "$2" = "proxy" ]; then
  read line
  printf '%s\n' '{{"id":1,"result":{{}}}}'
  read line
  printf '%s\n' '{{"id":2,"result":{{"data":[]}}}}'
  while read line; do sleep 1; done
  exit 0
fi
exit 64
"#,
            log = log_path.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&script_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script_path, permissions).unwrap();

    let mut transport =
        CodexAppServerStdioTransport::connect_with_command(script_path.to_str().unwrap()).unwrap();
    assert_eq!(
        transport.connection_mode(),
        CodexAppServerConnectionMode::DaemonProxy
    );
    let response = transport.request("thread/list", json!({})).unwrap();
    assert_eq!(response["data"].as_array().unwrap().len(), 0);
    drop(transport);

    let calls = fs::read_to_string(log_path).unwrap();
    assert!(calls.contains("app-server daemon start"), "{calls}");
    assert!(calls.contains("app-server proxy"), "{calls}");
    assert!(!calls.contains("app-server --stdio"), "{calls}");
}

#[cfg(unix)]
#[test]
fn codex_startup_metadata_records_version_commands_and_supported_methods() {
    let dir = tempdir().unwrap();
    let script_path = dir.path().join("fake-codex");
    fs::write(
        &script_path,
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo "codex-cli 9.9.9"
  exit 0
fi
if [ "$1" = "app-server" ] && [ "$2" = "--help" ]; then
  echo "Usage: codex app-server [COMMAND]"
  exit 0
fi
if [ "$1" = "app-server" ] && [ "$2" = "daemon" ] && [ "$3" = "--help" ]; then
  echo "Commands: start version"
  exit 0
fi
if [ "$1" = "app-server" ] && [ "$2" = "proxy" ] && [ "$3" = "--help" ]; then
  echo "Proxy stdio bytes"
  exit 0
fi
if [ "$1" = "app-server" ] && [ "$2" = "daemon" ] && [ "$3" = "version" ]; then
  echo '{"cliVersion":"9.9.9","serverVersion":"9.9.9"}'
  exit 0
fi
exit 64
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&script_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script_path, permissions).unwrap();

    let metadata = codex_app_server_startup_metadata(script_path.to_str().unwrap());

    assert_eq!(metadata["binary"]["available"], true);
    assert_eq!(metadata["cliVersion"], "codex-cli 9.9.9");
    assert_eq!(metadata["appServerVersion"]["serverVersion"], "9.9.9");
    assert_eq!(metadata["connectionModes"][0]["mode"], "daemon_proxy");
    assert_eq!(metadata["connectionModes"][0]["available"], true);
    assert!(metadata["supportedMethods"]
        .as_array()
        .unwrap()
        .iter()
        .any(|method| method == "thread/start"));
    assert!(metadata["supportedMethods"]
        .as_array()
        .unwrap()
        .iter()
        .any(|method| method == "review/start"));
}

#[derive(Default)]
struct FakeTransport {
    calls: Vec<(String, Value)>,
}

impl FakeTransport {
    fn methods(&self) -> Vec<&str> {
        self.calls
            .iter()
            .map(|(method, _)| method.as_str())
            .collect()
    }
}

impl CodexAppServerTransport for FakeTransport {
    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.calls.push((method.to_string(), params.clone()));
        let response = match method {
            "thread/start" => json!({
                "thread": {
                    "id": "thread_started",
                    "name": null,
                    "metadata": {}
                }
            }),
            "thread/resume" | "thread/read" => json!({
                "thread": {
                    "id": params["threadId"],
                    "name": "issue-finder: owner/repo#123",
                    "metadata": {}
                }
            }),
            "thread/fork" => json!({
                "thread": {
                    "id": "thread_forked",
                    "name": "issue-finder: owner/repo#123 (fork)",
                    "metadata": { "forkedFrom": params["threadId"] }
                }
            }),
            "thread/name/set" => json!({}),
            "thread/archive" => json!({}),
            "thread/turns/list" => json!({
                "data": [
                    { "id": "turn_1", "status": "completed" },
                    { "id": "turn_2", "status": "running" }
                ]
            }),
            "thread/turns/items/list" => {
                let text = if params["turnId"] == "turn_1" {
                    "first response"
                } else {
                    "second response"
                };
                json!({
                    "data": [
                        {
                            "turnId": params["turnId"],
                            "type": "assistant_message",
                            "text": text,
                            "payload": { "source": "fake" }
                        }
                    ]
                })
            }
            "turn/start" => json!({
                "turn": {
                    "id": "turn_started",
                    "status": "running"
                }
            }),
            "thread/goal/set" => json!({
                "goal": params["goal"]
            }),
            "thread/metadata/update" => json!({
                "thread": {
                    "id": params["threadId"],
                    "metadata": params["metadata"]
                }
            }),
            _ => anyhow::bail!("unexpected method {method}"),
        };
        Ok(response)
    }
}
