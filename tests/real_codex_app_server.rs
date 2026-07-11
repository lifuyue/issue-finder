use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use issue_finder::dispatch::adapters::codex_app_server::{
    CodexAppServerAdapter, CodexAppServerStdioTransport, CodexStartSessionRequest,
};
use serde_json::Value;

#[test]
#[ignore = "requires the installed Codex app-server and authenticated desktop state"]
fn real_codex_thread_round_trip() {
    let workspace = std::env::var("ISSUE_FINDER_CODEX_E2E_WORKSPACE")
        .unwrap_or_else(|_| "/tmp/issue-finder-codex-e2e-workspace".to_string());
    fs::create_dir_all(&workspace).unwrap();
    let marker = std::env::var("ISSUE_FINDER_CODEX_E2E_MARKER")
        .unwrap_or_else(|_| format!("IF-CODEX-E2E-{}", chrono::Utc::now().timestamp()));
    let name = format!("Issue Finder app-server E2E {marker}");

    let transport = CodexAppServerStdioTransport::connect().unwrap();
    let mut adapter = CodexAppServerAdapter::new(transport);
    let session = adapter
        .start_session(CodexStartSessionRequest {
            name: Some(name.clone()),
            goal: None,
            metadata: Value::Null,
            cwd: workspace.clone(),
        })
        .unwrap();
    let prompt = format!(
        "Issue Finder transport acceptance test. Reply with exactly `{marker} ACK`. Do not use tools, edit files, or perform any external action."
    );
    let turn = adapter
        .start_turn(
            &session.thread_id,
            &prompt,
            &workspace,
            &format!("issue-finder-e2e-{marker}"),
        )
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(120);
    let transcript = loop {
        let transcript = adapter.read_transcript(&session.thread_id).unwrap();
        let has_user = transcript.items.iter().any(|item| {
            item.item_type == "userMessage"
                && item
                    .text
                    .as_deref()
                    .is_some_and(|text| text.contains(&marker))
        });
        let has_response = transcript.items.iter().any(|item| {
            item.item_type == "agentMessage"
                && item
                    .text
                    .as_deref()
                    .is_some_and(|text| text.contains(&marker))
        });
        if has_user && has_response {
            break transcript;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for Codex response"
        );
        thread::sleep(Duration::from_secs(2));
    };

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "binary": issue_finder::dispatch::adapters::codex_app_server::discover_codex_binary().unwrap(),
            "threadId": session.thread_id,
            "turnId": turn.turn_id,
            "name": name,
            "workspace": workspace,
            "marker": marker,
            "turns": transcript.turns,
            "items": transcript.items,
        }))
        .unwrap()
    );
}
