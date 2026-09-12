//! Host-independent contracts, identity, capabilities and channel primitives.
mod action;
mod agent;
mod binding;
mod channel;
mod identity;
mod message_center;
mod model;
mod render;
mod session;

pub use action::{ActionRegistry, ActionScope, ActionTokenError};
pub use agent::{
    AgentAdapter, AgentCapabilities, AgentError, AgentEvent, GoalCommand, HistoryPage,
    InteractionDecision, InteractionKind, InteractionRequest, ItemSummary, MultiplexerKind,
    MultiplexerMutation, MultiplexerMutationResult, MultiplexerPane, MultiplexerSession,
    MultiplexerSnapshot, MultiplexerTarget, MultiplexerWindow, PaneSplitDirection, QueuedPrompt,
    QueuedPromptPort, SessionCommand, SessionCommandChoice, SessionCommandResult,
    SessionControlPort, SessionPage, SessionStatus, SessionSummary, TerminalLocation, ToolSummary,
    TurnStatus, TurnSummary, WorkspaceDirectoryEntry, WorkspaceDirectoryPage, WorkspaceRuntimePort,
};
pub use binding::{AttachOutcome, BindingTable, DeliveryClass, EventImportance};
pub use channel::{
    ActionButton, ActionStyle, ChannelAdapter, ChannelCommand, ChannelError, CommandMenu,
    InboundEnvelope, InboundPayload, MessageRef, OutboundView, OwnerClaimer, ViewSection,
    ViewStatus, include_reply_context,
};
pub use identity::{AgentKind, SessionKey, SessionRef};
pub use message_center::MessageCenter;
pub use model::{ChannelKind, ConversationRef, SessionId};
pub use render::{HistoryWatermark, RenderKey, chunk_text};
pub use session::{
    NativeSessionId, ReadOnlyReason, SessionAccess, SessionCapabilities, SessionCapability,
    SessionOperation, SessionOperationResult,
};
