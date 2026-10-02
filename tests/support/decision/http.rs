use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// One bounded, loopback-only native JSON exchange shared by provider mapping tests.
pub async fn serve_json(path: &str, response: Value) -> (String, JoinHandle<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}{path}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(3), async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            let header_end = loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert!(count > 0, "client closed before sending headers");
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(position) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    break position + 4;
                }
            };
            let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
            let length: usize = headers.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            }).unwrap();
            while bytes.len() < header_end + length {
                let count = stream.read(&mut buffer).await.unwrap();
                assert!(count > 0, "client closed before sending body");
                bytes.extend_from_slice(&buffer[..count]);
            }
            let request = serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
            let body = response.to_string();
            stream.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            ).as_bytes()).await.unwrap();
            (headers, request)
        }).await.expect("native mock exchange timed out")
    });
    (endpoint, task)
}
