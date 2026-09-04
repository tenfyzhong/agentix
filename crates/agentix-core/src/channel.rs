use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{ChannelKind, ConversationRef};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelCommand {
    pub name: String,
    pub description: String,
    pub contextual: bool,
}

impl ChannelCommand {
    #[must_use]
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            contextual: false,
        }
    }

    #[must_use]
    pub fn contextual(mut self) -> Self {
        self.contextual = true;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandMenu {
    pub commands: Vec<ChannelCommand>,
}

impl CommandMenu {
    #[must_use]
    pub fn new(commands: Vec<ChannelCommand>) -> Self {
        Self { commands }
    }
}

/// Adds an earlier IM message as model-visible context for a normal prompt.
#[must_use]
pub fn include_reply_context(input: &str, quoted: Option<&str>) -> String {
    if input.trim_start().starts_with('/') {
        return input.to_owned();
    }
    let Some(quoted) = quoted.map(str::trim).filter(|quoted| !quoted.is_empty()) else {
        return input.to_owned();
    };
    let quoted = quoted
        .lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("**Quoted message**\n\n{quoted}\n\n{input}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InboundPayload {
    Text(String),
    Action {
        token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<MessageRef>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundEnvelope {
    pub event_id: String,
    pub conversation: ConversationRef,
    pub owner_id: String,
    pub payload: InboundPayload,
}

impl InboundEnvelope {
    #[must_use]
    pub fn text(
        event_id: impl Into<String>,
        conversation: ConversationRef,
        owner_id: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            event_id: event_id.into(),
            conversation,
            owner_id: owner_id.into(),
            payload: InboundPayload::Text(text.into()),
        }
    }

    #[must_use]
    pub fn action(
        event_id: impl Into<String>,
        conversation: ConversationRef,
        owner_id: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            event_id: event_id.into(),
            conversation,
            owner_id: owner_id.into(),
            payload: InboundPayload::Action {
                token: token.into(),
                message: None,
            },
        }
    }

    #[must_use]
    pub fn action_from_message(
        event_id: impl Into<String>,
        conversation: ConversationRef,
        owner_id: impl Into<String>,
        token: impl Into<String>,
        message: MessageRef,
    ) -> Self {
        Self {
            event_id: event_id.into(),
            conversation,
            owner_id: owner_id.into(),
            payload: InboundPayload::Action {
                token: token.into(),
                message: Some(message),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewStatus {
    Info,
    Running,
    Waiting,
    Success,
    Warning,
    Error,
    Muted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStyle {
    Primary,
    Default,
    Danger,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionButton {
    pub label: String,
    pub token: String,
    pub style: ActionStyle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundView {
    pub title: String,
    pub subtitle: Option<String>,
    pub body: String,
    pub status: ViewStatus,
    pub actions: Vec<ActionButton>,
}

impl OutboundView {
    #[must_use]
    pub fn text(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            body: body.into(),
            status: ViewStatus::Info,
            actions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageRef {
    pub conversation: ConversationRef,
    pub message_id: String,
}

impl MessageRef {
    #[must_use]
    pub fn new(conversation: ConversationRef, message_id: impl Into<String>) -> Self {
        Self {
            conversation,
            message_id: message_id.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum ChannelError {
    #[error("channel transport failed: {0}")]
    Transport(String),
    #[error("channel rejected the message: {0}")]
    Rejected(String),
    #[error("channel payload is invalid: {0}")]
    InvalidPayload(String),
}

#[async_trait]
pub trait ChannelAdapter: Send + Sync {
    fn kind(&self) -> ChannelKind;

    async fn run(
        &self,
        _inbound: mpsc::Sender<InboundEnvelope>,
        _shutdown: CancellationToken,
    ) -> Result<(), ChannelError> {
        Ok(())
    }

    async fn send(
        &self,
        conversation: &ConversationRef,
        view: &OutboundView,
    ) -> Result<MessageRef, ChannelError>;

    async fn update(
        &self,
        conversation: &ConversationRef,
        message: &MessageRef,
        view: &OutboundView,
    ) -> Result<(), ChannelError>;

    async fn disable_actions(&self, _message: &MessageRef) -> Result<(), ChannelError> {
        Ok(())
    }

    async fn set_command_menu(
        &self,
        _conversation: &ConversationRef,
        _menu: &CommandMenu,
    ) -> Result<(), ChannelError> {
        Ok(())
    }
}
