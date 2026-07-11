use issue_finder::dispatch::native_runtime::{
    NativeThreadManager, NativeThreadStore, SendTurnRequest,
};
use issue_finder::paths::IssueFinderPaths;
use std::{fs, thread, time::Duration};

#[test]
#[ignore = "requires the installed Codex daemon and authenticated desktop state"]
fn real_daemon_runtime_round_trip_and_reconcile() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let root = std::env::temp_dir().join("issue-finder-native-runtime-e2e");
        fs::create_dir_all(&root).unwrap();
        let state = root.join("state");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let paths = test_paths(state);
        let store = NativeThreadStore::open(&paths).unwrap();
        let mut manager = NativeThreadManager::connect(store).await.unwrap();
        let marker = format!("IF-NATIVE-RUNTIME-{}", chrono::Utc::now().timestamp());
        let thread_id = manager
            .start_thread(
                &format!("Issue Finder native runtime {marker}"),
                workspace.to_str().unwrap(),
            )
            .await
            .unwrap();
        let started = manager
            .send(SendTurnRequest {
                thread_id: thread_id.clone(),
                prompt: format!(
                    "Write a 5000-word explanation of the marker `{marker}` without tools."
                ),
                cwd: workspace.to_string_lossy().to_string(),
                client_user_message_id: format!("client-{marker}"),
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        manager
            .steer(
                &thread_id,
                &started.turn_id,
                &format!("Replace the draft with exactly `{marker} STEERED`."),
                &format!("client-{marker}-steer"),
            )
            .await
            .unwrap();
        for _ in 0..30 {
            thread::sleep(Duration::from_secs(1));
            manager.reconcile(&thread_id).await.unwrap();
            let check = NativeThreadStore::open(&paths).unwrap();
            if check.items(&thread_id).unwrap().iter().any(|item| {
                item.payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .is_some_and(|text| text.contains(&format!("{marker} STEERED")))
            }) {
                manager
                    .reconnect(std::slice::from_ref(&thread_id))
                    .await
                    .unwrap();
                manager.reconcile(&thread_id).await.unwrap();
                println!("threadId={thread_id} turnId={}", started.turn_id);
                return;
            }
        }
        panic!("timed out waiting for native runtime response");
    });
}

#[test]
#[ignore = "requires the installed Codex daemon and authenticated desktop state"]
fn real_daemon_approval_request_is_persisted_and_declined() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let root = std::env::temp_dir().join("issue-finder-native-approval-e2e");
        let state = root.join("state");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let paths = test_paths(state);
        let store = NativeThreadStore::open(&paths).unwrap();
        let mut manager = NativeThreadManager::connect(store).await.unwrap();
        let marker = format!("IF-APPROVAL-{}", chrono::Utc::now().timestamp());
        let thread_id = manager
            .start_thread(
                &format!("Issue Finder approval {marker}"),
                workspace.to_str().unwrap(),
            )
            .await
            .unwrap();
        let turn = manager
            .send(SendTurnRequest {
                thread_id: thread_id.clone(),
                prompt: "Use the request_permissions tool to request network access, then wait for the decision. Do not edit files.".to_string(),
                cwd: workspace.to_string_lossy().to_string(),
                client_user_message_id: format!("client-{marker}"),
            })
            .await
            .unwrap();
        let deadline=std::time::Instant::now()+Duration::from_secs(30);
        while std::time::Instant::now()<deadline {
            let _ = tokio::time::timeout(Duration::from_secs(2), manager.pump_once()).await;
            let check = NativeThreadStore::open(&paths).unwrap();
            if let Some(request) = check
                .pending_server_requests()
                .unwrap()
                .into_iter()
                .find(|request| request.method.contains("requestApproval"))
            {
                let id = serde_json::from_str(&request.id).unwrap();
                manager.decide_approval(id, "decline").await.unwrap();
                let _ = manager.interrupt(&thread_id, &turn.turn_id).await;
                println!(
                    "approvalThreadId={thread_id} approvalTurnId={}",
                    turn.turn_id
                );
                return;
            }
        }
        panic!("expected a real Codex approval server request");
    });
}

fn test_paths(home: std::path::PathBuf) -> IssueFinderPaths {
    IssueFinderPaths {
        config: home.join("config.toml"),
        cache_dir: home.join("cache"),
        workspaces_dir: home.join("workspaces"),
        inbox_dir: home.join("inbox"),
        reports_dir: home.join("reports"),
        home,
    }
}
