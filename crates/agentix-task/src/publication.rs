//! Durable, deduplicated document work committed with domain changes.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use serde_json::json;
use sqlx::{Row, SqliteConnection};

use crate::{
    Snapshot, Store, new_id,
    scoped::{Scope, ids},
};

#[derive(Default)]
pub(crate) struct PublicationMetadata {
    pub plans: Vec<PublishedPlan>,
    pub goals: BTreeMap<String, String>,
    pub deliveries: BTreeMap<String, String>,
}

#[derive(serde::Serialize)]
pub(crate) struct PublishedPlan {
    pub id: String,
    pub version: i64,
    pub hash: String,
}

#[allow(clippy::too_many_lines)] // Publication keys describe one atomic state difference.
pub(crate) async fn enqueue_changes(
    conn: &mut SqliteConnection,
    before: &Snapshot,
    after: &Snapshot,
    _command: &str,
) -> Result<()> {
    let mut keys = BTreeSet::new();
    let old_projects: BTreeMap<_, _> = before.projects.iter().map(|v| (&v.id, v)).collect();
    let old_jobs: BTreeMap<_, _> = before.jobs.iter().map(|v| (&v.id, v)).collect();
    let old_tasks: BTreeMap<_, _> = before.tasks.iter().map(|v| (&v.id, v)).collect();
    let old_issues: BTreeMap<_, _> = before.issues.iter().map(|v| (&v.id, v)).collect();
    let old_milestones: BTreeMap<_, _> = before.milestones.iter().map(|v| (&v.id, v)).collect();
    let mut dependents = BTreeSet::new();
    for project in &after.projects {
        if old_projects.get(&project.id).copied() != Some(project) {
            keys.insert(format!("board:{}", project.id));
            keys.insert(format!("issue-base:{}", project.id));
            keys.insert(format!("milestone-base:{}", project.id));
            keys.extend(
                after
                    .issues
                    .iter()
                    .filter(|issue| issue.project_id == project.id)
                    .map(|issue| format!("issue:{}", issue.id)),
            );
            keys.extend(
                after
                    .milestones
                    .iter()
                    .filter(|milestone| milestone.project_id == project.id)
                    .map(|milestone| format!("milestone:{}", milestone.id)),
            );
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
    for issue in &after.issues {
        let old = old_issues.get(&issue.id).copied();
        if old == Some(issue) {
            continue;
        }
        keys.insert(format!("issue:{}", issue.id));
        if old.is_some_and(|old| old.name != issue.name || old.document_path != issue.document_path)
        {
            keys.extend(
                ids(
                    conn,
                    "SELECT id FROM jobs WHERE json_extract(data,'$.issue_id')=?",
                    &issue.id,
                )
                .await?
                .into_iter()
                .map(|id| format!("job:{id}")),
            );
        }
        keys.insert(format!("board:{}", issue.project_id));
        keys.insert(format!("issue-base:{}", issue.project_id));
        for milestone in issue
            .milestone_id
            .iter()
            .chain(old.and_then(|old| old.milestone_id.as_ref()))
        {
            keys.insert(format!("milestone:{milestone}"));
        }
    }
    for milestone in &after.milestones {
        if old_milestones.get(&milestone.id).copied() == Some(milestone) {
            continue;
        }
        keys.insert(format!("milestone:{}", milestone.id));
        if old_milestones.get(&milestone.id).is_some_and(|old| {
            old.name != milestone.name || old.document_path != milestone.document_path
        }) {
            keys.extend(
                after
                    .issues
                    .iter()
                    .filter(|issue| issue.milestone_id.as_ref() == Some(&milestone.id))
                    .map(|issue| format!("issue:{}", issue.id)),
            );
        }
        keys.insert(format!("milestone-base:{}", milestone.project_id));
        keys.insert(format!("board:{}", milestone.project_id));
    }
    let milestones: BTreeSet<_> = after
        .milestones
        .iter()
        .map(|milestone| &milestone.id)
        .collect();
    for milestone in before
        .milestones
        .iter()
        .filter(|milestone| !milestones.contains(&milestone.id))
    {
        keys.insert(format!("milestone:{}", milestone.id));
    }
    let jobs: BTreeSet<_> = after.jobs.iter().map(|j| &j.id).collect();
    let tasks: BTreeSet<_> = after.tasks.iter().map(|t| &t.id).collect();
    let projects: BTreeSet<_> = after.projects.iter().map(|p| &p.id).collect();
    for job in before.jobs.iter().filter(|j| !jobs.contains(&j.id)) {
        keys.insert(format!("job:{}", job.id));
        keys.insert(format!("board:{}", job.project_id));
    }
    for task in before.tasks.iter().filter(|t| !tasks.contains(&t.id)) {
        keys.insert(format!("task:{}", task.id));
        if let Some(plan) = &task.current_plan {
            keys.insert(format!("plan:{plan}"));
        }
    }
    for project in before.projects.iter().filter(|p| !projects.contains(&p.id)) {
        keys.insert(format!("board:{}", project.id));
        keys.insert(format!("issue-base:{}", project.id));
        keys.insert(format!("milestone-base:{}", project.id));
        keys.extend(
            before
                .issues
                .iter()
                .filter(|issue| issue.project_id == project.id)
                .map(|issue| format!("issue:{}", issue.id)),
        );
        keys.extend(
            before
                .milestones
                .iter()
                .filter(|milestone| milestone.project_id == project.id)
                .map(|milestone| format!("milestone:{}", milestone.id)),
        );
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
        metadata: &PublicationMetadata,
    ) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if !metadata.plans.is_empty() {
            sqlx::query("UPDATE plans SET data=json_remove(json_set(plans.data,'$.hash',receipt.hash),'$.pending_body')
                FROM (SELECT json_extract(value,'$.id') AS id,json_extract(value,'$.version') AS version,json_extract(value,'$.hash') AS hash FROM json_each(?)) receipt
                WHERE plans.id=receipt.id AND plans.version=receipt.version")
                .bind(serde_json::to_string(&metadata.plans)?).execute(&mut *tx).await?;
        }
        if !metadata.goals.is_empty() {
            sqlx::query("INSERT INTO projection_state(key,value) SELECT 'goal:'||key,json_quote(value) FROM json_each(?) WHERE true ON CONFLICT(key) DO UPDATE SET value=excluded.value")
                .bind(serde_json::to_string(&metadata.goals)?).execute(&mut *tx).await?;
        }
        if !metadata.deliveries.is_empty() {
            sqlx::query("INSERT INTO projection_state(key,value) SELECT 'delivery:'||key,json_quote(value) FROM json_each(?) WHERE true ON CONFLICT(key) DO UPDATE SET value=excluded.value")
                .bind(serde_json::to_string(&metadata.deliveries)?).execute(&mut *tx).await?;
        }
        if !remove.is_empty() {
            sqlx::query(
                "DELETE FROM document_registry WHERE key IN (SELECT value FROM json_each(?))",
            )
            .bind(serde_json::to_string(remove)?)
            .execute(&mut *tx)
            .await?;
        }
        if !paths.is_empty() {
            sqlx::query("INSERT INTO document_registry(key,path) SELECT key,value FROM json_each(?) WHERE true ON CONFLICT(key) DO UPDATE SET path=excluded.path")
                .bind(serde_json::to_string(paths)?).execute(&mut *tx).await?;
        }
        if !pending.is_empty() {
            sqlx::query("DELETE FROM pending_documents WHERE (key,generation) IN (SELECT key,value FROM json_each(?))")
                .bind(serde_json::to_string(pending)?).execute(&mut *tx).await?;
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
            Some(("issue", id)) => {
                scope.issues.insert(id.into());
            }
            Some(("milestone", id)) => {
                scope.milestones.insert(id.into());
            }
            Some(("job", id)) => {
                scope.jobs.insert(id.into());
                scope.job_tasks(&mut tx).await?;
                scope.tasks.extend(ids(&mut tx,"SELECT dependency_id FROM task_dependencies WHERE task_id IN (SELECT value FROM json_each(?))",&json!(scope.tasks).to_string()).await?);
            }
            Some(("board" | "issue-base" | "milestone-base", id)) => {
                scope.projects.insert(id.into());
            }
            _ => (),
        }
        scope.parents(&mut tx).await?;
        let mut state = scope.load(&mut tx).await?;
        if let Some(("milestone", id)) = key.split_once(':') {
            state.query_context.milestone_progress =
                crate::issue_read::milestone_progress(&mut tx, &[id.to_owned()]).await?;
        }
        tx.commit().await?;
        Ok(state)
    }

    pub(crate) async fn project_receipt(&self, project: &crate::Project) -> Result<(i64, i64)> {
        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE((SELECT sequence FROM event_watermarks WHERE scope=?),0)",
        )
        .bind(&project.id)
        .fetch_one(&self.pool)
        .await?;
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
}
