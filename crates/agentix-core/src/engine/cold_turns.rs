//! Private, disposable storage for turns that no longer need a hot render buffer.
use std::time::{Duration, Instant};

use agentix_storage::{StorageError, TurnCache};
use serde::{Deserialize, Serialize};

use super::TurnBuffer;
use crate::{MessageRef, SessionId, TurnStatus};

pub(super) struct ColdTurns {
    cache: TurnCache,
    origin: tokio::time::Instant,
    render_origin: Instant,
}

impl Default for ColdTurns {
    fn default() -> Self {
        Self {
            cache: TurnCache::default(),
            origin: tokio::time::Instant::now(),
            render_origin: Instant::now(),
        }
    }
}

pub(super) struct ColdTurn {
    pub buffer: TurnBuffer,
    pub message: Option<MessageRef>,
    pub last_render: Option<Instant>,
}

#[derive(Serialize, Deserialize)]
struct StoredTurn {
    user_text: String,
    agent_text: String,
    status: TurnStatus,
    started_at: Option<Duration>,
    rendered_elapsed_seconds: Option<u64>,
    message: Option<MessageRef>,
    last_render: Option<Duration>,
}

impl ColdTurns {
    #[cfg(test)]
    pub(super) async fn reject_writes(&self, reject: bool) {
        self.cache.reject_writes(reject).await;
    }

    pub async fn store(
        &self,
        session: &SessionId,
        turn: &str,
        value: ColdTurn,
    ) -> Result<(), StorageError> {
        let stored = StoredTurn {
            user_text: value.buffer.user_text,
            agent_text: value.buffer.agent_text,
            status: value.buffer.status,
            started_at: value
                .buffer
                .started_at
                .map(|start| start.saturating_duration_since(self.origin)),
            rendered_elapsed_seconds: value.buffer.rendered_elapsed_seconds,
            message: value.message,
            last_render: value
                .last_render
                .map(|start| start.saturating_duration_since(self.render_origin)),
        };
        self.cache.store(session, turn, &stored).await
    }

    pub async fn load(
        &self,
        session: &SessionId,
        turn: &str,
    ) -> Result<Option<ColdTurn>, StorageError> {
        self.cache
            .load::<StoredTurn>(session, turn)
            .await
            .map(|value| {
                value.map(|stored| ColdTurn {
                    buffer: TurnBuffer {
                        user_text: stored.user_text,
                        agent_text: stored.agent_text,
                        status: stored.status,
                        started_at: stored.started_at.map(|offset| self.origin + offset),
                        rendered_elapsed_seconds: stored.rendered_elapsed_seconds,
                    },
                    message: stored.message,
                    last_render: stored.last_render.map(|offset| self.render_origin + offset),
                })
            })
    }

    pub async fn session_turns(&self, session: &SessionId) -> Result<Vec<String>, StorageError> {
        self.cache.session_turns(session).await
    }
    pub async fn remove(&self, session: &SessionId, turn: &str) -> Result<(), StorageError> {
        self.cache.remove(session, turn).await
    }
    pub async fn remove_session(&self, session: &SessionId) -> Result<(), StorageError> {
        self.cache.remove_session(session).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChannelKind, ConversationRef};

    fn value(text: &str, started: tokio::time::Instant) -> ColdTurn {
        ColdTurn {
            buffer: TurnBuffer {
                user_text: "Question".into(),
                agent_text: text.into(),
                status: TurnStatus::Completed,
                started_at: Some(started),
                rendered_elapsed_seconds: Some(1),
            },
            message: Some(MessageRef::new(
                ConversationRef::new(ChannelKind::Telegram, "chat"),
                "message",
            )),
            last_render: Some(Instant::now()),
        }
    }

    #[tokio::test]
    async fn failed_cold_write_preserves_previous_body_and_exact_start_time() {
        let cold = ColdTurns::default();
        let session = SessionId::new("session");
        let started = tokio::time::Instant::now();
        cold.store(&session, "turn", value("Original", started))
            .await
            .unwrap();
        cold.reject_writes(true).await;
        assert!(
            cold.store(&session, "turn", value("Replacement", started))
                .await
                .is_err()
        );
        let restored = cold.load(&session, "turn").await.unwrap().unwrap();
        assert_eq!(restored.buffer.agent_text, "Original");
        assert_eq!(restored.buffer.started_at, Some(started));
        assert_eq!(restored.message.unwrap().message_id, "message");
        cold.remove(&session, "turn").await.unwrap();
        assert!(cold.session_turns(&session).await.unwrap().is_empty());
        assert!(cold.load(&session, "turn").await.unwrap().is_none());
    }
}
