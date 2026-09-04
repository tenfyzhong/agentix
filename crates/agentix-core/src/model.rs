use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// An opaque identifier assigned by an agent runtime.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn short(&self) -> &str {
        let end = self.0.floor_char_boundary(8);
        &self.0[..end]
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A supported instant-messaging transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Telegram,
    Feishu,
}

impl fmt::Display for ChannelKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Telegram => "telegram",
            Self::Feishu => "feishu",
        })
    }
}

impl FromStr for ChannelKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "telegram" => Ok(Self::Telegram),
            "feishu" => Ok(Self::Feishu),
            _ => Err(format!("unsupported channel kind: {value}")),
        }
    }
}

/// A chat namespace. Platform threads can be encoded in `conversation_id` by
/// their channel adapter without leaking that detail into the core.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationRef {
    pub channel: ChannelKind,
    pub conversation_id: String,
}

impl ConversationRef {
    #[must_use]
    pub fn new(channel: ChannelKind, conversation_id: impl Into<String>) -> Self {
        Self {
            channel,
            conversation_id: conversation_id.into(),
        }
    }
}
