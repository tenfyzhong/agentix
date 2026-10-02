//! Shared terminal interactions, independent of native agent commands.
use crate::{AgentError, AgentKind};
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalInteractionTarget {
    pub pid: u32,
    pub client_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalInteractionKind {
    Choice,
    Confirmation,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalInteraction {
    pub kind: TerminalInteractionKind,
    pub title: String,
    pub detail: String,
    pub choices: Vec<String>,
    pub selected: Option<usize>,
    /// The semantic dialog identity, excluding selection and ANSI styling.
    pub fingerprint: String,
    /// Filled by the terminal adapter, never inferred from displayed text.
    pub pane_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalInteractionResponse {
    Choice(usize),
    Cancel,
}

#[async_trait]
pub trait TerminalInteractionPort: Send + Sync {
    async fn inspect(
        &self,
        agent: AgentKind,
        target: &TerminalInteractionTarget,
    ) -> Result<Option<TerminalInteraction>, AgentError>;
    async fn respond(
        &self,
        agent: AgentKind,
        target: &TerminalInteractionTarget,
        expected: &TerminalInteraction,
        response: TerminalInteractionResponse,
    ) -> Result<(), AgentError>;
}
