//! Bounded routing facts and prompt preparation for host-side classifiers.
use anyhow::{Result, ensure};
use serde_json::{Value, json};

use crate::{Service, Store, WriteOptions};
use std::{collections::BTreeSet, path::Path};

// Fetch one sentinel beyond each limit so callers never mistake partial coverage
// for an exhaustive candidate set. Text is bounded before it leaves SQLite.
const JOBS: &str = "SELECT json_object(
    'id',id,'project_id',project_id,'status',json_extract(data,'$.status'),
    'revision',json_extract(data,'$.revision'),
    'title',substr(json_extract(data,'$.title'),1,300),
    'prompt',substr(json_extract(data,'$.prompt'),1,4000),
    'goal',substr(json_extract(data,'$.goal'),1,2000),
    'review_policy',json_extract(data,'$.review_policy'),
    'session_id',json_extract(data,'$.session_id'),
    'followup_session_id',json_extract(data,'$.followup_session_id'),
    'conversation',json((SELECT json_group_array(json(message)) FROM (
        SELECT json_object('role',json_extract(value,'$.role'),
            'text',substr(json_extract(value,'$.text'),1,2000),
            'excerpt',json(CASE WHEN length(json_extract(value,'$.text'))>2000 THEN 'true' ELSE 'false' END)) AS message
        FROM json_each(json_array(json_extract(jobs.data,'$.conversation[#-6]'),json_extract(jobs.data,'$.conversation[#-5]'),json_extract(jobs.data,'$.conversation[#-4]'),json_extract(jobs.data,'$.conversation[#-3]'),json_extract(jobs.data,'$.conversation[#-2]'),json_extract(jobs.data,'$.conversation[#-1]')))
        WHERE value IS NOT NULL
        ORDER BY key
    ))),
    'truncated',COALESCE(length(json_extract(data,'$.title'))>300,0)
        OR COALESCE(length(json_extract(data,'$.prompt'))>4000,0)
        OR COALESCE(length(json_extract(data,'$.goal'))>2000,0)

) FROM jobs WHERE project_id=?1 AND json_extract(data,'$.archived_at') IS NULL
    AND json_extract(data,'$.status') IN ('ACTIVE','PENDING_REVIEW')
ORDER BY rowid DESC LIMIT 33";

const TASKS: &str = "SELECT json_object(
    'id',id,'job_id',job_id,'status',json_extract(data,'$.status'),
    'phase',json_extract(data,'$.phase'),'revision',json_extract(data,'$.revision'),
    'title',substr(json_extract(data,'$.title'),1,300),
    'reason',substr(json_extract(data,'$.reason'),1,1000),
    'last_session',json_extract(data,'$.last_session'),
    'truncated',COALESCE(length(json_extract(data,'$.title'))>300,0)
        OR COALESCE(length(json_extract(data,'$.reason'))>1000,0)
) FROM tasks WHERE job_id IN (SELECT value FROM json_each(?1))
    AND json_extract(data,'$.status') NOT IN ('DONE','CANCELLED')
ORDER BY job_id,rowid LIMIT 257";

impl Store {
    /// Read only current assignment references; never expose lease credentials.
    pub async fn routing_assignment(&self, session: &str) -> Result<Value> {
        let mut tx = self.pool.begin().await?;
        let task: Option<String> = sqlx::query_scalar(
            "SELECT json_object(
            'project_id',j.project_id,'job_id',t.job_id,'task_id',t.id,'inbox_id',NULL)
            FROM task_leases l JOIN tasks t ON t.id=l.id JOIN jobs j ON j.id=t.job_id
            WHERE l.session_ref=?1 AND json_extract(l.data,'$.lease_expires_at')>?2
            ORDER BY l.rowid LIMIT 1",
        )
        .bind(session)
        .bind(self.now())
        .fetch_optional(&mut *tx)
        .await?;
        let inbox: Option<String> = sqlx::query_scalar("SELECT json_object(
            'project_id',project_id,'job_id',json_extract(data,'$.job_id'),'task_id',NULL,'inbox_id',id)
            FROM inbox_entries WHERE json_extract(data,'$.last_session')=?1
            AND json_extract(data,'$.lease.session_ref')=?1
            AND json_extract(data,'$.lease.lease_expires_at')>?2 ORDER BY rowid LIMIT 1")
            .bind(session).bind(self.now()).fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        let mut assignment: Value = task
            .as_deref()
            .or(inbox.as_deref())
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or_else(
                || json!({"project_id":null,"job_id":null,"task_id":null,"inbox_id":null}),
            );
        if let Some(inbox) = inbox {
            let inbox: Value = serde_json::from_str(&inbox)?;
            assignment["inbox_id"] = inbox["inbox_id"].clone();
        }
        Ok(assignment)
    }

    /// Read bounded TODO summaries. Ineligible or truncated entries require investigation.
    pub async fn routing_inbox(&self, project: &str) -> Result<Value> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT json_object(
            'id',id,'content',substr(json_extract(data,'$.content'),1,2000),
            'truncated',length(json_extract(data,'$.content'))>2000
                OR COALESCE(json_extract(data,'$.published'),0)=0
                OR COALESCE(json_extract(data,'$.deleted'),0)!=0
                OR COALESCE(json_extract(data,'$.content_pending'),0)!=0
                OR json_extract(data,'$.lease') IS NOT NULL)
            FROM inbox_entries WHERE project_id=? AND json_extract(data,'$.status')='TODO'
            ORDER BY rowid LIMIT 33",
        )
        .bind(project)
        .fetch_all(&self.pool)
        .await?;
        let mut complete = rows.len() <= 32;
        let mut entries = Vec::new();
        for row in rows.into_iter().take(32) {
            let mut entry: Value = serde_json::from_str(&row)?;
            complete &= entry["truncated"] == 0;
            entry.as_object_mut().unwrap().remove("truncated");
            entries.push(entry);
        }
        Ok(json!({"entries":entries,"complete":complete}))
    }

    /// Revalidate an exact Job ID without reading its prompt or conversation.
    pub async fn routing_revision(&self, job: &str) -> Result<Value> {
        let row: Option<String> = sqlx::query_scalar(
            "SELECT json_object(
            'id',id,'project_id',project_id,'revision',json_extract(data,'$.revision'),
            'status',json_extract(data,'$.status'),'archived_at',json_extract(data,'$.archived_at')
        ) FROM jobs WHERE id=?",
        )
        .bind(job)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row
            .map(|r| serde_json::from_str(&r))
            .transpose()?
            .unwrap_or(Value::Null))
    }

    /// Read candidate Jobs and their Tasks without transferring full conversations.
    /// `complete=false` requires Agent investigation instead of automatic routing.
    pub async fn routing_candidates(&self, project: &str) -> Result<Value> {
        let mut tx = self.pool.begin().await?;
        let rows: Vec<String> = sqlx::query_scalar(JOBS)
            .bind(project)
            .fetch_all(&mut *tx)
            .await?;
        let mut complete = rows.len() <= 32;
        let mut candidates = Vec::with_capacity(rows.len().min(32));
        for row in rows.into_iter().take(32) {
            let mut job: Value = serde_json::from_str(&row)?;
            complete &= job["truncated"] == 0;
            job.as_object_mut().unwrap().remove("truncated");
            candidates.push(json!({"job":job,"tasks":[]}));
        }
        let ids: Vec<_> = candidates.iter().map(|c| &c["job"]["id"]).collect();
        let rows: Vec<String> = sqlx::query_scalar(TASKS)
            .bind(serde_json::to_string(&ids)?)
            .fetch_all(&mut *tx)
            .await?;
        complete &= rows.len() <= 256;
        for row in rows.into_iter().take(256) {
            let mut task: Value = serde_json::from_str(&row)?;
            complete &= task["truncated"] == 0;
            task.as_object_mut().unwrap().remove("truncated");
            if let Some(candidate) = candidates
                .iter_mut()
                .find(|c| c["job"]["id"] == task["job_id"])
            {
                candidate["tasks"].as_array_mut().unwrap().push(task);
            }
        }
        tx.commit().await?;
        Ok(json!({"candidates":candidates,"complete":complete}))
    }
}

impl Service {
    /// Refresh human Inbox changes and leases, then return bounded classifier input.
    pub async fn routing_snapshot(
        &self,
        session: &str,
        cwd: &Path,
        project: Option<&str>,
        mut options: WriteOptions,
    ) -> Result<Value> {
        let assignment = self.store().routing_assignment(session).await?;
        let project = if let Some(id) = assignment["project_id"].as_str().or(project) {
            Some(self.store().project_result(id).await?)
        } else {
            match self.project_for_session(Some(cwd), Some(session)).await? {
                Some(project) => Some(project),
                None => self.project_for_session(None, Some(session)).await?,
            }
        };
        if let Some(project) = &project {
            let _lock = self.lock_output().await?;
            self.reconcile_inboxes_locked(Some(&BTreeSet::from([project.id.clone()])), None, true)
                .await?;
        }
        options.session_ref = Some(session.into());
        let heartbeat = self
            .execute(
                json!({"command":"session.heartbeat","session":session}),
                options,
            )
            .await?;
        ensure!(
            heartbeat.projection_pending.is_none(),
            "Inbox synchronization pending"
        );
        // Import and expiry can revoke the assignment; never reuse the initial IDs.
        let mut context = self.store().routing_assignment(session).await?;
        context["inbox_cancellations"] = heartbeat.result["inbox_cancellations"].clone();
        context["documents"] = serde_json::to_value(&self.config().documents)?;
        context["context_owner"] = json!("external_agent_team");
        context["editable_regions"] = json!(["Goal", "Notes", "Plan body"]);
        if let Some(project) = project {
            let mut candidates = self.store().routing_candidates(&project.id).await?;
            let inbox = self.store().routing_inbox(&project.id).await?;
            let same_project =
                context["project_id"].is_null() || context["project_id"] == project.id;
            candidates["complete"] = json!(
                candidates["complete"] == true
                    && inbox["complete"] == true
                    && same_project
                    && project.archived_at.is_none()
            );
            context["project_id"] = json!(project.id);
            context["inbox_todos"] = inbox["entries"].clone();
            context["routing"] = candidates;
        } else {
            context["inbox_todos"] = json!([]);
            context["routing"] = json!({"complete":false,"candidates":[]});
        }
        Ok(context)
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
    async fn candidate_query_does_not_iterate_entire_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("tasks.sqlite3"))
            .await
            .unwrap();
        let mut conn = store.pool.acquire().await.unwrap();
        sqlx::query("INSERT INTO projects(id,data) VALUES('p','{}')")
            .execute(&mut *conn)
            .await
            .unwrap();
        let messages = vec![json!({"role":"user","text":"History"}); 10_000];
        sqlx::query("INSERT INTO jobs(id,data) VALUES('j',?)")
            .bind(json!({"project_id":"p","status":"ACTIVE","conversation":messages}).to_string())
            .execute(&mut *conn)
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
        let rows: Vec<String> = sqlx::query_scalar(JOBS)
            .bind("p")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
        conn.lock_handle().await.unwrap().remove_progress_handler();
        assert_eq!(
            serde_json::from_str::<Value>(&rows[0]).unwrap()["conversation"]
                .as_array()
                .unwrap()
                .len(),
            6
        );
        assert!(
            steps.load(Ordering::Relaxed) < 2_000,
            "SQLite scanned history: {} steps",
            steps.load(Ordering::Relaxed)
        );
    }
}
