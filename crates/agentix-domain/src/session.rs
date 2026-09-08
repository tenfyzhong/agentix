//! Session identity and negotiated operations, independent of any host or IM channel.
use crate::{HistoryPage, SessionCommand, SessionCommandResult, SessionId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// An opaque host identifier. Colons and backend-looking prefixes have no meaning here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NativeSessionId(SessionId);
impl NativeSessionId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(SessionId::new(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
    /// Compatibility boundary for adapters using the original session ID API.
    #[must_use]
    pub const fn adapter_id(&self) -> &SessionId {
        &self.0
    }
}
impl From<SessionId> for NativeSessionId {
    fn from(value: SessionId) -> Self {
        Self(value)
    }
}
impl std::fmt::Display for NativeSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionCapability {
    Prompt,
    Steer,
    History,
    Stop,
    Queue,
    QueueControl,
    Compact,
    Fork,
    Fast,
    Clear,
    Exit,
    Diff,
    Rename,
    Model,
    Reasoning,
    Skills,
    Plan,
    Goal,
    Review,
    Status,
    Mcp,
}
impl SessionCapability {
    pub const COMMANDS: [Self; 15] = [
        Self::Compact,
        Self::Fork,
        Self::Fast,
        Self::Clear,
        Self::Exit,
        Self::Diff,
        Self::Rename,
        Self::Model,
        Self::Reasoning,
        Self::Skills,
        Self::Plan,
        Self::Goal,
        Self::Review,
        Self::Status,
        Self::Mcp,
    ];
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(name.into())).ok()
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionCapabilities(BTreeSet<SessionCapability>);
impl SessionCapabilities {
    #[must_use]
    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        Self(
            names
                .into_iter()
                .filter_map(SessionCapability::from_name)
                .collect(),
        )
    }
    #[must_use]
    pub fn supports(&self, capability: SessionCapability) -> bool {
        self.0.contains(&capability)
    }
    pub fn insert(&mut self, capability: SessionCapability) {
        self.0.insert(capability);
    }
}
impl FromIterator<SessionCapability> for SessionCapabilities {
    fn from_iter<T: IntoIterator<Item = SessionCapability>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadOnlyReason {
    OwnedByOtherProcess,
    OriginalHostUnavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum SessionAccess {
    Writable,
    ReadOnly(ReadOnlyReason),
    Offline,
}
impl SessionAccess {
    #[must_use]
    pub const fn can_write(self) -> bool {
        matches!(self, Self::Writable)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum SessionOperation {
    Send {
        session: SessionId,
        text: String,
        expected_turn: Option<String>,
    },
    Stop {
        session: SessionId,
        turn: String,
    },
    History {
        session: SessionId,
        cursor: Option<String>,
        limit: u32,
    },
    Command {
        session: SessionId,
        command: SessionCommand,
    },
}
/// Typed domain output; presentation adapters choose their own serialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum SessionOperationResult {
    Started { turn_id: String },
    Stopped {},
    History(HistoryPage),
    Command(Box<SessionCommandResult>),
}
