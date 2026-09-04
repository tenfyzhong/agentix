use std::path::Path;
use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

use crate::{AttachOutcome, ChannelKind, ConversationRef, MessageRef, SessionId, TurnStatus};

#[derive(Debug, Clone)]
pub(crate) struct StoredTurnView {
    pub session_id: SessionId,
    pub turn_id: String,
    pub message: MessageRef,
    pub owner_id: Option<String>,
    pub user_text: String,
    pub agent_text: String,
    pub status: TurnStatus,
}

#[derive(Debug, Clone)]
pub struct SqliteState {
    pool: SqlitePool,
}

impl SqliteState {
    pub async fn in_memory() -> Result<Self, sqlx::Error> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?
            .foreign_keys(true)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let state = Self { pool };
        state.migrate().await?;
        Ok(state)
    }

    pub async fn open(path: &Path) -> Result<Self, sqlx::Error> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .foreign_keys(true)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        let state = Self { pool };
        state.migrate().await?;
        Ok(state)
    }

    async fn migrate(&self) -> Result<(), sqlx::Error> {
        sqlx::query("PRAGMA journal_mode = WAL")
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS binding_epochs (\
                channel TEXT NOT NULL, \
                conversation_id TEXT NOT NULL, \
                epoch INTEGER NOT NULL, \
                PRIMARY KEY (channel, conversation_id)\
            )",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS bindings (\
                channel TEXT NOT NULL, \
                conversation_id TEXT NOT NULL, \
                session_id TEXT NOT NULL UNIQUE, \
                epoch INTEGER NOT NULL, \
                attached_at INTEGER NOT NULL DEFAULT (unixepoch()), \
                PRIMARY KEY (channel, conversation_id)\
            )",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS processed_events (\
                channel TEXT NOT NULL, \
                event_id TEXT NOT NULL, \
                status TEXT NOT NULL DEFAULT 'completed', \
                attempts INTEGER NOT NULL DEFAULT 1, \
                processed_at INTEGER NOT NULL DEFAULT (unixepoch()), \
                PRIMARY KEY (channel, event_id)\
            )",
        )
        .execute(&self.pool)
        .await?;
        self.ensure_processed_event_columns().await?;
        sqlx::query("UPDATE processed_events SET status = 'failed' WHERE status = 'processing'")
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pending_interactions (\
                token TEXT PRIMARY KEY, \
                channel TEXT NOT NULL, \
                conversation_id TEXT NOT NULL, \
                owner_id TEXT NOT NULL, \
                session_id TEXT NOT NULL, \
                turn_id TEXT NOT NULL, \
                item_id TEXT, \
                rpc_id TEXT, \
                connection_generation INTEGER NOT NULL, \
                binding_epoch INTEGER NOT NULL, \
                kind TEXT NOT NULL, \
                payload TEXT NOT NULL, \
                status TEXT NOT NULL DEFAULT 'pending', \
                created_at INTEGER NOT NULL DEFAULT (unixepoch())\
            )",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS turn_views (\
                session_id TEXT NOT NULL, \
                turn_id TEXT NOT NULL, \
                channel TEXT NOT NULL, \
                conversation_id TEXT NOT NULL, \
                message_id TEXT NOT NULL, \
                owner_id TEXT, \
                user_text TEXT NOT NULL, \
                agent_text TEXT NOT NULL, \
                status TEXT NOT NULL, \
                updated_at INTEGER NOT NULL DEFAULT (unixepoch()), \
                PRIMARY KEY (session_id, turn_id)\
            )",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn ensure_processed_event_columns(&self) -> Result<(), sqlx::Error> {
        let columns = sqlx::query("PRAGMA table_info(processed_events)")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect::<Vec<_>>();
        if !columns.iter().any(|column| column == "status") {
            sqlx::query(
                "ALTER TABLE processed_events ADD COLUMN status TEXT NOT NULL DEFAULT 'completed'",
            )
            .execute(&self.pool)
            .await?;
        }
        if !columns.iter().any(|column| column == "attempts") {
            sqlx::query(
                "ALTER TABLE processed_events ADD COLUMN attempts INTEGER NOT NULL DEFAULT 1",
            )
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    pub async fn attach(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
    ) -> Result<AttachOutcome, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let channel = conversation.channel.to_string();

        let previous_session = sqlx::query(
            "SELECT session_id FROM bindings WHERE channel = ? AND conversation_id = ?",
        )
        .bind(&channel)
        .bind(&conversation.conversation_id)
        .fetch_optional(&mut *transaction)
        .await?
        .map(|row| SessionId::new(row.get::<String, _>("session_id")));

        let displaced_conversation =
            sqlx::query("SELECT channel, conversation_id FROM bindings WHERE session_id = ?")
                .bind(session.as_str())
                .fetch_optional(&mut *transaction)
                .await?
                .map(|row| {
                    let stored_channel = row.get::<String, _>("channel");
                    let kind = stored_channel.parse::<ChannelKind>().map_err(|message| {
                        sqlx::Error::Decode(Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            message,
                        )))
                    })?;
                    Ok::<_, sqlx::Error>(ConversationRef::new(
                        kind,
                        row.get::<String, _>("conversation_id"),
                    ))
                })
                .transpose()?;

        sqlx::query("DELETE FROM bindings WHERE channel = ? AND conversation_id = ?")
            .bind(&channel)
            .bind(&conversation.conversation_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM bindings WHERE session_id = ?")
            .bind(session.as_str())
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO binding_epochs (channel, conversation_id, epoch) VALUES (?, ?, 1) \
             ON CONFLICT(channel, conversation_id) DO UPDATE SET epoch = epoch + 1",
        )
        .bind(&channel)
        .bind(&conversation.conversation_id)
        .execute(&mut *transaction)
        .await?;
        let epoch_i64 = sqlx::query(
            "SELECT epoch FROM binding_epochs WHERE channel = ? AND conversation_id = ?",
        )
        .bind(&channel)
        .bind(&conversation.conversation_id)
        .fetch_one(&mut *transaction)
        .await?
        .get::<i64, _>("epoch");
        sqlx::query(
            "INSERT INTO bindings (channel, conversation_id, session_id, epoch) VALUES (?, ?, ?, ?)",
        )
        .bind(&channel)
        .bind(&conversation.conversation_id)
        .bind(session.as_str())
        .bind(epoch_i64)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;

        Ok(AttachOutcome {
            previous_session: previous_session.filter(|previous| previous != session),
            displaced_conversation: displaced_conversation
                .filter(|displaced| displaced != conversation),
            epoch: u64::try_from(epoch_i64).unwrap_or_default(),
        })
    }

    pub async fn current_session(
        &self,
        conversation: &ConversationRef,
    ) -> Result<Option<SessionId>, sqlx::Error> {
        sqlx::query("SELECT session_id FROM bindings WHERE channel = ? AND conversation_id = ?")
            .bind(conversation.channel.to_string())
            .bind(&conversation.conversation_id)
            .fetch_optional(&self.pool)
            .await
            .map(|row| row.map(|row| SessionId::new(row.get::<String, _>("session_id"))))
    }

    pub(crate) async fn binding_epoch(
        &self,
        conversation: &ConversationRef,
    ) -> Result<u64, sqlx::Error> {
        let epoch =
            sqlx::query("SELECT epoch FROM bindings WHERE channel = ? AND conversation_id = ?")
                .bind(conversation.channel.to_string())
                .bind(&conversation.conversation_id)
                .fetch_one(&self.pool)
                .await?
                .get::<i64, _>("epoch");
        Ok(u64::try_from(epoch).unwrap_or_default())
    }

    pub async fn list_bindings(&self) -> Result<Vec<(ConversationRef, SessionId)>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT channel, conversation_id, session_id FROM bindings ORDER BY attached_at",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let channel = row
                    .get::<String, _>("channel")
                    .parse::<ChannelKind>()
                    .map_err(|message| {
                        sqlx::Error::Decode(Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            message,
                        )))
                    })?;
                Ok((
                    ConversationRef::new(channel, row.get::<String, _>("conversation_id")),
                    SessionId::new(row.get::<String, _>("session_id")),
                ))
            })
            .collect()
    }

    pub(crate) async fn checkpoint(&self) -> Result<(), sqlx::Error> {
        sqlx::query("PRAGMA wal_checkpoint(FULL)")
            .fetch_all(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn detach(&self, conversation: &ConversationRef) -> Result<(), sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("DELETE FROM bindings WHERE channel = ? AND conversation_id = ?")
            .bind(conversation.channel.to_string())
            .bind(&conversation.conversation_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO binding_epochs (channel, conversation_id, epoch) VALUES (?, ?, 1) \
             ON CONFLICT(channel, conversation_id) DO UPDATE SET epoch = epoch + 1",
        )
        .bind(conversation.channel.to_string())
        .bind(&conversation.conversation_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Invalidates actions for a temporarily unavailable binding without
    /// forgetting which session should be reattached when it returns.
    pub async fn suspend(&self, conversation: &ConversationRef) -> Result<u64, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let channel = conversation.channel.to_string();
        sqlx::query(
            "INSERT INTO binding_epochs (channel, conversation_id, epoch) VALUES (?, ?, 1) \
             ON CONFLICT(channel, conversation_id) DO UPDATE SET epoch = epoch + 1",
        )
        .bind(&channel)
        .bind(&conversation.conversation_id)
        .execute(&mut *transaction)
        .await?;
        let epoch_i64 = sqlx::query(
            "SELECT epoch FROM binding_epochs WHERE channel = ? AND conversation_id = ?",
        )
        .bind(&channel)
        .bind(&conversation.conversation_id)
        .fetch_one(&mut *transaction)
        .await?
        .get::<i64, _>("epoch");
        let updated =
            sqlx::query("UPDATE bindings SET epoch = ? WHERE channel = ? AND conversation_id = ?")
                .bind(epoch_i64)
                .bind(&channel)
                .bind(&conversation.conversation_id)
                .execute(&mut *transaction)
                .await?;
        if updated.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }
        transaction.commit().await?;
        Ok(u64::try_from(epoch_i64).unwrap_or_default())
    }

    pub async fn record_event(
        &self,
        channel: ChannelKind,
        event_id: &str,
    ) -> Result<bool, sqlx::Error> {
        let claimed = self.claim_event(channel, event_id).await?;
        if claimed {
            self.complete_event(channel, event_id).await?;
        }
        Ok(claimed)
    }

    pub(crate) async fn claim_event(
        &self,
        channel: ChannelKind,
        event_id: &str,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "INSERT INTO processed_events (channel, event_id, status, attempts) \
             VALUES (?, ?, 'processing', 1) \
             ON CONFLICT(channel, event_id) DO UPDATE SET \
                status = 'processing', attempts = attempts + 1, processed_at = unixepoch() \
             WHERE processed_events.status = 'failed'",
        )
        .bind(channel.to_string())
        .bind(event_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub(crate) async fn complete_event(
        &self,
        channel: ChannelKind,
        event_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE processed_events SET status = 'completed', processed_at = unixepoch() \
             WHERE channel = ? AND event_id = ?",
        )
        .bind(channel.to_string())
        .bind(event_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(crate) async fn release_event(
        &self,
        channel: ChannelKind,
        event_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE processed_events SET status = 'failed', processed_at = unixepoch() \
             WHERE channel = ? AND event_id = ? AND status = 'processing'",
        )
        .bind(channel.to_string())
        .bind(event_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(crate) async fn save_turn_view(&self, view: &StoredTurnView) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO turn_views (\
                session_id, turn_id, channel, conversation_id, message_id, owner_id, \
                user_text, agent_text, status\
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(session_id, turn_id) DO UPDATE SET \
                channel = excluded.channel, \
                conversation_id = excluded.conversation_id, \
                message_id = excluded.message_id, \
                owner_id = excluded.owner_id, \
                user_text = excluded.user_text, \
                agent_text = excluded.agent_text, \
                status = excluded.status, \
                updated_at = unixepoch()",
        )
        .bind(view.session_id.as_str())
        .bind(&view.turn_id)
        .bind(view.message.conversation.channel.to_string())
        .bind(&view.message.conversation.conversation_id)
        .bind(&view.message.message_id)
        .bind(&view.owner_id)
        .bind(&view.user_text)
        .bind(&view.agent_text)
        .bind(turn_status_name(&view.status))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(crate) async fn list_turn_views(&self) -> Result<Vec<StoredTurnView>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT session_id, turn_id, channel, conversation_id, message_id, owner_id, \
                    user_text, agent_text, status \
             FROM turn_views ORDER BY updated_at",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let channel = row
                    .get::<String, _>("channel")
                    .parse::<ChannelKind>()
                    .map_err(decode_error)?;
                let status = parse_turn_status(&row.get::<String, _>("status"))
                    .ok_or_else(|| decode_error("unsupported stored turn status"))?;
                let conversation =
                    ConversationRef::new(channel, row.get::<String, _>("conversation_id"));
                Ok(StoredTurnView {
                    session_id: SessionId::new(row.get::<String, _>("session_id")),
                    turn_id: row.get("turn_id"),
                    message: MessageRef::new(conversation, row.get::<String, _>("message_id")),
                    owner_id: row.get("owner_id"),
                    user_text: row.get("user_text"),
                    agent_text: row.get("agent_text"),
                    status,
                })
            })
            .collect()
    }

    pub(crate) async fn delete_turn_view(
        &self,
        session_id: &SessionId,
        turn_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM turn_views WHERE session_id = ? AND turn_id = ?")
            .bind(session_id.as_str())
            .bind(turn_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

fn decode_error(message: impl Into<String>) -> sqlx::Error {
    sqlx::Error::Decode(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    )))
}

fn turn_status_name(status: &TurnStatus) -> &'static str {
    match status {
        TurnStatus::InProgress => "in_progress",
        TurnStatus::Completed => "completed",
        TurnStatus::Interrupted => "interrupted",
        TurnStatus::Failed => "failed",
        TurnStatus::Unknown => "unknown",
    }
}

fn parse_turn_status(status: &str) -> Option<TurnStatus> {
    match status {
        "in_progress" => Some(TurnStatus::InProgress),
        "completed" => Some(TurnStatus::Completed),
        "interrupted" => Some(TurnStatus::Interrupted),
        "failed" => Some(TurnStatus::Failed),
        "unknown" => Some(TurnStatus::Unknown),
        _ => None,
    }
}
