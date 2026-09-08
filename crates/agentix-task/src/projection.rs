use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::{
    Config, Outcome, Plan, Snapshot, Store, Task, TaskStatus, WriteOptions, config::resolved_path,
    mutations::required, new_id, store::hash_bytes,
};

#[cfg(test)]
#[path = "projection_tests.rs"]
mod tests;

#[path = "projection_index.rs"]
mod index;
use index::ProjectionIndex;

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

    /// One registered note, using indexed entity reads instead of a snapshot.
    pub async fn obsidian_note(&self, id: &str) -> Result<Value> {
        let Some((entity, project_key)) = self.store.obsidian_record(id).await? else {
            return Ok(Value::Null);
        };
        self.obsidian_record_note(&entity, &project_key)
    }

    fn obsidian_record_note(&self, entity: &Value, project_key: &str) -> Result<Value> {
        let id = entity["id"].as_str().context("missing note ID")?;
        let kind = id.split_once('_').context("invalid ID")?.0;
        let relative = match kind {
            "task" => {
                let filename = crate::naming::numbered_name(
                    entity["name"].as_str().context("missing task name")?,
                    entity["created_at"]
                        .as_i64()
                        .context("missing task creation date")?,
                    entity["sequence"]
                        .as_u64()
                        .context("missing task sequence")?,
                )?;
                format!("Projects/{project_key}/Tasks/{filename}.md")
            }
            "job" => entity["document_path"]
                .as_str()
                .context("missing job path")?
                .to_owned(),
            _ => format!("Projects/{project_key}/Inbox.md"),
        };
        self.safe_path(&relative)?;
        let path = self
            .config
            .documents
            .directory
            .join(relative)
            .to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches("./")
            .to_owned();
        let status = if kind == "inbox" {
            serde_json::to_value(serde_json::from_value::<crate::InboxStatus>(
                entity["status"].clone(),
            )?)?
        } else {
            entity["status"].clone()
        };
        let mut properties = json!({"status":status, "revision":entity["revision"]});
        if kind == "task" {
            for key in ["phase", "dependencies"] {
                properties[key] = entity[key].clone();
            }
        } else if kind == "job" {
            properties["review_reason"] = entity["review_reason"].clone();
        }
        if kind != "inbox" {
            for key in ["created_at", "updated_at", "completed_at"] {
                properties[key] = optional_local_timestamp(entity[key].as_i64())?;
            }
            for key in if kind == "task" {
                &["started_at"][..]
            } else {
                &["cancelled_at", "pending_review_at"][..]
            } {
                properties[*key] = optional_local_timestamp(entity[*key].as_i64())?;
            }
        }
        Ok(
            json!({"kind":kind,"id":entity["id"],"project_id":entity["project_id"],
            "path":path,"status":status,"revision":entity["revision"],"properties":properties}),
        )
    }

    /// Authoritative identities and status properties for the Obsidian bridge.
    /// Paths are relative to the vault; no ownership credentials are exported.
    pub async fn obsidian_snapshot(&self) -> Result<Value> {
        let notes = self
            .store
            .obsidian_records()
            .await?
            .iter()
            .map(|(entity, project_key)| self.obsidian_record_note(entity, project_key))
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({"documents":self.config.documents,"notes":notes}))
    }

    pub async fn execute(&self, request: Value, options: WriteOptions) -> Result<Outcome> {
        self.store.reap_expired().await?;
        let command = required(&request, "command")?;
        if matches!(command, "plan.create" | "plan.revise") {
            return self.write_plan(request, options).await;
        }
        if let Some(mut outcome) = self.store.replay(&request, &options).await? {
            let deferred = (command == "inbox.set-status")
                .then(|| outcome.result["id"].as_str())
                .flatten();
            if let Err(error) = self.sync_pending(deferred).await {
                outcome.projection_pending = Some(error.to_string());
            }
            return Ok(outcome);
        }
        let state = self.store.request_snapshot(&request).await?;
        let projects = crate::inbox_document::request_projects(&state, &request);
        let needs_inbox = crate::inbox::has_job_prompt(&request)
            || command.starts_with("inbox.")
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
            // The explicit status command owns this entry's checkbox intent.
            // Still import content, order, withdrawals and other cancellations.
            let deferred = if command == "inbox.set-status" {
                Some(
                    state.inboxes[crate::inbox::index(&state, required(&request, "inbox")?)?]
                        .id
                        .as_str(),
                )
            } else {
                None
            };
            // Publishing the old checkbox here would look like a second edit
            // to the Obsidian listener while its status command is in flight.
            self.reconcile_inboxes_locked(Some(&projects), deferred, deferred.is_none())
                .await?;
        }
        // Replays must remain valid even if the Plan file subsequently disappears.
        if command == "task.start" && self.store.replay(&request, &options).await?.is_none() {
            let state = self.store.request_snapshot(&request).await?;
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
        let inbox_status = command == "inbox.set-status";
        let mut outcome = self.store.execute(request, options).await?;
        drop(lock);
        // A reopened entry still has its old [-] on disk until projection.
        // Do not import that stale mark as a new cancellation of this write.
        let deferred = inbox_status
            .then(|| outcome.result["id"].as_str())
            .flatten();
        if let Err(error) = self.sync_pending(deferred).await {
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
        self.sync_with_deferred_status(None).await
    }

    pub async fn sync_pending_documents(&self) -> Result<()> {
        self.sync_pending(None).await
    }

    async fn sync_with_deferred_status(&self, deferred: Option<&str>) -> Result<()> {
        let _lock = self.lock_output().await?;
        self.reconcile_inboxes_locked(None, deferred, true).await?;
        self.store.reap_expired().await?;
        self.render_locked().await
    }

    async fn sync_pending(&self, deferred: Option<&str>) -> Result<()> {
        let _lock = self.lock_output().await?;
        self.cleanup_deleted_documents().await?;
        if self.store.rebuild_pending().await? {
            self.reconcile_inboxes_locked(None, deferred, true).await?;
            return self.render_locked().await;
        }
        let mut error = None;
        let mut failed_inboxes = BTreeSet::new();
        for project in self.store.pending_inboxes().await? {
            if let Err(failure) = self
                .reconcile_inboxes_locked(Some(&BTreeSet::from([project.clone()])), deferred, true)
                .await
            {
                failed_inboxes.insert(format!("inbox:{project}"));
                error.get_or_insert(failure);
            }
        }
        // Capture generations after import. Bound queue memory and try each
        // document once; concurrent later generations remain durable for retry.
        let mut cursor = String::new();
        loop {
            let pending = self.store.pending_batch(&cursor).await?;
            if pending.is_empty() {
                break;
            }
            for (key, generation) in pending {
                cursor.clone_from(&key);
                if failed_inboxes.contains(&key) {
                    continue;
                }
                if let Err(failure) = self.render_pending_document(&key, generation).await {
                    error.get_or_insert(failure);
                }
            }
        }
        error.map_or(Ok(()), Err)
    }

    async fn render_pending_document(&self, key: &str, generation: String) -> Result<()> {
        let state = self.store.projection_snapshot(key).await?;
        // Inbox publication acknowledges its own pre-render generation. A new
        // generation arriving after that pass must wait for its next import.
        if key.starts_with("inbox:") && !state.projects.is_empty() {
            return Ok(());
        }
        self.render_state_locked(
            &state,
            Some(key),
            &BTreeMap::from([(key.to_owned(), generation)]),
        )
        .await
    }

    async fn write_plan(&self, request: Value, options: WriteOptions) -> Result<Outcome> {
        let lock = self.lock_output().await?;
        if let Some(mut outcome) = self.store.replay(&request, &options).await? {
            // An old successful request may predate the readable-path migration.
            if let Some(plan) = self
                .store
                .request_snapshot(
                    &json!({"command":"plan.revise","task":outcome.result["task_id"]}),
                )
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
            if let Err(error) = self.sync_pending(None).await {
                outcome.projection_pending = Some(error.to_string());
            }
            return Ok(outcome);
        }
        let state = self.store.request_snapshot(&request).await?;
        let projects = crate::inbox_document::request_projects(&state, &request);
        self.reconcile_inboxes_locked(Some(&projects), None, true)
            .await?;
        let state = self.store.request_snapshot(&request).await?;
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
        if let Err(error) = self.sync_pending(None).await {
            outcome.projection_pending = Some(error.to_string());
        }
        Ok(outcome)
    }

    pub async fn plan(&self, task: &str) -> Result<Value> {
        let plan = self.store.current_plan(task).await?;
        let path = self.safe_path(&plan.path)?;
        let body = std::fs::read_to_string(&path)?;
        self.store
            .update_plan_hash(&plan.id, &hash_bytes(body.as_bytes()))
            .await?;
        let mut result = serde_json::to_value(&plan)?;
        result["hash"] = json!(hash_bytes(body.as_bytes()));
        result["absolute_path"] = json!(path);
        let (properties, content) = split_properties(&body)?;
        result["body"] = json!(content);
        result["properties"] = properties;
        Ok(result)
    }

    /// Read the authored Task body without changing its Plan hash or task state.
    pub async fn task_markdown(&self, id: &str) -> Result<String> {
        let state = self.store.task_document_snapshot(id).await?;
        let task = &state.tasks[state.task_index(id)?];
        let path = self.safe_path(&crate::naming::task_path(&state, task)?)?;
        let document = std::fs::read_to_string(path)?;
        Ok(split_properties(&document)?.1.to_owned())
    }

    /// Read authored Job sections, excluding generated local navigation and graphs.
    pub async fn job_markdown(&self, id: &str) -> Result<String> {
        let job = self.store.job_record(id).await?;
        let document = std::fs::read_to_string(self.safe_path(&job.document_path)?)?;
        let goal = section(&document, "goal")?.unwrap_or_else(|| job.goal.clone());
        let notes = section(&document, "notes")?.unwrap_or_default();
        Ok(format!(
            "## Goal\n\n{goal}\n\n## Notes\n\n{notes}\n{}{}",
            prompt_markdown(&job.prompt),
            conversation_markdown(&job)
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
        let mut pending = self.store.pending_documents().await?;
        let state = self.store.snapshot().await?;
        pending.retain(|key, _| {
            key.strip_prefix("inbox:")
                .is_none_or(|id| !state.projects.iter().any(|p| p.id == id))
        });
        self.render_state_locked(&state, None, &pending).await
    }

    #[allow(clippy::too_many_lines)]
    async fn render_state_locked(
        &self,
        state: &Snapshot,
        selected: Option<&str>,
        pending: &BTreeMap<String, String>,
    ) -> Result<()> {
        // Mark only the snapshot's sequence as rendered, even if writers commit during IO.
        let sequence = self.store.latest_sequence().await?;
        let selected_keys = selected.map(|key| {
            let mut keys = BTreeSet::from([key.to_owned()]);
            if key == "dashboard" {
                keys.insert("pending-review".into());
            }
            if key.starts_with("task:") {
                keys.extend(state.plans.iter().map(|p| format!("plan:{}", p.id)));
            }
            keys
        });
        let previous = self.store.document_paths(selected_keys.as_ref()).await?;
        let previous_paths: BTreeSet<_> = previous.values().map(String::as_str).collect();
        let index = ProjectionIndex::new(state);
        let includes = |key: &str| selected.is_none_or(|selected| selected == key);
        let goals = self
            .store
            .metadata_batch(
                &state
                    .jobs
                    .iter()
                    .filter(|job| includes(&format!("job:{}", job.id)))
                    .map(|job| format!("goal:{}", job.id))
                    .collect(),
            )
            .await?;
        let mut paths = BTreeMap::new();
        let mut files = BTreeMap::new();
        for project in &state.projects {
            let board_path = format!("Projects/{}/Board.md", project.key);
            if includes(&format!("board:{}", project.id)) {
                let (project_sequence, activity) = self.store.project_receipt(project).await?;
                files.insert(
                    board_path.clone(),
                    self.tasknotes_board(project, project_sequence, activity)?,
                );
                paths.insert(format!("board:{}", project.id), board_path.clone());
            }
            for job in index
                .jobs_by_project
                .get(project.id.as_str())
                .into_iter()
                .flatten()
            {
                let key = format!("job:{}", job.id);
                if !includes(&key) {
                    continue;
                }
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
                let goal = if goals
                    .get(&format!("goal:{}", job.id))
                    .is_some_and(|v| v.as_str() != Some(&job.goal))
                {
                    job.goal.clone()
                } else {
                    goal
                };
                let mut properties = split_properties(&existing)?.0;
                properties.as_object_mut().unwrap().extend(
                    self.job_properties(job, project)?
                        .as_object()
                        .unwrap()
                        .clone(),
                );
                for field in ["goal", "prompt", "conversation", "document_path", "name"] {
                    properties.as_object_mut().unwrap().remove(field);
                }
                let mut doc = frontmatter(properties);
                doc.push_str(&Self::header(&job.name));
                doc.push_str(&format!("\n## {}\n\n<!-- taskcli:goal:start -->\n{}\n<!-- taskcli:goal:end -->\n\n## {}\n", "Goal", goal, "Tasks"));
                doc.push_str(&job_dependency_graph(self, &index, &job.id)?);
                for task in index
                    .tasks_by_job
                    .get(job.id.as_str())
                    .into_iter()
                    .flatten()
                {
                    doc.push_str(&format!("\n{}\n", self.task_line(&index, task)?));
                    doc.push_str(&format!("\n^{}\n", task.id.replace('_', "-")));
                    if let Some(reason) = &task.reason {
                        doc.push_str(&format!("\n  {}: {}\n", "Reason", escape(reason)));
                    }
                }
                doc.push_str(&format!("\n## {}\n\n<!-- taskcli:notes:start -->\n{notes}\n<!-- taskcli:notes:end -->\n", "Notes"));
                doc.push_str(&prompt_markdown(&job.prompt));
                doc.push_str(&conversation_markdown(job));
                files.insert(job.document_path.clone(), doc);
                paths.insert(key, job.document_path.clone());
            }
        }
        for task in &state.tasks {
            if !includes(&format!("task:{}", task.id)) {
                continue;
            }
            let plan = task
                .current_plan
                .as_deref()
                .and_then(|id| index.plans.get(id).copied());
            let path = index.task_path(task)?;
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
            let doc = self.task_document(&index, task, plan, &existing)?;
            files.insert(path.clone(), doc);
            paths.insert(key, path.clone());
            if let Some(plan) = plan {
                paths.insert(format!("plan:{}", plan.id), path);
            }
        }
        // Bases query vault notes dynamically. Preserve registered view settings
        // during incremental publication, even when Obsidian removed comments.
        let preserve_base = |key: &str, path: &str| -> Result<bool> {
            Ok(selected.is_some()
                && previous.get(key).is_some_and(|old| old == path)
                && self.safe_path(path)?.is_file())
        };
        if includes("dashboard") && preserve_base("dashboard", "Dashboard.base")? {
            paths.insert("dashboard".into(), "Dashboard.base".into());
        } else if includes("dashboard") {
            let (dashboard_path, dashboard) = self.dashboard()?;
            files.insert(dashboard_path.clone(), dashboard);
            paths.insert("dashboard".into(), dashboard_path);
        }
        if includes("pending-review") || includes("dashboard") {
            if !preserve_base("pending-review", "Recent Jobs.base")? {
                files.insert("Recent Jobs.base".into(), self.recent_jobs_base()?);
            }
            paths.insert("pending-review".into(), "Recent Jobs.base".into());
        }
        // Check all new destinations before publishing any file. Existing managed
        // paths can be regenerated; new paths must not clobber unrelated notes.
        for (relative, contents) in &files {
            let path = self.safe_path(relative)?;
            if path.exists() && !previous_paths.contains(relative.as_str()) {
                let existing = std::fs::read_to_string(&path)?;
                let owned = if relative == "Dashboard.base" {
                    existing.starts_with("# taskcli-generated: dashboard\n")
                } else if relative == "Recent Jobs.base" {
                    existing.starts_with("# taskcli-generated: pending-review\n")
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
        let mut metadata = crate::publication::PublicationMetadata::default();
        for plan in &state.plans {
            if !paths.contains_key(&format!("plan:{}", plan.id)) {
                continue;
            }
            let bytes = std::fs::read(self.safe_path(&plan.path)?)
                .with_context(|| format!("missing Plan {}", plan.path))?;
            metadata.plans.push(crate::publication::PublishedPlan {
                id: plan.id.clone(),
                version: plan.version,
                hash: hash_bytes(&bytes),
            });
        }
        for job in &state.jobs {
            if !paths.contains_key(&format!("job:{}", job.id)) {
                continue;
            }
            metadata.goals.insert(job.id.clone(), job.goal.clone());
        }
        let removed = previous
            .keys()
            .filter(|key| !paths.contains_key(*key))
            .cloned()
            .collect();
        self.store
            .acknowledge_documents(pending, &paths, &removed, sequence, &metadata)
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
        "> GENERATED — Use taskcli for managed fields; Obsidian status edits require Taskcli Sync."
    }

    fn recent_jobs_base(&self) -> Result<String> {
        let folder = self.config.documents.directory.join("Projects");
        let folder = folder.to_string_lossy().replace('\\', "/");
        let base = json!({
            "filters":{"and":[
                format!("file.inFolder({})", json!(folder.trim_start_matches("./"))),
                "file.ext == \"md\"", "note[\"taskcli-generated\"] == true",
                "file.hasTag(\"agent/job\")", "archived != true"
            ]},
            "formulas":{"name":"link(file.path, note.name)", "review_time":REVIEW_TIME_FORMULA, "updated":"date(note.updated_at).format(\"YYYY-MM-DD HH:mm:ss\")"},
            "properties":{
                "formula.name":{"displayName":"Job"},
                "formula.updated":{"displayName":"Updated"},
                "projects":{"displayName":"Project"},
                "formula.review_time":{"displayName":"Pending review since"}
            },
            "views": recent_jobs_views()
        });
        Ok(format!(
            "# taskcli-generated: pending-review\n{}",
            serde_yaml::to_string(&base)?
        ))
    }

    fn dashboard(&self) -> Result<(String, String)> {
        let folder = self.config.documents.directory.join("Projects");
        let folder = folder.to_string_lossy().replace('\\', "/");
        let folder = folder.trim_start_matches("./");
        let base = json!({
            "filters": {"and": [
                format!("file.inFolder({})", json!(folder)),
                "file.ext == \"md\"", "note[\"taskcli-generated\"] == true"
            ]},
            "formulas": {
                "name": "link(file.path, note.name)",
                "status": "note.status", "updated": "date(note.updated_at)",
                "review_time": REVIEW_TIME_FORMULA
            },
            "properties": {
                "formula.name": {"displayName":"Name"},
                "formula.status": {"displayName":"Status"},
                "formula.updated": {"displayName":"Updated"},
                "formula.review_time": {"displayName":"Pending review since"}
            },
            "views": [{
                "type":"table", "name":"Projects",
                "filters":{"and":["file.name == \"Board\"", "file.hasTag(\"agent/project\")", "note.status == \"ACTIVE\""]},
                "order":["formula.name", "formula.status", "formula.updated"],
                "sort":[{"column":"formula.updated","direction":"DESC"}, {"column":"formula.name","direction":"ASC"}]
            }, pending_review_view()]
        });
        Ok((
            "Dashboard.base".into(),
            format!(
                "# taskcli-generated: dashboard\n{}",
                serde_yaml::to_string(&base)?
            ),
        ))
    }

    fn job_properties(&self, job: &crate::Job, project: &crate::Project) -> Result<Value> {
        let mut properties = serde_json::to_value(job)?;
        for field in [
            "created_at",
            "updated_at",
            "started_at",
            "pending_review_at",
            "followup_at",
            "completed_at",
            "cancelled_at",
            "archived_at",
        ] {
            properties[field] = optional_local_timestamp(properties[field].as_i64())?;
        }
        properties["tags"] = if job.archived_at.is_some() {
            json!(["agent/archived/job"])
        } else {
            json!(["agent/job"])
        };
        properties["title"] = json!(job.name);
        properties["projects"] =
            json!([self.link(&format!("Projects/{}/Board.md", project.key), &project.name,)]);
        properties["archived"] = json!(job.archived_at.is_some() || project.archived_at.is_some());
        Ok(properties)
    }

    fn tasknotes_board(
        &self,
        project: &crate::Project,
        sequence: i64,
        updated_at: i64,
    ) -> Result<String> {
        let title = format!("{} — Board", project.name);
        let mut doc = frontmatter(
            json!({"id":project.id,"name":project.name,"created_at":timestamp(project.created_at),"updated_at":timestamp(updated_at),"title":title,"revision":project.revision,"root":project.root,"remote":project.remote,"archived_at":optional_timestamp(project.archived_at),"status":if project.archived_at.is_some() {"ARCHIVED"} else {"ACTIVE"},"sync_status":"synced","sync_sequence":sequence,"tags":["agent/project","agent/board"]}),
        );
        doc.push_str(&Self::header(&title));
        doc.push_str(&format!(
            "\n{}\n",
            self.link(&format!("Projects/{}/Inbox.md", project.key), "Inbox")
        ));
        for (kind, folder, statuses) in [
            ("Job", "Jobs", json!(crate::JobStatus::ALL)),
            ("Task", "Tasks", json!(TaskStatus::ALL)),
        ] {
            let folder = self
                .config
                .documents
                .directory
                .join(format!("Projects/{}/{folder}", project.key));
            let folder = folder.to_string_lossy().replace('\\', "/");
            let folder = folder.trim_start_matches("./");
            let base = json!({
                "filters": {"and": [format!("file.folder == {}", json!(folder)), format!("file.hasTag(\"agent/{}\")", kind.to_lowercase()), format!("project_id == {}", json!(project.id)), "archived != true"]},
                "views": [{
                    "type": "tasknotesKanban", "name": format!("{kind} board"),
                    "groupBy": {"property": "status", "direction": "ASC"},
                    "order": ["status"], "sort": [{"column": "updated_at", "direction": "ASC"}, {"column": "file.name", "direction": "ASC"}],
                    "columnOrder": {"status": statuses}, "pinnedColumns": statuses,
                    "hideEmptyColumns": true, "columnWidth": 300
                }]
            });
            doc.push_str(&format!(
                "\n## {kind} board\n\n```base\n{}\n```\n",
                serde_yaml::to_string(&base)?.trim_end()
            ));
        }
        Ok(doc)
    }

    fn task_document(
        &self,
        index: &ProjectionIndex<'_>,
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
        let project = index.project(&task.project_id)?;
        let job = index.job(&task.job_id)?;
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
        let mut generated = task_state_properties(task)?;
        generated.as_object_mut().unwrap().extend(json!({
            "id":task.id,"task_id":task.id,"plan_id":task.current_plan,"job_id":task.job_id,"project_id":task.project_id,
            "sequence":task.sequence,
            "agent":task.last_executor.as_deref().and_then(crate::model::agent_name),"session_id":task.last_session,
            "archived":job.archived_at.is_some() || project.archived_at.is_some(),
            "projects":[wiki(&format!("Projects/{}/Board.md",project.key))],"job":wiki(&job.document_path)
        }).as_object().unwrap().clone());
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

    fn task_line(&self, index: &ProjectionIndex<'_>, task: &Task) -> Result<String> {
        let path = index.task_path(task)?;
        let label = Path::new(&path)
            .file_stem()
            .and_then(|name| name.to_str())
            .context("Task path must have a UTF-8 filename")?;
        Ok(format!("- {}", self.link(&path, label)))
    }

    pub(crate) fn link(&self, to: &str, label: &str) -> String {
        let escaped_label = escape(label);
        let needs_plain_label = escaped_label != label;
        let label = escaped_label;
        let to = self
            .config
            .documents
            .directory
            .join(to)
            .to_string_lossy()
            .replace('\\', "/");
        let to = to.trim_start_matches("./").trim_end_matches(".md");
        if needs_plain_label {
            // Obsidian aliases do not decode HTML entities. Keep labels
            // with reserved characters outside a stable wiki link.
            let label = label
                .replace('\\', "&#92;")
                .replace('*', "&#42;")
                .replace('_', "&#95;")
                .replace('`', "&#96;")
                .replace('~', "&#126;");
            format!("[[{to}|Open]] {label}")
        } else {
            format!("[[{to}|{label}]]")
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
fn optional_timestamp(value: Option<i64>) -> Value {
    value.map_or(Value::Null, timestamp)
}

fn task_state_properties(task: &Task) -> Result<Value> {
    Ok(json!({
        "status":task.status,"revision":task.revision,"phase":task.phase,
        "dependencies":task.dependencies,
        "created_at":local_timestamp(task.created_at)?,"updated_at":local_timestamp(task.updated_at)?,
        "started_at":optional_local_timestamp(task.started_at)?,"completed_at":optional_local_timestamp(task.completed_at)?
    }))
}

fn frontmatter(mut properties: Value) -> String {
    #[cfg(test)]
    tests::FRONTMATTER_CALLS.with(|calls| calls.set(calls.get() + 1));
    normalize_timestamp_properties(&mut properties);
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

fn normalize_timestamp_properties(properties: &mut Value) -> bool {
    let object = properties.as_object_mut().unwrap();
    let mut changed = false;
    for (old, canonical) in [
        ("dateCreated", "created_at"),
        ("created", "created_at"),
        ("dateModified", "updated_at"),
        ("updated", "updated_at"),
        ("completedDate", "completed_at"),
    ] {
        if let Some(value) = object.remove(old) {
            object.entry(canonical).or_insert(value);
            changed = true;
        }
    }
    changed
}

pub(crate) fn normalize_document_timestamps(source: &str) -> Result<String> {
    let (mut properties, body) = split_properties(source)?;
    if !normalize_timestamp_properties(&mut properties) {
        return Ok(source.to_owned());
    }
    Ok(format!(
        "{}\n{body}",
        frontmatter(properties).trim_end_matches('\n')
    ))
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

fn job_dependency_graph(
    service: &Service,
    index: &ProjectionIndex<'_>,
    job_id: &str,
) -> Result<String> {
    let tasks = index
        .tasks_by_job
        .get(job_id)
        .map_or(&[][..], Vec::as_slice);
    if tasks.is_empty() {
        return Ok(String::new());
    }
    let mut nodes = BTreeSet::new();
    for task in tasks {
        nodes.insert(task.id.as_str());
        nodes.extend(task.dependencies.iter().map(String::as_str));
    }
    let mut diagram = String::from(
        "\nArrows point from prerequisites to dependent tasks.\n\n```mermaid\nflowchart TD\n",
    );
    for id in nodes {
        let task = index.task(id)?;
        let label = if task.job_id == job_id {
            task.name.clone()
        } else {
            let job = index.job(&task.job_id)?;
            format!("{} (Job: {})", task.name, job.name)
        };
        let label = format!("{} · {}", mermaid_label(&label), task.status);
        let path = index.task_path(task)?;
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
    for task in tasks {
        let mut indirect = BTreeSet::new();
        if task.dependencies.len() > 1 {
            let mut pending: Vec<_> = task.dependencies.iter().map(String::as_str).collect();
            let mut visited = BTreeSet::new();
            while let Some(id) = pending.pop() {
                if !visited.insert(id) {
                    continue;
                }
                let ancestor = index.task(id)?;
                // Other Jobs' incoming edges are not drawn. Only reduce paths
                // that remain visible in this diagram.
                if ancestor.job_id == job_id {
                    for prerequisite in &ancestor.dependencies {
                        indirect.insert(prerequisite.as_str());
                        pending.push(prerequisite.as_str());
                    }
                }
            }
        }
        for prerequisite in &task.dependencies {
            if !indirect.contains(prerequisite.as_str()) {
                diagram.push_str(&format!("    {prerequisite} --> {}\n", task.id));
            }
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
fn prompt_markdown(prompt: &str) -> String {
    let prompt = crate::conversation::user_text(prompt);
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

const REVIEW_TIME_FORMULA: &str = "if(note.pending_review_at, date(note.pending_review_at).format(\"YYYY-MM-DD HH:mm:ss\"), \"\")";

fn recent_jobs_views() -> Vec<Value> {
    let statuses = ["ACTIVE", "PENDING_REVIEW", "COMPLETED", "CANCELLED"];
    let sort = json!([
        {"column":"updated_at","direction":"DESC"},
        {"column":"file.name","direction":"ASC"}
    ]);
    let mut views = vec![json!({
        "type":"taskcliRecentJobs", "name":"Recent jobs",
        "groupBy":{"property":"status","direction":"ASC"},
        "order":["status", "projects", "formula.updated", "formula.review_time"],
        "sort":sort,
        "columnOrder":{"status":statuses}, "pinnedColumns":statuses,
        "hideEmptyColumns":true, "columnWidth":300
    })];
    // Native Bases limits apply to the whole view, so each status gets its
    // own table. The Taskcli Sync Kanban adapter limits each column instead.
    views.extend(statuses.map(|status| json!({
        "type":"table", "name":status, "limit":10,
        "filters":format!("note.status == {status:?}"),
        "order":["formula.name", "projects", "status", "formula.updated", "formula.review_time"],
        "sort":sort
    })));
    views
}

fn pending_review_view() -> Value {
    json!({
        "type":"tasknotesKanban", "name":"Pending review",
        "filters":{"and":["file.hasTag(\"agent/job\")", "note.status == \"PENDING_REVIEW\"", "archived != true"]},
        "groupBy":{"property":"status","direction":"ASC"},
        "order":["status", "projects", "formula.review_time"],
        "sort":[{"column":"pending_review_at","direction":"ASC"},{"column":"file.name","direction":"ASC"}],
        "columnOrder":{"status":["PENDING_REVIEW"]}, "pinnedColumns":["PENDING_REVIEW"],
        "hideEmptyColumns":true, "columnWidth":300
    })
}

fn conversation_markdown(job: &crate::Job) -> String {
    let mut turns: Vec<(String, Vec<&str>)> = Vec::new();
    for message in &job.conversation {
        if message.role == "user" {
            let text = crate::conversation::user_text(&message.text);
            if !text.trim().is_empty() {
                turns.push((text.into(), Vec::new()));
            }
        } else if message.role == "assistant" && !message.text.trim().is_empty() {
            if turns.is_empty() {
                turns.push((
                    crate::conversation::user_text(&job.prompt).into(),
                    Vec::new(),
                ));
            }
            turns
                .last_mut()
                .unwrap()
                .1
                .push(message.text.trim_end_matches(['\r', '\n']));
        }
    }
    if turns.is_empty() {
        return String::new();
    }
    let mut body = String::from("\n## Conversation\n");
    for (index, (prompt, output)) in turns.iter().enumerate() {
        body.push_str(&format!("\n### Turn {}\n", index + 1));
        if !prompt.trim().is_empty() {
            body.push_str("\n#### User input\n\n");
            for line in prompt.split('\n') {
                if !line.is_empty() {
                    body.push_str("    ");
                    body.push_str(line);
                }
                body.push('\n');
            }
        }
        if !output.is_empty() {
            body.push_str("\n#### Agent output\n\n");
            for line in output.join("\n\n").split('\n') {
                body.push('>');
                if !line.is_empty() {
                    body.push(' ');
                    body.push_str(line);
                }
                body.push('\n');
            }
        }
    }
    body
}
