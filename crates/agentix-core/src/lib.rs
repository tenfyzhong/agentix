//! Platform-neutral Agentix domain model and orchestration primitives.

mod action;
mod agent;
mod binding;
mod channel;
mod command;
mod engine;
mod message_center;
mod model;
mod render;
mod state;

pub use action::{ActionRegistry, ActionScope, ActionTokenError};
pub use agent::{
    AgentAdapter, AgentCapabilities, AgentError, AgentEvent, GoalCommand, HistoryPage,
    InteractionDecision, InteractionKind, InteractionRequest, ItemSummary, MultiplexerMutation,
    MultiplexerMutationResult, MultiplexerPane, MultiplexerSession, MultiplexerSnapshot,
    MultiplexerTarget, MultiplexerWindow, PaneSplitDirection, QueuedPrompt, QueuedPromptPort,
    SessionCommand, SessionCommandChoice, SessionCommandResult, SessionControlPort, SessionPage,
    SessionStatus, SessionSummary, TerminalLocation, ToolSummary, TurnStatus, TurnSummary,
    WorkspaceRuntimePort,
};
pub use binding::{AttachOutcome, BindingTable, DeliveryClass, EventImportance};
pub use channel::{
    ActionButton, ActionStyle, ChannelAdapter, ChannelCommand, ChannelError, CommandMenu,
    InboundEnvelope, InboundPayload, MessageRef, OutboundView, ViewStatus, include_reply_context,
};
pub use command::{AgentCommand, InputParseError, ParsedInput, parse_input};
pub use engine::{Engine, EngineError};
pub use message_center::MessageCenter;
pub use model::{ChannelKind, ConversationRef, SessionId};
pub use render::{HistoryWatermark, RenderKey, chunk_text};
pub use state::SqliteState;
