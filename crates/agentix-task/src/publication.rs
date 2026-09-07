//! Durable, deduplicated document work committed with domain changes.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use serde_json::json;
use sqlx::{Row, SqliteConnection};

use crate::{
    Snapshot, Store, new_id,
    scoped::{Scope, ids},
};

#[cfg(test)]
#[path = "publication_tests.rs"]
mod tests;

// Keep each MAX eligible for its Project/activity index instead of scanning
// every Task inside a global GROUP BY. Batch all index seeks in one statement.
const PROJECT_ACTIVITY_QUERY: &str = "SELECT selected.value AS project_id,
    (SELECT MAX(updated_at) FROM (
        SELECT MAX(json_extract(data,'$.updated_at')) AS updated_at FROM jobs WHERE project_id=selected.value
        UNION ALL
        SELECT MAX(json_extract(data,'$.updated_at')) FROM tasks WHERE json_extract(data,'$.project_id')=selected.value
    )) AS updated_at
FROM json_each(?1) AS selected";

pub(crate) async fn enqueue_changes(
    conn: &mut SqliteConnection,
    before: &Snapshot,
    after: &Snapshot,
    command: &str,
) -> Result<()> {
    if command == "inbox.publish" {
        return Ok(());
    }
    let mut keys = BTreeSet::new();
    let old_projects: BTreeMap<_, _> = before.projects.iter().map(|v| (&v.id, v)).collect();
    let old_jobs: BTreeMap<_, _> = before.jobs.iter().map(|v| (&v.id, v)).collect();
    let old_tasks: BTreeMap<_, _> = before.tasks.iter().map(|v| (&v.id, v)).collect();
    let old_inboxes: BTreeMap<_, _> = before.inboxes.iter().map(|v| (&v.id, v)).collect();
    let mut dependents = BTreeSet::new();
    for project in &after.projects {
        if old_projects.get(&project.id).copied() != Some(project) {
            keys.insert(format!("board:{}", project.id));
            keys.insert(format!("inbox:{}", project.id));
            for job in after.jobs.iter().filter(|j| j.project_id == project.id) {
                keys.insert(format!("job:{}", job.id));
            }
            for task in ids(
                conn,
                "SELECT id FROM tasks WHERE json_extract(data,'$.project_id')=?",
                &project.id,
            )
            .await?
            {
                keys.insert(format!("task:{task}"));
            }
        }
    }
    for job in &after.jobs {
        let old = old_jobs.get(&job.id).copied();
        if old == Some(job) {
            continue;
        }
        keys.insert(format!("job:{}", job.id));
        keys.insert(format!("board:{}", job.project_id));
        if old.is_some_and(|old| {
            old.document_path != job.document_path
                || old.archived_at != job.archived_at
                || old.name != job.name
        }) {
            let tasks = ids(conn, "SELECT id FROM tasks WHERE job_id=?", &job.id).await?;
            for task in &tasks {
                keys.insert(format!("task:{task}"));
            }
            dependents.extend(tasks);
            keys.insert(format!("inbox:{}", job.project_id));
        }
    }
    for task in &after.tasks {
        if old_tasks.get(&task.id).copied() == Some(task) {
            continue;
        }
        keys.insert(format!("task:{}", task.id));
        keys.insert(format!("job:{}", task.job_id));
        keys.insert(format!("board:{}", task.project_id));
        dependents.insert(task.id.clone());
    }
    let related_jobs=ids(conn,"SELECT DISTINCT tasks.job_id FROM task_dependencies JOIN tasks ON tasks.id=task_dependencies.task_id WHERE dependency_id IN (SELECT value FROM json_each(?))",&json!(dependents).to_string()).await?;
    for job in related_jobs {
        keys.insert(format!("job:{job}"));
    }
    for entry in &after.inboxes {
        if old_inboxes.get(&entry.id).copied() != Some(entry) {
            keys.insert(format!("inbox:{}", entry.project_id));
        }
    }
    let jobs: BTreeSet<_> = after.jobs.iter().map(|j| &j.id).collect();
    let tasks: BTreeSet<_> = after.tasks.iter().map(|t| &t.id).collect();
    let projects: BTreeSet<_> = after.projects.iter().map(|p| &p.id).collect();
    for job in before.jobs.iter().filter(|j| !jobs.contains(&j.id)) {
        keys.insert(format!("job:{}", job.id));
        keys.insert(format!("board:{}", job.project_id));
        keys.insert(format!("inbox:{}", job.project_id));
    }
    for task in before.tasks.iter().filter(|t| !tasks.contains(&t.id)) {
        keys.insert(format!("task:{}", task.id));
        if let Some(plan) = &task.current_plan {
            keys.insert(format!("plan:{plan}"));
        }
    }
    for project in before.projects.iter().filter(|p| !projects.contains(&p.id)) {
        keys.insert(format!("board:{}", project.id));
        keys.insert(format!("inbox:{}", project.id));
    }
    if !keys.is_empty() {
        keys.insert("dashboard".into());
    }
    for key in keys {
        sqlx::query("INSERT INTO pending_documents(key,generation) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET generation=excluded.generation")
            .bind(key).bind(new_id("publication")).execute(&mut *conn).await?;
    }
    Ok(())
}

impl Store {
    pub async fn has_pending_documents(&self) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pending_documents) OR EXISTS(SELECT 1 FROM document_deletions)").fetch_one(&self.pool).await?)
    }

    pub(crate) async fn document_paths(
        &self,
        keys: Option<&BTreeSet<String>>,
    ) -> Result<BTreeMap<String, String>> {
        let rows = if let Some(keys) = keys {
            sqlx::query("SELECT key,path FROM document_registry WHERE key IN (SELECT value FROM json_each(?))")
                .bind(json!(keys).to_string()).fetch_all(&self.pool).await?
        } else {
            sqlx::query("SELECT key,path FROM document_registry")
                .fetch_all(&self.pool)
                .await?
        };
        Ok(rows
            .into_iter()
            .map(|r| (r.get("key"), r.get("path")))
            .collect())
    }

    pub(crate) async fn pending_documents(&self) -> Result<BTreeMap<String, String>> {
        let rows = sqlx::query("SELECT key,generation FROM pending_documents ORDER BY key")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get("key"), r.get("generation")))
            .collect())
    }

    pub(crate) async fn pending_batch(&self, after: &str) -> Result<BTreeMap<String, String>> {
        let rows = sqlx::query(
            "SELECT key,generation FROM pending_documents WHERE key>? ORDER BY key LIMIT 128",
        )
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get("key"), r.get("generation")))
            .collect())
    }

    pub(crate) async fn document_generation(&self, key: &str) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT generation FROM pending_documents WHERE key=?")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub(crate) async fn pending_inboxes(&self) -> Result<BTreeSet<String>> {
        let keys: Vec<String> = sqlx::query_scalar(
            "SELECT key FROM pending_documents WHERE key>='inbox:' AND key<'inbox;'",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(keys.into_iter().map(|key| key[6..].to_owned()).collect())
    }

    pub(crate) async fn rebuild_pending(&self) -> Result<bool> {
        Ok(
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pending_documents WHERE key='rebuild')",
            )
            .fetch_one(&self.pool)
            .await?,
        )
    }

    pub(crate) async fn acknowledge_documents(
        &self,
        pending: &BTreeMap<String, String>,
        paths: &BTreeMap<String, String>,
        remove: &BTreeSet<String>,
        sequence: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        for key in remove {
            sqlx::query("DELETE FROM document_registry WHERE key=?")
                .bind(key)
                .execute(&mut *tx)
                .await?;
        }
        for (key, path) in paths {
            sqlx::query("INSERT INTO document_registry(key,path) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET path=excluded.path")
                .bind(key).bind(path).execute(&mut *tx).await?;
        }
        for (key, generation) in pending {
            sqlx::query("DELETE FROM pending_documents WHERE key=? AND generation=?")
                .bind(key)
                .bind(generation)
                .execute(&mut *tx)
                .await?;
        }
        let remaining: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pending_documents)")
            .fetch_one(&mut *tx)
            .await?;
        if !remaining {
            sqlx::query("INSERT INTO projection_state(key,value) VALUES ('sequence',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value")
                .bind(sequence.to_string()).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn projection_snapshot(&self, key: &str) -> Result<Snapshot> {
        let mut tx = self.pool.begin().await?;
        let mut scope = Scope::default();
        match key.split_once(':') {
            Some(("task", id)) => {
                scope.tasks.insert(id.into());
            }
            Some(("job", id)) => {
                scope.jobs.insert(id.into());
                scope.job_tasks(&mut tx).await?;
                scope.tasks.extend(ids(&mut tx,"SELECT dependency_id FROM task_dependencies WHERE task_id IN (SELECT value FROM json_each(?))",&json!(scope.tasks).to_string()).await?);
            }
            Some(("board" | "inbox", id)) => {
                scope.projects.insert(id.into());
            }
            _ => (),
        }
        scope.parents(&mut tx).await?;
        let state = scope.load(&mut tx).await?;
        tx.commit().await?;
        Ok(state)
    }

    pub(crate) async fn project_receipt(&self, project: &crate::Project) -> Result<(i64, i64)> {
        let sequence: i64=sqlx::query_scalar("SELECT COALESCE(MAX(sequence),0) FROM task_events WHERE json_extract(data,'$.project_id')=?")
            .bind(&project.id).fetch_one(&self.pool).await?;
        let jobs: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(json_extract(data,'$.updated_at')) FROM jobs WHERE project_id=?",
        )
        .bind(&project.id)
        .fetch_one(&self.pool)
        .await?;
        let tasks: Option<i64>=sqlx::query_scalar("SELECT MAX(json_extract(data,'$.updated_at')) FROM tasks WHERE json_extract(data,'$.project_id')=?")
            .bind(&project.id).fetch_one(&self.pool).await?;
        Ok((
            sequence,
            jobs.into_iter()
                .chain(tasks)
                .chain(project.archived_at)
                .chain([project.created_at])
                .max()
                .unwrap(),
        ))
    }

    pub(crate) async fn project_activity(
        &self,
        projects: &BTreeSet<&str>,
    ) -> Result<BTreeMap<String, i64>> {
        let rows = sqlx::query(PROJECT_ACTIVITY_QUERY)
            .bind(serde_json::to_string(projects)?)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                row.get::<Option<i64>, _>("updated_at")
                    .map(|updated| (row.get("project_id"), updated))
            })
            .collect())
    }
}
