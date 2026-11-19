//! Durable immutable inputs; acknowledgement means received, never extracted.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Row, SqliteConnection};

use crate::Store;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemorySource {
    pub instance_id: String,
    pub receipt_id: String,
    pub sequence: i64,
    pub project_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub revision: i64,
    pub job_id: Option<String>,
    pub recorded_at: i64,
    pub messages: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryBackfillPage {
    pub next_offset: i64,
    pub complete: bool,
}

/// Persist only the Project association. The discussion table holds the latest text.
pub(crate) async fn stage(
    conn: &mut SqliteConnection,
    session: &str,
    turn: &str,
    explicit_project: Option<&str>,
    project_hint: Option<&str>,
    job: Option<&str>,
) -> Result<()> {
    let prior_project: Option<String> = sqlx::query_scalar(
        "SELECT project_id FROM memory_pending_turns WHERE session_id=?1 AND turn_id=?2
         UNION ALL SELECT project_id FROM memory_source_heads WHERE session_id=?1 AND turn_id=?2 LIMIT 1",
    )
    .bind(session)
    .bind(turn)
    .fetch_optional(&mut *conn)
    .await?;
    let bound_project: Option<String> = if let Some(job) = job {
        sqlx::query_scalar("SELECT project_id FROM jobs WHERE id=?")
            .bind(job)
            .fetch_optional(&mut *conn)
            .await?
    } else {
        None
    };
    let Some(project) = explicit_project
        .or(bound_project.as_deref())
        .or(prior_project.as_deref())
        .or(project_hint)
    else {
        return Ok(());
    };
    ensure!(
        bound_project.as_deref().is_none_or(|p| p == project)
            && prior_project.as_deref().is_none_or(|p| p == project),
        "conflict: memory source belongs to another Project"
    );
    sqlx::query(
        "INSERT OR IGNORE INTO memory_pending_turns(session_id,turn_id,project_id) VALUES (?,?,?)",
    )
    .bind(session)
    .bind(turn)
    .bind(project)
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn bind_published_turn(
    conn: &mut SqliteConnection,
    session: &str,
    turn: &str,
    job: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO memory_source_job_bindings(session_id,turn_id,job_id)
         SELECT session_id,turn_id,?3 FROM memory_source_heads WHERE session_id=?1 AND turn_id=?2",
    )
    .bind(session)
    .bind(turn)
    .bind(job)
    .execute(conn)
    .await?;
    Ok(())
}

/// Publish the latest staged version of each turn in the same transaction as the boundary.
pub(crate) async fn flush(conn: &mut SqliteConnection, session: &str, now: i64) -> Result<()> {
    loop {
        let turns = sqlx::query(
            "SELECT d.turn_id,d.job_id,d.messages,p.project_id FROM memory_pending_turns p
             JOIN discussion_turns d USING(session_id,turn_id)
             WHERE p.session_id=? ORDER BY d.position LIMIT 100",
        )
        .bind(session)
        .fetch_all(&mut *conn)
        .await?;
        if turns.is_empty() {
            return Ok(());
        }
        for row in turns {
            let turn: String = row.get("turn_id");
            let project: String = row.get("project_id");
            let job: Option<String> = row.get("job_id");
            let messages: Vec<Value> = serde_json::from_str(&row.get::<String, _>("messages"))?;
            capture(
                conn,
                session,
                &turn,
                &project,
                job.as_deref(),
                &messages,
                now,
            )
            .await?;
            sqlx::query("DELETE FROM memory_pending_turns WHERE session_id=? AND turn_id=?")
                .bind(session)
                .bind(turn)
                .execute(&mut *conn)
                .await?;
        }
    }
}

async fn capture(
    conn: &mut SqliteConnection,
    session: &str,
    turn: &str,
    project: &str,
    job: Option<&str>,
    messages: &[Value],
    now: i64,
) -> Result<()> {
    if messages.is_empty() {
        return Ok(());
    }
    let head = sqlx::query("SELECT project_id,revision,content_hash FROM memory_source_heads WHERE session_id=? AND turn_id=?")
        .bind(session).bind(turn).fetch_optional(&mut *conn).await?;
    ensure!(
        head.as_ref()
            .is_none_or(|row| row.get::<String, _>("project_id") == project),
        "conflict: memory source belongs to another Project"
    );
    let content = json!({"job_id":job,"messages":messages});
    let hash = crate::store::hash_bytes(content.to_string().as_bytes());
    if let Some(head) = &head {
        let prior_hash: String = head.get("content_hash");
        if prior_hash == hash {
            return Ok(());
        }
        // A late Job attachment is represented separately. Avoid extracting the
        // same message content again just because its ownership changed.
        if job.is_some() {
            let unbound = json!({"job_id":null,"messages":messages});
            if prior_hash == crate::store::hash_bytes(unbound.to_string().as_bytes()) {
                return Ok(());
            }
        }
    }
    let revision = head.as_ref().map_or(1, |r| r.get::<i64, _>("revision") + 1);
    let snapshot = json!({"job_id":job,"messages":messages,"recorded_at":now});
    sqlx::query("INSERT INTO memory_source_heads(session_id,turn_id,project_id,revision,content_hash) VALUES (?,?,?,?,?) ON CONFLICT(session_id,turn_id) DO UPDATE SET revision=excluded.revision,content_hash=excluded.content_hash")
        .bind(session).bind(turn).bind(project).bind(revision).bind(hash).execute(&mut *conn).await?;
    sqlx::query("INSERT INTO memory_source_outbox(receipt_id,project_id,session_id,turn_id,revision,snapshot) VALUES (?,?,?,?,?,?)")
        .bind(crate::new_id("receipt")).bind(project).bind(session).bind(turn).bind(revision).bind(snapshot.to_string()).execute(conn).await?;
    Ok(())
}

impl Store {
    /// Explicit historical intake of one Job; no scan or backfill occurs during migration.
    pub async fn backfill_memory_job(
        &self,
        project: &str,
        job: &str,
        offset: i64,
        limit: i64,
    ) -> Result<MemoryBackfillPage> {
        ensure!(
            offset >= 0 && (1..=100).contains(&limit),
            "invalid: memory backfill page"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let count: i64 = sqlx::query_scalar("SELECT COALESCE(json_array_length(data,'$.conversation'),0) FROM jobs WHERE id=? AND project_id=?")
            .bind(job).bind(project).fetch_optional(&mut *tx).await?
            .context("not_found: memory backfill Job in Project")?;
        let rows = sqlx::query("SELECT m.key,m.value FROM jobs j,json_each(j.data,'$.conversation') m WHERE j.id=? AND j.project_id=? AND m.key>=? ORDER BY m.key LIMIT ?")
            .bind(job).bind(project).bind(offset).bind(limit).fetch_all(&mut *tx).await?;
        let mut next_offset = offset;
        for row in rows {
            let index: i64 = row.get("key");
            let message: Value = serde_json::from_str(&row.get::<String, _>("value"))?;
            let session = message["session_id"]
                .as_str()
                .context("invalid: historical session")?;
            let id = message["id"]
                .as_str()
                .context("invalid: historical message ID")?;
            let turn = format!("legacy:{job}:{id}");
            capture(
                &mut tx,
                session,
                &turn,
                project,
                Some(job),
                std::slice::from_ref(&message),
                message["recorded_at"].as_i64().unwrap_or(self.now()),
            )
            .await?;
            next_offset = index + 1;
        }
        tx.commit().await?;
        Ok(MemoryBackfillPage {
            next_offset,
            complete: next_offset >= count,
        })
    }

    /// Page pending inputs. Callers must revisit from zero: acknowledgements may be out of order.
    pub async fn memory_sources(&self, after: i64, limit: i64) -> Result<Vec<MemorySource>> {
        self.read_memory_sources(after, limit, true).await
    }

    /// Recovery includes already acknowledged immutable inputs.
    pub async fn replay_memory_sources(&self, after: i64, limit: i64) -> Result<Vec<MemorySource>> {
        self.read_memory_sources(after, limit, false).await
    }

    async fn read_memory_sources(
        &self,
        after: i64,
        limit: i64,
        pending: bool,
    ) -> Result<Vec<MemorySource>> {
        ensure!(
            (1..=100).contains(&limit) && after >= 0,
            "invalid: memory source page"
        );
        let query = if pending {
            "SELECT o.*,i.instance_id FROM memory_source_outbox o CROSS JOIN memory_source_identity i WHERE o.acknowledged=0 AND o.sequence>? ORDER BY o.sequence LIMIT ?"
        } else {
            "SELECT o.*,i.instance_id FROM memory_source_outbox o CROSS JOIN memory_source_identity i WHERE o.sequence>? ORDER BY o.sequence LIMIT ?"
        };
        let rows = sqlx::query(query)
            .bind(after)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(source_from_row).collect()
    }

    pub async fn memory_source_instance(&self) -> Result<String> {
        Ok(
            sqlx::query_scalar("SELECT instance_id FROM memory_source_identity WHERE singleton=1")
                .fetch_one(&self.pool)
                .await?,
        )
    }

    /// Validate a bounded recovery page, including content, against retained receipts.
    pub async fn verify_memory_sources(&self, sources: &[MemorySource]) -> Result<()> {
        ensure!(sources.len() <= 100, "invalid: recovery page");
        let sequences: std::collections::BTreeSet<_> = sources.iter().map(|s| s.sequence).collect();
        ensure!(
            sequences.len() == sources.len() && sequences.iter().all(|s| *s > 0),
            "invalid: duplicate or nonpositive source sequence"
        );
        // One bounded primary-key batch; preserve full content comparison on every restart.
        let rows = sqlx::query("SELECT o.*,i.instance_id FROM json_each(?) requested CROSS JOIN memory_source_outbox o ON o.sequence=requested.value CROSS JOIN memory_source_identity i")
            .bind(serde_json::to_string(&sequences)?)
            .fetch_all(&self.pool).await?;
        let mut current = std::collections::BTreeMap::new();
        for row in rows {
            let mut source = source_from_row(&row)?;
            for message in &mut source.messages {
                if let Some(object) = message.as_object_mut() {
                    object.retain(|key, value| {
                        !matches!(key.as_str(), "session_id" | "recorded_at") || !value.is_null()
                    });
                }
            }
            current.insert(source.sequence, source);
        }
        for source in sources {
            ensure!(
                current.get(&source.sequence) == Some(source),
                "conflict: tasks and memory histories differ; restore a compatible database pair"
            );
        }
        Ok(())
    }

    /// Idempotent individual acknowledgement, valid only for this source database instance.
    /// Retained snapshots are intentionally not pruned until coordinated recovery supports it.
    pub async fn acknowledge_memory_source(&self, instance: &str, receipt: &str) -> Result<()> {
        let result = sqlx::query("UPDATE memory_source_outbox SET acknowledged=1 WHERE receipt_id=? AND EXISTS(SELECT 1 FROM memory_source_identity WHERE instance_id=?)")
            .bind(receipt).bind(instance).execute(&self.pool).await?;
        ensure!(
            result.rows_affected() == 1,
            "conflict: unknown memory source receipt"
        );
        Ok(())
    }
}

fn source_from_row(r: &sqlx::sqlite::SqliteRow) -> Result<MemorySource> {
    let snapshot: Value = serde_json::from_str(&r.get::<String, _>("snapshot"))?;
    Ok(MemorySource {
        instance_id: r.get("instance_id"),
        receipt_id: r.get("receipt_id"),
        sequence: r.get("sequence"),
        project_id: r.get("project_id"),
        session_id: r.get("session_id"),
        turn_id: r.get("turn_id"),
        revision: r.get("revision"),
        job_id: snapshot["job_id"].as_str().map(str::to_owned),
        recorded_at: snapshot["recorded_at"]
            .as_i64()
            .context("invalid source timestamp")?,
        messages: serde_json::from_value(snapshot["messages"].clone())?,
    })
}

/// Durable source revocation; retained even after reopening or deleting a Job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryJobCancellation {
    pub sequence: i64,
    pub project_id: String,
    pub job_id: String,
    pub revision: i64,
    pub cancelled_at: i64,
}

impl Store {
    pub async fn memory_cancellation_head(&self) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT coalesce(max(sequence),0) FROM memory_job_cancellations")
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn memory_job_cancellations(
        &self,
        after: i64,
        limit: i64,
    ) -> Result<Vec<MemoryJobCancellation>> {
        ensure!((1..=100).contains(&limit), "invalid cancellation page size");
        let rows = sqlx::query("SELECT sequence,project_id,job_id,revision,cancelled_at FROM memory_job_cancellations WHERE sequence>? ORDER BY sequence LIMIT ?")
            .bind(after).bind(limit).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| {
                Ok(MemoryJobCancellation {
                    sequence: row.try_get("sequence")?,
                    project_id: row.try_get("project_id")?,
                    job_id: row.try_get("job_id")?,
                    revision: row.try_get("revision")?,
                    cancelled_at: row.try_get("cancelled_at")?,
                })
            })
            .collect()
    }

    /// Read-only fallback also supports snapshots made before cancellation history existed.
    pub async fn memory_jobs_cancelled(&self, jobs: &[String]) -> Result<bool> {
        ensure!(jobs.len() <= 1024, "too many source Jobs");
        let ids = serde_json::to_string(jobs)?;
        let current: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE id IN (SELECT value FROM json_each(?)) AND json_extract(data,'$.status')='CANCELLED')")
            .bind(&ids).fetch_one(&self.pool).await?;
        if current {
            return Ok(true);
        }
        let has_history: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='memory_job_cancellations')",
        )
        .fetch_one(&self.pool)
        .await?;
        if !has_history {
            return Ok(false);
        }
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM memory_job_cancellations WHERE job_id IN (SELECT value FROM json_each(?)))")
            .bind(ids).fetch_one(&self.pool).await?)
    }
}

impl Store {
    async fn has_memory_source_job_bindings(&self) -> Result<bool> {
        // Read-only stores can still be on schema 27, before late bindings were persisted.
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema
                WHERE type='table' AND name='memory_source_job_bindings')",
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn memory_job_sources(&self, job: &str, after: i64) -> Result<Vec<MemorySource>> {
        // A Job can attach a previously submitted turn before the next boundary.
        // Include its immutable receipts in cancellation without queuing a new revision.
        let query = if self.has_memory_source_job_bindings().await? {
            "WITH receipts AS (
                SELECT sequence FROM memory_source_outbox WHERE json_extract(snapshot,'$.job_id')=?1 AND sequence>?2
                UNION
                SELECT o.sequence FROM memory_source_job_bindings d JOIN memory_source_outbox o USING(session_id,turn_id)
                    WHERE d.job_id=?1 AND o.sequence>?2
             ORDER BY sequence LIMIT 100)
             SELECT o.*,i.instance_id FROM receipts r JOIN memory_source_outbox o USING(sequence)
                CROSS JOIN memory_source_identity i ORDER BY o.sequence"
        } else {
            "SELECT o.*,i.instance_id FROM memory_source_outbox o CROSS JOIN memory_source_identity i
             WHERE json_extract(o.snapshot,'$.job_id')=?1 AND o.sequence>?2
             ORDER BY o.sequence LIMIT 100"
        };
        let rows = sqlx::query(query)
            .bind(job)
            .bind(after)
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| {
                let mut source = source_from_row(row)?;
                source.job_id = Some(job.to_owned());
                Ok(source)
            })
            .collect()
    }

    pub async fn memory_turn_jobs(
        &self,
        instance: &str,
        session: &str,
        turn: &str,
    ) -> Result<Vec<String>> {
        let query = if self.has_memory_source_job_bindings().await? {
            "SELECT job_id FROM (
                SELECT json_extract(snapshot,'$.job_id') AS job_id FROM memory_source_outbox WHERE session_id=?1 AND turn_id=?2
                UNION
                SELECT job_id FROM memory_source_job_bindings WHERE session_id=?1 AND turn_id=?2
             ) WHERE job_id IS NOT NULL AND EXISTS(SELECT 1 FROM memory_source_identity WHERE instance_id=?3)"
        } else {
            "SELECT DISTINCT json_extract(snapshot,'$.job_id') AS job_id FROM memory_source_outbox
             WHERE session_id=?1 AND turn_id=?2 AND json_extract(snapshot,'$.job_id') IS NOT NULL
                AND EXISTS(SELECT 1 FROM memory_source_identity WHERE instance_id=?3)"
        };
        Ok(sqlx::query_scalar(query)
            .bind(session)
            .bind(turn)
            .bind(instance)
            .fetch_all(&self.pool)
            .await?)
    }
}
