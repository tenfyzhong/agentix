//! Bounded cleanup of disposable data; source and decision history is never pruned.
use crate::MemoryStore;
use anyhow::Result;

impl MemoryStore {
    pub async fn cleanup_derived(&self) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let work = scan_page(&mut tx, "work_items", "id", "state='cancelled'").await?;
        let vectors = scan_page(&mut tx, "memory_vectors", "rowid", "1").await?;
        let failures = scan_page(&mut tx, "embedding_failures", "rowid", "1").await?;
        // Start a conservative retention clock on first observation, including upgrades.
        sqlx::query("INSERT OR IGNORE INTO work_retention(work_id,observed_at) SELECT w.id,unixepoch() FROM work_items w WHERE w.id IN (SELECT value FROM json_each(?)) AND w.state='cancelled' AND NOT EXISTS(SELECT 1 FROM work_retention r WHERE r.work_id=w.id) AND NOT EXISTS(SELECT 1 FROM work_audits a WHERE a.work_id=w.id) AND NOT EXISTS(SELECT 1 FROM memory_reviews r WHERE r.work_id=w.id) ORDER BY w.id LIMIT 1000")
            .bind(work).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM work_items WHERE id IN (SELECT r.work_id FROM work_retention r JOIN work_items w ON w.id=r.work_id WHERE r.observed_at<unixepoch()-2592000 AND w.state='cancelled' AND NOT EXISTS(SELECT 1 FROM work_audits a WHERE a.work_id=w.id) AND NOT EXISTS(SELECT 1 FROM memory_reviews v WHERE v.work_id=w.id) ORDER BY r.observed_at,r.work_id LIMIT 1000)")
            .execute(&mut *tx).await?;
        sqlx::query("DELETE FROM memory_vectors WHERE rowid IN (SELECT v.rowid FROM memory_vectors v WHERE v.rowid IN (SELECT value FROM json_each(?)) AND NOT EXISTS(SELECT 1 FROM embedding_profiles p JOIN memories m ON m.project_id=p.project_id WHERE p.project_id=v.project_id AND p.generation=v.generation AND m.id=v.memory_id AND m.revision=v.revision AND m.status IN ('active','conflicted') AND (m.valid_until IS NULL OR m.valid_until>unixepoch())) LIMIT 1000)")
            .bind(vectors).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM embedding_failures WHERE rowid IN (SELECT f.rowid FROM embedding_failures f WHERE f.rowid IN (SELECT value FROM json_each(?)) AND NOT EXISTS(SELECT 1 FROM memories m JOIN embedding_profiles p ON p.project_id=m.project_id WHERE m.id=f.memory_id AND m.revision=f.revision AND p.generation=f.generation AND m.status IN ('active','conflicted') AND (m.valid_until IS NULL OR m.valid_until>unixepoch())) LIMIT 1000)")
            .bind(failures).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}

// Persisted keyset cursors bound rows examined as well as rows deleted.
async fn scan_page(
    conn: &mut sqlx::SqliteConnection,
    table: &str,
    id: &str,
    filter: &str,
) -> Result<String> {
    let key = format!("maintenance_cursor:{table}");
    let after: Option<i64> =
        sqlx::query_scalar("SELECT CAST(value AS INTEGER) FROM memory_metadata WHERE key=?")
            .bind(&key)
            .fetch_optional(&mut *conn)
            .await?;
    let rows: Vec<i64> = sqlx::query_scalar(&format!(
        "SELECT {id} FROM {table} WHERE {id}>? AND ({filter}) ORDER BY {id} LIMIT 1000"
    ))
    .bind(after.unwrap_or(0))
    .fetch_all(&mut *conn)
    .await?;
    let next = if rows.len() == 1000 {
        *rows.last().unwrap_or(&0)
    } else {
        0
    };
    sqlx::query("INSERT INTO memory_metadata(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value<>excluded.value")
        .bind(key).bind(next.to_string()).execute(&mut *conn).await?;
    Ok(serde_json::to_string(&rows)?)
}
