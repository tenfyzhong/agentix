//! Durable destinations learned from authenticated IM input.
use agentix_domain::{ChannelKind, ConversationRef};
use sqlx::Row;

use crate::SqliteState;

impl SqliteState {
    #[cfg(any(test, feature = "test-support"))]
    pub async fn reject_owner_writes(&self, reject: bool) {
        let query = if reject {
            "CREATE TRIGGER reject_owner_insert BEFORE INSERT ON conversation_owners BEGIN SELECT RAISE(ABORT,'injected owner failure'); END"
        } else {
            "DROP TRIGGER reject_owner_insert"
        };
        sqlx::query(query).execute(&self.pool).await.unwrap();
    }

    pub async fn save_conversation_owner(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO conversation_owners (channel, conversation_id, owner_id) VALUES (?, ?, ?) ON CONFLICT(channel, conversation_id) DO UPDATE SET owner_id = excluded.owner_id")
            .bind(conversation.channel.to_string())
            .bind(&conversation.conversation_id)
            .bind(owner_id)
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn list_conversation_owners(
        &self,
    ) -> Result<Vec<(ConversationRef, String)>, sqlx::Error> {
        let rows = sqlx::query("SELECT channel, conversation_id, owner_id FROM conversation_owners ORDER BY channel, conversation_id")
            .fetch_all(&self.pool).await?;
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
                    row.get("owner_id"),
                ))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn owners_are_updated_per_conversation_and_cleared_only_for_changed_bots() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let state = SqliteState::open(&path).await.unwrap();
        state
            .reconcile_channel_identity(ChannelKind::Feishu, "bot-a")
            .await
            .unwrap();
        state
            .reconcile_channel_identity(ChannelKind::Telegram, "bot-t")
            .await
            .unwrap();
        let feishu = ConversationRef::new(ChannelKind::Feishu, "same-chat");
        let telegram = ConversationRef::new(ChannelKind::Telegram, "same-chat");
        state
            .save_conversation_owner(&feishu, "old-owner")
            .await
            .unwrap();
        state
            .save_conversation_owner(&feishu, "new-owner")
            .await
            .unwrap();
        state
            .save_conversation_owner(&telegram, "telegram-owner")
            .await
            .unwrap();
        drop(state);
        let state = SqliteState::open(&path).await.unwrap();
        let owners = state.list_conversation_owners().await.unwrap();
        assert_eq!(owners.len(), 2);
        assert!(owners.contains(&(feishu, "new-owner".into())));
        state
            .reconcile_channel_identity(ChannelKind::Feishu, "bot-a")
            .await
            .unwrap();
        assert_eq!(state.list_conversation_owners().await.unwrap(), owners);
        state
            .reconcile_channel_identity(ChannelKind::Feishu, "bot-b")
            .await
            .unwrap();
        assert_eq!(
            state.list_conversation_owners().await.unwrap(),
            vec![(telegram, "telegram-owner".into())]
        );
    }
}
