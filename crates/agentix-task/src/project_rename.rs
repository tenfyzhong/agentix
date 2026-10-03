//! Import moves of published Project folders without changing workspace identity.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sqlx::SqliteConnection;

use crate::{Service, Snapshot, WriteOptions, mutations::required};

impl Service {
    /// Called with the output lock, before any authored content is read or rendered.
    pub(crate) async fn reconcile_project_folders_locked(&self) -> Result<()> {
        let projects = sqlx::query_scalar::<_, String>(
            "SELECT p.data FROM projects p JOIN document_registry d ON d.key='board:'||p.id",
        )
        .fetch_all(&self.store().pool)
        .await?
        .iter()
        .map(|data| crate::stored_paths::from_str::<crate::Project>(data))
        .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut listings = BTreeMap::new();
        let mut roots = BTreeSet::new();
        let mut missing = BTreeMap::new();
        for project in projects {
            let directory = project.document_directory();
            let path = self.safe_path(&directory)?;
            let parent = path.parent().context("missing project parent")?;
            if !listings.contains_key(parent) {
                let names = folder_names(parent)?;
                listings.insert(parent.to_owned(), names);
            }
            let present =
                listings[parent].contains(path.file_name().context("missing project name")?);
            if !present {
                roots.insert(
                    std::path::Path::new(&directory)
                        .parent()
                        .context("missing project parent")?
                        .to_string_lossy()
                        .into_owned(),
                );
                missing.insert(project.id.clone(), project);
            }
        }
        let mut moves = BTreeMap::new();
        for root in roots {
            let parent = self.safe_path(&root)?;
            if !parent.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(parent)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                let directory = format!("{root}/{name}");
                let path = self.safe_path(&format!("{directory}/Board.md"))?;
                if !path.is_file() {
                    continue;
                }
                let source = std::fs::read_to_string(path)?;
                let Ok((properties, _)) = crate::projection::split_properties(&source) else {
                    continue;
                };
                let Some(project) = properties["id"].as_str().and_then(|id| missing.get(id)) else {
                    continue;
                };
                ensure!(
                    properties["taskix-generated"] == true,
                    "conflict: moved Board is not managed by taskix"
                );
                ensure!(
                    crate::stored_paths::matches_root(&properties["root"], &project.root)?,
                    "conflict: moved Board workspace root does not match Project {}",
                    project.id
                );
                ensure!(
                    std::path::Path::new(&project.document_directory()).parent()
                        == Some(std::path::Path::new(&root)),
                    "conflict: Project folder moved outside its parent"
                );
                ensure!(
                    moves
                        .insert(project.id.clone(), (name, directory, project.revision))
                        .is_none(),
                    "conflict: ambiguous project folders for {}",
                    project.id
                );
            }
        }
        // Validate every candidate before importing any move.
        let mut requests = Vec::new();
        for (id, (name, directory, revision)) in moves {
            let request =
                json!({"command":"project.rename","project":id,"name":name,"directory":directory});
            let state = self.store().request_snapshot(&request).await?;
            let options = WriteOptions {
                expected_revision: Some(revision),
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
    let old = format!("{}/", project.document_directory());
    let directory = request["directory"].as_str().map_or_else(
        || {
            std::path::Path::new(&project.document_directory())
                .with_file_name(name)
                .to_string_lossy()
                .replace('\\', "/")
        },
        str::to_owned,
    );
    ensure!(
        std::path::Path::new(&directory)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
            && directory.split('/').count() >= 2,
        "invalid: project document directory"
    );
    let new = format!("{directory}/");
    project.document_directory = (directory != format!("Projects/{name}")).then_some(directory);
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
        let Some(old) = before.projects.iter().find(|old| {
            old.id == project.id && old.document_directory() != project.document_directory()
        }) else {
            continue;
        };
        let prefix = format!("{}/", old.document_directory());
        sqlx::query("UPDATE document_registry SET path=?||substr(path,length(?)+1) WHERE substr(path,1,length(?))=?")
            .bind(format!("{}/", project.document_directory())).bind(&prefix).bind(&prefix).bind(&prefix).execute(&mut *conn).await?;
    }
    Ok(())
}

fn folder_names(path: &std::path::Path) -> Result<BTreeSet<std::ffi::OsString>> {
    if !path.is_dir() {
        return Ok(BTreeSet::new());
    }
    Ok(std::fs::read_dir(path)?
        .map(|entry| {
            let entry = entry?;
            Ok(entry.file_type()?.is_dir().then(|| entry.file_name()))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect())
}
