//! Private, disposable storage for turns that no longer need a hot render buffer.
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use tokio::sync::Mutex;

use super::TurnBuffer;
use crate::{MessageRef, SessionId, TurnStatus};

pub(super) struct ColdTurns {
    connection: Mutex<Option<SqliteConnection>>,
    origin: tokio::time::Instant,
    render_origin: Instant,
}

impl Default for ColdTurns {
    fn default() -> Self {
        Self {
            connection: Mutex::new(None),
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
        let mut guard = self.connection.lock().await;
        let query = if reject {
            "CREATE TRIGGER reject_insert BEFORE INSERT ON turns BEGIN SELECT RAISE(ABORT,'injected cache failure'); END"
        } else {
            "DROP TRIGGER reject_insert"
        };
        sqlx::query(query)
            .execute(guard.as_mut().unwrap())
            .await
            .unwrap();
    }

    pub async fn store(
        &self,
        session: &SessionId,
        turn: &str,
        value: ColdTurn,
    ) -> Result<(), sqlx::Error> {
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
        let data =
            serde_json::to_string(&stored).map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
        let mut guard = self.connection.lock().await;
        if guard.is_none() {
            // SQLite's empty filename creates a private disk database deleted
            // when this connection closes. This is a cache, not a checkpoint.
            let mut connection = SqliteConnection::connect_with(
                &SqliteConnectOptions::new()
                    .filename("")
                    .create_if_missing(true)
                    .pragma("cache_size", "-256")
                    .pragma("auto_vacuum", "FULL")
                    .synchronous(sqlx::sqlite::SqliteSynchronous::Off),
            )
            .await?;
            sqlx::query("CREATE TABLE turns(session TEXT NOT NULL, turn TEXT NOT NULL, data TEXT NOT NULL, PRIMARY KEY(session,turn))")
                .execute(&mut connection).await?;
            *guard = Some(connection);
        }
        sqlx::query("INSERT INTO turns(session,turn,data) VALUES(?,?,?) ON CONFLICT(session,turn) DO UPDATE SET data=excluded.data")
            .bind(session.as_str()).bind(turn).bind(data)
            .execute(guard.as_mut().expect("initialized cold store")).await?;
        Ok(())
    }

    pub async fn load(
        &self,
        session: &SessionId,
        turn: &str,
    ) -> Result<Option<ColdTurn>, sqlx::Error> {
        let mut guard = self.connection.lock().await;
        let Some(connection) = guard.as_mut() else {
            return Ok(None);
        };
        let data: Option<String> =
            sqlx::query_scalar("SELECT data FROM turns WHERE session=? AND turn=?")
                .bind(session.as_str())
                .bind(turn)
                .fetch_optional(connection)
                .await?;
        data.map(|data| {
            let stored: StoredTurn = serde_json::from_str(&data)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
            Ok(ColdTurn {
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
        .transpose()
    }

    pub async fn session_turns(&self, session: &SessionId) -> Result<Vec<String>, sqlx::Error> {
        let mut guard = self.connection.lock().await;
        let Some(connection) = guard.as_mut() else {
            return Ok(Vec::new());
        };
        sqlx::query_scalar("SELECT turn FROM turns WHERE session=?")
            .bind(session.as_str())
            .fetch_all(connection)
            .await
    }

    pub async fn remove(&self, session: &SessionId, turn: &str) -> Result<(), sqlx::Error> {
        let mut guard = self.connection.lock().await;
        if let Some(connection) = guard.as_mut() {
            sqlx::query("DELETE FROM turns WHERE session=? AND turn=?")
                .bind(session.as_str())
                .bind(turn)
                .execute(connection)
                .await?;
        }
        Ok(())
    }

    pub async fn remove_session(&self, session: &SessionId) -> Result<(), sqlx::Error> {
        let mut guard = self.connection.lock().await;
        if let Some(connection) = guard.as_mut() {
            sqlx::query("DELETE FROM turns WHERE session=?")
                .bind(session.as_str())
                .execute(connection)
                .await?;
        }
        Ok(())
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
        {
            let mut guard = cold.connection.lock().await;
            let connection = guard.as_mut().unwrap();
            let budget: i64 = sqlx::query_scalar("PRAGMA cache_size")
                .fetch_one(&mut *connection)
                .await
                .unwrap();
            assert_eq!(budget, -256);
            sqlx::query("CREATE TRIGGER reject_update BEFORE UPDATE ON turns BEGIN SELECT RAISE(ABORT,'injected cache failure'); END")
                .execute(connection).await.unwrap();
        }
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
