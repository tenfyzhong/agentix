//! Import moves of published Project folders without changing workspace identity.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sqlx::{Row, SqliteConnection};

use crate::{Service, Snapshot, WriteOptions, mutations::required};

impl Service {
    /// Called with the output lock, before any authored content is read or rendered.
    pub(crate) async fn reconcile_project_folders_locked(&self) -> Result<()> {
        let projects_dir = self.safe_path("Projects")?;
        if !projects_dir.is_dir() {
            return Ok(());
        }
        let directories = std::fs::read_dir(&projects_dir)?
            .map(|entry| {
                let entry = entry?;
                Ok(entry.file_type()?.is_dir().then(|| entry.file_name()))
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .filter_map(|name| name.into_string().ok())
            .collect::<BTreeSet<_>>();
        // Only identities of published Boards are needed on the unchanged path.
        let rows = sqlx::query("SELECT p.id,json_extract(p.data,'$.key') AS key FROM projects p JOIN document_registry d ON d.key='board:'||p.id")
            .fetch_all(&self.store().pool).await?;
        let missing: BTreeSet<String> = rows
            .iter()
            .filter(|row| !directories.contains(&row.get::<String, _>("key")))
            .map(|row| row.get("id"))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        let mut moves = BTreeMap::new();
        for name in directories {
            let path = self.safe_path(&format!("Projects/{name}/Board.md"))?;
            if !path.is_file() {
                continue;
            }
            let source = std::fs::read_to_string(path)?;
            let Ok((properties, _)) = crate::projection::split_properties(&source) else {
                continue;
            };
            let Some(id) = properties["id"]
                .as_str()
                .filter(|id| missing.contains(*id))
                .map(str::to_owned)
            else {
                continue;
            };
            ensure!(
                properties["taskix-generated"] == true,
                "conflict: moved Board is not managed by taskix"
            );
            ensure!(
                moves.insert(id.clone(), (name, properties)).is_none(),
                "conflict: ambiguous project folders for {id}"
            );
        }
        // Validate every candidate before importing any move.
        let mut requests = Vec::new();
        for (id, (name, properties)) in moves {
            let state = self
                .store()
                .request_snapshot(&json!({"command":"project.rename","project":id,"name":name}))
                .await?;
            let project = &state.projects[state.project_index(&id)?];
            ensure!(
                properties["root"] == project.root,
                "conflict: moved Board workspace root does not match Project {id}"
            );
            let request = json!({"command":"project.rename","project":id,"name":name});
            let options = WriteOptions {
                expected_revision: Some(project.revision),
                actor_ref: "user:obsidian".into(),
                ..WriteOptions::default()
            };
            rename(&mut state.clone(), &request, &options)?;
            requests.push((request, options));
        }
        for (request, options) in requests {
            self.store().execute(request, options).await?;
        }
        Ok(())
    }
}

pub(crate) fn rename(
    state: &mut Snapshot,
    request: &Value,
    options: &WriteOptions,
) -> Result<Value> {
    let index = state.project_index(required(request, "project")?)?;
    let project = &mut state.projects[index];
    crate::mutations::check_revision(project.revision, options)?;
    let name = required(request, "name")?;
    ensure!(
        crate::naming::short_name(name) == name,
        "invalid: Project folder name must be a portable name of at most 48 characters"
    );
    let old = format!("Projects/{}/", project.key);
    let new = format!("Projects/{name}/");
    project.name = name.into();
    project.key = name.into();
    project.revision += 1;
    let result = serde_json::to_value(&*project)?;
    for job in state
        .jobs
        .iter_mut()
        .filter(|job| job.project_id == project.id)
    {
        job.document_path = format!(
            "{new}{}",
            job.document_path
                .strip_prefix(&old)
                .context("invalid Job document path")?
        );
        job.revision += 1;
    }
    let tasks: BTreeSet<_> = state
        .tasks
        .iter()
        .filter(|task| task.project_id == project.id)
        .map(|task| &task.id)
        .collect();
    for plan in state
        .plans
        .iter_mut()
        .filter(|plan| tasks.contains(&plan.task_id))
    {
        plan.path = format!(
            "{new}{}",
            plan.path
                .strip_prefix(&old)
                .context("invalid Plan document path")?
        );
    }
    Ok(result)
}

/// The filesystem move already happened. Rebase recovery locations in the same
/// transaction as the identities, so publication reads the moved authored text.
pub(crate) async fn rebase_registry(
    conn: &mut SqliteConnection,
    before: &Snapshot,
    after: &Snapshot,
) -> Result<()> {
    for project in &after.projects {
        let Some(old) = before
            .projects
            .iter()
            .find(|old| old.id == project.id && old.key != project.key)
        else {
            continue;
        };
        let prefix = format!("Projects/{}/", old.key);
        sqlx::query("UPDATE document_registry SET path=?||substr(path,length(?)+1) WHERE substr(path,1,length(?))=?")
            .bind(format!("Projects/{}/", project.key)).bind(&prefix).bind(&prefix).bind(&prefix).execute(&mut *conn).await?;
    }
    Ok(())
}
