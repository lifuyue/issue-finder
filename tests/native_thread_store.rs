use issue_finder::dispatch::native_runtime::NativeThreadStore;
use issue_finder::paths::IssueFinderPaths;
use serde_json::json;
use tempfile::tempdir;

#[test]
fn native_store_projects_thread_turn_item_and_outbox_into_one_database() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path().to_path_buf());
    let store = NativeThreadStore::open(&paths).unwrap();
    store
        .upsert_thread(&json!({"id":"thread-1","name":"test","cwd":"/tmp/test","status":"idle"}))
        .unwrap();
    store
        .upsert_turn("thread-1", &json!({"id":"turn-1","status":"completed"}))
        .unwrap();
    store
        .upsert_item(
            "thread-1",
            Some("turn-1"),
            &json!({"id":"item-1","type":"agentMessage","text":"ok"}),
        )
        .unwrap();
    store
        .enqueue(
            "outbox-1",
            "thread-1",
            "turn/start",
            Some("client-1"),
            &json!({"input":[]}),
        )
        .unwrap();
    store.mark_sent("outbox-1", Some("turn-1")).unwrap();
    assert_eq!(
        store.thread("thread-1").unwrap().unwrap().name.as_deref(),
        Some("test")
    );
    assert_eq!(store.items("thread-1").unwrap()[0].payload["text"], "ok");
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
