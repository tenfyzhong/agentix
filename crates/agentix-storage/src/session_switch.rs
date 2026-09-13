//! Durable handoff and FIFO state. Optimistic revisions fence stale workers.
use agentix_domain::{ConversationRef, MessageRef, SessionId};
use serde::{Deserialize, Serialize};

use crate::SqliteState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwitchPrompt {
    pub id: String,
    pub text: String,
    /// Written before sending. A crash here requires human reconciliation.
    pub sending: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSwitch {
    pub id: String,
    pub conversation: ConversationRef,
    pub old_session: SessionId,
    pub client_id: String,
    #[serde(default)]
    pub owner_id: Option<String>,
    pub target: Option<SessionId>,
    #[serde(default)]
    pub candidate: Option<SessionId>,
    pub epoch: u64,
    pub deadline: u64,
    pub paused: Option<String>,
    pub notice: Option<MessageRef>,
    pub messages: Vec<SwitchPrompt>,
    pub revision: i64,
}

impl SessionSwitch {
    #[must_use]
    pub fn new(
        id: String,
        conversation: ConversationRef,
        old_session: SessionId,
        client_id: String,
        epoch: u64,
        deadline: u64,
    ) -> Self {
        Self {
            id,
            conversation,
            old_session,
            client_id,
            owner_id: None,
            target: None,
            candidate: None,
            epoch,
            deadline,
            paused: None,
            notice: None,
            messages: Vec::new(),
            revision: 0,
        }
    }

    pub fn enqueue(&mut self, id: &str, text: &str) -> bool {
        if self.messages.iter().any(|message| message.id == id) {
            return false;
        }
        self.messages.push(SwitchPrompt {
            id: id.into(),
            text: text.into(),
            sending: false,
        });
        true
    }
}

fn encode(value: &impl Serialize) -> Result<String, sqlx::Error> {
    serde_json::to_string(value).map_err(|error| sqlx::Error::Protocol(error.to_string()))
}

fn decode(value: &str) -> Result<SessionSwitch, sqlx::Error> {
    serde_json::from_str(value).map_err(|error| sqlx::Error::Protocol(error.to_string()))
}

impl SqliteState {
    pub(crate) async fn migrate_session_switches(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS session_switches (
            id TEXT PRIMARY KEY, conversation TEXT UNIQUE NOT NULL,
            revision INTEGER NOT NULL, payload TEXT NOT NULL)",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn session_switch(
        &self,
        conversation: &ConversationRef,
    ) -> Result<Option<SessionSwitch>, sqlx::Error> {
        let value: Option<String> =
            sqlx::query_scalar("SELECT payload FROM session_switches WHERE conversation=?")
                .bind(encode(conversation)?)
                .fetch_optional(&self.pool)
                .await?;
        value.as_deref().map(decode).transpose()
    }

    pub async fn list_session_switches(&self) -> Result<Vec<SessionSwitch>, sqlx::Error> {
        let rows: Vec<String> =
            sqlx::query_scalar("SELECT payload FROM session_switches ORDER BY id")
                .fetch_all(&self.pool)
                .await?;
        rows.iter().map(|row| decode(row)).collect()
    }

    pub async fn save_session_switch(
        &self,
        switch: &mut SessionSwitch,
    ) -> Result<bool, sqlx::Error> {
        let revision = switch.revision;
        let mut next = switch.clone();
        next.revision += 1;
        let payload = encode(&next)?;
        let changed = if revision == 0 {
            sqlx::query("INSERT OR IGNORE INTO session_switches(id,conversation,revision,payload)
                SELECT ?,?,1,? WHERE EXISTS (
                    SELECT 1 FROM bindings WHERE channel=? AND conversation_id=? AND session_id=? AND epoch=?)")
                .bind(&switch.id).bind(encode(&switch.conversation)?).bind(payload)
                .bind(switch.conversation.channel.to_string()).bind(&switch.conversation.conversation_id)
                .bind(switch.old_session.as_str()).bind(i64::try_from(switch.epoch).unwrap_or(i64::MAX))
                .execute(&self.pool).await?.rows_affected()
        } else {
            sqlx::query("UPDATE session_switches SET revision=revision+1,payload=? WHERE id=? AND revision=?")
                .bind(payload).bind(&switch.id).bind(revision).execute(&self.pool).await?.rows_affected()
        };
        if changed == 1 {
            switch.revision = next.revision;
        }
        Ok(changed == 1)
    }

    pub async fn delete_session_switch(&self, switch: &SessionSwitch) -> Result<bool, sqlx::Error> {
        Ok(
            sqlx::query("DELETE FROM session_switches WHERE id=? AND revision=?")
                .bind(&switch.id)
                .bind(switch.revision)
                .execute(&self.pool)
                .await?
                .rows_affected()
                == 1,
        )
    }
}

impl SqliteState {
    /// Move the binding and durable FIFO together; a different attachment wins.
    pub async fn complete_session_switch(
        &self,
        switch: &mut SessionSwitch,
        target: &SessionId,
    ) -> Result<bool, sqlx::Error> {
        let mut next = switch.clone();
        next.target = Some(target.clone());
        next.epoch += 1;
        next.revision += 1;
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query("UPDATE bindings SET session_id=?,epoch=epoch+1 WHERE channel=? AND conversation_id=? AND session_id=? AND epoch=? AND NOT EXISTS (SELECT 1 FROM bindings WHERE session_id=?) AND EXISTS (SELECT 1 FROM session_switches WHERE id=? AND revision=?)")
            .bind(target.as_str()).bind(switch.conversation.channel.to_string()).bind(&switch.conversation.conversation_id)
            .bind(switch.old_session.as_str()).bind(i64::try_from(switch.epoch).unwrap_or(i64::MAX)).bind(target.as_str()).bind(&switch.id).bind(switch.revision)
            .execute(&mut *tx).await?.rows_affected();
        if updated != 1 {
            return Ok(false);
        }
        sqlx::query(
            "UPDATE binding_epochs SET epoch=epoch+1 WHERE channel=? AND conversation_id=?",
        )
        .bind(switch.conversation.channel.to_string())
        .bind(&switch.conversation.conversation_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE session_switches SET revision=?,payload=? WHERE id=?")
            .bind(next.revision)
            .bind(encode(&next)?)
            .bind(&next.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        *switch = next;
        Ok(true)
    }
}
