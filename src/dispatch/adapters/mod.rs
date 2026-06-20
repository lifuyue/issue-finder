use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod codex_app_server;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AdapterStartSessionRequest {
    pub display_name: String,
    pub goal: Option<String>,
    pub metadata_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AdapterSession {
    pub native_session_id: String,
    pub display_name: Option<String>,
    pub goal: Option<String>,
    pub metadata_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AdapterTurn {
    pub native_turn_id: String,
    pub status: Option<String>,
}

pub trait NativeExecutionAdapter {
    fn adapter_start_session(
        &mut self,
        request: AdapterStartSessionRequest,
    ) -> Result<AdapterSession>;

    fn adapter_resume_session(&mut self, native_session_id: &str) -> Result<AdapterSession>;

    fn adapter_fork_session(&mut self, native_session_id: &str) -> Result<AdapterSession>;

    fn adapter_rename_session(
        &mut self,
        native_session_id: &str,
        display_name: &str,
    ) -> Result<AdapterSession>;

    fn adapter_set_goal(&mut self, native_session_id: &str, goal: &str) -> Result<AdapterSession>;

    fn adapter_set_metadata(
        &mut self,
        native_session_id: &str,
        metadata_json: Value,
    ) -> Result<AdapterSession>;

    fn adapter_start_turn(&mut self, native_session_id: &str, prompt: &str) -> Result<AdapterTurn>;

    fn adapter_read_transcript(&mut self, native_session_id: &str) -> Result<Value>;

    fn adapter_archive_session(&mut self, native_session_id: &str) -> Result<AdapterSession>;

    fn adapter_list_sessions(&mut self, limit: Option<usize>) -> Result<Vec<AdapterSession>>;

    fn adapter_search_sessions(
        &mut self,
        search_term: &str,
        limit: Option<usize>,
    ) -> Result<Vec<AdapterSession>>;
}
