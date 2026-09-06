use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::{
    Config, DocumentFormat, Outcome, Plan, Snapshot, Store, Task, TaskStatus, WriteOptions,
    config::resolved_path, mutations::required, new_id, store::hash_bytes,
};

#[derive(Clone)]
pub struct Service {
    config: Config,
    store: Store,
}

impl Service {
    pub fn new(config: Config, store: Store) -> Result<Self> {
        config.validate()?;
        Ok(Self { config, store })
    }
    pub async fn open(config: Config) -> Result<Self> {
        config.validate()?;
        let store = Store::open(&config.storage.path).await?;
        Self::new(config, store)
    }
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    pub async fn execute(&self, request: Value, options: WriteOptions) -> Result<Outcome> {
        self.store.reap_expired().await?;
        let command = required(&request, "command")?;
        if matches!(command, "plan.create" | "plan.revise") {
            return self.write_plan(request, options).await;
        }
        let state = self.store.snapshot().await?;
        let projects = crate::inbox_document::request_projects(&state, &request);
        let needs_inbox = command.starts_with("inbox.")
            || state
                .inboxes
                .iter()
                .any(|e| projects.contains(&e.project_id));
        // Inbox work must observe human withdrawals before committing. Unrelated
        // metadata retains its existing commit-before-projection recovery path.
        let lock = if command == "task.start" || needs_inbox {
            Some(self.lock_output().await?)
        } else {
            None
        };
        if needs_inbox
            && !matches!(command, "project.delete" | "job.delete")
            && self.store.replay(&request, &options).await?.is_none()
        {
            self.reconcile_inboxes_locked(Some(&projects)).await?;
        }
        // Replays must remain valid even if the Plan file subsequently disappears.
        if command == "task.start" && self.store.replay(&request, &options).await?.is_none() {
            let state = self.store.snapshot().await?;
            let mut preview = state.clone();
            crate::mutations::apply(&mut preview, &request, &options, self.store.now())?;
            let task = &state.tasks[state.task_index(required(&request, "task")?)?];
            let plan = state
                .plans
                .iter()
                .find(|p| Some(&p.id) == task.current_plan.as_ref())
                .context("invalid: current Plan is required before start")?;
            let bytes = std::fs::read(self.safe_path(&plan.path)?)
                .context("current Plan file is missing")?;
            ensure!(
                !split_properties(std::str::from_utf8(&bytes)?)?
                    .1
                    .trim()
                    .is_empty(),
                "invalid: current Plan is empty"
            );
        }
        let mut outcome = self.store.execute(request, options).await?;
        drop(lock);
        if let Err(error) = self.sync().await {
            outcome.projection_pending = Some(error.to_string());
        }
        Ok(outcome)
    }

    async fn lock_output(&self) -> Result<File> {
        self.config.validate()?;
        std::fs::create_dir_all(self.config.output_dir())?;
        let path = self.safe_path(".taskcli.lock")?;
        tokio::task::spawn_blocking(move || {
            let lock = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path)?;
            lock.lock()?;
            Ok(lock)
        })
        .await?
    }

    pub async fn sync(&self) -> Result<()> {
        let _lock = self.lock_output().await?;
        self.reconcile_inboxes_locked(None).await?;
        self.store.reap_expired().await?;
        self.render_locked().await
    }

    async fn write_plan(&self, request: Value, options: WriteOptions) -> Result<Outcome> {
        let lock = self.lock_output().await?;
        if let Some(mut outcome) = self.store.replay(&request, &options).await? {
            // An old successful request may predate the readable-path migration.
            if let Some(plan) = self
                .store
                .snapshot()
                .await?
                .plans
                .iter()
                .find(|p| Some(p.task_id.as_str()) == outcome.result["task_id"].as_str())
            {
                outcome.result["path"] = json!(plan.path);
            }
            outcome.result["absolute_path"] =
                json!(self.safe_path(required(&outcome.result, "path")?)?);
            drop(lock);
            if let Err(error) = self.sync().await {
                outcome.projection_pending = Some(error.to_string());
            }
            return Ok(outcome);
        }
        let state = self.store.snapshot().await?;
        let projects = crate::inbox_document::request_projects(&state, &request);
        self.reconcile_inboxes_locked(Some(&projects)).await?;
        let state = self.store.snapshot().await?;
        let task = &state.tasks[state.task_index(required(&request, "task")?)?];
        let command = required(&request, "command")?;
        ensure!(
            if command == "plan.create" {
                task.current_plan.is_none()
            } else {
                task.current_plan.is_some()
            },
            "conflict: use plan create for first Plan, revise for later versions"
        );
        let body = required(&request, "body")?;
        ensure!(
            !split_properties(body)?.1.trim().is_empty(),
            "invalid: Plan body is empty"
        );
        let version = state
            .plans
            .iter()
            .filter(|p| p.task_id == task.id)
            .map(|p| p.version)
            .max()
            .unwrap_or(0)
            + 1;
        let current = state.plans.iter().find(|p| p.task_id == task.id);
        let plan = Plan {
            id: current.map_or_else(|| new_id("plan"), |p| p.id.clone()),
            task_id: task.id.clone(),
            version,
            path: crate::naming::task_path(&state, task)?,
            hash: hash_bytes(body.as_bytes()),
            created_at: current.map_or_else(|| self.store.now(), |p| p.created_at),
            updated_at: self.store.now(),
            pending_body: Some(body.into()),
        };
        let path = self.safe_path(&plan.path)?;
        let registration = json!({"command":"plan.register","task":task.id,"plan":plan});
        // Validate ownership and state before touching the filesystem.
        let mut preview = state.clone();
        crate::mutations::apply(&mut preview, &registration, &options, self.store.now())?;
        let managed_task = if current.is_none() && path.is_file() {
            let (properties, _) = split_properties(&std::fs::read_to_string(&path)?)?;
            properties["taskcli-generated"] == true && properties["id"] == task.id
        } else {
            false
        };
        ensure!(
            current.is_some() || !path.exists() || managed_task,
            "conflict: unregistered Plan file exists; preserve or move it before retrying"
        );
        // Commit authorization and the replacement body together. Sync publishes it
        // after commit, so a rejected write cannot overwrite the current Plan.
        let mut outcome = self
            .store
            .execute_as(registration, options, request)
            .await?;
        outcome.result["absolute_path"] = json!(path);
        drop(lock);
        if let Err(error) = self.sync().await {
            outcome.projection_pending = Some(error.to_string());
        }
        Ok(outcome)
    }

    pub async fn plan(&self, task: &str) -> Result<Value> {
        let state = self.store.snapshot().await?;
        let task = &state.tasks[state.task_index(task)?];
        let plan = state
            .plans
            .iter()
            .find(|p| Some(&p.id) == task.current_plan.as_ref())
            .context("not_found: current Plan")?;
        let path = self.safe_path(&plan.path)?;
        let body = std::fs::read_to_string(&path)?;
        self.store
            .update_plan_hash(&plan.id, &hash_bytes(body.as_bytes()))
            .await?;
        let mut result = serde_json::to_value(plan)?;
        result["hash"] = json!(hash_bytes(body.as_bytes()));
        result["absolute_path"] = json!(path);
        let (properties, content) = split_properties(&body)?;
        result["body"] = json!(content);
        result["properties"] = properties;
        Ok(result)
    }

    /// Read the authored Task body without changing its Plan hash or task state.
    pub async fn task_markdown(&self, id: &str) -> Result<String> {
        let state = self.store.snapshot().await?;
        let task = &state.tasks[state.task_index(id)?];
        let path = self.safe_path(&crate::naming::task_path(&state, task)?)?;
        let document = std::fs::read_to_string(path)?;
        Ok(split_properties(&document)?.1.to_owned())
    }

    /// Read authored Job sections, excluding generated local navigation and graphs.
    pub async fn job_markdown(&self, id: &str) -> Result<String> {
        let state = self.store.snapshot().await?;
        let job = &state.jobs[state.job_index(id)?];
        let document = std::fs::read_to_string(self.safe_path(&job.document_path)?)?;
        let goal = section(&document, "goal")?.unwrap_or_else(|| job.goal.clone());
        let notes = section(&document, "notes")?.unwrap_or_default();
        Ok(format!(
            "{}## Goal\n\n{goal}\n\n## Notes\n\n{notes}",
            prompt_markdown(&job.prompt)
        ))
    }

    pub(crate) fn safe_path(&self, relative: &str) -> Result<PathBuf> {
        let relative = Path::new(relative);
        ensure!(
            relative
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
            "invalid document path"
        );
        let path = self.config.output_dir().join(relative);
        ensure!(
            resolved_path(&path)?.starts_with(resolved_path(&self.config.output_dir())?),
            "document path escapes output directory"
        );
        Ok(path)
    }

    #[allow(clippy::too_many_lines)]
    async fn render_locked(&self) -> Result<()> {
        self.cleanup_deleted_documents().await?;
        // Mark only the snapshot's sequence as rendered, even if writers commit during IO.
        let sequence = self.store.latest_sequence().await?;
        let state = self.store.snapshot().await?;
        let previous = self
            .store
            .metadata("documents")
            .await?
            .unwrap_or_else(|| json!({}));
        let previous: BTreeMap<String, String> = serde_json::from_value(previous)?;
        let mut paths = BTreeMap::new();
        let mut files = BTreeMap::new();
        let created = state
            .projects
            .iter()
            .map(|p| p.created_at)
            .min()
            .unwrap_or(self.store.now());
        let (dashboard_path, dashboard) = self.dashboard(&state, created)?;
        for project in &state.projects {
            let board_path = format!("Projects/{}/Board.md", project.key);
            files.insert(
                board_path.clone(),
                self.tasknotes_board(project, sequence, project_activity(&state, project))?,
            );
            paths.insert(format!("board:{}", project.id), board_path.clone());
            for job in state.jobs.iter().filter(|j| j.project_id == project.id) {
                let key = format!("job:{}", job.id);
                let previous_path = previous.get(&key).unwrap_or(&job.document_path);
                let source = self.safe_path(previous_path)?;
                let source = if source.exists() {
                    source
                } else {
                    self.safe_path(&job.document_path)?
                };
                let existing = if source.exists() {
                    std::fs::read_to_string(&source)?
                } else {
                    String::new()
                };
                let notes = section(&existing, "notes")?.unwrap_or_default();
                let goal = section(&existing, "goal")?.unwrap_or_else(|| job.goal.clone());
                let goal = if self
                    .store
                    .metadata(&format!("goal:{}", job.id))
                    .await?
                    .is_some_and(|v| v.as_str() != Some(&job.goal))
                {
                    job.goal.clone()
                } else {
                    goal
                };
                let mut properties = serde_json::to_value(job)?;
                for field in [
                    "created_at",
                    "updated_at",
                    "started_at",
                    "completed_at",
                    "cancelled_at",
                    "archived_at",
                ] {
                    properties[field] = properties[field].as_i64().map_or(Value::Null, timestamp);
                }
                properties["tags"] = if job.archived_at.is_some() {
                    json!(["agent/archived/job"])
                } else {
                    json!(["agent/job"])
                };
                for field in ["goal", "prompt", "document_path", "title", "name"] {
                    properties.as_object_mut().unwrap().remove(field);
                }
                let mut doc = frontmatter(properties);
                doc.push_str(&Self::header(&job.name));
                doc.push_str(&prompt_markdown(&job.prompt));
                doc.push_str(&format!("\n## {}\n\n<!-- taskcli:goal:start -->\n{}\n<!-- taskcli:goal:end -->\n\n## {}\n", "Goal", goal, "Tasks"));
                doc.push_str(&job_dependency_graph(self, &state, &job.id)?);
                for task in state.tasks.iter().filter(|t| t.job_id == job.id) {
                    if self.config.documents.format == DocumentFormat::Markdown {
                        doc.push_str(&format!("\n<a id=\"{}\"></a>\n", task.id.replace('_', "-")));
                    }
                    doc.push_str(&format!(
                        "\n{}\n",
                        self.task_line(&state, task, &job.document_path)?
                    ));
                    if self.config.documents.format == DocumentFormat::Obsidian {
                        doc.push_str(&format!("\n^{}\n", task.id.replace('_', "-")));
                    }
                    if let Some(reason) = &task.reason {
                        doc.push_str(&format!("\n  {}: {}\n", "Reason", escape(reason)));
                    }
                }
                doc.push_str(&format!("\n## {}\n\n<!-- taskcli:notes:start -->\n{notes}\n<!-- taskcli:notes:end -->\n", "Notes"));
                files.insert(job.document_path.clone(), doc);
                paths.insert(key, job.document_path.clone());
            }
        }
        for task in &state.tasks {
            let plan = state
                .plans
                .iter()
                .find(|p| Some(&p.id) == task.current_plan.as_ref());
            let path = crate::naming::task_path(&state, task)?;
            let key = format!("task:{}", task.id);
            let candidates = [
                previous.get(&key),
                plan.and_then(|p| previous.get(&format!("plan:{}", p.id))),
                Some(&path),
            ];
            let mut source = self.safe_path(&path)?;
            for candidate in candidates.into_iter().flatten() {
                let candidate = self.safe_path(candidate)?;
                if candidate.exists() {
                    source = candidate;
                    break;
                }
            }
            let existing = if source.exists() {
                std::fs::read_to_string(&source)?
            } else {
                ensure!(
                    plan.is_none_or(|p| p.pending_body.is_some()),
                    "missing Plan {path}"
                );
                String::new()
            };
            let doc = self.task_document(&state, task, plan, &existing)?;
            files.insert(path.clone(), doc);
            paths.insert(key, path.clone());
            if let Some(plan) = plan {
                paths.insert(format!("plan:{}", plan.id), path);
            }
        }
        files.insert(dashboard_path.clone(), dashboard);
        paths.insert("dashboard".into(), dashboard_path);
        // Check all new destinations before publishing any file. Existing managed
        // paths can be regenerated; new paths must not clobber unrelated notes.
        for (relative, contents) in &files {
            let path = self.safe_path(relative)?;
            if path.exists() && !previous.values().any(|old| old == relative) {
                let existing = std::fs::read_to_string(&path)?;
                let owned = if relative == "Dashboard.base" {
                    existing.starts_with("# taskcli-generated: dashboard\n")
                } else {
                    let (old, _) = split_properties(&existing)?;
                    let (new, _) = split_properties(contents)?;
                    old["taskcli-generated"] == true && old["id"] == new["id"]
                };
                ensure!(
                    owned,
                    "conflict: unmanaged document exists at {}",
                    path.display()
                );
            }
        }
        // Read editable content before replacing any managed document.
        for (path, contents) in files {
            atomic_write(&self.safe_path(&path)?, &contents)?;
        }
        let current: BTreeSet<_> = paths.values().collect();
        for old in previous.values().filter(|old| !current.contains(old)) {
            let path = self.safe_path(old)?;
            if path.exists() {
                std::fs::remove_file(&path)?;
                // Remove only empty generated ancestors; leave user files intact.
                let mut parent = path.parent();
                while let Some(dir) = parent {
                    if dir == self.config.output_dir() || std::fs::remove_dir(dir).is_err() {
                        break;
                    }
                    parent = dir.parent();
                }
            }
        }
        for plan in &state.plans {
            let bytes = std::fs::read(self.safe_path(&plan.path)?)
                .with_context(|| format!("missing Plan {}", plan.path))?;
            self.store
                .publish_plan(&plan.id, plan.version, &hash_bytes(&bytes))
                .await?;
        }
        for job in &state.jobs {
            self.store
                .set_metadata(&format!("goal:{}", job.id), &json!(job.goal))
                .await?;
        }
        self.store
            .set_metadata("documents", &serde_json::to_value(paths)?)
            .await?;
        self.store
            .set_metadata("sequence", &json!(sequence))
            .await?;
        Ok(())
    }

    async fn cleanup_deleted_documents(&self) -> Result<()> {
        for deletion in self.store.pending_deletions().await? {
            for relative in deletion
                .files
                .iter()
                .chain(deletion.candidates.keys())
                .chain(&deletion.directories)
            {
                self.deletion_path(relative)?;
            }
            let mut files = deletion.files.clone();
            // A database destination may never have been published. Only reclaim
            // unregistered files when their generated identity proves ownership.
            for (relative, identities) in &deletion.candidates {
                if files.contains(relative) {
                    continue;
                }
                let path = self.deletion_path(relative)?;
                let body = match std::fs::read_to_string(&path) {
                    Ok(body) => body,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("inspect document {}", path.display()));
                    }
                };
                if let Ok((properties, _)) = split_properties(&body)
                    && properties["taskcli-generated"] == true
                    && properties["id"]
                        .as_str()
                        .is_some_and(|id| identities.contains(id))
                {
                    files.insert(relative.clone());
                }
            }
            for relative in &files {
                let path = self.deletion_path(relative)?;
                match std::fs::remove_file(&path) {
                    Ok(()) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("delete document {}", path.display()));
                    }
                }
            }
            for relative in &deletion.directories {
                ensure!(
                    relative.starts_with("Projects/") && relative.split('/').count() == 2,
                    "invalid project cleanup directory"
                );
                let path = self.deletion_path(relative)?;
                match std::fs::remove_dir_all(&path) {
                    Ok(()) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("delete project documents {}", path.display())
                        });
                    }
                }
            }
            self.store.finish_deletion(&deletion.id).await?;
        }
        Ok(())
    }

    fn deletion_path(&self, relative: &str) -> Result<PathBuf> {
        let path = self.safe_path(relative)?;
        let output = self.config.output_dir();
        for component in path.ancestors().take_while(|p| *p != output) {
            match std::fs::symlink_metadata(component) {
                Ok(metadata) => ensure!(
                    !metadata.file_type().is_symlink(),
                    "conflict: cleanup path contains a symlink: {}",
                    component.display()
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(path)
    }

    fn notice() -> &'static str {
        "> GENERATED — DO NOT EDIT task fields. Use taskcli or an Agent."
    }

    fn dashboard(&self, state: &Snapshot, created: i64) -> Result<(String, String)> {
        if self.config.documents.format == DocumentFormat::Obsidian {
            let folder = self.config.documents.directory.join("Projects");
            let folder = folder.to_string_lossy().replace('\\', "/");
            let folder = folder.trim_start_matches("./");
            let base = json!({
                "filters": {"and": [
                    format!("file.inFolder({})", json!(folder)),
                    "file.name == \"Board\"", "file.ext == \"md\"",
                    "file.hasTag(\"agent/project\")",
                    "note[\"taskcli-generated\"] == true", "note.status == \"ACTIVE\""
                ]},
                "formulas": {
                    "name": "link(file.path, note.name)",
                    "status": "note.status", "updated": "date(note.updated_at)"
                },
                "properties": {
                    "formula.name": {"displayName":"Name"},
                    "formula.status": {"displayName":"Status"},
                    "formula.updated": {"displayName":"Updated"}
                },
                "views": [{
                    "type":"table", "name":"Projects",
                    "order":["formula.name", "formula.status", "formula.updated"],
                    "sort":[{"column":"formula.updated","direction":"DESC"}, {"column":"formula.name","direction":"ASC"}]
                }]
            });
            return Ok((
                "Dashboard.base".into(),
                format!(
                    "# taskcli-generated: dashboard\n{}",
                    serde_yaml::to_string(&base)?
                ),
            ));
        }
        let mut doc = frontmatter(
            json!({"id":"dashboard", "created_at":timestamp(created), "tags":["agent/dashboard"]}),
        );
        doc.push_str(&Self::header("Task dashboard"));
        doc.push_str("\n| Name | Status | Updated |\n| --- | --- | --- |\n");
        let mut projects: Vec<_> = state
            .projects
            .iter()
            .filter(|p| p.archived_at.is_none())
            .collect();
        projects.sort_by_key(|p| (std::cmp::Reverse(project_activity(state, p)), &p.name));
        for project in projects {
            let board = format!("Projects/{}/Board.md", project.key);
            doc.push_str(&format!(
                "| {} | ACTIVE | {} |\n",
                self.link("Dashboard.md", &board, None, &project.name),
                timestamp(project_activity(state, project))
                    .as_str()
                    .unwrap_or_default()
            ));
        }
        Ok(("Dashboard.md".into(), doc))
    }

    fn tasknotes_board(
        &self,
        project: &crate::Project,
        sequence: i64,
        updated_at: i64,
    ) -> Result<String> {
        let title = format!("{} — Task board", project.name);
        let folder = self
            .config
            .documents
            .directory
            .join(format!("Projects/{}/Tasks", project.key));
        let folder = folder.to_string_lossy().replace('\\', "/");
        let folder = folder.trim_start_matches("./");
        let mut view = json!({
            "type": "tasknotesKanban",
            "name": "Task board",
            "groupBy": {"property":"status", "direction":"ASC"},
            "order": ["status"],
            "sort": [{"column":"file.name", "direction":"ASC"}]
        });
        view["columnOrder"] = json!({"status":TaskStatus::ALL});
        view["hideEmptyColumns"] = json!(false);
        view["columnWidth"] = json!(300);
        let base = json!({
            "filters": {"and": [format!("file.folder == {}", json!(folder)), "file.hasTag(\"agent/task\")", format!("project_id == {}", json!(project.id)), "archived != true"]},
            "views": [view]
        });
        let mut doc = frontmatter(
            json!({"id":project.id,"name":project.name,"created_at":timestamp(project.created_at),"updated_at":timestamp(updated_at),"title":title,"revision":project.revision,"root":project.root,"remote":project.remote,"archived_at":optional_timestamp(project.archived_at),"status":if project.archived_at.is_some() {"ARCHIVED"} else {"ACTIVE"},"sync_status":"synced","sync_sequence":sequence,"tags":["agent/project","agent/board"]}),
        );
        doc.push_str(&Self::header(&title));
        doc.push_str(&format!(
            "\n{}\n",
            self.link(
                &format!("Projects/{}/Board.md", project.key),
                &format!("Projects/{}/Inbox.md", project.key),
                None,
                "Inbox"
            )
        ));
        doc.push_str(&format!(
            "\n```base\n{}\n```\n",
            serde_yaml::to_string(&base)?.trim_end()
        ));
        Ok(doc)
    }

    fn task_document(
        &self,
        state: &Snapshot,
        task: &Task,
        plan: Option<&Plan>,
        existing: &str,
    ) -> Result<String> {
        let (mut authored, existing_body) = split_properties(existing)?;
        let default_body = format!("# {}\n\n{}\n", escape(&task.name), escape(&task.title));
        let body = if let Some(pending) = plan.and_then(|p| p.pending_body.as_ref()) {
            let (properties, body) = split_properties(pending)?;
            authored
                .as_object_mut()
                .unwrap()
                .extend(properties.as_object().unwrap().clone());
            body
        } else if existing.is_empty() {
            &default_body
        } else {
            existing_body
        };
        let project = &state.projects[state.project_index(&task.project_id)?];
        let job = &state.jobs[state.job_index(&task.job_id)?];
        let wiki = |path: &str| {
            format!(
                "[[{}]]",
                self.config
                    .documents
                    .directory
                    .join(path)
                    .to_string_lossy()
                    .replace('\\', "/")
                    .trim_start_matches("./")
                    .trim_end_matches(".md")
            )
        };
        let generated = json!({
            "id":task.id,"task_id":task.id,"plan_id":task.current_plan,"job_id":task.job_id,"project_id":task.project_id,
            "sequence":task.sequence,"revision":task.revision,"dependencies":task.dependencies,
            "agent":task.last_executor.as_deref().and_then(crate::model::agent_name),"session_id":task.last_session,
            "status":task.status,"phase":task.phase,"archived":job.archived_at.is_some() || project.archived_at.is_some(),
            "created_at":local_timestamp(task.created_at)?,"updated_at":local_timestamp(task.updated_at)?,
            "started_at":optional_local_timestamp(task.started_at)?,"completed_at":optional_local_timestamp(task.completed_at)?,
            "dateCreated":local_timestamp(task.created_at)?,"dateModified":local_timestamp(task.updated_at)?,"completedDate":optional_local_timestamp(task.completed_at)?,
            "projects":[wiki(&format!("Projects/{}/Board.md",project.key))],"job":wiki(&job.document_path)
        });
        authored.as_object_mut().unwrap().remove("version");
        if authored["title"].is_null() || authored["title"] == authored["name"] {
            authored["title"] = json!(task.name);
        }
        authored["name"] = json!(task.name);
        for (key, value) in generated.as_object().unwrap() {
            authored[key] = value.clone();
        }
        let mut tags = match authored["tags"].clone() {
            Value::Array(tags) => tags,
            Value::String(tag) => vec![json!(tag)],
            _ => Vec::new(),
        };
        tags.retain(|tag| tag != "agent/plan" && tag != "archived");
        if authored["archived"] == true {
            tags.push(json!("archived"));
        }
        for tag in ["agent/task", "task"] {
            if !tags.contains(&json!(tag)) {
                tags.push(json!(tag));
            }
        }
        authored["tags"] = json!(tags);
        let mut doc = frontmatter(authored);
        doc.push_str(body);
        Ok(doc)
    }

    fn header(title: &str) -> String {
        format!("# {}\n\n{}\n", escape(title), Self::notice())
    }

    fn task_line(&self, state: &Snapshot, task: &Task, from: &str) -> Result<String> {
        let path = crate::naming::task_path(state, task)?;
        let label = Path::new(&path)
            .file_stem()
            .and_then(|name| name.to_str())
            .context("Task path must have a UTF-8 filename")?;
        Ok(format!("- {}", self.link(from, &path, None, label)))
    }

    pub(crate) fn link(&self, from: &str, to: &str, anchor: Option<&str>, label: &str) -> String {
        let escaped_label = escape(label);
        let needs_plain_label = escaped_label != label;
        let label = escaped_label;
        match self.config.documents.format {
            DocumentFormat::Obsidian => {
                let to = self
                    .config
                    .documents
                    .directory
                    .join(to)
                    .to_string_lossy()
                    .replace('\\', "/");
                let to = to.trim_start_matches("./").trim_end_matches(".md");
                let anchor = anchor.map_or_else(String::new, |s| format!("#^{s}"));
                if needs_plain_label {
                    // Obsidian aliases do not decode HTML entities. Keep labels
                    // with reserved characters outside a stable wiki link.
                    let label = label
                        .replace('\\', "&#92;")
                        .replace('*', "&#42;")
                        .replace('_', "&#95;")
                        .replace('`', "&#96;")
                        .replace('~', "&#126;");
                    format!("[[{to}{anchor}|Open]] {label}")
                } else {
                    format!("[[{to}{anchor}|{label}]]")
                }
            }
            DocumentFormat::Markdown => {
                let mut path = relative_url(from, to);
                if let Some(anchor) = anchor {
                    path.push('#');
                    path.push_str(&encode(anchor));
                }
                format!("[{label}]({path})")
            }
        }
    }
}

fn local_timestamp(value: i64) -> Result<Value> {
    let instant = time::OffsetDateTime::from_unix_timestamp(value)?;
    let offset = time::UtcOffset::local_offset_at(instant)
        .context("cannot resolve the computer's local time zone")?;
    Ok(json!(
        instant
            .to_offset(offset)
            .format(&time::format_description::well_known::Rfc3339)?
    ))
}

fn optional_local_timestamp(value: Option<i64>) -> Result<Value> {
    Ok(value
        .map(local_timestamp)
        .transpose()?
        .unwrap_or(Value::Null))
}

fn timestamp(value: i64) -> Value {
    time::OffsetDateTime::from_unix_timestamp(value)
        .ok()
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .map_or(Value::Null, Value::String)
}

// Project activity must not change merely because projections were synchronized.
fn project_activity(state: &Snapshot, project: &crate::Project) -> i64 {
    state
        .jobs
        .iter()
        .filter(|job| job.project_id == project.id)
        .map(|job| job.updated_at)
        .chain(
            state
                .tasks
                .iter()
                .filter(|task| task.project_id == project.id)
                .map(|task| task.updated_at),
        )
        .chain(project.archived_at)
        .chain(std::iter::once(project.created_at))
        .max()
        .unwrap_or(project.created_at)
}

fn optional_timestamp(value: Option<i64>) -> Value {
    value.map_or(Value::Null, timestamp)
}

fn frontmatter(mut properties: Value) -> String {
    properties["taskcli-generated"] = json!(true);
    let mut result = String::from("---\n");
    for (key, value) in properties.as_object().unwrap() {
        let key = if key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            && matches!(serde_yaml::from_str::<Value>(key), Ok(Value::String(_)))
        {
            key.clone()
        } else {
            serde_json::to_string(key).expect("property keys are serializable strings")
        };
        result.push_str(&format!("{key}: {value}\n"));
    }
    result.push_str("---\n\n");
    result
}

fn split_properties(body: &str) -> Result<(Value, &str)> {
    let remainder = body
        .strip_prefix("---\r\n")
        .or_else(|| body.strip_prefix("---\n"));
    let frontmatter = remainder.and_then(|remainder| {
        let mut offset = 0;
        for line in remainder.split_inclusive('\n') {
            if line.trim_end_matches(['\r', '\n']) == "---" {
                return Some((&remainder[..offset], &remainder[offset + line.len()..]));
            }
            offset += line.len();
        }
        None
    });
    if let Some((properties, content)) = frontmatter {
        let properties: Value =
            serde_yaml::from_str(properties).context("invalid Plan frontmatter")?;
        ensure!(
            properties.is_object() || properties.is_null(),
            "Plan frontmatter must be a mapping"
        );
        Ok((
            if properties.is_null() {
                json!({})
            } else {
                properties
            },
            content
                .strip_prefix("\r\n")
                .or_else(|| content.strip_prefix('\n'))
                .unwrap_or(content),
        ))
    } else {
        Ok((json!({}), body))
    }
}

fn job_dependency_graph(service: &Service, state: &Snapshot, job_id: &str) -> Result<String> {
    let tasks = state
        .tasks
        .iter()
        .filter(|task| task.job_id == job_id)
        .collect::<Vec<_>>();
    if tasks.is_empty() {
        return Ok(String::new());
    }
    let mut nodes = BTreeSet::new();
    for task in &tasks {
        nodes.insert(task.id.as_str());
        nodes.extend(task.dependencies.iter().map(String::as_str));
    }
    let mut diagram = String::from(
        "\nArrows point from prerequisites to dependent tasks.\n\n```mermaid\nflowchart TD\n",
    );
    for id in nodes {
        let task = &state.tasks[state.task_index(id)?];
        let label = if task.job_id == job_id {
            task.name.clone()
        } else {
            let job = &state.jobs[state.job_index(&task.job_id)?];
            format!("{} (Job: {})", task.name, job.name)
        };
        let label = format!("{} · {}", mermaid_label(&label), task.status);
        let path = crate::naming::task_path(state, task)?;
        match service.config.documents.format {
            DocumentFormat::Obsidian => {
                // Obsidian strips custom URI schemes from Mermaid SVG links.
                // HTML internal links keep the file target separate from the status label.
                let file = service
                    .config
                    .documents
                    .directory
                    .join(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let target = escape(file.trim_start_matches("./"))
                    .replace('\'', "&#39;")
                    .replace('"', "&quot;");
                diagram.push_str(&format!(
                    "    {id}[\"<a class='internal-link' data-href='{target}' href='{target}' style='color:#1f2937'>{label}</a>\"]:::status_{}\n",
                    task.status,
                ));
            }
            DocumentFormat::Markdown => {
                let url = relative_url(&state.jobs[state.job_index(job_id)?].document_path, &path);
                diagram.push_str(&format!("    {id}[\"{label}\"]:::status_{}\n", task.status));
                diagram.push_str(&format!("    click {id} href \"{url}\" \"Open task\"\n"));
            }
        }
    }
    for task in tasks {
        for prerequisite in &task.dependencies {
            diagram.push_str(&format!("    {prerequisite} --> {}\n", task.id));
        }
    }
    for status in TaskStatus::ALL {
        let color = task_status_color(status);
        diagram.push_str(&format!(
            "    classDef status_{status} fill:{color},stroke:{color},color:#1f2937\n"
        ));
    }
    diagram.push_str("```\n");
    Ok(diagram)
}

fn task_status_color(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Todo => "#cbd5e1",
        TaskStatus::InProgress => "#bfdbfe",
        TaskStatus::Blocked => "#fed7aa",
        TaskStatus::WaitingUser => "#ddd6fe",
        TaskStatus::Done => "#bbf7d0",
        TaskStatus::Failed => "#fecaca",
        TaskStatus::Cancelled => "#e2d7e7",
    }
}

fn relative_url(from: &str, to: &str) -> String {
    let from: Vec<_> = from.split('/').collect();
    let to: Vec<_> = to.split('/').collect();
    let from = &from[..from.len() - 1];
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut path = "../".repeat(from.len() - common);
    path.push_str(
        &to[common..]
            .iter()
            .map(|s| encode(s))
            .collect::<Vec<_>>()
            .join("/"),
    );
    path
}

fn mermaid_label(text: &str) -> String {
    let mut label = String::new();
    for character in text.chars() {
        match character {
            '"' | '&' | '<' | '>' | '#' | '`' | '[' | ']' | '\\' => {
                label.push_str(&format!("#{};", u32::from(character)));
            }
            c if c.is_control() => label.push(' '),
            c => label.push(c),
        }
    }
    label
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "&#124;")
        .replace('[', "&#91;")
        .replace(']', "&#93;")
        .replace(['\r', '\n'], " ")
}
fn encode(text: &str) -> String {
    let mut result = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            result.push(char::from(byte));
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}
fn prompt_markdown(prompt: &str) -> String {
    if prompt.is_empty() {
        return String::new();
    }
    let mut body = String::from("\n## Prompt\n\n");
    // An indented text block preserves Markdown source and cannot close a fence
    // or introduce editable section markers from the user's original request.
    for line in prompt.split('\n') {
        if !line.is_empty() {
            body.push_str("    ");
            body.push_str(line);
        }
        body.push('\n');
    }
    body.push('\n');
    body
}

fn section(body: &str, name: &str) -> Result<Option<String>> {
    if body.is_empty() {
        return Ok(None);
    }
    let start = format!("<!-- taskcli:{name}:start -->");
    let end = format!("<!-- taskcli:{name}:end -->");
    // Only standalone markers delimit editable sections. Quoted or indented
    // user text (including the original prompt) must not alter the document.
    let positions = |marker: &str| {
        let mut offset = 0;
        body.split_inclusive('\n')
            .filter_map(|line| {
                let position = offset;
                offset += line.len();
                (line.trim_end_matches(['\r', '\n']) == marker).then_some(position)
            })
            .collect::<Vec<_>>()
    };
    let starts = positions(&start);
    let ends = positions(&end);
    ensure!(
        starts.len() == 1 && ends.len() == 1,
        "editable {name} markers missing or duplicated; restore markers before sync"
    );
    let content_start = starts[0] + start.len();
    ensure!(content_start <= ends[0], "reversed section markers");
    Ok(Some(
        body[content_start..ends[0]]
            .trim_matches(['\r', '\n'])
            .to_owned(),
    ))
}

pub(crate) fn atomic_write(path: &Path, body: &str) -> Result<()> {
    // Unchanged Base documents should keep their live Obsidian views mounted.
    if std::fs::read(path).is_ok_and(|existing| existing == body.as_bytes()) {
        return Ok(());
    }
    let parent = path.parent().context("document has no parent")?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(body.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
