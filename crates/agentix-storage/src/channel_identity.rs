//! Channel identity owns all persisted IM routing state for one configured bot.
use crate::SqliteState;
use agentix_domain::ChannelKind;

impl SqliteState {
    /// Call once at startup, before any route restore or notification worker.
    /// Unknown legacy identity is deliberately treated as a change.
    pub async fn reconcile_channel_identity(
        &self,
        channel: ChannelKind,
        identity: &str,
    ) -> Result<u64, sqlx::Error> {
        if identity.trim().is_empty() {
            return Err(sqlx::Error::Protocol("empty channel identity".into()));
        }
        let channel = channel.to_string();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let previous: Option<String> =
            sqlx::query_scalar("SELECT identity FROM channel_identities WHERE channel = ?")
                .bind(&channel)
                .fetch_optional(&mut *tx)
                .await?;
        if previous.as_deref() == Some(identity) {
            tx.commit().await?;
            return Ok(0);
        }
        let removed = sqlx::query("DELETE FROM bindings WHERE channel = ?")
            .bind(&channel)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        // Keep epochs monotonic so old controls cannot regain validity after reattach.
        sqlx::query("UPDATE binding_epochs SET epoch = epoch + 1 WHERE channel = ?")
            .bind(&channel)
            .execute(&mut *tx)
            .await?;
        for table in ["turn_views", "pending_interactions", "processed_events"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE channel = ?"))
                .bind(&channel)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query(
            "DELETE FROM notification_outbox WHERE json_extract(conversation, '$.channel') = ?",
        )
        .bind(&channel)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO channel_identities(channel, identity) VALUES (?, ?) ON CONFLICT(channel) DO UPDATE SET identity = excluded.identity")
            .bind(&channel).bind(identity).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentix_domain::{ConversationRef, SessionId};

    #[tokio::test]
    async fn identity_change_clears_old_messages_actions_and_notifications_atomically() {
        let state = SqliteState::in_memory().await.unwrap();
        state
            .reconcile_channel_identity(ChannelKind::Slack, "T1:B1")
            .await
            .unwrap();
        let chat = ConversationRef::new(ChannelKind::Slack, "T1:D1");
        let before = state
            .attach(&chat, &SessionId::new("session"))
            .await
            .unwrap()
            .epoch;
        sqlx::query("INSERT INTO turn_views (session_id,turn_id,channel,conversation_id,message_id,user_text,agent_text,status) VALUES ('session','turn','slack','T1:D1','message','','','in_progress')")
            .execute(&state.pool).await.unwrap();
        sqlx::query("INSERT INTO pending_interactions (token,channel,conversation_id,owner_id,session_id,turn_id,connection_generation,binding_epoch,kind,payload) VALUES ('token','slack','T1:D1','owner','session','turn',1,1,'approval','{}')")
            .execute(&state.pool).await.unwrap();
        state
            .reject_overloaded("consumer", &chat, "event", 1)
            .await
            .unwrap();
        // Unchanged bot must preserve all durable state.
        assert_eq!(
            state
                .reconcile_channel_identity(ChannelKind::Slack, "T1:B1")
                .await
                .unwrap(),
            0
        );
        assert!(state.current_session(&chat).await.unwrap().is_some());
        assert_eq!(
            state
                .reconcile_channel_identity(ChannelKind::Slack, "T1:B2")
                .await
                .unwrap(),
            1
        );
        for table in [
            "bindings",
            "turn_views",
            "pending_interactions",
            "notification_outbox",
            "processed_events",
        ] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&state.pool)
                .await
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
        let after = state
            .attach(&chat, &SessionId::new("session"))
            .await
            .unwrap()
            .epoch;
        assert!(after > before);
        assert!(
            state
                .reconcile_channel_identity(ChannelKind::Slack, " ")
                .await
                .is_err()
        );
        assert!(state.current_session(&chat).await.unwrap().is_some());
    }
}
