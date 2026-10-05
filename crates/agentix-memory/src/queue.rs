use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Row, SqliteConnection};

use crate::{Actor, AgentConfig, MemoryInput, MemoryStore, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkKind {
    Extract,
    Consolidate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkLease {
    pub id: i64,
    pub project_id: String,
    pub receipt_id: String,
    pub kind: WorkKind,
    pub payload: Value,
    pub generation: i64,
    pub owner: String,
    pub lease_until: i64,
}

#[derive(Debug, Default, Serialize)]
pub struct WorkCounts {
    pub pending: i64,
    pub running: i64,
    pub done: i64,
    pub failed: i64,
    pub cancelled: i64,
}

pub(crate) async fn enqueue_source(conn: &mut SqliteConnection, source: &Source) -> Result<()> {
    let prior: Option<(i64,String,String)> = sqlx::query_as("SELECT revision,receipt_id,project_id FROM source_heads WHERE instance_id=? AND session_id=? AND turn_id=?")
        .bind(&source.instance_id).bind(&source.session_id).bind(&source.turn_id).fetch_optional(&mut *conn).await?;
    if let Some((revision, receipt, project)) = prior {
        ensure!(
            project == source.project_id,
            "conflict: source Project changed"
        );
        if revision >= source.revision {
            if revision == source.revision {
                let data: String =
                    sqlx::query_scalar("SELECT data FROM sources WHERE receipt_id=?")
                        .bind(receipt)
                        .fetch_one(&mut *conn)
                        .await?;
                let prior: Source = serde_json::from_str(&data)?;
                ensure!(
                    prior.messages == source.messages,
                    "conflict: source revision fork requires restore reconciliation"
                );
            }
            return Ok(());
        }
        sqlx::query("UPDATE work_items SET state='cancelled',error='source superseded',owner=NULL,lease_until=NULL WHERE receipt_id=? AND state IN ('pending','running')")
            .bind(receipt).execute(&mut *conn).await?;
    }
    sqlx::query("INSERT INTO source_heads(instance_id,session_id,turn_id,project_id,revision,receipt_id) VALUES(?,?,?,?,?,?) ON CONFLICT(instance_id,session_id,turn_id) DO UPDATE SET revision=excluded.revision,receipt_id=excluded.receipt_id")
        .bind(&source.instance_id).bind(&source.session_id).bind(&source.turn_id).bind(&source.project_id).bind(source.revision).bind(&source.receipt_id).execute(&mut *conn).await?;
    sqlx::query("INSERT OR IGNORE INTO scheduler_projects(project_id) VALUES(?)")
        .bind(&source.project_id)
        .execute(&mut *conn)
        .await?;
    for message in &source.messages {
        let mut offset = 0;
        while offset < message.text.len() {
            let mut end = (offset + 16384).min(message.text.len());
            while !message.text.is_char_boundary(end) {
                end -= 1;
            }
            let payload = json!({"message_id":message.id,"role":message.role,"text":&message.text[offset..end],"offset":offset,"next_offset":if end < message.text.len() {Some(end)} else {None}});
            insert_work(conn, source, "extract", &payload).await?;
            offset = end;
        }
    }
    Ok(())
}

async fn insert_work(
    conn: &mut SqliteConnection,
    source: &Source,
    kind: &str,
    payload: &Value,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO work_items(project_id,receipt_id,kind,payload,priority) VALUES(?,?,?,?,?)",
    )
    .bind(&source.project_id)
    .bind(&source.receipt_id)
    .bind(kind)
    .bind(serde_json::to_string(payload)?)
    .bind(i64::from(source.turn_id.starts_with("legacy:")))
    .execute(conn)
    .await?;
    Ok(())
}

impl MemoryStore {
    pub async fn work_is_current(&self, lease: &WorkLease, now: i64) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM work_items WHERE id=? AND project_id=? AND receipt_id=? AND state='running' AND generation=? AND owner=? AND lease_until>?)")
            .bind(lease.id).bind(&lease.project_id).bind(&lease.receipt_id).bind(lease.generation).bind(&lease.owner).bind(now).fetch_one(&self.pool).await?)
    }

    pub async fn work_details(&self, id: i64) -> Result<Value> {
        let data:String=sqlx::query_scalar("SELECT json_object('id',w.id,'project_id',w.project_id,'receipt_id',w.receipt_id,'kind',w.kind,'state',w.state,'attempts',w.attempts,'generation',w.generation,'error',w.error,'audit',json(a.data)) FROM work_items w LEFT JOIN work_audits a ON a.work_id=w.id AND a.generation=w.generation WHERE w.id=?").bind(id).fetch_optional(&self.pool).await?.context("not_found: memory work")?;
        Ok(serde_json::from_str(&data)?)
    }
    pub(crate) async fn record_work_audit(
        &self,
        lease: &WorkLease,
        audit: &Value,
        now: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        fence(&mut tx, lease, now).await?;
        sqlx::query("INSERT INTO work_audits(work_id,generation,data) VALUES(?,?,?) ON CONFLICT(work_id) DO UPDATE SET generation=excluded.generation,data=excluded.data").bind(lease.id).bind(lease.generation).bind(serde_json::to_string(audit)?).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn claim_work(
        &self,
        owner: &str,
        config: &AgentConfig,
        now: i64,
    ) -> Result<Option<WorkLease>> {
        ensure!(!owner.is_empty(), "missing worker owner");
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let expired: Vec<String> = sqlx::query_scalar("UPDATE work_items SET state='failed',error='lease expired after retry limit',owner=NULL,lease_until=NULL WHERE state='running' AND lease_until<=? AND attempts>=max_attempts RETURNING CASE WHEN json_type(payload,'$.compact') IS NOT NULL THEN project_id ELSE '' END")
            .bind(now).fetch_all(&mut *tx).await?;
        let running: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM work_items WHERE state='running' AND lease_until>?",
        )
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        if running >= i64::try_from(config.max_concurrent_loops)? {
            tx.commit().await?;
            for project in &expired {
                if !project.is_empty() {
                    self.notify_compaction(project);
                }
            }
            return Ok(None);
        }
        let row = sqlx::query("SELECT w.* FROM work_items w JOIN scheduler_projects p ON p.project_id=w.project_id WHERE ((w.state='pending' AND w.available_at<=?) OR (w.state='running' AND w.lease_until<=?)) AND (w.max_attempts IS NULL OR w.attempts<w.max_attempts) AND (SELECT count(*) FROM work_items r WHERE r.project_id=w.project_id AND r.kind=w.kind AND r.state='running' AND r.lease_until>?) < CASE WHEN w.kind='consolidate' THEN 1 ELSE ? END ORDER BY p.last_served,w.priority,w.id LIMIT 1")
            .bind(now).bind(now).bind(now).bind(i64::try_from(config.max_extraction_loops_per_project)?).fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            tx.commit().await?;
            for project in &expired {
                if !project.is_empty() {
                    self.notify_compaction(project);
                }
            }
            return Ok(None);
        };
        let lease = WorkLease {
            id: row.try_get("id")?,
            project_id: row.try_get("project_id")?,
            receipt_id: row.try_get("receipt_id")?,
            kind: if row.try_get::<String, _>("kind")? == "extract" {
                WorkKind::Extract
            } else {
                WorkKind::Consolidate
            },
            payload: serde_json::from_str(&row.try_get::<String, _>("payload")?)?,
            generation: row.try_get::<i64, _>("generation")? + 1,
            owner: owner.into(),
            lease_until: now
                .checked_add(i64::try_from(config.lease_seconds)?)
                .context("invalid lease duration")?,
        };
        sqlx::query("UPDATE work_items SET state='running',attempts=attempts+1,generation=?,owner=?,lease_until=?,max_attempts=coalesce(max_attempts,?),error=NULL WHERE id=?")
            .bind(lease.generation).bind(owner).bind(lease.lease_until).bind(i64::from(config.max_attempts)).bind(lease.id).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE memory_metadata SET value=CAST(value AS INTEGER)+1 WHERE key='scheduler_tick'",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE scheduler_projects SET last_served=(SELECT CAST(value AS INTEGER) FROM memory_metadata WHERE key='scheduler_tick') WHERE project_id=?").bind(&lease.project_id).execute(&mut *tx).await?;
        tx.commit().await?;
        for project in &expired {
            if !project.is_empty() {
                self.notify_compaction(project);
            }
        }
        Ok(Some(lease))
    }

    pub async fn complete_extraction(
        &self,
        lease: &WorkLease,
        candidates: Vec<MemoryInput>,
        now: i64,
    ) -> Result<()> {
        ensure!(
            lease.kind == WorkKind::Extract && candidates.len() <= 16,
            "invalid extraction result"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        fence(&mut tx, lease, now).await?;
        for candidate in &candidates {
            candidate.validate(Actor::Agent)?;
            ensure!(
                candidate
                    .evidence
                    .iter()
                    .any(|e| e.receipt_id == lease.receipt_id
                        && Some(e.message_id.as_str()) == lease.payload["message_id"].as_str()),
                "extraction evidence must include its source message"
            );
            crate::store::validate_evidence(&mut tx, &lease.project_id, candidate).await?;
        }
        if !candidates.is_empty() {
            let data: String = sqlx::query_scalar("SELECT data FROM sources WHERE receipt_id=?")
                .bind(&lease.receipt_id)
                .fetch_one(&mut *tx)
                .await?;
            let source: Source = serde_json::from_str(&data)?;
            let mut batch = Vec::new();
            let mut bytes = 2;
            for candidate in &candidates {
                let value = serde_json::to_value(candidate)?;
                let size = value.to_string().len() + 1;
                if !batch.is_empty() && bytes + size > 48 * 1024 {
                    insert_work(
                        &mut tx,
                        &source,
                        "consolidate",
                        &Value::Array(std::mem::take(&mut batch)),
                    )
                    .await?;
                    bytes = 2;
                }
                batch.push(value);
                bytes += size;
            }
            if !batch.is_empty() {
                insert_work(&mut tx, &source, "consolidate", &Value::Array(batch)).await?;
            }
        }
        finish(&mut tx, lease, &serde_json::to_value(candidates)?).await?;
        tx.commit().await?;
        self.notify_work();
        Ok(())
    }

    pub async fn fail_work(&self, lease: &WorkLease, error: &str, now: i64) -> Result<()> {
        self.fail_work_with_retry(lease, error, now, true).await
    }

    pub(crate) async fn fail_work_with_retry(
        &self,
        lease: &WorkLease,
        error: &str,
        now: i64,
        retry: bool,
    ) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        fence(&mut tx, lease, now).await?;
        ensure!(error.len() <= 2048, "worker error exceeds budget");
        sqlx::query("UPDATE work_items SET state=CASE WHEN NOT ? OR attempts>=max_attempts THEN 'failed' ELSE 'pending' END,available_at=?+min(300,1 << min(attempts,8)),owner=NULL,lease_until=NULL,error=? WHERE id=?")
            .bind(retry).bind(now).bind(error).bind(lease.id).execute(&mut *tx).await?;
        tx.commit().await?;
        self.notify_work();
        if lease.payload.get("compact").is_some() {
            self.notify_compaction(&lease.project_id);
        }
        Ok(())
    }

    pub async fn work_counts(&self) -> Result<WorkCounts> {
        let rows: Vec<(String, i64)> =
            sqlx::query_as("SELECT state,count(*) FROM work_items GROUP BY state")
                .fetch_all(&self.pool)
                .await?;
        let mut counts = WorkCounts::default();
        for (state, count) in rows {
            match state.as_str() {
                "pending" => counts.pending = count,
                "running" => counts.running = count,
                "done" => counts.done = count,
                "failed" => counts.failed = count,
                "cancelled" => counts.cancelled = count,
                _ => {}
            }
        }
        Ok(counts)
    }
}

pub(crate) async fn fence(conn: &mut SqliteConnection, lease: &WorkLease, now: i64) -> Result<()> {
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM work_items WHERE id=? AND project_id=? AND receipt_id=? AND state='running' AND generation=? AND owner=? AND lease_until>?)")
        .bind(lease.id).bind(&lease.project_id).bind(&lease.receipt_id).bind(lease.generation).bind(&lease.owner).bind(now).fetch_one(conn).await?;
    ensure!(valid, "conflict: stale worker lease or source revision");
    Ok(())
}

pub(crate) async fn finish(
    conn: &mut SqliteConnection,
    lease: &WorkLease,
    result: &Value,
) -> Result<()> {
    sqlx::query("UPDATE work_items SET state='done',result=?,owner=NULL,lease_until=NULL,error=NULL WHERE id=?")
        .bind(serde_json::to_string(result)?).bind(lease.id).execute(conn).await?;
    Ok(())
}
