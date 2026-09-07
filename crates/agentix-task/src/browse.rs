//! Read-only scopes for IM browsing; never load Plan or Inbox bodies.
use anyhow::Result;
use sqlx::Row;

use crate::{
    Project, Snapshot, Store,
    scoped::{Scope, entities, ids, resolve},
};

/// Records needed by a read-only board or detail view.
#[derive(Debug, Clone, Copy)]
pub enum BrowseScope<'a> {
    Project(&'a str),
    Session(&'a str),
    Job(&'a str),
    Task(&'a str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_board_uses_session_indexes_for_both_association_sources() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("tasks.sqlite3"))
            .await
            .unwrap();
        let rows = sqlx::query(&format!("EXPLAIN QUERY PLAN {SESSION_JOBS}"))
            .bind("target")
            .fetch_all(&store.pool)
            .await
            .unwrap();
        let details: Vec<String> = rows.iter().map(|row| row.get("detail")).collect();
        for index in ["tasks_by_session", "leases_by_session"] {
            assert!(
                details
                    .iter()
                    .any(|detail| detail.contains("SEARCH") && detail.contains(index)),
                "board association must seek through {index}: {details:?}"
            );
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProjectSummary {
    pub project: Project,
    pub job_count: usize,
    pub task_count: usize,
}

const SESSION_JOBS: &str = "WITH related(job_id) AS (
    SELECT job_id FROM tasks WHERE json_extract(data,'$.last_session')=?1
    UNION
    SELECT tasks.job_id FROM task_leases JOIN tasks ON tasks.id=task_leases.id WHERE session_ref=?1
) SELECT jobs.id FROM related JOIN jobs ON jobs.id=related.job_id
JOIN projects ON projects.id=jobs.project_id
WHERE json_extract(jobs.data,'$.archived_at') IS NULL AND json_extract(projects.data,'$.archived_at') IS NULL";

impl Store {
    /// Load only the entities needed by one board or detail view. Expired leases
    /// still associate historical work; callers decide whether to mark it current.
    pub async fn browse_snapshot(&self, selection: BrowseScope<'_>) -> Result<Snapshot> {
        let mut tx = self.pool.begin().await?;
        let mut scope = Scope::default();
        match selection {
            BrowseScope::Project(id) => {
                let id = resolve(&mut tx, "projects", id).await?;
                scope.jobs = ids(&mut tx,
                    "SELECT jobs.id FROM jobs JOIN projects ON projects.id=jobs.project_id WHERE jobs.project_id=? AND json_extract(jobs.data,'$.archived_at') IS NULL AND json_extract(projects.data,'$.archived_at') IS NULL",
                    &id).await?;
                scope.projects.insert(id);
            }
            BrowseScope::Session(session) => {
                scope.jobs = ids(&mut tx, SESSION_JOBS, session).await?;
            }
            BrowseScope::Job(id) => {
                scope.jobs.insert(resolve(&mut tx, "jobs", id).await?);
            }
            BrowseScope::Task(id) => {
                scope.tasks.insert(resolve(&mut tx, "tasks", id).await?);
            }
        }
        if !matches!(selection, BrowseScope::Task(_)) {
            scope.job_tasks(&mut tx).await?;
        }
        scope.parents(&mut tx).await?;
        let state = Snapshot {
            projects: entities(&mut tx, "projects", &scope.projects).await?,
            jobs: entities(&mut tx, "jobs", &scope.jobs).await?,
            tasks: entities(&mut tx, "tasks", &scope.tasks).await?,
            leases: entities(&mut tx, "task_leases", &scope.tasks).await?,
            ..Snapshot::default()
        };
        tx.commit().await?;
        Ok(state)
    }

    /// Count visible work without deserializing Job or Task bodies.
    pub async fn project_summaries(&self) -> Result<Vec<ProjectSummary>> {
        let rows = sqlx::query(
            "SELECT projects.data,COUNT(DISTINCT jobs.id) AS jobs,COUNT(tasks.id) AS tasks
            FROM projects
            LEFT JOIN jobs ON jobs.project_id=projects.id AND json_extract(jobs.data,'$.archived_at') IS NULL
            LEFT JOIN tasks ON tasks.job_id=jobs.id
            WHERE json_extract(projects.data,'$.archived_at') IS NULL
            GROUP BY projects.id ORDER BY projects.rowid",
        ).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| {
                Ok(ProjectSummary {
                    project: serde_json::from_str(&row.get::<String, _>("data"))?,
                    job_count: row.get::<i64, _>("jobs").try_into()?,
                    task_count: row.get::<i64, _>("tasks").try_into()?,
                })
            })
            .collect()
    }
}
