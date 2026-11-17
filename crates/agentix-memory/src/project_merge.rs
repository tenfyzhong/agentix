//! Transactional reassignment of project memory, including durable work and replay aliases.
use crate::{Memory, MemoryStore, Source, Status, store};
use anyhow::{Result, ensure};
use sqlx::SqliteConnection;

pub(crate) async fn canonical(conn: &mut SqliteConnection, project: &str) -> Result<String> {
    Ok(
        sqlx::query_scalar("SELECT target FROM project_merges WHERE source=?")
            .bind(project)
            .fetch_optional(conn)
            .await?
            .unwrap_or_else(|| project.to_owned()),
    )
}

pub(crate) async fn source(conn: &mut SqliteConnection, source: &Source) -> Result<Source> {
    let mut source = source.clone();
    source.project_id = canonical(conn, &source.project_id).await?;
    Ok(source)
}

impl MemoryStore {
    /// Preserve IDs, immutable historical versions and evidence while changing ownership.
    pub async fn merge_project(&self, source: &str, target: &str) -> Result<bool> {
        ensure!(
            !source.is_empty() && !target.is_empty(),
            "invalid: Project identity"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let source = canonical(&mut tx, source).await?;
        let target = canonical(&mut tx, target).await?;
        if source == target {
            return Ok(false);
        }
        let collisions:Vec<String>=sqlx::query_scalar("SELECT a.memory_id FROM memory_facts a JOIN memory_facts b ON a.fact_key=b.fact_key WHERE a.project_id=? AND b.project_id=? AND a.status='active' AND b.status='active' UNION SELECT b.memory_id FROM memory_facts a JOIN memory_facts b ON a.fact_key=b.fact_key WHERE a.project_id=? AND b.project_id=? AND a.status='active' AND b.status='active'")
            .bind(&source).bind(&target).bind(&source).bind(&target).fetch_all(&mut *tx).await?;
        let rows:Vec<String>=sqlx::query_scalar("SELECT data FROM memories WHERE project_id=? OR id IN (SELECT value FROM json_each(?))")
            .bind(&source).bind(serde_json::to_string(&collisions)?).fetch_all(&mut *tx).await?;
        // Release the unique active fact slots before moving either side of a collision.
        sqlx::query("UPDATE memory_facts SET status='conflicted' WHERE memory_id IN (SELECT value FROM json_each(?))")
            .bind(serde_json::to_string(&collisions)?).execute(&mut *tx).await?;
        migrate_associations(&mut tx, &source, &target).await?;
        for data in rows {
            let mut memory: Memory = serde_json::from_str(&data)?;
            memory.project_id.clone_from(&target);
            memory.revision += 1;
            memory.updated_at = time::OffsetDateTime::now_utc().unix_timestamp();
            if collisions.contains(&memory.id) {
                memory.status = Status::Conflicted;
                memory.reason = "Project merge retained overlapping active facts for review".into();
            }
            store::save(&mut tx, &memory).await?;
        }
        sqlx::query("UPDATE project_merges SET target=? WHERE target=?")
            .bind(&target)
            .bind(&source)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO project_merges(source,target) VALUES (?,?)")
            .bind(&source)
            .bind(&target)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.notify_work();
        self.notify_change(&target);
        Ok(true)
    }
}

async fn migrate_associations(
    conn: &mut SqliteConnection,
    source: &str,
    target: &str,
) -> Result<()> {
    // Moving current ownership does not rewrite the immutable memory_versions table.
    sqlx::query(
        "UPDATE sources SET project_id=?,data=json_set(data,'$.project_id',?) WHERE project_id=?",
    )
    .bind(target)
    .bind(target)
    .bind(source)
    .execute(&mut *conn)
    .await?;
    // Existing workers must reclaim under their new Project, never publish stale results.
    sqlx::query("UPDATE work_items SET state='pending',attempts=max(0,attempts-1),generation=generation+1,owner=NULL,lease_until=NULL WHERE project_id=? AND state='running'")
        .bind(source).execute(&mut *conn).await?;
    for table in [
        "memories",
        "source_heads",
        "source_turns",
        "source_jobs",
        "source_job_turns",
        "work_items",
        "memory_compactions",
        "memory_facts",
    ] {
        sqlx::query(&format!(
            "UPDATE {table} SET project_id=? WHERE project_id=?"
        ))
        .bind(target)
        .bind(source)
        .execute(&mut *conn)
        .await?;
    }
    // These per-project caches may overlap; rebuild caches while retaining suppression fences.
    sqlx::query("INSERT OR IGNORE INTO suppressions(project_id,evidence_key,memory_id) SELECT ?,evidence_key,memory_id FROM suppressions WHERE project_id=?")
        .bind(target).bind(source).execute(&mut *conn).await?;
    sqlx::query("INSERT INTO cancelled_jobs(project_id,job_id,revision,cancelled_at) SELECT ?,job_id,revision,cancelled_at FROM cancelled_jobs WHERE project_id=? ON CONFLICT(project_id,job_id) DO UPDATE SET revision=max(revision,excluded.revision),cancelled_at=max(cancelled_at,excluded.cancelled_at)")
        .bind(target).bind(source).execute(&mut *conn).await?;
    for table in [
        "suppressions",
        "cancelled_jobs",
        "context_deliveries",
        "context_receipts",
        "embedding_profiles",
        "memory_vectors",
        "scheduler_projects",
    ] {
        sqlx::query(&format!("DELETE FROM {table} WHERE project_id=?"))
            .bind(source)
            .execute(&mut *conn)
            .await?;
    }
    sqlx::query("INSERT OR IGNORE INTO scheduler_projects(project_id) VALUES (?)")
        .bind(target)
        .execute(conn)
        .await?;
    Ok(())
}
