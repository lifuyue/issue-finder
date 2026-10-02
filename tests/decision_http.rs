use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::join_all;
use issue_finder::decision::aliyun::{AliyunProvider, MODEL};
use issue_finder::decision::contract::{DecisionRequest, Provider, Question, QuestionKind};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::Instant;

const API_KEY: &str = "synthetic_secret_test_key";
const PRIVATE_MATERIAL: &str = "synthetic_private_issue_material";

fn request(index: usize) -> DecisionRequest {
    DecisionRequest {
        candidate_id: format!("org/repo:{index}"),
        input_id: format!("input-{index}"),
        questions: vec![Question {
            id: "ready".into(),
            prompt: "Is the issue ready?".into(),
            criteria: vec!["Use the supplied material.".into()],
            context: json!({"text":PRIVATE_MATERIAL}),
            kind: QuestionKind::Boolean,
        }],
    }
}

fn success_body() -> String {
    json!({"model":MODEL,"answers":{"ready":{"type":"noul","noul":0.8}}}).to_string()
}

#[derive(Clone)]
struct Reply {
    wire: String,
    delay: Duration,
}

impl Reply {
    fn response(status: u16, extra_headers: &str, body: &str) -> Self {
        Self {
            wire: format!(
                "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{body}",
                body.len()
            ),
            delay: Duration::ZERO,
        }
    }

    fn success() -> Self {
        Self::response(200, "", &success_body())
    }
}

struct Captured {
    headers: String,
    body: Value,
    at: Instant,
}

async fn read_request(stream: &mut TcpStream) -> Captured {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).await.unwrap();
        assert!(count > 0, "client closed before sending request headers");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(position) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap();
    while bytes.len() < header_end + length {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).await.unwrap();
        assert!(count > 0, "client closed before sending the request body");
        bytes.extend_from_slice(&buffer[..count]);
    }
    Captured {
        headers,
        body: serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap(),
        at: Instant::now(),
    }
}

struct MockServer {
    endpoint: String,
    captured: Arc<Mutex<Vec<Captured>>>,
    task: JoinHandle<()>,
}

impl MockServer {
    async fn start(replies: Vec<Reply>) -> Self {
        assert!(!replies.is_empty());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/compatible-mode/v1/systemone",
            listener.local_addr().unwrap()
        );
        let captured = Arc::new(Mutex::new(Vec::new()));
        let records = Arc::clone(&captured);
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_request(&mut stream).await;
                let index = {
                    let mut records = records.lock().unwrap();
                    records.push(request);
                    records.len() - 1
                };
                // Repeating the final reply also catches any unexpected extra retry.
                let reply = &replies[index.min(replies.len() - 1)];
                tokio::time::sleep(reply.delay).await;
                // Timeout and response-loss cases intentionally close early.
                let _ = stream.write_all(reply.wire.as_bytes()).await;
            }
        });
        Self {
            endpoint,
            captured,
            task,
        }
    }

    fn provider(&self, timeout: Duration) -> AliyunProvider {
        AliyunProvider::new(self.endpoint.clone(), API_KEY.into(), timeout).unwrap()
    }

    fn count(&self) -> usize {
        self.captured.lock().unwrap().len()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn assert_safe_error(error: &issue_finder::decision::contract::ProviderError) {
    let diagnostic = format!("{error:?}\n{error}");
    assert!(!diagnostic.contains("synthetic_secret"));
    assert!(!diagnostic.contains(PRIVATE_MATERIAL));
    assert!(!diagnostic.contains("127.0.0.1"));
}

#[tokio::test]
async fn sends_bearer_auth_and_json_content_type_to_native_endpoint() {
    let server = MockServer::start(vec![Reply::success()]).await;
    let provider = server.provider(Duration::from_secs(2));
    provider.decide(&request(0)).await.unwrap();
    let records = server.captured.lock().unwrap();
    assert_eq!(records.len(), 1);
    let headers = records[0].headers.to_ascii_lowercase();
    assert!(headers.starts_with("post /compatible-mode/v1/systemone http/1.1\r\n"));
    assert!(headers.contains(&format!("authorization: bearer {API_KEY}\r\n")));
    assert!(headers.contains("content-type: application/json\r\n"));
    assert_eq!(records[0].body["model"], MODEL);
    assert!(records[0].body.get("messages").is_none());
}

#[tokio::test]
async fn auth_permission_payment_quota_and_uncertain_server_errors_are_not_retried() {
    let ordinary = format!("{API_KEY} {PRIVATE_MATERIAL}");
    let exhausted = format!("{{\"code\":\"insufficient_quota\",\"detail\":\"{ordinary}\"}}");
    for (status, body, expected) in [
        (401, ordinary.as_str(), "authentication_failed"),
        (403, ordinary.as_str(), "permission_denied"),
        (402, ordinary.as_str(), "quota_exhausted"),
        (429, exhausted.as_str(), "quota_exhausted"),
        (503, exhausted.as_str(), "quota_exhausted"),
        (
            429,
            "You have exceeded your free allocation of 10,000 neurons per day",
            "quota_exhausted",
        ),
        (500, ordinary.as_str(), "provider_failed"),
    ] {
        let server =
            MockServer::start(vec![Reply::response(status, "Retry-After: 0\r\n", body)]).await;
        let error = server
            .provider(Duration::from_secs(2))
            .decide(&request(0))
            .await
            .unwrap_err();
        assert_eq!(error.code, expected, "HTTP {status}");
        assert_eq!(server.count(), 1, "HTTP {status} must not be resubmitted");
        assert_safe_error(&error);
    }
}

#[tokio::test]
async fn explicit_throttling_or_overload_allows_only_one_extra_attempt() {
    for (status, expected) in [(429, "rate_limited"), (503, "overloaded")] {
        let server = MockServer::start(vec![Reply::response(
            status,
            "Retry-After: 0\r\n",
            "synthetic_secret returned in failure body",
        )])
        .await;
        let error = server
            .provider(Duration::from_secs(2))
            .decide(&request(0))
            .await
            .unwrap_err();
        assert_eq!(error.code, expected);
        assert_eq!(server.count(), 2);
        assert_safe_error(&error);
    }
}

#[tokio::test]
async fn retry_after_is_observed_before_a_successful_retry() {
    let server = MockServer::start(vec![
        Reply::response(429, "Retry-After: 1\r\n", "throttled"),
        Reply::success(),
    ])
    .await;
    server
        .provider(Duration::from_secs(3))
        .decide(&request(0))
        .await
        .unwrap();
    let records = server.captured.lock().unwrap();
    assert_eq!(records.len(), 2);
    assert!(records[1].at.duration_since(records[0].at) >= Duration::from_secs(1));
    assert_eq!(records[0].body, records[1].body);
}

#[tokio::test]
async fn does_not_retry_when_retry_after_exceeds_the_remaining_deadline() {
    let server = MockServer::start(vec![Reply::response(
        503,
        "Retry-After: 1\r\n",
        "overloaded synthetic_secret",
    )])
    .await;
    let error = server
        .provider(Duration::from_millis(100))
        .decide(&request(0))
        .await
        .unwrap_err();
    assert_eq!(error.code, "overloaded");
    assert_eq!(server.count(), 1);
    assert_safe_error(&error);
}

#[tokio::test]
async fn a_retry_uses_the_original_call_deadline() {
    let mut slow = Reply::success();
    slow.delay = Duration::from_secs(2);
    let mut throttled = Reply::response(429, "Retry-After: 0\r\n", "throttled");
    throttled.delay = Duration::from_millis(600);
    let server = MockServer::start(vec![throttled, slow]).await;
    let start = Instant::now();
    let error = server
        .provider(Duration::from_millis(900))
        .decide(&request(0))
        .await
        .unwrap_err();
    assert_eq!(error.code, "timeout");
    assert_eq!(server.count(), 2);
    // Restarting a 900ms timeout after the first reply would take over 1.5s.
    assert!(start.elapsed() < Duration::from_millis(1300));
    assert_safe_error(&error);
}

#[tokio::test]
async fn lost_responses_and_admitted_request_timeouts_are_not_resubmitted() {
    let broken = Reply {
        wire:
            "HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\nsynthetic_secret"
                .into(),
        delay: Duration::ZERO,
    };
    let mut slow = Reply::success();
    slow.delay = Duration::from_secs(1);
    for (reply, expected) in [
        (
            Reply {
                wire: String::new(),
                delay: Duration::ZERO,
            },
            "transport_error",
        ),
        (broken, "transport_error"),
        (slow, "timeout"),
    ] {
        let server = MockServer::start(vec![reply]).await;
        let error = server
            .provider(Duration::from_millis(100))
            .decide(&request(0))
            .await
            .unwrap_err();
        assert_eq!(error.code, expected);
        assert_eq!(server.count(), 1);
        assert_safe_error(&error);
    }
}

#[tokio::test]
async fn redirects_are_rejected_before_forwarding_credentials() {
    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_url = format!("http://{}/stolen", target.local_addr().unwrap());
    let server = MockServer::start(vec![Reply::response(
        307,
        &format!("Location: {target_url}\r\n"),
        "synthetic_secret",
    )])
    .await;
    let error = server
        .provider(Duration::from_secs(2))
        .decide(&request(0))
        .await
        .unwrap_err();
    assert_eq!(error.code, "redirect_rejected");
    assert_eq!(server.count(), 1);
    assert_safe_error(&error);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), target.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn oversized_and_invalid_json_responses_fail_with_safe_diagnostics() {
    let large = "x".repeat(2 * 1024 * 1024 + 1);
    let chunked = Reply {
        wire: format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{large}\r\n0\r\n\r\n",
            large.len()
        ),
        delay: Duration::ZERO,
    };
    for (reply, message) in [
        (
            Reply {
                wire: "HTTP/1.1 200 OK\r\nContent-Length: 2097153\r\nConnection: close\r\n\r\n"
                    .into(),
                delay: Duration::ZERO,
            },
            "size limit",
        ),
        (chunked, "size limit"),
        (
            Reply::response(200, "", "invalid JSON synthetic_secret"),
            "valid JSON",
        ),
    ] {
        let server = MockServer::start(vec![reply]).await;
        let error = server
            .provider(Duration::from_secs(2))
            .decide(&request(0))
            .await
            .unwrap_err();
        assert_eq!(error.code, "invalid_output");
        assert!(error.message.contains(message));
        assert_eq!(server.count(), 1);
        assert_safe_error(&error);
    }
}

#[tokio::test]
async fn four_distinct_requests_can_be_in_flight_on_the_same_provider() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/compatible-mode/v1/systemone",
        listener.local_addr().unwrap()
    );
    let barrier = tokio::spawn(async move {
        let mut streams = Vec::new();
        let mut inputs = std::collections::BTreeSet::new();
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let record = read_request(&mut stream).await;
            inputs.insert(
                record.body["state"]["input_id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
            streams.push(stream);
        }
        // No response is released until all four full request bodies arrive.
        for mut stream in streams {
            stream
                .write_all(Reply::success().wire.as_bytes())
                .await
                .unwrap();
        }
        inputs
    });
    let provider = AliyunProvider::new(endpoint, API_KEY.into(), Duration::from_secs(2)).unwrap();
    let requests: Vec<_> = (0..4).map(request).collect();
    let results = join_all(requests.iter().map(|request| provider.decide(request))).await;
    if results.iter().any(Result::is_err) {
        barrier.abort();
        panic!("all four requests must reach the server before any response: {results:?}");
    }
    assert_eq!(barrier.await.unwrap().len(), 4);
    for (index, result) in results.into_iter().enumerate() {
        assert_eq!(result.unwrap().input_id, format!("input-{index}"));
    }
}
