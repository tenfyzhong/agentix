//! Taskix-owned, opportunistic retention with bounded work and durable progress.
use crate::{Store, event_maintenance::compact_payload};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection};
use std::time::{Duration, Instant};

const BATCH: i64 = 64;
#[cfg(test)]
const EXPIRED: &str = "SELECT sequence FROM task_events WHERE json_extract(data,'$.occurred_at') < ? ORDER BY json_extract(data,'$.occurred_at'),sequence LIMIT 64";

impl Store {
    /// Inspect or update the database-owned policy without changing task state.
    pub async fn event_policy(
        &self,
        enabled: Option<bool>,
        days: Option<i64>,
        interval: Option<i64>,
    ) -> Result<Value> {
        ensure!(
            days.is_none_or(|v| (1..=36500).contains(&v)),
            "invalid: retain_days must be between 1 and 36500"
        );
        ensure!(
            interval.is_none_or(|v| (60..=31_536_000).contains(&v)),
            "invalid: interval_seconds must be between 60 and 31536000"
        );
        if enabled.is_some() || days.is_some() || interval.is_some() {
            sqlx::query("UPDATE event_retention SET enabled=COALESCE(?,enabled),retain_days=COALESCE(?,retain_days),interval_seconds=COALESCE(?,interval_seconds),next_run_at=0 WHERE id=1")
                .bind(enabled).bind(days).bind(interval).execute(&self.pool).await?;
        }
        let row = sqlx::query("SELECT * FROM event_retention WHERE id=1")
            .fetch_one(&self.pool)
            .await?;
        Ok(
            json!({"enabled":row.get::<bool,_>("enabled"),"retain_days":row.get::<i64,_>("retain_days"),"interval_seconds":row.get::<i64,_>("interval_seconds"),"next_run_at":row.get::<i64,_>("next_run_at"),"compact_after":row.get::<i64,_>("compact_after"),"compact_through":row.get::<i64,_>("compact_through"),"pruned_through":row.get::<i64,_>("pruned_through"),"runs":row.get::<i64,_>("runs")}),
        )
    }

    /// A single indexed-row check when not due; no consumer or daemon is required.
    pub async fn auto_maintain_events(&self) -> Result<Option<Value>> {
        self.retention_attempt(false).await
    }

    pub(crate) async fn retention_attempt(&self, continuing: bool) -> Result<Option<Value>> {
        let now = self.now();
        let result = self.retention_batch(now, continuing).await;
        // A deferred transaction that loses its snapshot/write race skips this attempt.
        if let Err(error) = &result
            && let Some(sqlx::Error::Database(error)) = error.downcast_ref::<sqlx::Error>()
            && matches!(error.code().as_deref(), Some("5" | "6" | "517"))
        {
            return Ok(None);
        }
        result
    }

    async fn retention_batch(&self, now: i64, continuing: bool) -> Result<Option<Value>> {
        let mut conn = self.maintenance_pool.acquire().await?;
        let due: bool = sqlx::query_scalar(
            "SELECT enabled AND (next_run_at<=? OR ?) FROM event_retention WHERE id=1",
        )
        .bind(now)
        .bind(continuing)
        .fetch_one(&mut *conn)
        .await?;
        if !due {
            return Ok(None);
        }
        let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
        let row = sqlx::query("SELECT * FROM event_retention WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
        if !row.get::<bool, _>("enabled") || (!continuing && row.get::<i64, _>("next_run_at") > now)
        {
            return Ok(None);
        }
        let cutoff = now.saturating_sub(row.get::<i64, _>("retain_days") * 86400);
        let started = Instant::now();
        let candidates = sqlx::query("SELECT sequence,length(CAST(data AS BLOB)) AS bytes FROM task_events WHERE json_extract(data,'$.occurred_at') < ? ORDER BY json_extract(data,'$.occurred_at'),sequence LIMIT 64")
            .bind(cutoff).fetch_all(&mut *tx).await?;
        let mut deleted = Vec::new();
        let mut bytes = 0_i64;
        for candidate in candidates {
            let size: i64 = candidate.get("bytes");
            if !deleted.is_empty()
                && (bytes + size > 2 * 1024 * 1024
                    || started.elapsed() >= Duration::from_millis(10))
            {
                break;
            }
            let sequence: i64 = candidate.get("sequence");
            sqlx::query("DELETE FROM task_events WHERE sequence=?")
                .bind(sequence)
                .execute(&mut *tx)
                .await?;
            deleted.push(sequence);
            bytes += size;
        }
        let through: i64 = row.get("compact_through");
        let expired_pending: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM task_events WHERE json_extract(data,'$.occurred_at') < ?)",
        )
        .bind(cutoff)
        .fetch_one(&mut *tx)
        .await?;
        let (after, compacted) =
            if expired_pending || started.elapsed() >= Duration::from_millis(10) {
                (row.get("compact_after"), 0)
            } else {
                compact_batch(&mut tx, row.get("compact_after"), through).await?
            };
        // Up to 256 pages (~1 MiB at the default page size), never a full VACUUM.
        sqlx::query("PRAGMA incremental_vacuum(256)")
            .execute(&mut *tx)
            .await?;
        let free_pages: i64 = sqlx::query_scalar("PRAGMA freelist_count")
            .fetch_one(&mut *tx)
            .await?;
        let remaining: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM task_events WHERE json_extract(data,'$.occurred_at') < ?) OR EXISTS(SELECT 1 FROM task_events WHERE sequence>? AND sequence<=?)")
            .bind(cutoff).bind(after).bind(through).fetch_one(&mut *tx).await?;
        let remaining = remaining || free_pages >= 256;
        let delay = if remaining {
            5
        } else {
            row.get::<i64, _>("interval_seconds")
        };
        sqlx::query("UPDATE event_retention SET next_run_at=?,compact_after=?,pruned_through=MAX(pruned_through,?),runs=runs+1 WHERE id=1")
            .bind(now.saturating_add(delay)).bind(after).bind(deleted.iter().max().copied().unwrap_or(0)).execute(&mut *tx).await?;
        tx.commit().await?;
        // PASSIVE never waits for readers; physical truncation follows checkpoint progress.
        sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
            .execute(&mut *conn)
            .await?;
        Ok(Some(
            json!({"deleted_events":deleted.len(),"compacted_events":compacted,"more":remaining,"next_run_at":now.saturating_add(delay)}),
        ))
    }
}

async fn compact_batch(
    conn: &mut SqliteConnection,
    after: i64,
    through: i64,
) -> Result<(i64, usize)> {
    if after >= through {
        return Ok((after, 0));
    }
    let started = Instant::now();
    let mut cursor = after;
    let mut count = 0;
    let mut bytes = 0_i64;
    for _ in 0..BATCH {
        let row = sqlx::query("SELECT sequence,length(CAST(data AS BLOB)) AS bytes FROM task_events WHERE sequence>? AND sequence<=? ORDER BY sequence LIMIT 1")
            .bind(cursor).bind(through).fetch_optional(&mut *conn).await?;
        let Some(row) = row else {
            return Ok((through, count));
        };
        let size: i64 = row.get("bytes");
        if bytes > 0 && bytes + size > 2 * 1024 * 1024 {
            break;
        }
        cursor = row.get("sequence");
        // Materialize only one legacy record at a time, even when it exceeds the byte budget.
        let data: String = sqlx::query_scalar("SELECT data FROM task_events WHERE sequence=?")
            .bind(cursor)
            .fetch_one(&mut *conn)
            .await?;
        let mut event: Value = serde_json::from_str(&data)?;
        let compact = compact_payload(&event["payload"]);
        if compact != event["payload"] {
            event["payload"] = compact;
            sqlx::query("UPDATE task_events SET data=? WHERE sequence=?")
                .bind(serde_json::to_string(&event)?)
                .bind(cursor)
                .execute(&mut *conn)
                .await?;
            count += 1;
        }
        bytes += size;
        if started.elapsed() >= Duration::from_millis(10) {
            break;
        }
    }
    Ok((cursor, count))
}

#[cfg(test)]
#[path = "event_retention_tests.rs"]
mod tests;
