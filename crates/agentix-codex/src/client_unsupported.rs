use std::path::Path;

use agentix_core::{
    AgentAdapter, AgentError, AgentEvent, HistoryPage, InteractionDecision, SessionId, SessionPage,
};
use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::broadcast;

use crate::CodexEndpoint;

#[derive(Debug, Error)]
#[error("the Codex app-server Unix socket transport is unavailable on this platform")]
pub struct ClientError;

#[derive(Debug, Clone, Copy, Default)]
pub struct CodexClient;

impl CodexClient {
    pub async fn connect(_endpoint: CodexEndpoint) -> Result<Self, ClientError> {
        Err(ClientError)
    }

    pub async fn connect_with_command(
        _endpoint: CodexEndpoint,
        _command: &Path,
    ) -> Result<Self, ClientError> {
        Err(ClientError)
    }

    pub async fn connect_with_command_and_rmux_directory(
        _endpoint: CodexEndpoint,
        _command: &Path,
        _rmux_directory: &Path,
    ) -> Result<Self, ClientError> {
        Err(ClientError)
    }

    pub async fn connect_with_background_turn_notifications(
        _endpoint: CodexEndpoint,
        _command: &Path,
        _rmux_directory: &Path,
        _background_turn_notifications: bool,
    ) -> Result<Self, ClientError> {
        Err(ClientError)
    }

    pub async fn request(&self, _method: &str, _params: Value) -> Result<Value, ClientError> {
        Err(ClientError)
    }
}

#[async_trait]
impl AgentAdapter for CodexClient {
    fn display_name(&self) -> &'static str {
        "Codex"
    }

    async fn list_sessions(
        &self,
        _cursor: Option<String>,
        _limit: u32,
    ) -> Result<SessionPage, AgentError> {
        Err(unsupported())
    }

    async fn read_history(
        &self,
        _session_id: &SessionId,
        _cursor: Option<String>,
        _limit: u32,
    ) -> Result<HistoryPage, AgentError> {
        Err(unsupported())
    }

    async fn attach(&self, _session_id: &SessionId) -> Result<(), AgentError> {
        Err(unsupported())
    }

    async fn unsubscribe(&self, _session_id: &SessionId) -> Result<(), AgentError> {
        Err(unsupported())
    }

    async fn start_turn(&self, _session_id: &SessionId, _text: &str) -> Result<String, AgentError> {
        Err(unsupported())
    }

    async fn steer(
        &self,
        _session_id: &SessionId,
        _expected_turn_id: &str,
        _text: &str,
    ) -> Result<String, AgentError> {
        Err(unsupported())
    }

    async fn interrupt(&self, _session_id: &SessionId, _turn_id: &str) -> Result<(), AgentError> {
        Err(unsupported())
    }

    async fn resolve_interaction(&self, _decision: InteractionDecision) -> Result<(), AgentError> {
        Err(unsupported())
    }

    fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        let (_events, receiver) = broadcast::channel(1);
        receiver
    }

    fn generation(&self) -> u64 {
        0
    }
}

fn unsupported() -> AgentError {
    AgentError::Unavailable(ClientError.to_string())
}
