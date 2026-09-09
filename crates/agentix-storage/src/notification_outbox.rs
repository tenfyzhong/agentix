use agentix_domain::{ConversationRef, OutboundView};
use sqlx::Row;

use crate::SqliteState;

/// A leased delivery. The opaque token fences acknowledgments from old workers.
#[derive(Debug, Clone)]
pub struct TaskNotification {
    pub consumer: String,
    pub sequence: i64,
    pub conversation: ConversationRef,
    pub view: OutboundView,
    pub attempts: i64,
    token: String,
}

impl SqliteState {
    pub(crate) async fn migrate_notification_outbox(&self) -> Result<(), sqlx::Error> {
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS notification_cursors (
                consumer TEXT PRIMARY KEY, sequence INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS notification_outbox (
                consumer TEXT NOT NULL, sequence INTEGER NOT NULL,
                conversation TEXT NOT NULL, payload TEXT NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0,
                coalesced_count INTEGER NOT NULL DEFAULT 1,
                next_attempt INTEGER NOT NULL DEFAULT 0,
                lease_until INTEGER NOT NULL DEFAULT 0, token TEXT,
                last_error TEXT,
                PRIMARY KEY (consumer, sequence)
            );
            CREATE INDEX IF NOT EXISTS notification_conversation_order
                ON notification_outbox(consumer, conversation, sequence);",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Reject an unstarted input and persist a coalesced user-visible notice in
    /// the same transaction. Completed, uncertain and in-flight duplicates keep
    /// their existing outcome; a rejected input cannot later execute on replay.
    pub async fn reject_overloaded(
        &self,
        consumer: &str,
        conversation: &ConversationRef,
        event_id: &str,
        limit: usize,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let rejected = sqlx::query("INSERT INTO processed_events(channel,event_id,status) VALUES (?,?,'rejected')
            ON CONFLICT(channel,event_id) DO UPDATE SET status='rejected', attempts=attempts+1, processed_at=unixepoch()
            WHERE processed_events.status='failed'")
            .bind(conversation.channel.to_string()).bind(event_id).execute(&mut *tx).await?.rows_affected();
        if rejected == 0 {
            return Ok(false);
        }
        let destination = serde_json::to_string(conversation).map_err(protocol)?;
        let existing: Option<(i64, i64)> = sqlx::query_as(
            "SELECT sequence, coalesced_count FROM notification_outbox
            WHERE consumer=? AND conversation=? AND token IS NULL ORDER BY sequence DESC LIMIT 1",
        )
        .bind(consumer)
        .bind(&destination)
        .fetch_optional(&mut *tx)
        .await?;
        let count = existing.map_or(1, |(_, count)| count.saturating_add(1));
        let view = OutboundView::text(
            "Session busy",
            format!(
                "{count} requests were not accepted because this conversation already has {limit} unfinished requests. Accepted requests remain in order. Please resend rejected requests after the queue clears."
            ),
        );
        let payload = serde_json::to_string(&view).map_err(protocol)?;
        if let Some((sequence, _)) = existing {
            sqlx::query("UPDATE notification_outbox SET payload=?, coalesced_count=? WHERE consumer=? AND sequence=?")
                .bind(payload).bind(count).bind(consumer).bind(sequence).execute(&mut *tx).await?;
        } else {
            let sequence: i64 = sqlx::query_scalar(
                "INSERT INTO notification_cursors VALUES (?,1)
                ON CONFLICT(consumer) DO UPDATE SET sequence=sequence+1 RETURNING sequence",
            )
            .bind(consumer)
            .fetch_one(&mut *tx)
            .await?;
            sqlx::query("INSERT INTO notification_outbox(consumer,sequence,conversation,payload) VALUES (?,?,?,?)")
                .bind(consumer).bind(sequence).bind(destination).bind(payload).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Import a legacy consumer cursor exactly once, before staging events.
    pub async fn notification_cursor(
        &self,
        consumer: &str,
        legacy: i64,
    ) -> Result<i64, sqlx::Error> {
        sqlx::query("INSERT OR IGNORE INTO notification_cursors VALUES (?, ?)")
            .bind(consumer)
            .bind(legacy)
            .execute(&self.pool)
            .await?;
        sqlx::query_scalar("SELECT sequence FROM notification_cursors WHERE consumer = ?")
            .bind(consumer)
            .fetch_one(&self.pool)
            .await
    }

    /// Cursor advancement and durable delivery creation are one transaction.
    /// Replayed events cannot recreate already acknowledged notifications.
    pub async fn stage_notification(
        &self,
        consumer: &str,
        sequence: i64,
        notification: Option<(&ConversationRef, &OutboundView)>,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let cursor: Option<i64> =
            sqlx::query_scalar("SELECT sequence FROM notification_cursors WHERE consumer = ?")
                .bind(consumer)
                .fetch_optional(&mut *tx)
                .await?;
        if cursor.is_none() {
            return Err(protocol(
                "notification consumer must be initialized before staging events",
            ));
        }
        let advanced = sqlx::query(
            "UPDATE notification_cursors SET sequence = ? WHERE consumer = ? AND sequence < ?",
        )
        .bind(sequence)
        .bind(consumer)
        .bind(sequence)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if advanced > 0
            && let Some((conversation, view)) = notification
        {
            sqlx::query("INSERT INTO notification_outbox (consumer, sequence, conversation, payload) VALUES (?, ?, ?, ?)")
                .bind(consumer).bind(sequence)
                .bind(serde_json::to_string(conversation).map_err(protocol)?)
                .bind(serde_json::to_string(view).map_err(protocol)?)
                .execute(&mut *tx).await?;
        }
        tx.commit().await
    }

    /// Lease only the oldest outstanding notification for each conversation.
    /// A slow or retrying conversation does not block another conversation.
    pub async fn claim_notifications(
        &self,
        consumer: &str,
        now: i64,
        lease_seconds: i64,
        limit: u32,
    ) -> Result<Vec<TaskNotification>, sqlx::Error> {
        if lease_seconds <= 0 {
            return Err(protocol("notification lease must be positive"));
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let sequences: Vec<i64> = sqlx::query_scalar(
            "SELECT n.sequence FROM notification_outbox n
             WHERE n.consumer = ? AND n.next_attempt <= ? AND n.lease_until <= ?
             AND NOT EXISTS (SELECT 1 FROM notification_outbox older
                 WHERE older.consumer = n.consumer AND older.conversation = n.conversation
                 AND older.sequence < n.sequence)
             ORDER BY n.next_attempt, n.sequence LIMIT ?",
        )
        .bind(consumer)
        .bind(now)
        .bind(now)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;
        let mut deliveries = Vec::with_capacity(sequences.len());
        for sequence in sequences {
            let row = sqlx::query(
                "UPDATE notification_outbox SET attempts = attempts + 1,
                 lease_until = ?, token = lower(hex(randomblob(16)))
                 WHERE consumer = ? AND sequence = ?
                 RETURNING conversation, payload, attempts, token",
            )
            .bind(now.saturating_add(lease_seconds))
            .bind(consumer)
            .bind(sequence)
            .fetch_one(&mut *tx)
            .await?;
            deliveries.push(TaskNotification {
                consumer: consumer.into(),
                sequence,
                conversation: serde_json::from_str(row.try_get("conversation")?)
                    .map_err(protocol)?,
                view: serde_json::from_str(row.try_get("payload")?).map_err(protocol)?,
                attempts: row.try_get("attempts")?,
                token: row.try_get("token")?,
            });
        }
        tx.commit().await?;
        Ok(deliveries)
    }

    /// At-least-once delivery: failure retries with bounded exponential backoff.
    /// A crash after sending but before this acknowledgment can duplicate a card.
    pub async fn finish_notification(
        &self,
        delivery: &TaskNotification,
        now: i64,
        error: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let result = if let Some(error) = error {
            let delay = 1_i64 << u32::try_from((delivery.attempts - 1).clamp(0, 8)).unwrap();
            sqlx::query("UPDATE notification_outbox SET next_attempt = ?, lease_until = 0, token = NULL, last_error = ? WHERE consumer = ? AND sequence = ? AND token = ?")
                .bind(now.saturating_add(delay)).bind(error.chars().take(1024).collect::<String>())
                .bind(&delivery.consumer).bind(delivery.sequence).bind(&delivery.token)
                .execute(&self.pool).await?
        } else {
            sqlx::query(
                "DELETE FROM notification_outbox WHERE consumer = ? AND sequence = ? AND token = ?",
            )
            .bind(&delivery.consumer)
            .bind(delivery.sequence)
            .bind(&delivery.token)
            .execute(&self.pool)
            .await?
        };
        Ok(result.rows_affected() == 1)
    }
}

fn protocol(error: impl std::fmt::Display) -> sqlx::Error {
    sqlx::Error::Protocol(error.to_string())
}
