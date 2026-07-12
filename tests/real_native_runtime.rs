use issue_finder::dispatch::native_runtime::{
    NativeThreadManager, NativeThreadStore, SendTurnRequest,
};
use issue_finder::paths::IssueFinderPaths;
use std::{fs, time::Duration};

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
                    "Reply with exactly `{marker} RECOVERED`. Do not use tools or edit files."
                ),
                cwd: workspace.to_string_lossy().to_string(),
                client_user_message_id: format!("client-{marker}"),
            })
            .await
            .unwrap();
        // Replace the live client while the turn is in flight. The daemon-owned turn
        // must be recovered by reconciliation without another client message.
        manager
            .reconnect(std::slice::from_ref(&thread_id))
            .await
            .unwrap();
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            manager.reconcile(&thread_id).await.unwrap();
            let check = NativeThreadStore::open(&paths).unwrap();
            if check.items(&thread_id).unwrap().iter().any(|item| {
                item.item_type == "agentMessage"
                    && item
                        .payload
                        .to_string()
                        .contains(&format!("{marker} RECOVERED"))
            }) {
                manager.reconcile(&thread_id).await.unwrap();
                println!("threadId={thread_id} turnId={}", started.turn_id);
                return;
            }
            if let Some(turn) = check.latest_turn(&thread_id).unwrap() {
                if matches!(turn.status.as_str(), "interrupted" | "failed" | "canceled") {
                    let items = check.items(&thread_id).unwrap();
                    assert!(items.iter().any(|item| {
                        item.item_type == "userMessage"
                            && item.payload.to_string().contains(&marker)
                    }));
                    assert_eq!(
                        items
                            .iter()
                            .filter(|item| {
                                item.item_type == "userMessage"
                                    && item.payload.to_string().contains(&marker)
                            })
                            .count(),
                        1,
                        "disconnect recovery must not duplicate the submitted user turn"
                    );
                    println!(
                        "threadId={thread_id} turnId={} recoveredAs=needs_user status={}",
                        started.turn_id, turn.status
                    );
                    return;
                }
            }
        }
        panic!("timed out waiting for native runtime response");
    });
}

#[test]
#[ignore = "requires the installed Codex daemon and authenticated desktop state"]
fn real_daemon_resumes_selected_thread_without_creating_another_thread() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let marker = format!("IF-NATIVE-RESUME-{}", chrono::Utc::now().timestamp_millis());
        let root = std::env::temp_dir().join(&marker);
        let state = root.join("state");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let paths = test_paths(state);
        let store = NativeThreadStore::open(&paths).unwrap();
        let mut manager = NativeThreadManager::connect(store).await.unwrap();
        let selected_thread_id = manager
            .start_thread(
                &format!("Issue Finder native resume {marker}"),
                workspace.to_str().unwrap(),
            )
            .await
            .unwrap();

        manager
            .send(SendTurnRequest {
                thread_id: selected_thread_id.clone(),
                prompt: format!(
                    "Reply with exactly `{marker} SEED`. Do not use tools or edit files."
                ),
                cwd: workspace.to_string_lossy().to_string(),
                client_user_message_id: format!("client-{marker}-seed"),
            })
            .await
            .unwrap();
        assert!(
            wait_for_agent_marker(
                &manager,
                &paths,
                &selected_thread_id,
                &format!("{marker} SEED")
            )
            .await,
            "seed turn did not produce a persisted agent response"
        );

        manager
            .reconnect(std::slice::from_ref(&selected_thread_id))
            .await
            .unwrap();
        let turn = manager
            .send(SendTurnRequest {
                thread_id: selected_thread_id.clone(),
                prompt: format!(
                    "Reply with exactly `{marker} RESUMED`. Do not use tools or edit files."
                ),
                cwd: workspace.to_string_lossy().to_string(),
                client_user_message_id: format!("client-{marker}"),
            })
            .await
            .unwrap();

        assert!(
            wait_for_agent_marker(
                &manager,
                &paths,
                &selected_thread_id,
                &format!("{marker} RESUMED")
            )
            .await,
            "timed out waiting for resumed native thread response"
        );
        assert_eq!(turn.thread_id, selected_thread_id);
        println!(
            "resumedThreadId={selected_thread_id} resumedTurnId={}",
            turn.turn_id
        );
    });
}

async fn wait_for_agent_marker(
    manager: &NativeThreadManager,
    paths: &IssueFinderPaths,
    thread_id: &str,
    marker: &str,
) -> bool {
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        manager.reconcile(thread_id).await.unwrap();
        let check = NativeThreadStore::open(paths).unwrap();
        if check.items(thread_id).unwrap().iter().any(|item| {
            item.item_type == "agentMessage" && item.payload.to_string().contains(marker)
        }) {
            return true;
        }
    }
    false
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
                let id = request.wire_id;
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
