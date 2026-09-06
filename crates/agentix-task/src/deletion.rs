use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::SqliteConnection;

use crate::{
    Snapshot, TaskEvent, WriteOptions,
    mutations::{check_revision, required},
    new_id,
    store::{append_event, event},
};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Cleanup {
    pub id: String,
    pub files: BTreeSet<String>,
    #[serde(default)]
    pub candidates: BTreeMap<String, BTreeSet<String>>,
    pub directories: BTreeSet<String>,
}

pub(crate) fn sequence_key(project: &str, kind: &str, created: i64) -> String {
    format!("{project}:{kind}:{}", created.div_euclid(86_400))
}

pub(crate) fn apply(
    state: &mut Snapshot,
    request: &Value,
    options: &WriteOptions,
) -> Result<Value> {
    let project_delete = required(request, "command")? == "project.delete";
    let (id, revision, jobs) = if project_delete {
        let project = &state.projects[state.project_index(required(request, "project")?)?];
        (
            project.id.clone(),
            project.revision,
            state
                .jobs
                .iter()
                .filter(|j| j.project_id == project.id)
                .map(|j| j.id.clone())
                .collect::<BTreeSet<_>>(),
        )
    } else {
        let job = &state.jobs[state.job_index(required(request, "job")?)?];
        (
            job.id.clone(),
            job.revision,
            BTreeSet::from([job.id.clone()]),
        )
    };
    check_revision(revision, options)?;
    let tasks: BTreeSet<_> = state
        .tasks
        .iter()
        .filter(|t| jobs.contains(&t.job_id))
        .map(|t| t.id.clone())
        .collect();
    ensure!(
        !state.leases.iter().any(|l| tasks.contains(&l.task_id)),
        "conflict: release active Task leases before deleting"
    );
    ensure!(
        !state
            .tasks
            .iter()
            .any(|t| !tasks.contains(&t.id) && t.dependencies.iter().any(|d| tasks.contains(d))),
        "conflict: other Jobs depend on these Tasks; remove dependencies before deleting"
    );
    // Retain the high-water marks even when the highest-numbered work is deleted.
    for job in state.jobs.iter().filter(|j| jobs.contains(&j.id)) {
        let key = sequence_key(&job.project_id, "job", job.created_at);
        let number = state.document_sequences.entry(key).or_default();
        *number = (*number).max(job.sequence);
    }
    for task in state.tasks.iter().filter(|t| tasks.contains(&t.id)) {
        let key = sequence_key(&task.project_id, "task", task.created_at);
        let number = state.document_sequences.entry(key).or_default();
        *number = (*number).max(task.sequence);
    }
    let plans = state
        .plans
        .iter()
        .filter(|p| tasks.contains(&p.task_id))
        .count();
    state.plans.retain(|p| !tasks.contains(&p.task_id));
    state.tasks.retain(|t| !tasks.contains(&t.id));
    state.jobs.retain(|j| !jobs.contains(&j.id));
    if project_delete {
        state.inboxes.retain(|entry| entry.project_id != id);
        state.projects.retain(|p| p.id != id);
        state
            .document_sequences
            .retain(|key, _| !key.starts_with(&format!("{id}:")));
    }
    Ok(json!({"id":id,"deleted":true,"jobs":jobs.len(),"tasks":tasks.len(),"plans":plans}))
}

pub(crate) async fn check_pending_paths(
    conn: &mut SqliteConnection,
    before: &Snapshot,
    after: &Snapshot,
) -> Result<()> {
    if after.projects.len() <= before.projects.len() {
        return Ok(());
    }
    let rows: Vec<String> = sqlx::query_scalar("SELECT data FROM document_deletions")
        .fetch_all(conn)
        .await?;
    for row in rows {
        let pending: Cleanup = serde_json::from_str(&row)?;
        for project in after
            .projects
            .iter()
            .filter(|p| !before.projects.iter().any(|old| old.id == p.id))
        {
            ensure!(
                !pending
                    .directories
                    .contains(&format!("Projects/{}", project.key)),
                "conflict: project directory cleanup is pending; run sync before registering this name"
            );
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // Delete dependent rows and record recoverable cleanup atomically.
pub(crate) async fn persist(
    conn: &mut SqliteConnection,
    before: &Snapshot,
    after: &Snapshot,
    command: &str,
    options: &WriteOptions,
    now: i64,
) -> Result<()> {
    let jobs: Vec<_> = before
        .jobs
        .iter()
        .filter(|j| !after.jobs.iter().any(|a| a.id == j.id))
        .collect();
    let projects: Vec<_> = before
        .projects
        .iter()
        .filter(|p| !after.projects.iter().any(|a| a.id == p.id))
        .collect();
    if jobs.is_empty() && projects.is_empty() {
        return Ok(());
    }
    let old: Option<String> =
        sqlx::query_scalar("SELECT value FROM projection_state WHERE key = 'documents'")
            .fetch_optional(&mut *conn)
            .await?;
    let previous: BTreeMap<String, String> = old
        .map(|v| serde_json::from_str(&v))
        .transpose()?
        .unwrap_or_default();
    let mut cleanup = Cleanup {
        id: new_id("cleanup"),
        files: BTreeSet::new(),
        candidates: BTreeMap::new(),
        directories: BTreeSet::new(),
    };
    for plan in before
        .plans
        .iter()
        .filter(|p| !after.plans.iter().any(|a| a.id == p.id))
    {
        cleanup
            .candidates
            .entry(plan.path.clone())
            .or_default()
            .extend([plan.id.clone(), plan.task_id.clone()]);
        if let Some(path) = previous.get(&format!("plan:{}", plan.id)) {
            cleanup.files.insert(path.clone());
        }
        sqlx::query("DELETE FROM plans WHERE id = ?")
            .bind(&plan.id)
            .execute(&mut *conn)
            .await?;
    }
    for task in before
        .tasks
        .iter()
        .filter(|t| !after.tasks.iter().any(|a| a.id == t.id))
    {
        cleanup
            .candidates
            .entry(crate::naming::task_path(before, task)?)
            .or_default()
            .insert(task.id.clone());
        if let Some(path) = previous.get(&format!("task:{}", task.id)) {
            cleanup.files.insert(path.clone());
        }
        sqlx::query("DELETE FROM task_dependencies WHERE task_id = ? OR dependency_id = ?")
            .bind(&task.id)
            .bind(&task.id)
            .execute(&mut *conn)
            .await?;
    }
    for task in before
        .tasks
        .iter()
        .filter(|t| !after.tasks.iter().any(|a| a.id == t.id))
    {
        sqlx::query("DELETE FROM tasks WHERE id = ?")
            .bind(&task.id)
            .execute(&mut *conn)
            .await?;
        append_event(
            conn,
            TaskEvent {
                project_id: Some(task.project_id.clone()),
                job_id: Some(task.job_id.clone()),
                task_id: Some(task.id.clone()),
                revision: task.revision + 1,
                payload: json!({"id":task.id,"deleted":true}),
                ..event("task.deleted", options, now)
            },
        )
        .await?;
    }
    for job in jobs {
        cleanup
            .candidates
            .entry(job.document_path.clone())
            .or_default()
            .insert(job.id.clone());
        if let Some(path) = previous.get(&format!("job:{}", job.id)) {
            cleanup.files.insert(path.clone());
        }
        sqlx::query("DELETE FROM jobs WHERE id = ?")
            .bind(&job.id)
            .execute(&mut *conn)
            .await?;
        sqlx::query("DELETE FROM projection_state WHERE key = ?")
            .bind(format!("goal:{}", job.id))
            .execute(&mut *conn)
            .await?;
        append_event(
            conn,
            TaskEvent {
                project_id: Some(job.project_id.clone()),
                job_id: Some(job.id.clone()),
                revision: job.revision + 1,
                payload: json!({"id":job.id,"deleted":true}),
                ..event("job.deleted", options, now)
            },
        )
        .await?;
    }
    for project in projects {
        cleanup
            .directories
            .insert(format!("Projects/{}", project.key));
        for kind in ["meta", "board", "tasks", "sync"] {
            if let Some(path) = previous.get(&format!("{kind}:{}", project.id))
                && let Some((parent, _)) = path.rsplit_once('/')
            {
                cleanup.directories.insert(parent.into());
            }
        }
        sqlx::query("DELETE FROM projects WHERE id = ?")
            .bind(&project.id)
            .execute(&mut *conn)
            .await?;
        append_event(
            conn,
            TaskEvent {
                project_id: Some(project.id.clone()),
                revision: project.revision + 1,
                payload: json!({"id":project.id,"deleted":true}),
                ..event(command, options, now)
            },
        )
        .await?;
    }
    sqlx::query("INSERT INTO document_deletions(id,data) VALUES (?,?)")
        .bind(&cleanup.id)
        .bind(serde_json::to_string(&cleanup)?)
        .execute(&mut *conn)
        .await?;
    Ok(())
}
