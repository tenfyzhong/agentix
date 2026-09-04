use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::broadcast;

use crate::SessionId;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentCapabilities {
    pub queued_prompts: bool,
    pub session_control: bool,
    pub workspace_runtime: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    NotLoaded,
    Idle,
    Active,
    SystemError,
    Offline,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalLocation {
    pub session: String,
    pub window_index: String,
    pub window_name: String,
    pub pane_index: String,
    pub pane_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: SessionId,
    pub name: Option<String>,
    pub preview: Option<String>,
    pub cwd: Option<String>,
    pub updated_at: Option<i64>,
    pub status: SessionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<TerminalLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPage {
    pub sessions: Vec<SessionSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiplexerSnapshot {
    pub sessions: Vec<MultiplexerSession>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiplexerSession {
    pub id: String,
    pub name: String,
    pub windows: Vec<MultiplexerWindow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiplexerWindow {
    pub id: String,
    pub index: String,
    pub name: String,
    pub panes: Vec<MultiplexerPane>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiplexerPane {
    pub id: String,
    pub index: String,
    pub active: bool,
    pub current_command: String,
    pub cwd: String,
    pub codex_session: Option<SessionId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneSplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MultiplexerTarget {
    NewSession {
        name: String,
        cwd: String,
    },
    NewWindow {
        session_id: String,
        name: String,
        cwd: String,
    },
    SplitPane {
        pane_id: String,
        direction: PaneSplitDirection,
        cwd: String,
    },
    ExistingPane {
        pane_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiplexerMutation {
    pub target: MultiplexerTarget,
    pub launch_codex: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiplexerMutationResult {
    pub message: String,
    pub session: Option<SessionSummary>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    #[default]
    InProgress,
    Completed,
    Interrupted,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSummary {
    pub kind: String,
    pub label: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemSummary {
    pub id: String,
    pub kind: String,
    pub text: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnSummary {
    pub id: String,
    pub status: TurnStatus,
    pub user_text: Option<String>,
    pub agent_text: Option<String>,
    pub tools: Vec<ToolSummary>,
    pub items: Vec<ItemSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryPage {
    pub turns: Vec<TurnSummary>,
    pub older_cursor: Option<String>,
    pub newer_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedPrompt {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalCommand {
    Show,
    Set(String),
    Pause,
    Resume,
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCommand {
    Compact,
    Fork,
    Fast(Option<bool>),
    Clear(Option<String>),
    Exit,
    Diff,
    Rename(Option<String>),
    Model(Option<String>),
    Reasoning(Option<String>),
    Skills,
    Plan {
        enabled: bool,
        prompt: Option<String>,
    },
    Goal(GoalCommand),
    Review,
    Status,
    Mcp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCommandChoice {
    pub label: String,
    pub command: SessionCommand,
}

impl SessionCommandChoice {
    #[must_use]
    pub fn new(label: impl Into<String>, command: SessionCommand) -> Self {
        Self {
            label: label.into(),
            command,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCommandResult {
    pub title: String,
    pub body: String,
    pub replacement_session: Option<SessionSummary>,
    pub active_turn: Option<String>,
    pub choices: Vec<SessionCommandChoice>,
}

impl SessionCommandResult {
    #[must_use]
    pub fn message(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            replacement_session: None,
            active_turn: None,
            choices: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionKind {
    CommandApproval,
    FileApproval,
    UserInput,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractionRequest {
    pub rpc_id: Value,
    pub method: String,
    pub session_id: String,
    pub turn_id: String,
    pub item_id: Option<String>,
    pub kind: InteractionKind,
    pub title: String,
    pub detail: String,
    pub available_decisions: Vec<String>,
    pub payload: Value,
    pub auto_resolution_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractionDecision {
    pub rpc_id: Value,
    pub response: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentEvent {
    Connected {
        generation: u64,
    },
    Disconnected {
        generation: u64,
        reason: String,
    },
    SessionStatusChanged {
        session_id: String,
        status: SessionStatus,
    },
    SessionExited {
        session_id: String,
    },
    SessionResumed {
        session_id: String,
    },
    QueueChanged {
        session_id: String,
    },
    TurnStarted {
        session_id: String,
        turn_id: String,
    },
    UserMessage {
        session_id: String,
        turn_id: String,
        item_id: String,
        text: String,
    },
    AgentMessageDelta {
        session_id: String,
        turn_id: String,
        item_id: String,
        delta: String,
    },
    ItemStarted {
        session_id: String,
        turn_id: String,
        item_id: String,
        kind: String,
        label: String,
    },
    ItemCompleted {
        session_id: String,
        turn_id: String,
        item: ItemSummary,
    },
    TurnCompleted {
        session_id: String,
        turn_id: String,
        status: TurnStatus,
        error: Option<String>,
    },
    InteractionRequested(InteractionRequest),
    InteractionResolved {
        session_id: String,
        request_id: String,
    },
}

impl AgentEvent {
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::Connected { .. } | Self::Disconnected { .. } => None,
            Self::SessionStatusChanged { session_id, .. }
            | Self::SessionExited { session_id }
            | Self::SessionResumed { session_id }
            | Self::QueueChanged { session_id }
            | Self::TurnStarted { session_id, .. }
            | Self::UserMessage { session_id, .. }
            | Self::AgentMessageDelta { session_id, .. }
            | Self::ItemStarted { session_id, .. }
            | Self::ItemCompleted { session_id, .. }
            | Self::TurnCompleted { session_id, .. }
            | Self::InteractionResolved { session_id, .. } => Some(session_id),
            Self::InteractionRequested(request) => Some(&request.session_id),
        }
    }
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("agent transport is unavailable: {0}")]
    Unavailable(String),
    #[error("agent rejected the request: {0}")]
    Rejected(String),
    #[error("agent protocol error: {0}")]
    Protocol(String),
}

#[async_trait]
pub trait QueuedPromptPort: Send + Sync {
    async fn queue_prompt(
        &self,
        session_id: &SessionId,
        text: &str,
        client_message_id: &str,
    ) -> Result<QueuedPrompt, AgentError>;

    async fn list_queued_prompts(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<QueuedPrompt>, AgentError>;
}

#[async_trait]
pub trait SessionControlPort: Send + Sync {
    async fn run_session_command(
        &self,
        session_id: &SessionId,
        command: SessionCommand,
    ) -> Result<SessionCommandResult, AgentError>;
}

#[async_trait]
pub trait WorkspaceRuntimePort: Send + Sync {
    fn default_directory(&self) -> String;

    async fn snapshot(&self) -> Result<Option<MultiplexerSnapshot>, AgentError>;

    async fn mutate(
        &self,
        mutation: MultiplexerMutation,
    ) -> Result<MultiplexerMutationResult, AgentError>;
}

#[async_trait]
pub trait AgentAdapter: Send + Sync {
    fn display_name(&self) -> &'static str;

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            queued_prompts: self.queued_prompts().is_some(),
            session_control: self.session_control().is_some(),
            workspace_runtime: self.workspace_runtime().is_some(),
        }
    }

    fn queued_prompts(&self) -> Option<&dyn QueuedPromptPort> {
        None
    }

    fn session_control(&self) -> Option<&dyn SessionControlPort> {
        None
    }

    fn workspace_runtime(&self) -> Option<&dyn WorkspaceRuntimePort> {
        None
    }

    async fn list_sessions(
        &self,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<SessionPage, AgentError>;

    async fn read_history(
        &self,
        session_id: &SessionId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<HistoryPage, AgentError>;

    async fn attach(&self, session_id: &SessionId) -> Result<(), AgentError>;
    async fn unsubscribe(&self, session_id: &SessionId) -> Result<(), AgentError>;
    async fn start_turn(&self, session_id: &SessionId, text: &str) -> Result<String, AgentError>;
    async fn steer(
        &self,
        session_id: &SessionId,
        expected_turn_id: &str,
        text: &str,
    ) -> Result<String, AgentError>;
    async fn interrupt(&self, session_id: &SessionId, turn_id: &str) -> Result<(), AgentError>;
    async fn resolve_interaction(&self, decision: InteractionDecision) -> Result<(), AgentError>;
    fn subscribe(&self) -> broadcast::Receiver<AgentEvent>;
    fn generation(&self) -> u64;
}
