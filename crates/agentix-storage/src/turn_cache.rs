//! Bounded `SQLite` page cache for disposable, serialized application records.
use agentix_domain::SessionId;
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use tokio::sync::Mutex;

#[derive(Default)]
pub struct TurnCache {
    connection: Mutex<Option<SqliteConnection>>,
}
impl TurnCache {
    #[cfg(any(test, feature = "test-support"))]
    pub async fn reject_writes(&self, reject: bool) {
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

    pub async fn store<T: Serialize + Sync>(
        &self,
        session: &SessionId,
        turn: &str,
        value: &T,
    ) -> Result<(), sqlx::Error> {
        let data =
            serde_json::to_string(value).map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
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

    pub async fn load<T: DeserializeOwned>(
        &self,
        session: &SessionId,
        turn: &str,
    ) -> Result<Option<T>, sqlx::Error> {
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
        data.map(|value| {
            serde_json::from_str(&value).map_err(|error| sqlx::Error::Decode(Box::new(error)))
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
    #[tokio::test]
    async fn cache_budget_and_failed_overwrite_preserve_stored_value() {
        let cache = TurnCache::default();
        let session = SessionId::new("one");
        cache.store(&session, "turn", &"original").await.unwrap();
        let budget: i64 = sqlx::query_scalar("PRAGMA cache_size")
            .fetch_one(cache.connection.lock().await.as_mut().unwrap())
            .await
            .unwrap();
        assert_eq!(budget, -256);
        cache.reject_writes(true).await;
        assert!(cache.store(&session, "turn", &"replacement").await.is_err());
        assert_eq!(
            cache
                .load::<String>(&session, "turn")
                .await
                .unwrap()
                .as_deref(),
            Some("original")
        );
        cache.remove_session(&session).await.unwrap();
        assert!(cache.session_turns(&session).await.unwrap().is_empty());
    }
}
