use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::{
    Config, DocumentFormat, Job, Outcome, Plan, Snapshot, Store, Task, TaskStatus, WriteOptions,
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
        // Serialize Plan selection/validation with Plan writes until the start commits.
        let lock = if command == "task.start" {
            Some(self.lock_output().await?)
        } else {
            None
        };
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
        let project = &state.projects[state.project_index(&task.project_id)?];
        let current = state.plans.iter().find(|p| p.task_id == task.id);
        let filename = crate::naming::numbered_name(&task.name, task.created_at, task.sequence)?;
        let plan = Plan {
            id: current.map_or_else(|| new_id("plan"), |p| p.id.clone()),
            task_id: task.id.clone(),
            version,
            path: current.map_or_else(
                || format!("Projects/{}/Plans/{filename}.md", project.key),
                |p| p.path.clone(),
            ),
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
        ensure!(
            current.is_some() || !path.exists(),
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

    fn safe_path(&self, relative: &str) -> Result<PathBuf> {
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
        let mut dashboard = frontmatter(
            json!({"id":"dashboard", "created_at":timestamp(created), "tags":["agent/dashboard"]}),
        );
        dashboard.push_str(&Self::header("Task dashboard"));
        for project in &state.projects {
            let board_path = format!("Projects/{}/Board.md", project.key);
            let tasks_path = format!("Projects/{}/Tasks.md", project.key);
            let meta_path = format!("Projects/{}/meta.md", project.key);
            if project.archived_at.is_none() {
                dashboard.push_str(&format!(
                    "\n## {}\n\n{} · {} · {}\n",
                    escape(&project.name),
                    self.link("Dashboard.md", &meta_path, None, "Project"),
                    self.link("Dashboard.md", &board_path, None, "Kanban board"),
                    self.link("Dashboard.md", &tasks_path, None, "Tasks")
                ));
            }
            let mut meta = frontmatter(
                json!({"id":project.id, "name":project.name, "created_at":timestamp(project.created_at), "revision":project.revision, "root":project.root, "remote":project.remote, "archived_at":optional_timestamp(project.archived_at), "status":if project.archived_at.is_some() {"ARCHIVED"} else {"ACTIVE"}, "sync_status":"synced", "sync_sequence":sequence, "tags":["agent/project"]}),
            );
            meta.push_str(&Self::header(&project.name));
            meta.push_str(&format!(
                "\n{} · {}\n",
                self.link(&meta_path, &board_path, None, "Kanban board"),
                self.link(&meta_path, &tasks_path, None, "Tasks")
            ));
            files.insert(meta_path.clone(), meta);
            paths.insert(format!("meta:{}", project.id), meta_path);
            let mut board = frontmatter(
                json!({"id":format!("board:{}",project.id), "created_at":timestamp(project.created_at), "tags":["agent/board"], "title":format!("{} — task board",project.name), "show-checkboxes":false,"show-add-list":false,"show-archive-all":false,"show-board-settings":false}),
            );
            board = board.replacen("---\n", "---\nkanban-plugin: board\n", 1);
            board.push_str(&format!(
                "\n{}\n\n{}\n",
                Self::notice(),
                self.link(&board_path, &tasks_path, None, "Tasks view")
            ));
            let mut columns: Vec<Vec<String>> = vec![Vec::new(); 7];
            let mut tasks: Vec<_> = state
                .tasks
                .iter()
                .filter(|t| {
                    t.project_id == project.id
                        && state
                            .jobs
                            .iter()
                            .any(|j| j.id == t.job_id && j.archived_at.is_none())
                })
                .collect();
            tasks.sort_by_key(|t| (t.position, t.id.clone()));
            for task in tasks {
                let column = TaskStatus::ALL
                    .iter()
                    .position(|s| *s == task.status)
                    .context("unknown task state")?;
                columns[column].push(self.task_line(&state, task, &board_path)?);
            }
            for (status, cards) in TaskStatus::ALL.iter().zip(columns) {
                board.push_str(&format!("\n## {status}\n\n"));
                for card in cards {
                    board.push_str(&card);
                    board.push('\n');
                }
            }
            files.insert(board_path.clone(), board);
            paths.insert(format!("board:{}", project.id), board_path.clone());
            files.insert(
                tasks_path.clone(),
                self.tasks_view(project, &tasks_path, &board_path),
            );
            paths.insert(format!("tasks:{}", project.id), tasks_path);
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
                for field in ["goal", "document_path", "title", "name"] {
                    properties.as_object_mut().unwrap().remove(field);
                }
                let mut doc = frontmatter(properties);
                doc.push_str(&Self::header(&job.name));
                doc.push_str(&format!("\n## {}\n\n<!-- taskcli:goal:start -->\n{}\n<!-- taskcli:goal:end -->\n\n## {}\n", "Goal", goal, "Tasks"));
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
                    if !task.dependencies.is_empty() {
                        let mut links = Vec::new();
                        for dependency in &task.dependencies {
                            let t = &state.tasks[state.task_index(dependency)?];
                            let (path, anchor) = task_target(&state, t)?;
                            links.push(self.link(
                                &job.document_path,
                                &path,
                                anchor.as_deref(),
                                &t.name,
                            ));
                        }
                        doc.push_str(&format!("\n  {}: {}\n", "Dependencies", links.join(" ")));
                    }
                }
                doc.push_str(&format!("\n## {}\n\n<!-- taskcli:notes:start -->\n{notes}\n<!-- taskcli:notes:end -->\n", "Notes"));
                files.insert(job.document_path.clone(), doc);
                paths.insert(key, job.document_path.clone());
            }
        }
        for plan in &state.plans {
            let task = &state.tasks[state.task_index(&plan.task_id)?];
            let key = format!("plan:{}", plan.id);
            let source = self.safe_path(previous.get(&key).unwrap_or(&plan.path))?;
            let source = if source.exists() {
                source
            } else {
                self.safe_path(&plan.path)?
            };
            let body = if let Some(body) = &plan.pending_body {
                body.clone()
            } else {
                std::fs::read_to_string(source)
                    .with_context(|| format!("missing Plan {}", plan.path))?
            };
            let (mut authored, body) = split_properties(&body)?;
            let generated = json!({"id":plan.id,"task_id":task.id,"job_id":task.job_id,"project_id":task.project_id,"sequence":task.sequence,"version":plan.version,"created_at":timestamp(plan.created_at),"updated_at":timestamp(plan.updated_at),"started_at":optional_timestamp(task.started_at),"completed_at":optional_timestamp(task.completed_at),"status":task.status});
            let authored_tags = authored["tags"].clone();
            if authored["title"].is_null() {
                authored["title"] = json!(task.name);
            }
            for (key, value) in generated.as_object().unwrap() {
                authored[key] = value.clone();
            }
            let mut tags = match authored_tags {
                Value::Array(tags) => tags,
                Value::String(tag) => vec![json!(tag)],
                _ => Vec::new(),
            };
            if !tags.contains(&json!("agent/plan")) {
                tags.push(json!("agent/plan"));
            }
            authored["tags"] = json!(tags);
            let mut doc = frontmatter(authored);
            doc.push_str(body);
            files.insert(plan.path.clone(), doc);
            paths.insert(key, plan.path.clone());
        }
        files.insert("Dashboard.md".into(), dashboard);
        paths.insert("dashboard".into(), "Dashboard.md".into());
        // Check all new destinations before publishing any file. Existing managed
        // paths can be regenerated; new paths must not clobber unrelated notes.
        for (relative, contents) in &files {
            let path = self.safe_path(relative)?;
            if path.exists() && !previous.values().any(|old| old == relative) {
                let existing = std::fs::read_to_string(&path)?;
                let (old, _) = split_properties(&existing)?;
                let (new, _) = split_properties(contents)?;
                ensure!(
                    old["taskcli-generated"] == true && old["id"] == new["id"],
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
            for relative in deletion.files.iter().chain(&deletion.directories) {
                self.deletion_path(relative)?;
            }
            for relative in &deletion.files {
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

    fn header(title: &str) -> String {
        format!("# {}\n\n{}\n", escape(title), Self::notice())
    }

    fn task_line(&self, state: &Snapshot, task: &Task, from: &str) -> Result<String> {
        let mut line = format!("- [{}] {}", checkbox(task.status), escape(&task.name));
        if let Some(phase) = task.phase {
            line.push_str(&format!(" · {phase}"));
        }
        if let Some(plan) = state
            .plans
            .iter()
            .find(|p| Some(&p.id) == task.current_plan.as_ref())
        {
            let reference = self.link(from, &plan.path, None, "Plan");
            line.push(' ');
            if self.config.documents.format == DocumentFormat::Markdown {
                line.push('!');
            }
            line.push_str(&reference);
        } else {
            let job = &state.jobs[state.job_index(&task.job_id)?];
            if from != job.document_path {
                line.push_str(&format!(
                    " {}",
                    self.link(
                        from,
                        &job.document_path,
                        Some(&task.id.replace('_', "-")),
                        "Job"
                    )
                ));
            }
        }
        line.push_str(" #task");
        Ok(line)
    }

    fn tasks_view(&self, project: &crate::Project, path: &str, board: &str) -> String {
        let title = format!("{} — Tasks view", project.name);
        let mut document = frontmatter(
            json!({"id":format!("tasks:{}",project.id), "created_at":timestamp(project.created_at), "title":title,"tags":["agent/tasks"]}),
        );
        document.push_str(&Self::header(&title));
        document.push_str(&format!("\n{}\n\n{}\n",self.link(path, board, None, "Kanban board"), "Requires the Obsidian Tasks plugin with taskcli custom statuses. Queries use Board cards only to avoid duplicate Job and Plan checklists."));
        for status in TaskStatus::ALL {
            document.push_str(&format!("\n## {}\n\n```tasks\nfilter by function task.file.path === query.file.path.replace(/Tasks\\.md$/, 'Board.md') && task.status.symbol === '{}'\nhide edit button\nhide postpone button\n```\n",status,checkbox(status)));
        }
        document
    }

    fn link(&self, from: &str, to: &str, anchor: Option<&str>, label: &str) -> String {
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
                if let Some(anchor) = anchor {
                    path.push('#');
                    path.push_str(&encode(anchor));
                }
                format!("[{label}]({path})")
            }
        }
    }
}

fn checkbox(status: TaskStatus) -> char {
    match status {
        TaskStatus::Todo => ' ',
        TaskStatus::InProgress => '/',
        TaskStatus::Blocked => '!',
        TaskStatus::WaitingUser => '?',
        TaskStatus::Done => 'x',
        TaskStatus::Failed => 'f',
        TaskStatus::Cancelled => '-',
    }
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

fn optional_timestamp(value: Option<i64>) -> Value {
    value.map_or(Value::Null, timestamp)
}

fn frontmatter(mut properties: Value) -> String {
    properties["taskcli-generated"] = json!(true);
    let mut result = String::from("---\n");
    for (key, value) in properties.as_object().unwrap() {
        result.push_str(&format!("{key}: {value}\n"));
    }
    result.push_str("---\n\n");
    result
}

fn split_properties(body: &str) -> Result<(Value, &str)> {
    if let Some((properties, content)) = body
        .strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---\n"))
    {
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
            content.strip_prefix('\n').unwrap_or(content),
        ))
    } else {
        Ok((json!({}), body))
    }
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
fn task_target(state: &Snapshot, task: &Task) -> Result<(String, Option<String>)> {
    if let Some(plan) = state
        .plans
        .iter()
        .find(|p| Some(&p.id) == task.current_plan.as_ref())
    {
        return Ok((plan.path.clone(), None));
    }
    let job: &Job = &state.jobs[state.job_index(&task.job_id)?];
    Ok((job.document_path.clone(), Some(task.id.replace('_', "-"))))
}
fn section(body: &str, name: &str) -> Result<Option<String>> {
    if body.is_empty() {
        return Ok(None);
    }
    let start = format!("<!-- taskcli:{name}:start -->");
    let end = format!("<!-- taskcli:{name}:end -->");
    ensure!(
        body.matches(&start).count() == 1 && body.matches(&end).count() == 1,
        "editable {name} markers missing or duplicated; restore markers before sync"
    );
    let tail = body.split_once(&start).context("missing start marker")?.1;
    Ok(Some(
        tail.split_once(&end)
            .context("reversed section markers")?
            .0
            .trim_matches('\n')
            .to_owned(),
    ))
}

fn atomic_write(path: &Path, body: &str) -> Result<()> {
    let parent = path.parent().context("document has no parent")?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(body.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
