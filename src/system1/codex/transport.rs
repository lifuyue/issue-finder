use std::collections::hash_map::RandomState;
use std::collections::{HashMap, VecDeque};
use std::hash::BuildHasher;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StateMutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, Mutex, Notify};

use super::{output_schema, parse_response, transport, INSTRUCTIONS, MODEL, REASONING_EFFORT};
use crate::system1::contract::{DecisionRequest, DecisionResponse, ProviderError};

const MAX_PROTOCOL_LINE: usize = 4 * 1024 * 1024;
const MAX_EVENTS: usize = 256;
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct Activity {
    state: StateMutex<(bool, usize)>,
    changed: Notify,
}
pub(super) struct Lease(Arc<Activity>);
impl Activity {
    pub(super) fn enter(self: &Arc<Self>) -> Result<Arc<Lease>, ProviderError> {
        let mut state = self.state.lock().unwrap();
        if state.0 {
            return Err(ProviderError::new(
                "transport_closed",
                "provider is closing",
            ));
        }
        state.1 += 1;
        Ok(Arc::new(Lease(self.clone())))
    }
    pub(super) async fn close(&self) {
        loop {
            // notify_waiters records notifications from creation of Notified,
            // including a last caller dropping before this future is polled.
            let changed = self.changed.notified();
            {
                let mut state = self.state.lock().unwrap();
                state.0 = true;
                if state.1 == 0 {
                    return;
                }
            }
            changed.await;
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().1 -= 1;
        self.0.changed.notify_waiters();
    }
}

pub(super) struct Failure {
    pub error: ProviderError,
    pub retry_after: Option<Duration>,
}
impl From<ProviderError> for Failure {
    fn from(error: ProviderError) -> Self {
        Self {
            error,
            retry_after: None,
        }
    }
}
fn rpc_failure(error: &Value) -> Failure {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Codex RPC failed");
    let normalized = message.to_lowercase();
    let forbidden = [
        "auth", "quota", "token", "sign in", "model", "effort", "invalid", "401", "403", "credit",
    ]
    .iter()
    .any(|fragment| normalized.contains(fragment));
    let safe_network = matches!(
        error
            .pointer("/data/codexErrorInfo")
            .and_then(Value::as_str),
        Some("httpConnectionFailed")
    );
    let unknown_or_accepted = error.pointer("/data/accepted").and_then(Value::as_bool)
        == Some(true)
        || normalized.contains("already accepted")
        || (normalized.contains("acceptance") && normalized.contains("unknown"));
    let retryable = !forbidden
        && !unknown_or_accepted
        && (error.get("code").and_then(Value::as_i64) == Some(-32001) || safe_network);
    let delay = error
        .pointer("/data/retry_after_ms")
        .and_then(Value::as_u64)
        .map(Duration::from_millis)
        .or_else(|| {
            error
                .pointer("/data/retry_after")
                .and_then(Value::as_f64)
                .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        })
        .unwrap_or(Duration::from_millis(50));
    Failure {
        error: ProviderError::new(
            if ["access token", "sign in", "unauthorized", "401"]
                .iter()
                .any(|fragment| normalized.contains(fragment))
            {
                "authentication_failed"
            } else {
                "provider_error"
            },
            message,
        ),
        retry_after: retryable.then_some(delay),
    }
}

#[derive(Default)]
struct Identity {
    thread: Option<String>,
    turn: Option<String>,
    completed: bool,
    failure: Option<ProviderError>,
}
struct Subscription {
    identity: StateMutex<Identity>,
    sender: mpsc::Sender<Value>,
}
struct Pending {
    method: String,
    result: oneshot::Sender<Result<Value, Failure>>,
    subscription: Option<Arc<Subscription>>,
}
#[derive(Default)]
struct Routes {
    pending: HashMap<u64, Pending>,
    threads: HashMap<String, Arc<Subscription>>,
    early: VecDeque<Value>,
    closed: Option<ProviderError>,
}
struct Write {
    value: Value,
    result: oneshot::Sender<Result<(), ProviderError>>,
}
struct Shared {
    writer: mpsc::Sender<Write>,
    routes: StateMutex<Routes>,
    sequence: AtomicU64,
}
impl Shared {
    fn fail(&self, error: ProviderError) {
        let mut routes = self.routes.lock().unwrap();
        if routes.closed.is_some() {
            return;
        }
        routes.closed = Some(error.clone());
        for (_, pending) in routes.pending.drain() {
            let _ = pending.result.send(Err(error.clone().into()));
        }
        for subscription in routes.threads.values() {
            subscription.identity.lock().unwrap().failure = Some(error.clone());
            let _ = subscription.sender.try_send(Value::Null);
        }
        routes.early.clear();
    }
    async fn send(&self, value: Value) -> Result<(), ProviderError> {
        if let Some(error) = &self.routes.lock().unwrap().closed {
            return Err(error.clone());
        }
        let (result, receiver) = oneshot::channel();
        self.writer
            .send(Write { value, result })
            .await
            .map_err(|_| transport("Codex writer closed"))?;
        receiver
            .await
            .map_err(|_| transport("Codex write confirmation lost"))?
    }

    async fn rpc(
        &self,
        method: &str,
        params: Value,
        subscription: Option<Arc<Subscription>>,
    ) -> Result<Value, Failure> {
        let id = self.sequence.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        {
            let mut routes = self.routes.lock().unwrap();
            if let Some(error) = &routes.closed {
                return Err(error.clone().into());
            }
            routes.pending.insert(
                id,
                Pending {
                    method: method.into(),
                    result: sender,
                    subscription,
                },
            );
        }
        if let Err(error) = self
            .send(json!({"id":id,"method":method,"params":params}))
            .await
        {
            self.fail(error.clone());
            return Err(error.into());
        }
        receiver
            .await
            .unwrap_or_else(|_| Err(transport("Codex RPC response channel closed").into()))
    }
}

pub(super) struct Session {
    shared: Arc<Shared>,
    child: Mutex<Child>,
    reader: tokio::task::JoinHandle<()>,
    writer: tokio::task::JoinHandle<()>,
}
impl Session {
    pub(super) fn spawn(binary: &Path, workspace: &Path) -> Result<Arc<Self>, ProviderError> {
        let mut child = Command::new(binary)
            .args([
                "app-server",
                "--listen",
                "stdio://",
                "-c",
                "web_search=\"disabled\"",
                "-c",
                "features.shell_tool=false",
                "-c",
                "features.multi_agent=false",
                "-c",
                "project_doc_max_bytes=0",
            ])
            .current_dir(workspace)
            .env("RUST_LOG", "warn")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| transport(format!("cannot start Codex app-server: {error}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| transport("app-server stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| transport("app-server stdout unavailable"))?;
        let (writer_queue, writes) = mpsc::channel(MAX_EVENTS);
        let shared = Arc::new(Shared {
            writer: writer_queue,
            routes: StateMutex::new(Routes::default()),
            sequence: AtomicU64::new(1),
        });
        let writer_shared = shared.clone();
        let writer = tokio::spawn(async move {
            write_loop(stdin, writes, &writer_shared).await;
        });
        let reader_shared = shared.clone();
        let reader = tokio::spawn(async move {
            if let Err(error) = read_loop(stdout, &reader_shared).await {
                reader_shared.fail(error);
            }
        });
        Ok(Arc::new(Self {
            shared,
            child: Mutex::new(child),
            reader,
            writer,
        }))
    }
    pub(super) fn next_jitter(&self) -> u64 {
        RandomState::new().hash_one(self.shared.sequence.load(Ordering::Relaxed)) % 21
    }
    pub(super) async fn initialize(&self) -> Result<(), ProviderError> {
        self.shared.rpc("initialize", json!({"clientInfo":{"name":"issue_finder_system1","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}), None).await.map_err(|failure| failure.error)?;
        self.shared
            .send(json!({"method":"initialized","params":{}}))
            .await
    }
    pub(super) async fn stop(&self) {
        self.shared.fail(ProviderError::new(
            "transport_closed",
            "Codex provider closed",
        ));
        let mut child = self.child.lock().await;
        let _ = child.start_kill();
        let _ = child.wait().await;
        self.reader.abort();
        self.writer.abort();
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort();
    }
}

pub(super) struct Attempt {
    session: Arc<Session>,
    subscription: Arc<Subscription>,
    events: mpsc::Receiver<Value>,
    lease: Arc<Lease>,
}
impl Attempt {
    pub(super) fn new(session: Arc<Session>, lease: Arc<Lease>) -> Self {
        let (sender, events) = mpsc::channel(MAX_EVENTS);
        Self {
            session,
            subscription: Arc::new(Subscription {
                identity: StateMutex::new(Identity::default()),
                sender,
            }),
            events,
            lease,
        }
    }
    pub(super) async fn generate(
        &mut self,
        request: &DecisionRequest,
        workspace: &Path,
        start: Instant,
    ) -> Result<DecisionResponse, Failure> {
        let thread = self.session.shared.rpc("thread/start", json!({"model":MODEL,"allowProviderModelFallback":false,"baseInstructions":INSTRUCTIONS,"ephemeral":true,"environments":[],"selectedCapabilityRoots":[],"dynamicTools":[],"approvalPolicy":"never","cwd":workspace,"config":{"model_reasoning_effort":REASONING_EFFORT,"web_search":"disabled","features.shell_tool":false,"features.multi_agent":false,"project_doc_max_bytes":0}}), Some(self.subscription.clone())).await?;
        if thread.get("model").and_then(Value::as_str) != Some(MODEL) {
            return Err(ProviderError::new(
                "model_mismatch",
                "Codex did not acknowledge gpt-6-luna; fallback is prohibited",
            )
            .into());
        }
        if thread.get("reasoningEffort").and_then(Value::as_str) != Some(REASONING_EFFORT) {
            return Err(ProviderError::new(
                "reasoning_mismatch",
                "Codex did not acknowledge reasoning effort none",
            )
            .into());
        }
        let thread_id = thread
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProviderError::new("invalid_protocol", "thread/start missing thread ID")
            })?
            .to_owned();
        let prompt = serde_json::to_string(request)
            .map_err(|error| ProviderError::new("invalid_contract", error.to_string()))?;
        let turn = self.session.shared.rpc("turn/start", json!({"threadId":thread_id,"model":MODEL,"effort":REASONING_EFFORT,"environments":[],"input":[{"type":"text","text":prompt,"text_elements":[]}],"outputSchema":output_schema(request)}), Some(self.subscription.clone())).await?;
        let turn_id = turn
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::new("invalid_protocol", "turn/start missing turn ID"))?
            .to_owned();
        let mut final_text = None;
        let mut usage = None;
        let mut completed = false;
        loop {
            if let Some(error) = self.subscription.identity.lock().unwrap().failure.clone() {
                return Err(error.into());
            }
            if completed && final_text.is_some() {
                break;
            }
            let event = self
                .events
                .recv()
                .await
                .ok_or_else(|| transport("Codex event channel closed"))?;
            let method = event.get("method").and_then(Value::as_str).unwrap_or("");
            let body = &event["params"];
            let event_turn = body
                .get("turnId")
                .or_else(|| body.pointer("/turn/id"))
                .and_then(Value::as_str);
            if event_turn.is_some_and(|id| id != turn_id) {
                continue;
            }
            match method {
                "item/started" | "item/completed" => {
                    let item = &body["item"];
                    let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
                    if !matches!(
                        kind,
                        "userMessage" | "agentMessage" | "reasoning" | "contextCompaction"
                    ) {
                        return Err(ProviderError::new(
                            "unexpected_tool_use",
                            "tool use invalidates classification",
                        )
                        .into());
                    }
                    if method == "item/completed"
                        && kind == "agentMessage"
                        && matches!(
                            item.get("phase").and_then(Value::as_str),
                            None | Some("final_answer")
                        )
                    {
                        final_text = item.get("text").and_then(Value::as_str).map(str::to_owned);
                    }
                }
                "thread/tokenUsage/updated" => usage = body.pointer("/tokenUsage/last").cloned(),
                "turn/completed" => {
                    if body.pointer("/turn/status").and_then(Value::as_str) != Some("completed") {
                        let error = &body["turn"]["error"];
                        let message = error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("Codex turn failed");
                        let normalized = message.to_lowercase();
                        let code = if ["access token", "sign in", "unauthorized", "401"]
                            .iter()
                            .any(|fragment| normalized.contains(fragment))
                        {
                            "authentication_failed"
                        } else {
                            "turn_failed"
                        };
                        return Err(ProviderError::new(code, message).into());
                    }
                    completed = true;
                }
                _ => {}
            }
        }
        parse_response(request, final_text, usage, start).map_err(Into::into)
    }
}
impl Drop for Attempt {
    fn drop(&mut self) {
        let session = self.session.clone();
        let subscription = self.subscription.clone();
        let lease = self.lease.clone();
        tokio::spawn(async move {
            let _lease = lease;
            let _ = tokio::time::timeout(CLEANUP_TIMEOUT, cleanup(&session.shared, &subscription))
                .await;
            let thread = subscription.identity.lock().unwrap().thread.clone();
            let mut routes = session.shared.routes.lock().unwrap();
            if let Some(thread) = thread {
                if routes
                    .threads
                    .get(&thread)
                    .is_some_and(|value| Arc::ptr_eq(value, &subscription))
                {
                    routes.threads.remove(&thread);
                }
                routes
                    .early
                    .retain(|event| event_thread(event) != Some(thread.as_str()));
            }
            routes
                .pending
                .retain(|_, pending| match &pending.subscription {
                    Some(value) => !Arc::ptr_eq(value, &subscription),
                    None => !pending.result.is_closed(),
                });
            if !routes
                .pending
                .values()
                .any(|pending| pending.method == "thread/start")
            {
                routes.early.clear();
            }
        });
    }
}
async fn cleanup(shared: &Shared, subscription: &Arc<Subscription>) {
    // A canceled thread/start may still be accepted. Give its response a bounded
    // chance to identify the thread before unsubscribing it.
    for _ in 0..50 {
        if subscription.identity.lock().unwrap().thread.is_some() {
            break;
        }
        if !shared
            .routes
            .lock()
            .unwrap()
            .pending
            .values()
            .any(|pending| {
                pending
                    .subscription
                    .as_ref()
                    .is_some_and(|value| Arc::ptr_eq(value, subscription))
            })
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (thread, mut turn, completed) = {
        let identity = subscription.identity.lock().unwrap();
        (
            identity.thread.clone(),
            identity.turn.clone(),
            identity.completed,
        )
    };
    let Some(thread) = thread else {
        return;
    };
    if !completed {
        if turn.is_none() {
            if let Ok(Ok(status)) = tokio::time::timeout(
                Duration::from_millis(450),
                shared.rpc(
                    "thread/read",
                    json!({"threadId":thread,"includeTurns":true}),
                    None,
                ),
            )
            .await
            {
                turn = status
                    .pointer("/thread/turns")
                    .and_then(Value::as_array)
                    .and_then(|turns| {
                        turns
                            .iter()
                            .rev()
                            .find(|turn| turn["status"] == "inProgress")
                    })
                    .and_then(|turn| turn["id"].as_str())
                    .map(str::to_owned);
            }
        }
        if let Some(turn) = turn {
            let _ = tokio::time::timeout(
                Duration::from_millis(450),
                shared.rpc(
                    "turn/interrupt",
                    json!({"threadId":thread,"turnId":turn}),
                    None,
                ),
            )
            .await;
        }
    }
    let _ = tokio::time::timeout(
        Duration::from_millis(450),
        shared.rpc("thread/unsubscribe", json!({"threadId":thread}), None),
    )
    .await;
}

async fn write_loop(mut stdin: ChildStdin, mut writes: mpsc::Receiver<Write>, shared: &Shared) {
    while let Some(write) = writes.recv().await {
        // Canceled requests that have not begun writing are safe to discard.
        if write.result.is_closed() {
            continue;
        }
        let result = async {
            let mut bytes =
                serde_json::to_vec(&write.value).map_err(|error| transport(error.to_string()))?;
            bytes.push(b'\n');
            // Once dequeued, finish the entire JSONL write even if its caller
            // is canceled. Partial lines must never corrupt peer requests.
            stdin
                .write_all(&bytes)
                .await
                .map_err(|error| transport(error.to_string()))?;
            stdin
                .flush()
                .await
                .map_err(|error| transport(error.to_string()))
        }
        .await;
        if let Err(error) = &result {
            shared.fail(error.clone());
        }
        let failed = result.is_err();
        let _ = write.result.send(result);
        if failed {
            return;
        }
    }
}

async fn read_line(stdout: &mut BufReader<ChildStdout>) -> Result<Value, ProviderError> {
    let mut bytes = Vec::new();
    loop {
        let available = stdout
            .fill_buf()
            .await
            .map_err(|error| transport(error.to_string()))?;
        if available.is_empty() {
            return Err(ProviderError::new(
                "transport_closed",
                "Codex app-server closed its output stream",
            ));
        }
        let length = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|position| position + 1)
            .unwrap_or(available.len());
        let ended = available[length - 1] == b'\n';
        if bytes.len() + length > MAX_PROTOCOL_LINE {
            return Err(ProviderError::new(
                "invalid_protocol",
                "Codex protocol line exceeded its size bound",
            ));
        }
        bytes.extend_from_slice(&available[..length]);
        stdout.consume(length);
        if ended {
            break;
        }
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| ProviderError::new("invalid_protocol", "Codex emitted invalid JSONL"))?;
    if !value.is_object() {
        return Err(ProviderError::new(
            "invalid_protocol",
            "Codex protocol messages must be JSON objects",
        ));
    }
    Ok(value)
}
fn event_thread(value: &Value) -> Option<&str> {
    value
        .pointer("/params/threadId")
        .or_else(|| value.pointer("/params/thread/id"))
        .and_then(Value::as_str)
}

fn deliver(subscription: &Subscription, value: Value) {
    let mut identity = subscription.identity.lock().unwrap();
    if let Some(turn) = value
        .pointer("/params/turnId")
        .or_else(|| value.pointer("/params/turn/id"))
        .and_then(Value::as_str)
    {
        identity.turn = Some(turn.into());
    }
    if value.get("method").is_some() && value.get("id").is_some() {
        identity.failure = Some(ProviderError::new(
            "unexpected_tool_request",
            "Codex requested a tool or approval during classification",
        ));
    }
    if value["method"] == "turn/completed" {
        identity.completed = true;
    }
    if subscription.sender.try_send(value).is_err() {
        identity.failure = Some(ProviderError::new(
            "invalid_protocol",
            "Codex thread event buffer exceeded its bound",
        ));
    }
}
async fn read_loop(stdout: ChildStdout, shared: &Shared) -> Result<(), ProviderError> {
    let mut stdout = BufReader::new(stdout);
    loop {
        let value = read_line(&mut stdout).await?;
        if value.get("method").is_some() && value.get("id").is_some() {
            shared.send(json!({"id":value["id"],"error":{"code":-32601,"message":"System 1 classification does not execute tools or approvals"}})).await?;
            let error = ProviderError::new(
                "unexpected_tool_request",
                "Codex requested a tool or approval during classification",
            );
            let routes = shared.routes.lock().unwrap();
            if let Some(thread) = event_thread(&value) {
                if let Some(subscription) = routes.threads.get(thread) {
                    deliver(subscription, value);
                } else if routes
                    .pending
                    .values()
                    .any(|pending| pending.method == "thread/start")
                {
                    drop(routes);
                    let mut routes = shared.routes.lock().unwrap();
                    if routes.early.len() >= MAX_EVENTS {
                        return Err(ProviderError::new(
                            "invalid_protocol",
                            "Codex early event buffer exceeded its bound",
                        ));
                    }
                    routes.early.push_back(value);
                }
            } else {
                // Older servers omit thread identity. Refuse the request and
                // invalidate current classifications conservatively.
                for subscription in routes.threads.values() {
                    subscription.identity.lock().unwrap().failure = Some(error.clone());
                    let _ = subscription.sender.try_send(Value::Null);
                }
            }
            continue;
        }
        let mut routes = shared.routes.lock().unwrap();
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            if let Some(pending) = routes.pending.remove(&id) {
                let result = if let Some(error) = value.get("error") {
                    Err(rpc_failure(error))
                } else {
                    value.get("result").cloned().ok_or_else(|| {
                        ProviderError::new("invalid_protocol", "Codex RPC response lacks a result")
                            .into()
                    })
                };
                if let (Ok(result), Some(subscription)) = (&result, &pending.subscription) {
                    if pending.method == "thread/start" {
                        if let Some(thread) = result.pointer("/thread/id").and_then(Value::as_str) {
                            subscription.identity.lock().unwrap().thread = Some(thread.into());
                            routes.threads.insert(thread.into(), subscription.clone());
                            let mut remaining = VecDeque::new();
                            for event in routes.early.drain(..) {
                                if event_thread(&event) == Some(thread) {
                                    deliver(subscription, event);
                                } else {
                                    remaining.push_back(event);
                                }
                            }
                            routes.early = remaining;
                        }
                    } else if pending.method == "turn/start" {
                        if let Some(turn) = result.pointer("/turn/id").and_then(Value::as_str) {
                            subscription.identity.lock().unwrap().turn = Some(turn.into());
                        }
                    }
                }
                let _ = pending.result.send(result);
                if !routes
                    .pending
                    .values()
                    .any(|pending| pending.method == "thread/start")
                {
                    routes.early.clear();
                }
            }
        } else if let Some(thread) = event_thread(&value) {
            if let Some(subscription) = routes.threads.get(thread) {
                deliver(subscription, value);
            } else if routes
                .pending
                .values()
                .any(|pending| pending.method == "thread/start")
            {
                if routes.early.len() >= MAX_EVENTS {
                    return Err(ProviderError::new(
                        "invalid_protocol",
                        "Codex early event buffer exceeded its bound",
                    ));
                }
                routes.early.push_back(value);
            }
        }
    }
}
