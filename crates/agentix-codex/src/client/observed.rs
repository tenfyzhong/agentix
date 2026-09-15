use std::collections::HashSet;

use agentix_domain::{AgentEvent, ItemSummary, SessionId, TurnStatus, TurnSummary};

use super::{ClientError, CodexClient};

fn item_key(item: &ItemSummary) -> (&str, &str, Option<&str>, Option<&str>) {
    let ItemSummary {
        id,
        kind,
        text,
        status,
    } = item;
    (id, kind, text.as_deref(), status.as_deref())
}

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

    pub(super) async fn poll_observed_session(&self, session: &SessionId) {
        let turn = match self.latest_stored_turn(session).await {
            Ok(Some(turn)) => turn,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, %session, "failed to read observed Codex session");
                return;
            }
        };
        let mut observed = self.observed.lock().await;
        let Some(previous) = observed.get_mut(session) else {
            return; // Detach may have completed while the request was in flight.
        };
        if previous.as_ref() == Some(&turn) {
            return;
        }
        self.publish_turn_snapshot(session, previous.as_ref(), &turn)
            .await;
        *previous = Some(turn);
    }

    pub(super) async fn publish_turn_snapshot(
        &self,
        session: &SessionId,
        previous: Option<&TurnSummary>,
        turn: &TurnSummary,
    ) {
        if turn.status == TurnStatus::InProgress && previous.is_none_or(|old| old.id != turn.id) {
            let _ = self.events.send(AgentEvent::TurnStarted {
                session_id: session.to_string(),
                turn_id: turn.id.clone(),
            });
        }
        let old = previous.filter(|old| old.id == turn.id);
        // Keep the aggregate prompt, but preserve native output identities so
        // updates replace items restored by read-only attach.
        let mut items = Vec::new();
        if old.is_none_or(|old| old.user_text != turn.user_text) {
            items.push(ItemSummary {
                id: format!("observed-{}-userMessage", turn.id),
                kind: "userMessage".into(),
                text: turn.user_text.clone(),
                status: None,
            });
        }
        // Borrow all equality fields: duplicate IDs can hold different content.
        // Tiny snapshots avoid allocating an index; long turns avoid quadratic scans.
        let old_items = old
            .filter(|old| old.items.len() > 16)
            .map(|old| old.items.iter().map(item_key).collect::<HashSet<_>>());
        items.extend(
            turn.items
                .iter()
                .filter(|item| item.kind != "userMessage")
                .filter(|item| match &old_items {
                    Some(index) => !index.contains(&item_key(item)),
                    None => old.is_none_or(|old| !old.items.contains(item)),
                })
                .cloned(),
        );
        if !turn.items.iter().any(|item| item.kind == "agentMessage")
            && old.is_none_or(|old| old.agent_text != turn.agent_text)
        {
            items.push(ItemSummary {
                id: format!("observed-{}-agentMessage", turn.id),
                kind: "agentMessage".into(),
                text: turn.agent_text.clone(),
                status: None,
            });
        }
        for item in items {
            let _ = self.events.send(AgentEvent::ItemCompleted {
                session_id: session.to_string(),
                turn_id: turn.id.clone(),
                item,
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
    }
}
