use crate::{
    AgentAdapter, AgentError, HistoryPage, SessionCapability, SessionCommand, SessionCommandResult,
    SessionId, SessionOperation, SessionOperationResult,
};
use std::sync::Arc;

/// Shared policy boundary used by control clients and IM projections.
#[derive(Clone)]
pub struct SessionOperations {
    agent: Arc<dyn AgentAdapter>,
}
impl SessionOperations {
    #[must_use]
    pub fn new(agent: Arc<dyn AgentAdapter>) -> Self {
        Self { agent }
    }
    async fn require(
        &self,
        session: &SessionId,
        capability: SessionCapability,
    ) -> Result<(), AgentError> {
        if !self.agent.session_access(session).await.can_write() {
            return Err(AgentError::Rejected(
                "The original session is unavailable for changes".into(),
            ));
        }
        if !self
            .agent
            .session_capabilities(session)
            .await
            .supports(capability)
        {
            return Err(AgentError::Rejected(
                "Operation is not supported by this session".into(),
            ));
        }
        Ok(())
    }
    pub async fn send(
        &self,
        session: &SessionId,
        text: &str,
        expected_turn: Option<&str>,
    ) -> Result<String, AgentError> {
        if text.trim().is_empty() {
            return Err(AgentError::Rejected("A nonempty prompt is required".into()));
        }
        self.require(
            session,
            if expected_turn.is_some() {
                SessionCapability::Steer
            } else {
                SessionCapability::Prompt
            },
        )
        .await?;
        match expected_turn {
            Some(turn) => self.agent.steer(session, turn, text).await,
            None => self.agent.start_turn(session, text).await,
        }
    }
    pub async fn stop(&self, session: &SessionId, turn: &str) -> Result<(), AgentError> {
        self.require(session, SessionCapability::Stop).await?;
        self.agent.interrupt(session, turn).await
    }
    pub async fn history(
        &self,
        session: &SessionId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<HistoryPage, AgentError> {
        self.agent.read_history(session, cursor, limit).await
    }
    pub async fn command(
        &self,
        session: &SessionId,
        command: SessionCommand,
    ) -> Result<SessionCommandResult, AgentError> {
        self.require(session, command.capability()).await?;
        self.agent
            .session_control()
            .ok_or_else(|| AgentError::Rejected("Session commands are unavailable".into()))?
            .run_session_command(session, command)
            .await
    }
}

impl SessionOperations {
    pub async fn execute(
        &self,
        operation: SessionOperation,
    ) -> Result<SessionOperationResult, AgentError> {
        Ok(match operation {
            SessionOperation::Send {
                session,
                text,
                expected_turn,
            } => SessionOperationResult::Started {
                turn_id: self.send(&session, &text, expected_turn.as_deref()).await?,
            },
            SessionOperation::Stop { session, turn } => {
                self.stop(&session, &turn).await?;
                SessionOperationResult::Stopped {}
            }
            SessionOperation::History {
                session,
                cursor,
                limit,
            } => SessionOperationResult::History(self.history(&session, cursor, limit).await?),
            SessionOperation::Command { session, command } => {
                SessionOperationResult::Command(Box::new(self.command(&session, command).await?))
            }
        })
    }
}
