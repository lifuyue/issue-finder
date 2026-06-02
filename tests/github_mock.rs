use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use patchbay_cli::config::Config;
use patchbay_cli::paths::PatchbayPaths;
use patchbay_cli::workflow;
use tempfile::tempdir;

#[tokio::test]
async fn scout_uses_mocked_github_search_responses() {
    let (base_url, handle) = start_mock_github();
    std::env::set_var("PATCHBAY_GITHUB_API_BASE", &base_url);

    let dir = tempdir().unwrap();
    let paths = PatchbayPaths {
        home: dir.path().to_path_buf(),
        config: dir.path().join("config.toml"),
        cache_dir: dir.path().join("cache"),
        workspaces_dir: dir.path().join("workspaces"),
        inbox_dir: dir.path().join("inbox"),
        reports_dir: dir.path().join("reports"),
    };
    let config = Config::default();

    let ranked = workflow::scout(&paths, &config, 10, true).await.unwrap();

    std::env::remove_var("PATCHBAY_GITHUB_API_BASE");
    handle.join().unwrap();

    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].issue.repo_full_name, "owner/repo");
    assert_eq!(ranked[0].issue.number, 12);
    assert!(ranked[0].score > 0);
}

fn start_mock_github() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let base_url_for_thread = base_url.clone();
    let search_count = Arc::new(AtomicUsize::new(0));
    let search_count_for_thread = Arc::clone(&search_count);

    let handle = thread::spawn(move || {
        let started = Instant::now();
        let mut served = 0usize;

        while served < 3 && started.elapsed() < Duration::from_secs(5) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buffer = [0u8; 4096];
                    let bytes_read = stream.read(&mut buffer).unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
                    let body = if request.starts_with("GET /search/issues") {
                        let count = search_count_for_thread.fetch_add(1, Ordering::SeqCst);
                        if count == 0 {
                            search_body(&base_url_for_thread)
                        } else {
                            "{\"items\":[]}".to_string()
                        }
                    } else if request.starts_with("GET /repos/owner/repo") {
                        repo_body()
                    } else {
                        "{\"message\":\"not found\"}".to_string()
                    };
                    write_response(&mut stream, &body);
                    served += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

    (base_url, handle)
}

fn search_body(base_url: &str) -> String {
    format!(
        r#"{{
  "items": [
    {{
      "id": 99,
      "number": 12,
      "title": "Fix Rust CLI parser",
      "body": "Expected graceful behavior in src/main.rs",
      "html_url": "https://github.com/owner/repo/issues/12",
      "repository_url": "{base_url}/repos/owner/repo",
      "labels": [{{ "name": "good first issue" }}],
      "locked": false,
      "created_at": "2026-06-01T00:00:00Z",
      "updated_at": "2026-06-02T00:00:00Z"
    }}
  ]
}}"#
    )
}

fn repo_body() -> String {
    r#"{
  "full_name": "owner/repo",
  "name": "repo",
  "description": "Rust CLI developer tools",
  "stargazers_count": 123,
  "archived": false
}"#
    .to_string()
}

fn write_response(stream: &mut std::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).unwrap();
}
