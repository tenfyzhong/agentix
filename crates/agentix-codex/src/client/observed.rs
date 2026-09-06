use agentix_core::{AgentEvent, ItemSummary, SessionId, TurnStatus, TurnSummary};

use super::{ClientError, CodexClient};

impl CodexClient {
    pub(super) async fn latest_stored_turn(
        &self,
        session: &SessionId,
    ) -> Result<Option<TurnSummary>, ClientError> {
        let history = match self.paged_history(session, None, 1).await {
            Err(ClientError::Rpc { code: -32601, .. }) => self.stable_history(session, 1).await?,
            result => result?,
        };
        Ok(history.turns.into_iter().last())
    }

    pub(super) async fn poll_observed_sessions(&self) {
        let sessions = self
            .observed
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for session in sessions {
            let turn = match self.latest_stored_turn(&session).await {
                Ok(Some(turn)) => turn,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(%error, %session, "failed to read observed Codex session");
                    continue;
                }
            };
            let mut observed = self.observed.lock().await;
            let Some(previous) = observed.get_mut(&session) else {
                continue; // Detach may have completed while the request was in flight.
            };
            if previous.as_ref() == Some(&turn) {
                continue;
            }
            if turn.status == TurnStatus::InProgress
                && previous.as_ref().is_none_or(|old| old.id != turn.id)
            {
                let _ = self.events.send(AgentEvent::TurnStarted {
                    session_id: session.to_string(),
                    turn_id: turn.id.clone(),
                });
            }
            for (kind, text) in [
                ("userMessage", &turn.user_text),
                ("agentMessage", &turn.agent_text),
            ] {
                let _ = self.events.send(AgentEvent::ItemCompleted {
                    session_id: session.to_string(),
                    turn_id: turn.id.clone(),
                    item: ItemSummary {
                        id: format!("observed-{}-{kind}", turn.id),
                        kind: kind.into(),
                        text: text.clone(),
                        status: None,
                    },
                });
            }
            if matches!(
                turn.status,
                TurnStatus::Completed | TurnStatus::Failed | TurnStatus::Interrupted
            ) {
                self.completed_turns
                    .lock()
                    .await
                    .insert(session.clone(), turn.id.clone());
                let _ = self.events.send(AgentEvent::TurnCompleted {
                    session_id: session.to_string(),
                    turn_id: turn.id.clone(),
                    status: turn.status.clone(),
                    error: None,
                });
            }
            *previous = Some(turn);
        }
    }
}
