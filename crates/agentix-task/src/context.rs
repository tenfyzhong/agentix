//! Narrow reads for the CLI's current assignment and previous session work.
use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::{Value, json};

use crate::{
    JobStatus, Snapshot, Store,
    scoped::{Scope, entities, ids, resolve},
};

// Keep the session candidate set outermost: project-first joins scan unrelated history.
const PREVIOUS_JOB: &str = "WITH activity AS (
    SELECT job_id, MAX(json_extract(data,'$.updated_at')) AS updated_at
    FROM tasks WHERE json_extract(data,'$.last_session')=?1 GROUP BY job_id
), candidates AS (
    SELECT id FROM jobs WHERE json_extract(data,'$.session_id')=?1
    UNION SELECT id FROM jobs WHERE json_extract(data,'$.followup_session_id')=?1 AND json_extract(data,'$.followup_at') IS NOT NULL
    UNION SELECT job_id FROM activity
), ranked AS (
    SELECT j.id, j.rowid AS position, j.data,
        MAX(json_extract(j.data,'$.created_at'),
            COALESCE(a.updated_at,json_extract(j.data,'$.created_at')),
            CASE WHEN json_extract(j.data,'$.followup_session_id')=?1
                THEN COALESCE(json_extract(j.data,'$.followup_at'),json_extract(j.data,'$.created_at'))
                ELSE json_extract(j.data,'$.created_at') END) AS activity
    FROM candidates c CROSS JOIN jobs j ON j.id=c.id LEFT JOIN activity a ON a.job_id=j.id
    WHERE j.project_id=?2
)
SELECT id FROM ranked r ORDER BY activity DESC, COALESCE((
    SELECT MAX(value) FROM (
        SELECT CASE WHEN substr(r.id,1,4)='job_' THEN substr(r.id,5) END AS value
        UNION ALL
        SELECT substr(t.id,6) FROM tasks t WHERE t.job_id=r.id
            AND json_extract(t.data,'$.last_session')=?1
            AND json_extract(t.data,'$.updated_at')=r.activity AND substr(t.id,1,5)='task_'
        UNION ALL
        SELECT substr(json_extract(r.data,'$.followup_id'),9)
            WHERE json_extract(r.data,'$.followup_session_id')=?1
            AND json_extract(r.data,'$.followup_at')=r.activity
            AND substr(json_extract(r.data,'$.followup_id'),1,8)='message_'
    )
),r.id) DESC, position DESC LIMIT 1";

impl Store {
    /// Load the current assignment and cancellation notifications, not its siblings.
    pub async fn context_snapshot(
        &self,
        task: Option<&str>,
        job: Option<&str>,
        session: Option<&str>,
    ) -> Result<Snapshot> {
        let mut tx = self.pool.begin().await?;
        let mut scope = Scope::default();
        if let Some(task) = task {
            scope.tasks.insert(resolve(&mut tx, "tasks", task).await?);
        } else if let Some(session) = session {
            scope.tasks = ids(
                &mut tx,
                "SELECT id FROM task_leases WHERE session_ref=? ORDER BY rowid LIMIT 1",
                session,
            )
            .await?;
        }
        if let Some(job) = job {
            scope.jobs.insert(resolve(&mut tx, "jobs", job).await?);
        }
        if let Some(session) = session {
            scope.inboxes = ids(&mut tx, "SELECT id FROM inbox_entries WHERE json_extract(data,'$.lease.session_ref')=? ORDER BY rowid LIMIT 1", session).await?;
        }
        scope.parents(&mut tx).await?;
        let tasks: Vec<crate::Task> = entities(&mut tx, "tasks", &scope.tasks).await?;
        let plans = tasks
            .iter()
            .filter_map(|t| t.current_plan.clone())
            .collect();
        let mut state = Snapshot {
            projects: entities(&mut tx, "projects", &scope.projects).await?,
            jobs: entities(&mut tx, "jobs", &scope.jobs).await?,
            leases: entities(&mut tx, "task_leases", &scope.tasks).await?,
            plans: entities(&mut tx, "plans", &plans).await?,
            tasks,
            ..Snapshot::default()
        };
        if let Some(session) = session {
            let cancelled = ids(&mut tx, "SELECT id FROM inbox_entries WHERE json_extract(data,'$.status')='CANCELLED' AND id IN (
                SELECT id FROM inbox_entries WHERE json_extract(data,'$.last_session')=?1
                UNION SELECT id FROM inbox_entries WHERE json_extract(data,'$.job_id') IN (SELECT job_id FROM tasks WHERE json_extract(data,'$.last_session')=?1)
            )", session).await?;
            scope.inboxes.extend(cancelled.iter().cloned());
            state
                .query_context
                .cancelled_inboxes
                .insert(session.to_owned(), cancelled);
        }
        state.inboxes = entities(&mut tx, "inbox_entries", &scope.inboxes).await?;
        Ok(state)
    }

    /// Choose the latest session activity before applying the pending-review filter.
    pub async fn previous_job(&self, session: &str, project: &str) -> Result<Option<Value>> {
        let mut tx = self.pool.begin().await?;
        let id: Option<String> = sqlx::query_scalar(PREVIOUS_JOB)
            .bind(session)
            .bind(project)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(id) = id else { return Ok(None) };
        let jobs: Vec<crate::Job> =
            entities(&mut tx, "jobs", &BTreeSet::from([id.clone()])).await?;
        let job = &jobs[0];
        if job.status != JobStatus::PendingReview || job.archived_at.is_some() {
            return Ok(None);
        }
        let tasks: Vec<String> =
            sqlx::query_scalar("SELECT id FROM tasks WHERE job_id=? ORDER BY rowid")
                .bind(id)
                .fetch_all(&mut *tx)
                .await?;
        let mut value = json!(job);
        value["task_ids"] = json!(tasks);
        Ok(Some(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[tokio::test]
    async fn previous_job_does_not_scan_unrelated_session_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("tasks.sqlite3"))
            .await
            .unwrap();
        let mut conn = store.pool.acquire().await.unwrap();
        sqlx::query("INSERT INTO projects(id,data) VALUES('p','{}')")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("WITH RECURSIVE n(v) AS (VALUES(1) UNION ALL SELECT v+1 FROM n WHERE v<10000)
            INSERT INTO jobs(id,data) SELECT 'job_'||v,json_object('project_id','p','created_at',v,'session_id','other') FROM n").execute(&mut *conn).await.unwrap();
        sqlx::query("UPDATE jobs SET data=json_set(data,'$.followup_session_id','selected','$.followup_at',10001,'$.followup_id','message_selected') WHERE id='job_1'").execute(&mut *conn).await.unwrap();
        let plan: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(&format!("EXPLAIN QUERY PLAN {PREVIOUS_JOB}"))
                .bind("selected")
                .bind("p")
                .fetch_all(&mut *conn)
                .await
                .unwrap();
        let steps = Arc::new(AtomicUsize::new(0));
        let measured = steps.clone();
        conn.lock_handle()
            .await
            .unwrap()
            .set_progress_handler(100, move || {
                measured.fetch_add(100, Ordering::Relaxed);
                true
            });
        let selected: String = sqlx::query_scalar(PREVIOUS_JOB)
            .bind("selected")
            .bind("p")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        conn.lock_handle().await.unwrap().remove_progress_handler();
        assert_eq!(selected, "job_1");
        assert!(
            steps.load(Ordering::Relaxed) < 1000,
            "unrelated history scanned: {} VM steps; {plan:?}",
            steps.load(Ordering::Relaxed)
        );
    }
}
