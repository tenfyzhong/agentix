//! Bounded routing facts and prompt preparation for host-side classifiers.
use anyhow::{Result, ensure};
use serde_json::{Value, json};

use crate::{Service, Store, WriteOptions};
use std::path::Path;

// Fetch one sentinel beyond each limit so callers never mistake partial coverage
// for an exhaustive candidate set. Text is bounded before it leaves SQLite.
const JOBS: &str = "SELECT json_object(
    'id',id,'project_id',project_id,'status',json_extract(data,'$.status'),
    'revision',json_extract(data,'$.revision'),'issue_id',json_extract(data,'$.issue_id'),
    'title',substr(json_extract(data,'$.title'),1,300),
    'prompt',substr(json_extract(data,'$.prompt'),1,4000),
    'goal',substr(json_extract(data,'$.goal'),1,2000),
    'review_policy',json_extract(data,'$.review_policy'),
    'session_id',json_extract(data,'$.session_id'),
    'followup_session_id',json_extract(data,'$.followup_session_id'),
    'completed_tasks_complete',json((SELECT CASE
        WHEN count(*)<=8 AND COALESCE(max(title_length),0)<=300 THEN 'true' ELSE 'false' END
        FROM (SELECT length(json_extract(data,'$.title')) AS title_length
            FROM tasks WHERE job_id=jobs.id AND json_extract(data,'$.status')='DONE'
            ORDER BY rowid DESC LIMIT 9))),
    'completed_tasks',json((SELECT json_group_array(json(summary)) FROM (
        SELECT * FROM (
            SELECT rowid AS task_rowid,json_object('id',id,'status','DONE',
                'title',substr(json_extract(data,'$.title'),1,300)) AS summary
            FROM tasks WHERE job_id=jobs.id AND json_extract(data,'$.status')='DONE'
            ORDER BY rowid DESC LIMIT 8
        ) ORDER BY task_rowid
    ))),
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
        let task: Option<String> = sqlx::query_scalar(
            "SELECT json_object(
            'project_id',j.project_id,'job_id',t.job_id,'task_id',t.id,
            'issue_id',json_extract(j.data,'$.issue_id'))
            FROM task_leases l JOIN tasks t ON t.id=l.id JOIN jobs j ON j.id=t.job_id
            WHERE l.session_ref=?1 AND json_extract(l.data,'$.lease_expires_at')>?2
            ORDER BY l.rowid LIMIT 1",
        )
        .bind(session)
        .bind(self.now())
        .fetch_optional(&self.pool)
        .await?;
        Ok(task
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or_else(
                || json!({"project_id":null,"job_id":null,"task_id":null,"issue_id":null}),
            ))
    }

    /// Read bounded TODO/ACTIVE requirements. Always reread a selected Issue before linking.
    pub async fn routing_issues(&self, project: &str) -> Result<Value> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT json_object(
            'id',id,'project_id',project_id,'status',json_extract(data,'$.status'),
            'revision',json_extract(data,'$.revision'),'content_hash',json_extract(data,'$.content_hash'),
            'milestone_id',json_extract(data,'$.milestone_id'),
            'title',substr(json_extract(data,'$.title'),1,300),
            'requirement',substr(json_extract(data,'$.requirement'),1,2000),
            'acceptance',substr(json_extract(data,'$.acceptance'),1,2000),
            'truncated',COALESCE(length(json_extract(data,'$.title'))>300,0)
                OR COALESCE(length(json_extract(data,'$.requirement'))>2000,0)
                OR COALESCE(length(json_extract(data,'$.acceptance'))>2000,0))
            FROM issues WHERE project_id=? AND json_extract(data,'$.status') IN ('TODO','ACTIVE')
                AND json_extract(data,'$.archived_at') IS NULL
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

    /// Only this session's Jobs can produce withdrawal notices.
    pub async fn issue_cancellations(&self, session: &str) -> Result<Value> {
        let rows: Vec<String> = sqlx::query_scalar(
            "WITH candidates AS (
                SELECT id FROM jobs WHERE json_extract(data,'$.session_id')=?1
                UNION SELECT id FROM jobs WHERE json_extract(data,'$.followup_session_id')=?1
                UNION SELECT job_id FROM tasks WHERE json_extract(data,'$.last_session')=?1
            ) SELECT json_object('id',i.id,'job_id',j.id)
            FROM candidates c CROSS JOIN jobs j ON j.id=c.id
            JOIN issues i ON i.id=json_extract(j.data,'$.issue_id')
            WHERE json_extract(i.data,'$.status')='CANCELLED'
            ORDER BY json_extract(i.data,'$.cancelled_at') DESC,i.id DESC,j.id DESC LIMIT 32",
        )
        .bind(session)
        .fetch_all(&self.pool)
        .await?;
        Ok(json!(
            rows.iter()
                .map(|r| serde_json::from_str::<Value>(r))
                .collect::<std::result::Result<Vec<_>, _>>()?
        ))
    }

    /// Point-read a current association with the same bounded text as candidates.
    pub async fn routing_issue(&self, id: &str) -> Result<Value> {
        let row: Option<String> = sqlx::query_scalar(
            "SELECT json_set(json_remove(data,'$.notes'),
                '$.title',substr(json_extract(data,'$.title'),1,300),
                '$.requirement',substr(json_extract(data,'$.requirement'),1,2000),
                '$.acceptance',substr(json_extract(data,'$.acceptance'),1,2000)) FROM issues WHERE id=?",
        ).bind(id).fetch_optional(&self.pool).await?;
        Ok(row
            .map(|r| serde_json::from_str(&r))
            .transpose()?
            .unwrap_or(Value::Null))
    }

    pub async fn routing_milestone(&self, id: &str) -> Result<Value> {
        let row: Option<String> = sqlx::query_scalar(
            "SELECT json_set(data,'$.title',substr(json_extract(data,'$.title'),1,300),
                '$.goal',substr(json_extract(data,'$.goal'),1,2000)) FROM milestones WHERE id=?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row
            .map(|r| serde_json::from_str(&r))
            .transpose()?
            .unwrap_or(Value::Null))
    }

    /// Revalidate an exact Job ID without reading its prompt or conversation.
    pub async fn routing_revision(&self, job: &str) -> Result<Value> {
        let row: Option<String> = sqlx::query_scalar(
            "SELECT json_object(
            'id',id,'project_id',project_id,'revision',json_extract(data,'$.revision'),
            'status',json_extract(data,'$.status'),'archived_at',json_extract(data,'$.archived_at'),
            'issue_id',json_extract(data,'$.issue_id')
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
    /// Refresh leases, then return bounded classifier input without a vault-wide import.
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
        options.session_ref = Some(session.into());
        let heartbeat = self
            .execute(
                json!({"command":"session.heartbeat","session":session}),
                options,
            )
            .await?;
        ensure!(
            heartbeat.projection_pending.is_none(),
            "Taskix synchronization pending"
        );
        // Expiry can revoke the assignment; never reuse the initial IDs.
        let mut context = self.store().routing_assignment(session).await?;
        context["issue_cancellations"] = self.store().issue_cancellations(session).await?;
        context["issue"] = match context["issue_id"].as_str() {
            Some(id) => self.store().routing_issue(id).await?,
            None => Value::Null,
        };
        context["milestone"] = match context["issue"]["milestone_id"].as_str() {
            Some(id) => self.store().routing_milestone(id).await?,
            None => Value::Null,
        };
        context["documents"] = serde_json::to_value(&self.config().documents)?;
        context["context_owner"] = json!("external_agent_team");
        context["editable_regions"] = json!(["Goal", "Notes", "Plan body"]);
        if let Some(project) = project {
            let mut candidates = self.store().routing_candidates(&project.id).await?;
            let issues = self.store().routing_issues(&project.id).await?;
            let same_project =
                context["project_id"].is_null() || context["project_id"] == project.id;
            candidates["complete"] = json!(
                candidates["complete"] == true
                    && issues["complete"] == true
                    && same_project
                    && project.archived_at.is_none()
            );
            context["project_id"] = json!(project.id);
            context["issue_candidates"] = issues["entries"].clone();
            context["routing"] = candidates;
        } else {
            context["issue_candidates"] = json!([]);
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
