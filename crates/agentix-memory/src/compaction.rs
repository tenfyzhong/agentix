//! Low-priority incremental semantic maintenance, separate from derived-data cleanup.
use crate::{Actor, AgentConfig, Memory, MemoryStore};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Default, Serialize)]
pub struct CompactionPage {
    pub scanned: usize,
    pub scheduled: usize,
    pub next_after: String,
    pub work_ids: Vec<i64>,
}

impl MemoryStore {
    pub(crate) async fn skip_obsolete_compaction(
        &self,
        lease: &crate::WorkLease,
        now: i64,
    ) -> Result<bool> {
        let Some(value) = lease.payload.get("compact") else {
            return Ok(false);
        };
        let seed: Memory = serde_json::from_value(value.clone())?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        crate::queue::fence(&mut tx, lease, now).await?;
        let current = crate::store::load(&mut tx, &lease.project_id, &seed.id).await?;
        let obsolete = current.revision != seed.revision
            || current.actor != Actor::Agent
            || !current.status.searchable()
            || current
                .content
                .valid_until
                .is_some_and(|expiry| expiry <= now);
        if obsolete {
            crate::queue::finish(&mut tx, lease, &json!({"skipped":"compaction seed changed","id":seed.id,"expected_revision":seed.revision,"current_revision":current.revision})).await?;
        }
        tx.commit().await?;
        Ok(obsolete)
    }
    /// Scan a bounded ID page. Enqueue proposals, never mutate memory inline.
    #[allow(clippy::too_many_arguments)]
    pub async fn schedule_compaction(
        &self,
        project: &str,
        after: &str,
        limit: i64,
        force: bool,
        interval: u64,
        debounce: u64,
        now: i64,
    ) -> Result<CompactionPage> {
        ensure!(
            !project.is_empty() && (1..=100).contains(&limit),
            "invalid compaction page"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT data FROM memories WHERE project_id=? AND id>? ORDER BY id LIMIT ?",
        )
        .bind(project)
        .bind(after)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;
        let mut page = CompactionPage {
            scanned: rows.len(),
            ..CompactionPage::default()
        };
        for data in &rows {
            let memory: Memory = serde_json::from_str(data)?;
            page.next_after.clone_from(&memory.id);
            if let Some(id) = enqueue(&mut tx, &memory, force, interval, debounce, now).await? {
                page.work_ids.push(id);
            }
        }
        page.scheduled = page.work_ids.len();
        if rows.len() < usize::try_from(limit)? {
            page.next_after.clear();
        }
        tx.commit().await?;
        if page.scheduled > 0 {
            self.notify_work();
        }
        Ok(page)
    }

    pub async fn schedule_background_compaction(
        &self,
        project: &str,
        config: &AgentConfig,
        now: i64,
    ) -> Result<usize> {
        if config.compaction_interval_seconds == 0 {
            return Ok(0);
        }
        // An indexed dirty queue prioritizes new revisions ahead of the historical sweep.
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let rows: Vec<String> = sqlx::query_scalar("SELECT m.data FROM memory_compactions c JOIN memories m ON m.id=c.memory_id WHERE c.project_id=? AND c.dirty=1 AND c.dirty_at<=? ORDER BY c.dirty_at,c.memory_id LIMIT 10")
            .bind(project).bind(now.saturating_sub(i64::try_from(config.compaction_debounce_seconds)?)).fetch_all(&mut *tx).await?;
        let mut count = 0;
        for data in rows {
            let memory = serde_json::from_str(&data)?;
            count += usize::from(
                enqueue(
                    &mut tx,
                    &memory,
                    false,
                    config.compaction_interval_seconds,
                    config.compaction_debounce_seconds,
                    now,
                )
                .await?
                .is_some(),
            );
        }
        tx.commit().await?;
        let key = format!("compaction_cursor:{project}");
        let after: Option<String> =
            sqlx::query_scalar("SELECT value FROM memory_metadata WHERE key=?")
                .bind(&key)
                .fetch_optional(&self.pool)
                .await?;
        let page = self
            .schedule_compaction(
                project,
                after.as_deref().unwrap_or(""),
                10,
                false,
                config.compaction_interval_seconds,
                config.compaction_debounce_seconds,
                now,
            )
            .await?;
        sqlx::query("INSERT INTO memory_metadata(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value<>excluded.value")
            .bind(key).bind(page.next_after).execute(&self.pool).await?;
        if count > 0 {
            self.notify_work();
        }
        Ok(count + page.scheduled)
    }
}

async fn enqueue(
    conn: &mut sqlx::SqliteConnection,
    memory: &Memory,
    force: bool,
    interval: u64,
    debounce: u64,
    now: i64,
) -> Result<Option<i64>> {
    if memory.actor != Actor::Agent
        || !memory.status.searchable()
        || memory
            .content
            .valid_until
            .is_some_and(|expiry| expiry <= now)
    {
        observed(conn, memory, now).await?;
        return Ok(None);
    }
    let eligible: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM memory_compactions c WHERE c.memory_id=? AND (? OR (c.dirty_at<=? AND (c.dirty=1 OR c.checked_at<=?))) AND (? OR c.dirty=1 OR c.suspended=0) AND NOT EXISTS(SELECT 1 FROM work_items w WHERE w.id=c.work_id AND w.state IN ('pending','running')))")
        .bind(&memory.id).bind(force).bind(now.saturating_sub(i64::try_from(debounce)?)).bind(now.saturating_sub(i64::try_from(interval)?)).bind(force).fetch_one(&mut *conn).await?;
    if !eligible {
        return Ok(None);
    }
    let receipt = &memory
        .content
        .evidence
        .first()
        .context("compaction requires source evidence")?
        .receipt_id;
    sqlx::query("INSERT OR IGNORE INTO scheduler_projects(project_id) VALUES(?)")
        .bind(&memory.project_id)
        .execute(&mut *conn)
        .await?;
    let result = sqlx::query("INSERT INTO work_items(project_id,receipt_id,kind,payload,priority) VALUES(?,?,'consolidate',?,3)")
        .bind(&memory.project_id).bind(receipt).bind(json!({"compact":memory}).to_string()).execute(&mut *conn).await?;
    let id = result.last_insert_rowid();
    sqlx::query("UPDATE memory_compactions SET dirty=0,checked_at=?,suspended=0,work_id=? WHERE memory_id=?")
        .bind(now)
        .bind(id)
        .bind(&memory.id)
        .execute(conn)
        .await?;
    Ok(Some(id))
}

pub(crate) async fn observed(
    conn: &mut sqlx::SqliteConnection,
    memory: &Memory,
    now: i64,
) -> Result<()> {
    sqlx::query(
        "UPDATE memory_compactions SET dirty=0,checked_at=? WHERE memory_id=? AND revision=?",
    )
    .bind(now)
    .bind(&memory.id)
    .bind(memory.revision)
    .execute(conn)
    .await?;
    Ok(())
}
