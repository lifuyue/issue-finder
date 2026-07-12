use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::dispatch::AgentCapabilityName;

use super::{AdapterSession, AdapterStartSessionRequest, AdapterTurn, NativeExecutionAdapter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexCapabilityMapping {
    pub capability: AgentCapabilityName,
    pub method: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexStartSessionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexSession {
    #[serde(rename = "threadId", alias = "id")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexTurn {
    #[serde(rename = "turnId", alias = "id")]
    pub turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexSteerResponse {
    pub turn_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexTurnRecord {
    #[serde(rename = "turnId", alias = "id")]
    pub turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexTranscriptItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(rename = "type")]
    pub item_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexTranscript {
    pub thread: CodexSession,
    pub turns: Vec<CodexTurnRecord>,
    pub items: Vec<CodexTranscriptItem>,
}

pub trait CodexAppServerTransport {
    fn request(&mut self, method: &str, params: Value) -> Result<Value>;

    fn notify(&mut self, _method: &str, _params: Value) -> Result<()> {
        Ok(())
    }
}

pub struct CodexAppServerAdapter<T> {
    transport: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexAppServerConnectionMode {
    DaemonProxy,
    Stdio,
}

pub struct CodexAppServerStdioTransport {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: BufReader<ChildStderr>,
    next_id: u64,
    mode: CodexAppServerConnectionMode,
    request_timeout: Duration,
}

impl CodexAppServerStdioTransport {
    pub fn connect() -> Result<Self> {
        let command = discover_codex_binary()?;
        match std::env::var("ISSUE_FINDER_CODEX_TRANSPORT")
            .ok()
            .as_deref()
        {
            Some("stdio") => Self::connect_stdio_with_command(&command),
            Some(other) => anyhow::bail!("unsupported ISSUE_FINDER_CODEX_TRANSPORT {other}"),
            None => Self::connect_with_command(&command),
        }
    }

    pub fn connect_with_command(command: &str) -> Result<Self> {
        Self::connect_with_command_and_mode(command, CodexAppServerConnectionMode::DaemonProxy)
    }

    pub fn connect_stdio_with_command(command: &str) -> Result<Self> {
        Self::connect_with_command_and_mode(command, CodexAppServerConnectionMode::Stdio)
    }

    pub fn connect_with_command_and_mode(
        command: &str,
        mode: CodexAppServerConnectionMode,
    ) -> Result<Self> {
        if mode == CodexAppServerConnectionMode::DaemonProxy {
            start_daemon(command)?;
        }
        let args = match mode {
            CodexAppServerConnectionMode::DaemonProxy => ["app-server", "proxy"].as_slice(),
            CodexAppServerConnectionMode::Stdio => ["app-server", "--stdio"].as_slice(),
        };
        Self::spawn_with_args(command, args, mode, Duration::from_secs(10))
    }

    pub fn connect_with_command_and_timeout(command: &str, timeout: Duration) -> Result<Self> {
        start_daemon(command)?;
        Self::spawn_with_args(
            command,
            &["app-server", "proxy"],
            CodexAppServerConnectionMode::DaemonProxy,
            timeout,
        )
    }

    pub fn connection_mode(&self) -> CodexAppServerConnectionMode {
        self.mode
    }

    fn spawn_with_args(
        command: &str,
        args: &[&str],
        mode: CodexAppServerConnectionMode,
        request_timeout: Duration,
    ) -> Result<Self> {
        let config_args = codex_config_override_args();
        let mut child = Command::new(command)
            .args(&config_args)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("unable to start {command} {}", args.join(" ")))?;
        let stdin = child
            .stdin
            .take()
            .context("codex app-server stdio stdin is unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("codex app-server stdio stdout is unavailable")?;
        let stderr = child
            .stderr
            .take()
            .context("codex app-server stderr is unavailable")?;
        let mut transport = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            stderr: BufReader::new(stderr),
            next_id: 1,
            mode,
            request_timeout,
        };
        transport.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "issue-finder",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "experimentalApi": true
                }
            }),
        )?;
        transport.notify("initialized", json!({}))?;
        Ok(transport)
    }
}

fn start_daemon(command: &str) -> Result<()> {
    let config_args = codex_config_override_args();
    let output = Command::new(command)
        .args(&config_args)
        .args(["app-server", "daemon", "start"])
        .output()
        .with_context(|| format!("unable to start {command} app-server daemon start"))?;
    if !output.status.success() {
        let stderr = trim_output(&output.stderr);
        let stdout = trim_output(&output.stdout);
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        anyhow::bail!(
            "{command} app-server daemon start exited with {}: {detail}",
            output.status
        );
    }
    Ok(())
}

pub fn codex_config_override_args() -> Vec<String> {
    codex_config_override_args_with(|name| std::env::var(name).ok())
}

fn codex_config_override_args_with(get: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let mut args = Vec::new();
    let provider_id = get("ISSUE_FINDER_CODEX_MODEL_PROVIDER")
        .filter(|value| {
            !value.is_empty()
                && value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
                })
        })
        .unwrap_or_else(|| "cliproxy".to_string());
    let mut push = |key: &str, env_name: &str| {
        if let Some(value) = get(env_name) {
            if !value.is_empty() {
                args.push("-c".to_string());
                args.push(format!("{key}={}", serde_json::to_string(&value).unwrap()));
            }
        }
    };
    push("model", "ISSUE_FINDER_CODEX_MODEL");
    push("model_provider", "ISSUE_FINDER_CODEX_MODEL_PROVIDER");
    push(
        "model_reasoning_effort",
        "ISSUE_FINDER_CODEX_REASONING_EFFORT",
    );
    push(
        &format!("model_providers.{provider_id}.name"),
        "ISSUE_FINDER_CODEX_PROVIDER_NAME",
    );
    push(
        &format!("model_providers.{provider_id}.base_url"),
        "ISSUE_FINDER_CODEX_BASE_URL",
    );
    push(
        &format!("model_providers.{provider_id}.env_key"),
        "ISSUE_FINDER_CODEX_API_KEY_ENV",
    );
    push(
        &format!("model_providers.{provider_id}.wire_api"),
        "ISSUE_FINDER_CODEX_WIRE_API",
    );
    args
}

impl CodexAppServerTransport for CodexAppServerStdioTransport {
    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({
            "id": id,
            "method": method,
            "params": params
        });
        serde_json::to_writer(&mut self.stdin, &message)
            .with_context(|| format!("unable to write {method} request"))?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;

        let mut line = String::new();
        loop {
            line.clear();
            wait_for_stdout(&self.stdout, self.request_timeout)
                .with_context(|| format!("timed out waiting for {method} response"))?;
            let read = self
                .stdout
                .read_line(&mut line)
                .with_context(|| format!("unable to read {method} response"))?;
            if read == 0 {
                let mut stderr = String::new();
                let _ = self.stderr.read_to_string(&mut stderr);
                let stderr = stderr.trim();
                if stderr.is_empty() {
                    anyhow::bail!("codex app-server closed before {method} response");
                }
                anyhow::bail!("codex app-server closed before {method} response: {stderr}");
            }
            let message: Value = serde_json::from_str(line.trim_end()).with_context(|| {
                format!("invalid codex app-server JSON while waiting for {method}")
            })?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                anyhow::bail!("codex app-server {method} error: {error}");
            }
            return message
                .get("result")
                .cloned()
                .with_context(|| format!("codex app-server {method} response missing result"));
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let message = json!({ "method": method, "params": params });
        serde_json::to_writer(&mut self.stdin, &message)
            .with_context(|| format!("unable to write {method} notification"))?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }
}

#[cfg(unix)]
fn wait_for_stdout(stdout: &BufReader<ChildStdout>, timeout: Duration) -> Result<()> {
    let mut poll_fd = libc::pollfd {
        fd: stdout.get_ref().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    let result = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
    if result < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if result == 0 {
        anyhow::bail!("no app-server output within {timeout_ms} ms");
    }
    Ok(())
}

#[cfg(not(unix))]
fn wait_for_stdout(_stdout: &BufReader<ChildStdout>, _timeout: Duration) -> Result<()> {
    Ok(())
}

impl Drop for CodexAppServerStdioTransport {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl<T> CodexAppServerAdapter<T>
where
    T: CodexAppServerTransport,
{
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn into_transport(self) -> T {
        self.transport
    }

    pub fn capability_mappings() -> Vec<CodexCapabilityMapping> {
        codex_capability_mappings()
    }

    pub fn start_session(&mut self, request: CodexStartSessionRequest) -> Result<CodexSession> {
        let value = self.transport.request(
            "thread/start",
            json!({
                "cwd": request.cwd,
                "runtimeWorkspaceRoots": [request.cwd],
                "approvalPolicy": "on-request",
                "sandbox": "workspace-write"
            }),
        )?;
        let mut session = decode_session(value, "thread/start")?;

        if let Some(name) = request.name {
            let mut renamed = self.rename_session(&session.thread_id, &name)?;
            renamed.goal = renamed.goal.or(session.goal);
            renamed.metadata = merge_known_metadata(renamed.metadata, session.metadata);
            session = renamed;
        }
        if let Some(goal) = request.goal {
            let mut updated = self.set_goal(&session.thread_id, &goal)?;
            updated.name = updated.name.or(session.name);
            updated.metadata = merge_known_metadata(updated.metadata, session.metadata);
            session = updated;
        }
        if !request.metadata.is_null() {
            let mut updated = self.set_metadata(&session.thread_id, request.metadata)?;
            updated.name = updated.name.or(session.name);
            updated.goal = updated.goal.or(session.goal);
            session = updated;
        }

        Ok(session)
    }

    pub fn resume_session(&mut self, thread_id: &str) -> Result<CodexSession> {
        let value = self
            .transport
            .request("thread/resume", json!({ "threadId": thread_id }))?;
        decode_session(value, "thread/resume")
    }

    pub fn fork_session(&mut self, thread_id: &str) -> Result<CodexSession> {
        let value = self
            .transport
            .request("thread/fork", json!({ "threadId": thread_id }))?;
        decode_session(value, "thread/fork")
    }

    pub fn rename_session(&mut self, thread_id: &str, name: &str) -> Result<CodexSession> {
        let value = self.transport.request(
            "thread/name/set",
            json!({
                "threadId": thread_id,
                "name": name
            }),
        )?;
        decode_session_or_known(
            value,
            "thread/name/set",
            thread_id,
            Some(name.to_string()),
            None,
            Value::Null,
        )
    }

    pub fn archive_session(&mut self, thread_id: &str) -> Result<CodexSession> {
        let value = self
            .transport
            .request("thread/archive", json!({ "threadId": thread_id }))?;
        decode_session_or_known(
            value,
            "thread/archive",
            thread_id,
            None,
            None,
            json!({ "archived": true }),
        )
    }

    pub fn read_thread(&mut self, thread_id: &str) -> Result<CodexSession> {
        let value = self
            .transport
            .request("thread/read", json!({ "threadId": thread_id }))?;
        decode_session(value, "thread/read")
    }

    pub fn list_turns(&mut self, thread_id: &str) -> Result<Vec<CodexTurnRecord>> {
        let value = self
            .transport
            .request("thread/turns/list", json!({ "threadId": thread_id }))?;
        decode_array(value, "thread/turns/list", &["turns", "data"])
    }

    pub fn list_turn_items(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<Vec<CodexTranscriptItem>> {
        let value = self.transport.request(
            "thread/items/list",
            json!({
                "threadId": thread_id,
                "turnId": turn_id
            }),
        )?;
        decode_array::<Value>(value, "thread/items/list", &["items", "data"])?
            .into_iter()
            .map(decode_transcript_item)
            .collect()
    }

    pub fn read_transcript(&mut self, thread_id: &str) -> Result<CodexTranscript> {
        let read_value = self.transport.request(
            "thread/read",
            json!({ "threadId": thread_id, "includeTurns": true }),
        )?;
        let thread = decode_session(read_value.clone(), "thread/read")?;
        let turns = self.list_turns(thread_id)?;
        let mut items = transcript_items_from_thread_read(&read_value).unwrap_or_default();
        for turn in &turns {
            match self.list_turn_items(thread_id, &turn.turn_id) {
                Ok(turn_items) => extend_unique_transcript_items(&mut items, turn_items),
                Err(error) if error.to_string().contains("not supported yet") => break,
                Err(error) => return Err(error),
            }
        }
        Ok(CodexTranscript {
            thread,
            turns,
            items,
        })
    }

    pub fn list_sessions(&mut self, limit: Option<usize>) -> Result<Vec<CodexSession>> {
        let value = self.transport.request(
            "thread/list",
            json!({
                "limit": limit,
                "archived": false,
                "sortDirection": "desc",
                "sortKey": "updated_at"
            }),
        )?;
        decode_array(value, "thread/list", &["data"])
    }

    pub fn search_sessions(
        &mut self,
        search_term: &str,
        limit: Option<usize>,
    ) -> Result<Vec<CodexSession>> {
        let value = self.transport.request(
            "thread/list",
            json!({
                "searchTerm": search_term,
                "limit": limit,
                "archived": false,
                "sortDirection": "desc",
                "sortKey": "updated_at"
            }),
        )?;
        decode_array(value, "thread/list", &["data"])
    }

    pub fn start_turn(
        &mut self,
        thread_id: &str,
        prompt: &str,
        cwd: &str,
        client_user_message_id: &str,
    ) -> Result<CodexTurn> {
        let value = self.transport.request(
            "turn/start",
            json!({
                "threadId": thread_id,
                "clientUserMessageId": client_user_message_id,
                "input": [{ "type": "text", "text": prompt }],
                "cwd": cwd,
                "runtimeWorkspaceRoots": [cwd],
                "approvalPolicy": "on-request",
                "sandboxPolicy": { "type": "workspaceWrite", "writableRoots": [cwd], "networkAccess": false }
            }),
        )?;
        decode_turn(value, "turn/start")
    }

    pub fn steer_turn(
        &mut self,
        thread_id: &str,
        expected_turn_id: &str,
        prompt: &str,
        client_user_message_id: &str,
    ) -> Result<CodexSteerResponse> {
        let value = self.transport.request(
            "turn/steer",
            json!({
                "threadId": thread_id,
                "expectedTurnId": expected_turn_id,
                "clientUserMessageId": client_user_message_id,
                "input": [{ "type": "text", "text": prompt }]
            }),
        )?;
        decode(value, "turn/steer")
    }

    pub fn set_goal(&mut self, thread_id: &str, goal: &str) -> Result<CodexSession> {
        let value = self.transport.request(
            "thread/goal/set",
            json!({
                "threadId": thread_id,
                "objective": goal
            }),
        )?;
        decode_session_or_known(
            value,
            "thread/goal/set",
            thread_id,
            None,
            Some(goal.to_string()),
            Value::Null,
        )
    }

    pub fn set_metadata(&mut self, thread_id: &str, metadata: Value) -> Result<CodexSession> {
        let Some(git_info) = metadata.get("gitInfo") else {
            return Ok(CodexSession {
                thread_id: thread_id.to_string(),
                name: None,
                goal: None,
                metadata,
            });
        };
        let value = self.transport.request(
            "thread/metadata/update",
            json!({
                "threadId": thread_id,
                "gitInfo": git_info
            }),
        )?;
        decode_session(value, "thread/metadata/update")
    }
}

fn extend_unique_transcript_items(
    items: &mut Vec<CodexTranscriptItem>,
    additional: Vec<CodexTranscriptItem>,
) {
    for item in additional {
        let id = item.payload.get("id").and_then(Value::as_str);
        let duplicate = id.is_some_and(|id| {
            items
                .iter()
                .any(|existing| existing.payload.get("id").and_then(Value::as_str) == Some(id))
        });
        if !duplicate {
            items.push(item);
        }
    }
}

impl<T> NativeExecutionAdapter for CodexAppServerAdapter<T>
where
    T: CodexAppServerTransport,
{
    fn adapter_start_session(
        &mut self,
        request: AdapterStartSessionRequest,
    ) -> Result<AdapterSession> {
        let session = CodexAppServerAdapter::start_session(
            self,
            CodexStartSessionRequest {
                name: Some(request.display_name),
                goal: request.goal,
                metadata: request.metadata_json,
                cwd: request.cwd,
            },
        )?;
        Ok(session.into())
    }

    fn adapter_resume_session(&mut self, native_session_id: &str) -> Result<AdapterSession> {
        Ok(CodexAppServerAdapter::resume_session(self, native_session_id)?.into())
    }

    fn adapter_fork_session(&mut self, native_session_id: &str) -> Result<AdapterSession> {
        Ok(CodexAppServerAdapter::fork_session(self, native_session_id)?.into())
    }

    fn adapter_rename_session(
        &mut self,
        native_session_id: &str,
        display_name: &str,
    ) -> Result<AdapterSession> {
        Ok(CodexAppServerAdapter::rename_session(self, native_session_id, display_name)?.into())
    }

    fn adapter_set_goal(&mut self, native_session_id: &str, goal: &str) -> Result<AdapterSession> {
        Ok(CodexAppServerAdapter::set_goal(self, native_session_id, goal)?.into())
    }

    fn adapter_set_metadata(
        &mut self,
        native_session_id: &str,
        metadata_json: Value,
    ) -> Result<AdapterSession> {
        Ok(CodexAppServerAdapter::set_metadata(self, native_session_id, metadata_json)?.into())
    }

    fn adapter_start_turn(
        &mut self,
        native_session_id: &str,
        prompt: &str,
        cwd: &str,
        client_user_message_id: &str,
    ) -> Result<AdapterTurn> {
        Ok(CodexAppServerAdapter::start_turn(
            self,
            native_session_id,
            prompt,
            cwd,
            client_user_message_id,
        )?
        .into())
    }

    fn adapter_read_transcript(&mut self, native_session_id: &str) -> Result<Value> {
        Ok(serde_json::to_value(
            CodexAppServerAdapter::read_transcript(self, native_session_id)?,
        )?)
    }

    fn adapter_archive_session(&mut self, native_session_id: &str) -> Result<AdapterSession> {
        Ok(CodexAppServerAdapter::archive_session(self, native_session_id)?.into())
    }

    fn adapter_list_sessions(&mut self, limit: Option<usize>) -> Result<Vec<AdapterSession>> {
        Ok(CodexAppServerAdapter::list_sessions(self, limit)?
            .into_iter()
            .map(AdapterSession::from)
            .collect())
    }

    fn adapter_search_sessions(
        &mut self,
        search_term: &str,
        limit: Option<usize>,
    ) -> Result<Vec<AdapterSession>> {
        Ok(
            CodexAppServerAdapter::search_sessions(self, search_term, limit)?
                .into_iter()
                .map(AdapterSession::from)
                .collect(),
        )
    }
}

pub fn codex_capability_mappings() -> Vec<CodexCapabilityMapping> {
    vec![
        mapping(AgentCapabilityName::StartSession, "thread/start"),
        mapping(AgentCapabilityName::ResumeSession, "thread/resume"),
        mapping(AgentCapabilityName::ForkSession, "thread/fork"),
        mapping(AgentCapabilityName::RenameSession, "thread/name/set"),
        mapping(AgentCapabilityName::ListSessions, "thread/list"),
        mapping(AgentCapabilityName::SearchSessions, "thread/list"),
        mapping(AgentCapabilityName::ReadTranscript, "thread/read"),
        mapping(AgentCapabilityName::SetGoal, "thread/goal/set"),
        mapping(AgentCapabilityName::SetMetadata, "thread/metadata/update"),
        mapping(AgentCapabilityName::ArchiveSession, "thread/archive"),
        mapping(AgentCapabilityName::InterruptRun, "turn/interrupt"),
        mapping(AgentCapabilityName::ReviewMode, "review/start"),
        mapping(AgentCapabilityName::StreamEvents, "turn/start"),
    ]
}

pub fn default_codex_app_server_startup_metadata() -> Value {
    static METADATA: OnceLock<Value> = OnceLock::new();
    METADATA
        .get_or_init(|| match discover_codex_binary() {
            Ok(command) => codex_app_server_startup_metadata(&command),
            Err(error) => json!({
                "binary": { "name": "codex", "available": false },
                "connectionModes": [],
                "supportedMethods": supported_method_names(),
                "probe": { "status": "binary_unavailable", "error": error.to_string() }
            }),
        })
        .clone()
}

pub fn codex_app_server_startup_metadata(command: &str) -> Value {
    let binary = codex_binary_metadata(command);
    let available = binary
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !available {
        return json!({
            "binary": binary,
            "connectionModes": [],
            "supportedMethods": supported_method_names(),
            "probe": {
                "status": "binary_unavailable"
            }
        });
    }

    let cli_version = command_stdout(command, &["--version"]);
    let app_server_help = command_succeeds(command, &["app-server", "--help"]);
    let daemon_help = command_succeeds(command, &["app-server", "daemon", "--help"]);
    let proxy_help = command_succeeds(command, &["app-server", "proxy", "--help"]);
    let daemon_version = command_json(command, &["app-server", "daemon", "version"]);
    let daemon_version_error = daemon_version
        .is_none()
        .then(|| command_error(command, &["app-server", "daemon", "version"]))
        .flatten();
    json!({
        "binary": binary,
        "cliVersion": cli_version,
        "appServerVersion": daemon_version,
        "appServerVersionError": daemon_version_error,
        "connectionModes": [
            {
                "mode": "daemon_proxy",
                "daemonStartCommand": ["app-server", "daemon", "start"],
                "proxyCommand": ["app-server", "proxy"],
                "available": daemon_help && proxy_help
            },
            {
                "mode": "stdio",
                "command": ["app-server", "--stdio"],
                "available": app_server_help
            }
        ],
        "supportedMethods": supported_method_names(),
        "probe": { "status": "not_run" }
    })
}

pub fn probe_codex_app_server() -> Value {
    let command = match discover_codex_binary() {
        Ok(command) => command,
        Err(error) => {
            return json!({
                "status": "binary_unavailable",
                "error": error.to_string()
            });
        }
    };
    match CodexAppServerStdioTransport::connect_with_command(&command) {
        Ok(mut transport) => match transport.request(
            "thread/list",
            json!({ "limit": 1, "archived": false, "sortDirection": "desc" }),
        ) {
            Ok(_) => json!({
                "status": "handshake_succeeded",
                "method": "thread/list",
                "connectionMode": "daemon_proxy",
                "binary": command
            }),
            Err(error) => json!({
                "status": "handshake_failed",
                "error": error.to_string(),
                "connectionMode": "daemon_proxy",
                "binary": command
            }),
        },
        Err(error) => json!({
            "status": "handshake_failed",
            "error": error.to_string(),
            "connectionMode": "daemon_proxy",
            "binary": command
        }),
    }
}

pub fn dispatch_prompt() -> &'static str {
    "You are receiving an Issue Finder task package v3.\n\
Goal: follow the package contract to reproduce when practical, make a scoped fix, validate, and report the result.\n\
Read the package artifacts first.\n\
Respect workspace_policy, reproduction_contract, change_budget, environment_contract,\n\
interaction_policy, session_context, and outcome_contract.\n\
Return fix_result.json with reproduction evidence, success criteria status, files changed,\n\
validation run, residual risks, failure reason when applicable, session context,\n\
and suggested GitHub reply."
}

fn mapping(capability: AgentCapabilityName, method: &'static str) -> CodexCapabilityMapping {
    CodexCapabilityMapping { capability, method }
}

fn decode_transcript_item(value: Value) -> Result<CodexTranscriptItem> {
    let item_type = value
        .get("type")
        .and_then(Value::as_str)
        .context("invalid thread/items/list item: missing type")?
        .to_string();
    let turn_id = value
        .get("turnId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let text = value
        .get("text")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            value
                .get("content")
                .and_then(Value::as_array)
                .map(|content| {
                    content
                        .iter()
                        .filter_map(|entry| entry.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
        })
        .filter(|text| !text.is_empty());
    Ok(CodexTranscriptItem {
        turn_id,
        item_type,
        text,
        payload: value,
    })
}

fn transcript_items_from_thread_read(value: &Value) -> Result<Vec<CodexTranscriptItem>> {
    let thread = value.get("thread").unwrap_or(value);
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .context("thread/read includeTurns response is missing turns")?;
    let mut items = Vec::new();
    for turn in turns {
        let turn_id = turn.get("id").and_then(Value::as_str);
        for item in turn
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let mut item = item.clone();
            if item.get("turnId").is_none() {
                if let (Some(turn_id), Some(object)) = (turn_id, item.as_object_mut()) {
                    object.insert("turnId".to_string(), Value::String(turn_id.to_string()));
                }
            }
            items.push(decode_transcript_item(item)?);
        }
    }
    Ok(items)
}

fn supported_method_names() -> Vec<&'static str> {
    vec![
        "thread/start",
        "thread/resume",
        "thread/fork",
        "thread/name/set",
        "thread/list",
        "thread/read",
        "thread/turns/list",
        "thread/items/list",
        "thread/goal/set",
        "thread/metadata/update",
        "thread/archive",
        "turn/start",
        "turn/steer",
        "turn/interrupt",
        "review/start",
    ]
}

pub fn discover_codex_binary() -> Result<String> {
    if let Ok(explicit) = std::env::var("ISSUE_FINDER_CODEX_BIN") {
        let explicit = explicit.trim();
        if explicit.is_empty() {
            anyhow::bail!("ISSUE_FINDER_CODEX_BIN is empty");
        }
        validate_codex_binary(explicit)
            .with_context(|| format!("ISSUE_FINDER_CODEX_BIN {explicit} is not usable"))?;
        return Ok(explicit.to_string());
    }

    if let Some(path) = find_command_path("codex") {
        if validate_codex_binary(&path).is_ok() {
            return Ok(path);
        }
    }

    #[cfg(target_os = "macos")]
    {
        let app_binary = "/Applications/ChatGPT.app/Contents/Resources/codex";
        if validate_codex_binary(app_binary).is_ok() {
            return Ok(app_binary.to_string());
        }
    }

    anyhow::bail!("no usable Codex binary found; set ISSUE_FINDER_CODEX_BIN")
}

fn validate_codex_binary(command: &str) -> Result<()> {
    if !Path::new(command).is_file() {
        anyhow::bail!("binary does not exist");
    }
    if !command_succeeds(command, &["--version"]) {
        anyhow::bail!("--version failed");
    }
    if !command_succeeds(command, &["app-server", "--help"]) {
        anyhow::bail!("app-server --help failed");
    }
    Ok(())
}

fn codex_binary_metadata(command: &str) -> Value {
    match find_command_path(command) {
        Some(path) => json!({
            "name": command,
            "available": true,
            "path": path
        }),
        None => json!({
            "name": command,
            "available": false
        }),
    }
}

fn find_command_path(command: &str) -> Option<String> {
    let command_path = Path::new(command);
    if command_path.is_absolute() || command.contains(std::path::MAIN_SEPARATOR) {
        return command_path
            .is_file()
            .then(|| command_path.to_string_lossy().to_string());
    }

    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(command))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().to_string())
}

fn command_succeeds(command: &str, args: &[&str]) -> bool {
    Command::new(command)
        .args(args)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn command_stdout(command: &str, args: &[&str]) -> Option<String> {
    Command::new(command)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| trim_output(&output.stdout))
        .filter(|output| !output.is_empty())
}

fn command_json(command: &str, args: &[&str]) -> Option<Value> {
    command_stdout(command, args).and_then(|stdout| serde_json::from_str(&stdout).ok())
}

fn command_error(command: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(command).args(args).output().ok()?;
    if output.status.success() {
        return None;
    }
    let stderr = trim_output(&output.stderr);
    if !stderr.is_empty() {
        return Some(stderr);
    }
    let stdout = trim_output(&output.stdout);
    if !stdout.is_empty() {
        return Some(stdout);
    }
    Some(format!("command exited with {}", output.status))
}

fn trim_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim()
        .chars()
        .take(1_000)
        .collect()
}

fn decode<T>(value: Value, method: &str) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(value).with_context(|| format!("invalid {method} response"))
}

fn decode_session(value: Value, method: &str) -> Result<CodexSession> {
    if let Some(thread) = value.get("thread") {
        return decode(thread.clone(), method);
    }
    decode(value, method)
}

fn decode_session_or_known(
    value: Value,
    method: &str,
    thread_id: &str,
    name: Option<String>,
    goal: Option<String>,
    metadata: Value,
) -> Result<CodexSession> {
    if let Some(thread) = value.get("thread") {
        return decode(thread.clone(), method);
    }
    if value.get("threadId").is_some() || value.get("id").is_some() {
        return decode(value, method);
    }
    Ok(CodexSession {
        thread_id: thread_id.to_string(),
        name,
        goal,
        metadata,
    })
}

fn decode_turn(value: Value, method: &str) -> Result<CodexTurn> {
    if let Some(turn) = value.get("turn") {
        return decode(turn.clone(), method);
    }
    decode(value, method)
}

fn merge_known_metadata(current: Value, previous: Value) -> Value {
    if current.is_null() {
        previous
    } else {
        current
    }
}

fn decode_array<T>(value: Value, method: &str, fields: &[&str]) -> Result<Vec<T>>
where
    T: serde::de::DeserializeOwned,
{
    if value.is_array() {
        return decode(value, method);
    }

    for field in fields {
        if let Some(array) = value.get(field) {
            return decode(array.clone(), method);
        }
    }

    anyhow::bail!("invalid {method} response: missing {}", fields.join(" or "))
}

impl From<CodexSession> for AdapterSession {
    fn from(session: CodexSession) -> Self {
        Self {
            native_session_id: session.thread_id,
            display_name: session.name,
            goal: session.goal,
            metadata_json: session.metadata,
        }
    }
}

impl From<CodexTurn> for AdapterTurn {
    fn from(turn: CodexTurn) -> Self {
        Self {
            native_turn_id: turn.turn_id,
            status: turn.status,
        }
    }
}

#[cfg(test)]
mod config_override_tests {
    use super::codex_config_override_args_with;

    #[test]
    fn native_runtime_config_uses_environment_key_name_without_secret_value() {
        let args = codex_config_override_args_with(|name| match name {
            "ISSUE_FINDER_CODEX_MODEL" => Some("gpt-test".to_string()),
            "ISSUE_FINDER_CODEX_MODEL_PROVIDER" => Some("cliproxy".to_string()),
            "ISSUE_FINDER_CODEX_REASONING_EFFORT" => Some("xhigh".to_string()),
            "ISSUE_FINDER_CODEX_API_KEY_ENV" => Some("CLIPROXY_API_KEY".to_string()),
            _ => None,
        });

        assert!(args.iter().any(|arg| arg == "model=\"gpt-test\""));
        assert!(args.iter().any(|arg| arg == "model_provider=\"cliproxy\""));
        assert!(args
            .iter()
            .any(|arg| arg == "model_reasoning_effort=\"xhigh\""));
        assert!(args
            .iter()
            .any(|arg| arg == "model_providers.cliproxy.env_key=\"CLIPROXY_API_KEY\""));
        assert!(!args.iter().any(|arg| arg.contains("secret")));
    }
}
