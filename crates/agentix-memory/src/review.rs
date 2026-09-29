//! Low-priority repository reviews share the per-Project consolidation lane.
use crate::{Actor, Memory, MemoryStore, Status, WorkLease, queue, store};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewDecision {
    pub archive: bool,
    pub path: String,
    pub offset: u64,
    pub quote: String,
    pub reason: String,
}

impl MemoryStore {
    /// A revision is reviewed once per repository fingerprint. A pending review
    /// is never duplicated; source intake and explicit backfill take precedence.
    pub async fn schedule_reviews(
        &self,
        project: &str,
        fingerprint: &str,
        limit: i64,
    ) -> Result<usize> {
        ensure!(
            !fingerprint.is_empty() && fingerprint.len() <= 256 && (1..=100).contains(&limit),
            "invalid repository review page"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let rows: Vec<String> = sqlx::query_scalar("SELECT m.data FROM memories m LEFT JOIN memory_reviews r ON r.memory_id=m.id LEFT JOIN work_items w ON w.id=r.work_id WHERE m.project_id=? AND m.status='active' AND json_extract(m.data,'$.actor')='agent' AND (m.valid_until IS NULL OR m.valid_until>unixepoch()) AND (r.memory_id IS NULL OR ((r.revision<>m.revision OR r.fingerprint<>? OR w.state='cancelled') AND w.state NOT IN ('pending','running'))) ORDER BY m.id LIMIT ?")
            .bind(project).bind(fingerprint).bind(limit).fetch_all(&mut *tx).await?;
        for data in &rows {
            let memory: Memory = serde_json::from_str(data)?;
            let receipt = &memory
                .content
                .evidence
                .first()
                .context("agent memory requires evidence")?
                .receipt_id;
            sqlx::query("INSERT OR IGNORE INTO scheduler_projects(project_id) VALUES(?)")
                .bind(project)
                .execute(&mut *tx)
                .await?;
            let result = sqlx::query("INSERT INTO work_items(project_id,receipt_id,kind,payload,priority) VALUES(?,?,'consolidate',?,2)")
                .bind(project).bind(receipt).bind(json!({"review":memory,"fingerprint":fingerprint}).to_string()).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO memory_reviews(memory_id,revision,fingerprint,work_id) VALUES(?,?,?,?) ON CONFLICT(memory_id) DO UPDATE SET revision=excluded.revision,fingerprint=excluded.fingerprint,work_id=excluded.work_id")
                .bind(&memory.id).bind(memory.revision).bind(fingerprint).bind(result.last_insert_rowid()).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        if !rows.is_empty() {
            self.notify_work();
        }
        Ok(rows.len())
    }

    pub(crate) async fn complete_review(
        &self,
        lease: &WorkLease,
        decision: &ReviewDecision,
        now: i64,
    ) -> Result<()> {
        ensure!(
            !decision.reason.trim().is_empty() && decision.reason.len() <= 2048,
            "review reason required"
        );
        let expected: Memory = serde_json::from_value(lease.payload["review"].clone())?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        queue::fence(&mut tx, lease, now).await?;
        let mut current = store::load(&mut tx, &lease.project_id, &expected.id).await?;
        // Human edits, lifecycle changes and new revisions always win over a review.
        if decision.archive
            && current.revision == expected.revision
            && current.actor == Actor::Agent
            && current.status == Status::Active
        {
            current.revision += 1;
            current.updated_at = now;
            current.status = Status::Archived;
            current.reason.clone_from(&decision.reason);
            store::save(&mut tx, &current).await?;
        }
        queue::finish(&mut tx, lease, &serde_json::to_value(decision)?).await?;
        tx.commit().await?;
        Ok(())
    }
}
