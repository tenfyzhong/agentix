//! Bounded IM list payloads with counts and ordering evaluated in the database.
use anyhow::{Result, bail, ensure};
use serde::Deserialize;
use sqlx::{Row, SqliteConnection};

use crate::{BrowseScope, JobStatus, Project, Store, TaskPhase, TaskStatus, scoped::resolve};

#[derive(Debug, Deserialize)]
pub struct TaskListItem {
    pub id: String,
    pub title: String,
    pub status: TaskStatus,
    pub phase: Option<TaskPhase>,
    pub reason: Option<String>,
    pub current: bool,
}

#[derive(Debug)]
pub struct TaskBrowsePage {
    pub project: Option<Project>,
    pub job_count: usize,
    pub status_counts: Vec<(TaskStatus, usize)>,
    pub tasks: Vec<TaskListItem>,
    pub total: usize,
    pub page: usize,
    pub pages: usize,
}

#[derive(Debug, Deserialize)]
pub struct JobListItem {
    pub id: String,
    pub title: String,
    pub status: JobStatus,
    pub task_count: usize,
}

#[derive(Debug)]
pub struct JobBrowsePage {
    pub jobs: Vec<JobListItem>,
    pub total: usize,
    pub page: usize,
    pub pages: usize,
}

const CURRENT: &str = "EXISTS(SELECT 1 FROM task_leases l WHERE l.id=t.id
    AND l.session_ref=?2 AND json_extract(l.data,'$.lease_expires_at')>?3)";
const STATUS_ORDER: &str = "CASE json_extract(t.data,'$.status')
    WHEN 'IN_PROGRESS' THEN 0 WHEN 'BLOCKED' THEN 1 WHEN 'WAITING_USER' THEN 2
    WHEN 'TODO' THEN 3 WHEN 'FAILED' THEN 4 WHEN 'DONE' THEN 5 ELSE 6 END";

async fn selection(
    conn: &mut SqliteConnection,
    scope: BrowseScope<'_>,
) -> Result<(String, String, Option<Project>)> {
    match scope {
        BrowseScope::Project(id) => {
            let id = resolve(conn, "projects", id).await?;
            let data: String = sqlx::query_scalar("SELECT data FROM projects WHERE id=?")
                .bind(&id)
                .fetch_one(&mut *conn)
                .await?;
            Ok((
                "SELECT jobs.id FROM jobs JOIN projects ON projects.id=jobs.project_id
                WHERE jobs.project_id=?1 AND json_extract(jobs.data,'$.archived_at') IS NULL
                AND json_extract(projects.data,'$.archived_at') IS NULL"
                    .into(),
                id,
                Some(serde_json::from_str(&data)?),
            ))
        }
        BrowseScope::Session(session) => {
            Ok((crate::browse::SESSION_JOBS.into(), session.into(), None))
        }
        BrowseScope::Job(id) => Ok((
            "SELECT id FROM jobs WHERE id=?1".into(),
            resolve(conn, "jobs", id).await?,
            None,
        )),
        BrowseScope::Task(_) => bail!("invalid: Task detail does not contain a task list"),
    }
}

fn page_bounds(
    total: usize,
    page: usize,
    size: usize,
    minimum_pages: usize,
) -> Result<(usize, usize, i64)> {
    ensure!(size > 0, "invalid: page size must be positive");
    let pages = total.div_ceil(size).max(minimum_pages).max(1);
    let page = page.min(pages - 1);
    // Pages containing only Markdown have no task rows, even at large offsets.
    let offset = page.saturating_mul(size).min(total).try_into()?;
    Ok((page, pages, offset))
}

impl Store {
    /// Read only visible task summaries. `minimum_pages` reserves extra pages
    /// for Job Markdown; those pages may contain no task buttons.
    pub async fn browse_task_page(
        &self,
        scope: BrowseScope<'_>,
        current_session: Option<&str>,
        page: usize,
        size: usize,
        minimum_pages: usize,
    ) -> Result<TaskBrowsePage> {
        let mut tx = self.pool.begin().await?;
        let (selected, value, project) = selection(&mut tx, scope).await?;
        let cte = format!("WITH selected AS ({selected})");
        let job_count: i64 = sqlx::query_scalar(&format!("{cte} SELECT COUNT(*) FROM selected"))
            .bind(&value)
            .fetch_one(&mut *tx)
            .await?;
        let rows = sqlx::query(&format!("{cte} SELECT json_extract(t.data,'$.status') AS status,COUNT(*) AS count
            FROM tasks t WHERE t.job_id IN (SELECT id FROM selected) GROUP BY json_extract(t.data,'$.status')"))
            .bind(&value).fetch_all(&mut *tx).await?;
        let status_counts = rows
            .into_iter()
            .map(|row| {
                Ok((
                    serde_json::from_value(serde_json::Value::String(row.get("status")))?,
                    usize::try_from(row.get::<i64, _>("count"))?,
                ))
            })
            .collect::<Result<Vec<(TaskStatus, usize)>>>()?;
        let total = status_counts.iter().map(|(_, count)| count).sum();
        let (page, pages, offset) = page_bounds(total, page, size, minimum_pages)?;
        let order = if matches!(scope, BrowseScope::Job(_)) {
            "p.position,p.id"
        } else {
            "p.status_order,p.current DESC,p.position,p.id"
        };
        let rows: Vec<String> = sqlx::query_scalar(&format!(
            "{cte}, paged AS MATERIALIZED (
            SELECT t.id,{CURRENT} AS current,{STATUS_ORDER} AS status_order,
                json_extract(t.data,'$.position') AS position
            FROM tasks t WHERE t.job_id IN (SELECT id FROM selected)
            ORDER BY {} LIMIT ?4 OFFSET ?5)
            SELECT json_object('id',t.id,'title',json_extract(t.data,'$.title'),
                'status',json_extract(t.data,'$.status'),'phase',json_extract(t.data,'$.phase'),
                'reason',json_extract(t.data,'$.reason'),
                'current',json(CASE WHEN p.current THEN 'true' ELSE 'false' END))
            FROM paged p JOIN tasks t ON t.id=p.id ORDER BY {order}",
            order.replace("p.", "")
        ))
        .bind(&value)
        .bind(current_session)
        .bind(self.now())
        .bind(i64::try_from(size)?)
        .bind(offset)
        .fetch_all(&mut *tx)
        .await?;
        let tasks = rows
            .iter()
            .map(|row| serde_json::from_str(row).map_err(Into::into))
            .collect::<Result<_>>()?;
        tx.commit().await?;
        Ok(TaskBrowsePage {
            project,
            job_count: job_count.try_into()?,
            status_counts,
            tasks,
            total,
            page,
            pages,
        })
    }

    /// Session-associated Jobs in insertion order, with counts but no authored bodies.
    pub async fn session_job_page(
        &self,
        session: &str,
        page: usize,
        size: usize,
    ) -> Result<JobBrowsePage> {
        let mut tx = self.pool.begin().await?;
        let cte = format!("WITH selected AS ({})", crate::browse::SESSION_JOBS);
        let total: i64 = sqlx::query_scalar(&format!("{cte} SELECT COUNT(*) FROM selected"))
            .bind(session)
            .fetch_one(&mut *tx)
            .await?;
        let total = total.try_into()?;
        let (page, pages, offset) = page_bounds(total, page, size, 1)?;
        let rows: Vec<String> = sqlx::query_scalar(&format!(
            "{cte}, paged AS MATERIALIZED (
            SELECT j.id,j.rowid AS position FROM jobs j WHERE j.id IN (SELECT id FROM selected)
            ORDER BY j.rowid LIMIT ?2 OFFSET ?3)
            SELECT json_object('id',j.id,'title',json_extract(j.data,'$.title'),
                'status',json_extract(j.data,'$.status'),
                'task_count',(SELECT COUNT(*) FROM tasks WHERE job_id=j.id))
            FROM paged p JOIN jobs j ON j.id=p.id ORDER BY p.position"
        ))
        .bind(session)
        .bind(i64::try_from(size)?)
        .bind(offset)
        .fetch_all(&mut *tx)
        .await?;
        let jobs = rows
            .iter()
            .map(|row| serde_json::from_str(row).map_err(Into::into))
            .collect::<Result<_>>()?;
        tx.commit().await?;
        Ok(JobBrowsePage {
            jobs,
            total,
            page,
            pages,
        })
    }
}
