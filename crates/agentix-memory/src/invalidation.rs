//! Deterministic source revocation, independent of model output and compaction.
use crate::{Memory, MemoryStore, Source, Status, store};
use anyhow::{Result, ensure};
use sqlx::SqliteConnection;

impl MemoryStore {
    /// Attach cancelled ownership even when its newer source revision has not been ingested.
    pub async fn bind_cancelled_source(&self, source: &Source) -> Result<()> {
        let Some(job) = &source.job_id else {
            return Ok(());
        };
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        bind_turn(&mut tx, source, job).await?;
        index_receipts(&mut tx, source).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn cancellation_cursor(&self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT coalesce((SELECT CAST(value AS INTEGER) FROM memory_metadata WHERE key='cancellation_cursor'),0)")
            .fetch_one(&self.pool).await?)
    }

    pub async fn checkpoint_cancellations(&self, sequence: i64) -> Result<()> {
        sqlx::query("INSERT INTO memory_metadata(key,value) VALUES('cancellation_cursor',?) ON CONFLICT(key) DO UPDATE SET value=max(CAST(value AS INTEGER),CAST(excluded.value AS INTEGER))")
            .bind(sequence).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn invalidate_job(
        &self,
        project: &str,
        job: &str,
        revision: i64,
        at: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("INSERT INTO cancelled_jobs(project_id,job_id,revision,cancelled_at) VALUES(?,?,?,?) ON CONFLICT(project_id,job_id) DO UPDATE SET revision=max(revision,excluded.revision),cancelled_at=max(cancelled_at,excluded.cancelled_at)")
            .bind(project).bind(job).bind(revision).bind(at).execute(&mut *tx).await?;
        invalidate(&mut tx, project, job, at).await?;
        tx.commit().await?;
        self.notify_change(project);
        Ok(())
    }

    /// Bounded candidate provenance for read-only offline validation.
    pub async fn memory_source_jobs(&self, memory: &Memory) -> Result<Vec<String>> {
        let mut jobs = std::collections::BTreeSet::new();
        for evidence in &memory.content.evidence {
            let source = self
                .source(&memory.project_id, &evidence.receipt_id)
                .await?;
            // Include later attachment of an initially unbound discussion turn.
            let rows: Vec<String> = sqlx::query_scalar("SELECT DISTINCT json_extract(data,'$.job_id') FROM sources WHERE instance_id=? AND json_extract(data,'$.session_id')=? AND json_extract(data,'$.turn_id')=? AND json_extract(data,'$.job_id') IS NOT NULL")
                .bind(&source.instance_id).bind(&source.session_id).bind(&source.turn_id).fetch_all(&self.pool).await?;
            jobs.extend(rows);
        }
        Ok(jobs.into_iter().collect())
    }
}

pub(crate) async fn source_valid(conn: &mut SqliteConnection, receipt: &str) -> Result<bool> {
    Ok(!sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM source_jobs s JOIN cancelled_jobs c ON c.project_id=s.project_id AND c.job_id=s.job_id WHERE s.receipt_id=?)")
        .bind(receipt).fetch_one(conn).await?)
}

pub(crate) async fn index_source(conn: &mut SqliteConnection, source: &Source) -> Result<()> {
    if let Some(job) = &source.job_id {
        bind_turn(conn, source, job).await?;
    }
    index_receipts(conn, source).await?;
    let cancelled: Vec<(String,i64)> = sqlx::query_as("SELECT c.job_id,c.cancelled_at FROM source_jobs s JOIN cancelled_jobs c ON c.project_id=s.project_id AND c.job_id=s.job_id WHERE s.receipt_id=?")
        .bind(&source.receipt_id).fetch_all(&mut *conn).await?;
    for (job, at) in cancelled {
        invalidate(conn, &source.project_id, &job, at).await?;
    }
    Ok(())
}

pub(crate) async fn validate_memory(conn: &mut SqliteConnection, memory: &Memory) -> Result<()> {
    if !memory.status.searchable() {
        return Ok(());
    }
    let revoked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM memories WHERE id=? AND status='invalidated')",
    )
    .bind(&memory.id)
    .fetch_one(&mut *conn)
    .await?;
    ensure!(
        !revoked,
        "conflict: invalidated memory requires a new independently verified version"
    );
    for evidence in &memory.content.evidence {
        ensure!(
            source_valid(conn, &evidence.receipt_id).await?,
            "conflict: source Job was cancelled"
        );
    }
    Ok(())
}

async fn invalidate(conn: &mut SqliteConnection, project: &str, job: &str, at: i64) -> Result<()> {
    sqlx::query("UPDATE work_items SET state='cancelled',generation=generation+1,owner=NULL,lease_until=NULL,error='source_job_cancelled' WHERE receipt_id IN (SELECT receipt_id FROM source_jobs WHERE project_id=? AND job_id=?) AND state IN ('pending','running','failed')")
        .bind(project).bind(job).execute(&mut *conn).await?;
    // Page materialization bounds memory use; the transaction fences all concurrent commits.
    loop {
        let ids: Vec<String> = sqlx::query_scalar("SELECT DISTINCT m.id FROM source_jobs s JOIN memory_evidence e ON e.receipt_id=s.receipt_id JOIN memories m ON m.id=e.memory_id WHERE s.project_id=? AND s.job_id=? AND m.status NOT IN ('invalidated','forgotten') LIMIT 100")
            .bind(project).bind(job).fetch_all(&mut *conn).await?;
        if ids.is_empty() {
            break;
        }
        for id in ids {
            let mut memory = store::load(conn, project, &id).await?;
            memory.revision += 1;
            memory.status = Status::Invalidated;
            memory.updated_at = memory.updated_at.max(at);
            memory.reason = format!("source_job_cancelled: {job} at {at}");
            store::save(conn, &memory).await?;
            sqlx::query("UPDATE work_items SET state='cancelled',generation=generation+1,owner=NULL,lease_until=NULL,error='source_job_cancelled' WHERE id IN (SELECT work_id FROM memory_compactions WHERE memory_id=?) AND state IN ('pending','running','failed')")
                .bind(&id).execute(&mut *conn).await?;
        }
    }
    Ok(())
}

async fn bind_turn(conn: &mut SqliteConnection, source: &Source, job: &str) -> Result<()> {
    sqlx::query("INSERT OR IGNORE INTO source_job_turns(instance_id,session_id,turn_id,project_id,job_id) VALUES(?,?,?,?,?)")
        .bind(&source.instance_id).bind(&source.session_id).bind(&source.turn_id).bind(&source.project_id).bind(job).execute(conn).await?;
    Ok(())
}

async fn index_receipts(conn: &mut SqliteConnection, source: &Source) -> Result<()> {
    sqlx::query("INSERT OR IGNORE INTO source_jobs(receipt_id,project_id,job_id) SELECT s.receipt_id,s.project_id,t.job_id FROM source_job_turns t JOIN sources s ON s.instance_id=t.instance_id AND s.project_id=t.project_id AND json_extract(s.data,'$.session_id')=t.session_id AND json_extract(s.data,'$.turn_id')=t.turn_id WHERE t.instance_id=? AND t.session_id=? AND t.turn_id=?")
        .bind(&source.instance_id).bind(&source.session_id).bind(&source.turn_id).execute(conn).await?;
    Ok(())
}
