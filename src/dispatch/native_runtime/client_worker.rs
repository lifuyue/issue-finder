use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::dispatch::adapters::codex_app_server::discover_codex_binary;

const QUEUE_CAPACITY: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppServerTransportMode {
    DaemonSocket,
    Stdio,
}

#[derive(Debug, Clone)]
pub struct ServerRequest {
    pub id: Value,
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone)]
pub enum AppServerEvent {
    Notification { method: String, params: Value },
    ServerRequest(ServerRequest),
    Lagged { skipped: usize },
    Disconnected { message: String },
}

enum ClientCommand {
    Request {
        method: String,
        params: Value,
        reply: oneshot::Sender<Result<Value>>,
    },
    Respond {
        id: Value,
        result: std::result::Result<Value, Value>,
        reply: oneshot::Sender<Result<()>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

pub struct AppServerClient {
    command_tx: mpsc::Sender<ClientCommand>,
    event_rx: mpsc::Receiver<AppServerEvent>,
    worker: tokio::task::JoinHandle<()>,
    pub mode: AppServerTransportMode,
}

impl AppServerClient {
    pub async fn connect() -> Result<Self> {
        let mode = match std::env::var("ISSUE_FINDER_CODEX_TRANSPORT")
            .ok()
            .as_deref()
        {
            Some("stdio") => AppServerTransportMode::Stdio,
            Some(other) => anyhow::bail!("unsupported ISSUE_FINDER_CODEX_TRANSPORT {other}"),
            None => AppServerTransportMode::DaemonSocket,
        };
        match mode {
            AppServerTransportMode::DaemonSocket => Self::connect_daemon().await,
            AppServerTransportMode::Stdio => Self::connect_stdio().await,
        }
    }

    async fn connect_daemon() -> Result<Self> {
        let binary = discover_codex_binary()?;
        let status = Command::new(&binary)
            .args(["app-server", "daemon", "start"])
            .status()
            .await
            .context("unable to start Codex app-server daemon")?;
        if !status.success() {
            anyhow::bail!("Codex app-server daemon start failed with {status}");
        }
        let socket = codex_control_socket_path();
        let stream = UnixStream::connect(&socket)
            .await
            .with_context(|| format!("unable to connect Codex socket {}", socket.display()))?;
        let request = "ws://localhost/rpc".into_client_request()?;
        let (socket, _) = tokio_tungstenite::client_async(request, stream).await?;
        let (mut sink, mut stream) = socket.split();
        let (command_tx, mut command_rx) = mpsc::channel(QUEUE_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel(QUEUE_CAPACITY);
        let worker = tokio::spawn(async move {
            let mut next_id = 1_u64;
            let mut pending = HashMap::new();
            let mut skipped = 0_usize;
            if initialize_websocket(&mut sink, &mut stream).await.is_err() {
                let _ = event_tx
                    .send(AppServerEvent::Disconnected {
                        message: "initialize handshake failed".to_string(),
                    })
                    .await;
                return;
            }
            loop {
                tokio::select! {
                    command = command_rx.recv() => {
                        let Some(command) = command else { break };
                        match command {
                            ClientCommand::Request { method, params, reply } => {
                                let id = next_id;
                                next_id += 1;
                                pending.insert(id, reply);
                                if sink.send(Message::Text(json!({"id":id,"method":method,"params":params}).to_string().into())).await.is_err() { break; }
                            }
                            ClientCommand::Respond { id, result, reply } => {
                                let value = match result { Ok(result) => json!({"id":id,"result":result}), Err(error) => json!({"id":id,"error":error}) };
                                let sent = sink.send(Message::Text(value.to_string().into())).await.context("server response write failed");
                                let _ = reply.send(sent);
                            }
                            ClientCommand::Shutdown { reply } => { let _ = sink.close().await; let _ = reply.send(()); break; }
                        }
                    }
                    message = stream.next() => {
                        let Some(Ok(Message::Text(text))) = message else { break };
                        if !route_message(&event_tx, &mut pending, &mut skipped, &text).await { break; }
                    }
                }
            }
            for (_, reply) in pending {
                let _ = reply.send(Err(anyhow::anyhow!("app-server disconnected")));
            }
            let _ = event_tx
                .send(AppServerEvent::Disconnected {
                    message: "app-server disconnected".to_string(),
                })
                .await;
        });
        Ok(Self {
            command_tx,
            event_rx,
            worker,
            mode: AppServerTransportMode::DaemonSocket,
        })
    }

    async fn connect_stdio() -> Result<Self> {
        let binary = discover_codex_binary()?;
        let mut child = Command::new(binary)
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().context("missing app-server stdin")?;
        let stdout = child.stdout.take().context("missing app-server stdout")?;
        spawn_stdio_worker(child, stdin, stdout).await
    }

    pub async fn request(&self, method: impl Into<String>, params: Value) -> Result<Value> {
        let (reply, rx) = oneshot::channel();
        self.command_tx
            .send(ClientCommand::Request {
                method: method.into(),
                params,
                reply,
            })
            .await?;
        rx.await?
    }

    pub async fn respond(
        &self,
        id: Value,
        result: std::result::Result<Value, Value>,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.command_tx
            .send(ClientCommand::Respond { id, result, reply })
            .await?;
        rx.await?
    }

    pub async fn next_event(&mut self) -> Option<AppServerEvent> {
        self.event_rx.recv().await
    }

    pub async fn shutdown(self) {
        let (reply, rx) = oneshot::channel();
        let _ = self
            .command_tx
            .send(ClientCommand::Shutdown { reply })
            .await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), rx).await;
        self.worker.abort();
    }
}

fn codex_control_socket_path() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".codex")
        })
        .join("app-server-control/app-server-control.sock")
}

async fn initialize_websocket<S>(
    sink: &mut S,
    stream: &mut (impl StreamExt<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>
              + Unpin),
) -> Result<()>
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    sink.send(Message::Text(initialize_request().to_string().into()))
        .await?;
    while let Some(message) = stream.next().await {
        let message = message?;
        if let Message::Text(text) = message {
            let value: Value = serde_json::from_str(&text)?;
            if value.get("id") == Some(&json!(0)) {
                break;
            }
        }
    }
    sink.send(Message::Text(
        json!({"method":"initialized","params":{}})
            .to_string()
            .into(),
    ))
    .await?;
    Ok(())
}

fn initialize_request() -> Value {
    json!({"id":0,"method":"initialize","params":{"clientInfo":{"name":"issue-finder","title":"Issue Finder","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}})
}

async fn route_message(
    event_tx: &mpsc::Sender<AppServerEvent>,
    pending: &mut HashMap<u64, oneshot::Sender<Result<Value>>>,
    skipped: &mut usize,
    text: &str,
) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return true;
    };
    if let Some(id) = value.get("id").and_then(Value::as_u64) {
        if let Some(reply) = pending.remove(&id) {
            let result = value
                .get("error")
                .map(|e| Err(anyhow::anyhow!("app-server error: {e}")))
                .unwrap_or_else(|| Ok(value.get("result").cloned().unwrap_or(Value::Null)));
            let _ = reply.send(result);
            return true;
        }
    }
    let event = if let (Some(id), Some(method)) =
        (value.get("id"), value.get("method").and_then(Value::as_str))
    {
        AppServerEvent::ServerRequest(ServerRequest {
            id: id.clone(),
            method: method.to_string(),
            params: value.get("params").cloned().unwrap_or(Value::Null),
        })
    } else if let Some(method) = value.get("method").and_then(Value::as_str) {
        AppServerEvent::Notification {
            method: method.to_string(),
            params: value.get("params").cloned().unwrap_or(Value::Null),
        }
    } else {
        return true;
    };
    let lossless = matches!(&event, AppServerEvent::ServerRequest(_))
        || matches!(&event, AppServerEvent::Notification { method, .. } if lossless_notification(method));
    if lossless {
        if *skipped > 0
            && event_tx
                .send(AppServerEvent::Lagged {
                    skipped: std::mem::take(skipped),
                })
                .await
                .is_err()
        {
            return false;
        }
        event_tx.send(event).await.is_ok()
    } else {
        match event_tx.try_send(event) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                *skipped = skipped.saturating_add(1);
                true
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }
}

fn lossless_notification(method: &str) -> bool {
    matches!(
        method,
        "turn/completed"
            | "item/completed"
            | "item/agentMessage/delta"
            | "item/plan/delta"
            | "item/reasoning/textDelta"
            | "item/reasoning/summaryTextDelta"
            | "thread/settings/updated"
    )
}

async fn spawn_stdio_worker(
    mut child: Child,
    mut stdin: ChildStdin,
    stdout: ChildStdout,
) -> Result<AppServerClient> {
    stdin
        .write_all(format!("{}\n", initialize_request()).as_bytes())
        .await?;
    let mut lines = BufReader::new(stdout).lines();
    while let Some(line) = lines.next_line().await? {
        let value: Value = serde_json::from_str(&line)?;
        if value.get("id") == Some(&json!(0)) {
            break;
        }
    }
    stdin
        .write_all(b"{\"method\":\"initialized\",\"params\":{}}\n")
        .await?;
    let (command_tx, mut command_rx) = mpsc::channel(QUEUE_CAPACITY);
    let (event_tx, event_rx) = mpsc::channel(QUEUE_CAPACITY);
    let worker = tokio::spawn(async move {
        let mut next_id = 1_u64;
        let mut pending = HashMap::new();
        let mut skipped = 0_usize;
        loop {
            tokio::select! { command = command_rx.recv() => { let Some(command)=command else {break}; match command { ClientCommand::Request{method,params,reply} => { let id=next_id; next_id+=1; pending.insert(id,reply); if stdin.write_all(format!("{}\n",json!({"id":id,"method":method,"params":params})).as_bytes()).await.is_err(){break;} }, ClientCommand::Respond{id,result,reply} => { let value=match result{Ok(v)=>json!({"id":id,"result":v}),Err(e)=>json!({"id":id,"error":e})}; let result=stdin.write_all(format!("{value}\n").as_bytes()).await.context("server response write failed"); let _=reply.send(result); }, ClientCommand::Shutdown{reply}=>{let _=child.kill().await;let _=reply.send(());break;} } }, line=lines.next_line()=>{match line {Ok(Some(line))=>{if !route_message(&event_tx,&mut pending,&mut skipped,&line).await{break;}},_=>break}} }
        }
    });
    Ok(AppServerClient {
        command_tx,
        event_rx,
        worker,
        mode: AppServerTransportMode::Stdio,
    })
}
