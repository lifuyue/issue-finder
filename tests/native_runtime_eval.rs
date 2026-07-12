#[cfg(unix)]
mod unix {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    use tempfile::tempdir;

    #[test]
    fn native_runtime_eval_requires_marker_bearing_model_response() {
        let dir = tempdir().unwrap();
        let script = dir.path().join("fake-codex");
        fs::write(
            &script,
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "codex-cli eval-test"; exit 0; fi
if [ "$1" = "app-server" ] && [ "$2" = "--help" ]; then exit 0; fi
if [ "$1" = "app-server" ] && [ "$2" = "--stdio" ]; then
  read line
  printf '%s\n' '{"id":0,"result":{}}'
  read line
  read line
  printf '%s\n' '{"id":1,"result":{"thread":{"id":"thread-eval","name":null,"status":"idle"}}}'
  read line
  printf '%s\n' '{"id":2,"result":{"turn":{"id":"turn-eval","status":"running"}}}'
  read line
  printf '%s\n' '{"id":3,"result":{"thread":{"id":"thread-eval","status":"idle","turns":[{"id":"turn-eval","status":"completed","items":[{"id":"user-1","type":"userMessage","text":"NATIVE-MOCK"},{"id":"agent-1","type":"agentMessage","text":"NATIVE-MOCK ACK"}]}]}}}'
  exit 0
fi
exit 64
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        let workspace = dir.path().join("workspace");

        let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
            .args([
                "eval",
                "native-runtime",
                "--workspace",
                workspace.to_str().unwrap(),
                "--timeout-seconds",
                "2",
                "--marker",
                "NATIVE-MOCK",
            ])
            .env("ISSUE_FINDER_CODEX_BIN", &script)
            .env("ISSUE_FINDER_CODEX_TRANSPORT", "stdio")
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["businessOutcome"]["state"], "completed");
        assert_eq!(report["protocolHandshake"], true);
        assert_eq!(report["authenticatedModelTurn"], true);
        assert_eq!(report["threadId"], "thread-eval");
        assert_eq!(report["turnId"], "turn-eval");
        assert_eq!(report["agentMarkerObserved"], true);
    }
}
