use crate::SessionId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Codex,
    Pi,
    #[serde(alias = "oh-my-pi")]
    Omp,
    Claude,
}
impl AgentKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Pi => "pi",
            Self::Omp => "omp",
        }
    }
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Pi => "Pi",
            Self::Omp => "OMP",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionRef {
    pub agent: AgentKind,
    pub native_id: crate::NativeSessionId,
}
pub type SessionKey = SessionRef;
impl SessionRef {
    #[must_use]
    pub fn new(agent: AgentKind, native_id: SessionId) -> Self {
        Self::from_native(agent, native_id.into())
    }
    #[must_use]
    pub const fn from_native(agent: AgentKind, native_id: crate::NativeSessionId) -> Self {
        Self { agent, native_id }
    }
    #[must_use]
    pub fn encode(&self) -> SessionId {
        SessionId::new(format!("{}:{}", self.agent.as_str(), self.native_id))
    }
    #[must_use]
    pub fn decode(id: &SessionId) -> Option<Self> {
        let (kind, native) = id.as_str().split_once(':')?;
        let agent = match kind {
            "claude" => AgentKind::Claude,
            "codex" => AgentKind::Codex,
            "pi" => AgentKind::Pi,
            "omp" | "oh-my-pi" => AgentKind::Omp,
            _ => return None,
        };
        (!native.is_empty()).then(|| Self::new(agent, SessionId::new(native)))
    }
}
