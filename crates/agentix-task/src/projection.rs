use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::{
    Config, DocumentFormat, Job, JobStatus, Outcome, Plan, Snapshot, Store, Task, TaskStatus,
    WriteOptions, config::resolved_path, mutations::required, new_id, store::hash_bytes,
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
        if command == "session.start" {
            let state = self.store.snapshot().await?;
            let session = required(&request, "session")?;
            for task in state
                .tasks
                .iter()
                .filter(|t| t.system_block && t.last_session.as_deref() == Some(session))
            {
                self.plan(&task.id).await?;
            }
        }
        if command == "task.claim" {
            let state = self.store.snapshot().await?;
            let task = &state.tasks[state.task_index(required(&request, "task")?)?];
            let plan = state
                .plans
                .iter()
                .find(|p| Some(&p.id) == task.current_plan.as_ref())
                .context("invalid: current Plan is required before claim")?;
            let bytes = std::fs::read(self.safe_path(&plan.path)?)
                .context("current Plan file is missing")?;
            ensure!(!bytes.is_empty(), "invalid: current Plan is empty");
            self.store
                .update_plan_hash(&plan.id, &hash_bytes(&bytes))
                .await?;
        }
        let mut outcome = self.store.execute(request, options).await?;
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
        let version = state
            .plans
            .iter()
            .filter(|p| p.task_id == task.id)
            .map(|p| p.version)
            .max()
            .unwrap_or(0)
            + 1;
        let project = &state.projects[state.project_index(&task.project_id)?];
        let plan = Plan {
            id: new_id("plan"),
            task_id: task.id.clone(),
            version,
            path: format!(
                "Projects/{}/Plans/{}/v{version:03}.md",
                project.key, task.id
            ),
            hash: hash_bytes(body.as_bytes()),
            created_at: self.store.now(),
        };
        let path = self.safe_path(&plan.path)?;
        let registration = json!({"command":"plan.register","task":task.id,"plan":plan});
        // Validate ownership and state before touching the filesystem.
        let mut preview = state.clone();
        crate::mutations::apply(&mut preview, &registration, &options, self.store.now())?;
        if path.exists() {
            ensure!(
                std::fs::read(&path)? == body.as_bytes(),
                "conflict: unregistered Plan file exists; preserve or move it before retrying"
            );
        } else {
            atomic_write(&path, body)?;
        }
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
        result["body"] = json!(body);
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
        let mut dashboard = header("Task dashboard");
        for project in &state.projects {
            let board_path = format!("Projects/{}/Board.md", project.key);
            dashboard.push_str(&format!(
                "\n## {}\n\n{}\n",
                escape(&project.name),
                self.link("Dashboard.md", &board_path, None, "Board")
            ));
            let mut board = header(&format!("{} — task board", project.name));
            board.push_str("\n| TODO | IN_PROGRESS | BLOCKED | WAITING_USER | DONE | FAILED | CANCELLED |\n| --- | --- | --- | --- | --- | --- | --- |\n");
            let mut columns: Vec<Vec<String>> = vec![Vec::new(); 7];
            let mut tasks: Vec<_> = state
                .tasks
                .iter()
                .filter(|t| {
                    t.project_id == project.id
                        && state.jobs.iter().any(|j| {
                            j.id == t.job_id
                                && j.status == JobStatus::Active
                                && j.archived_at.is_none()
                        })
                })
                .collect();
            tasks.sort_by_key(|t| (t.position, t.id.clone()));
            for task in tasks {
                let column = TaskStatus::ALL
                    .iter()
                    .position(|s| *s == task.status)
                    .context("unknown task state")?;
                let (path, anchor) = task_target(&state, task)?;
                columns[column].push(
                    self.link(&board_path, &path, anchor.as_deref(), &task.title)
                        .replace('|', "\\|"),
                );
            }
            for row in 0..columns.iter().map(Vec::len).max().unwrap_or(0) {
                board.push_str("| ");
                board.push_str(
                    &columns
                        .iter()
                        .map(|col| col.get(row).map_or("", String::as_str))
                        .collect::<Vec<_>>()
                        .join(" | "),
                );
                board.push_str(" |\n");
            }
            files.insert(board_path.clone(), board);
            paths.insert(format!("board:{}", project.id), board_path);
            for job in state.jobs.iter().filter(|j| j.project_id == project.id) {
                if job.archived_at.is_none() {
                    dashboard.push_str(&format!(
                        "\n- {} — {}\n",
                        self.link("Dashboard.md", &job.document_path, None, &job.title),
                        job.status
                    ));
                }
                let key = format!("job:{}", job.id);
                let previous_path = previous.get(&key).unwrap_or(&job.document_path);
                let source = self.safe_path(previous_path)?;
                // A previous sync may have moved the file but not acknowledged its path.
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
                let mut doc = header(&job.title);
                doc.push_str(&format!("\nJob: `{}` · {} · revision {}\n\n## Goal\n\n<!-- taskcli:goal:start -->\n{}\n<!-- taskcli:goal:end -->\n\n## Tasks\n",job.id,job.status,job.revision,goal));
                for task in state.tasks.iter().filter(|t| t.job_id == job.id) {
                    let anchor = task.id.replace('_', "-");
                    match self.config.documents.format {
                        DocumentFormat::Obsidian => doc.push_str(&format!(
                            "\n{} · {} · `{}` ^{anchor}\n",
                            escape(&task.title),
                            task.status,
                            task.id
                        )),
                        DocumentFormat::Markdown => doc.push_str(&format!(
                            "\n<a id=\"{anchor}\"></a>\n\n{} · {} · `{}`\n",
                            escape(&task.title),
                            task.status,
                            task.id
                        )),
                    }
                    if let Some(plan) = state
                        .plans
                        .iter()
                        .find(|p| Some(&p.id) == task.current_plan.as_ref())
                    {
                        doc.push_str(&format!(
                            "\n{}\n",
                            self.link(&job.document_path, &plan.path, None, "Plan")
                        ));
                    }
                    if let Some(reason) = &task.reason {
                        doc.push_str(&format!("\nReason: {}\n", escape(reason)));
                    }
                    if !task.dependencies.is_empty() {
                        doc.push_str("\nDependencies: ");
                        for (index, dependency) in task.dependencies.iter().enumerate() {
                            if index > 0 {
                                doc.push(' ');
                            }
                            let t = &state.tasks[state.task_index(dependency)?];
                            let j = &state.jobs[state.job_index(&t.job_id)?];
                            doc.push_str(&self.link(
                                &job.document_path,
                                &j.document_path,
                                Some(&t.id.replace('_', "-")),
                                &t.title,
                            ));
                        }
                        doc.push('\n');
                    }
                }
                doc.push_str(&format!("\n## Notes\n\n<!-- taskcli:notes:start -->\n{notes}\n<!-- taskcli:notes:end -->\n"));
                files.insert(job.document_path.clone(), doc);
                paths.insert(key, job.document_path.clone());
            }
            let sync_path = format!("Projects/{}/Sync Status.md", project.key);
            files.insert(sync_path.clone(),format!("{}\nSnapshot sequence: {sequence}\n\nTask documents are generated from SQLite. Run `taskcli sync` to repair projections.\n",header("Sync status")));
            paths.insert(format!("sync:{}", project.id), sync_path);
        }
        files.insert("Dashboard.md".into(), dashboard);
        paths.insert("dashboard".into(), "Dashboard.md".into());
        // Read editable content before replacing any managed document.
        for (path, contents) in files {
            atomic_write(&self.safe_path(&path)?, &contents)?;
        }
        let current: BTreeSet<_> = paths.values().collect();
        for old in previous.values().filter(|old| !current.contains(old)) {
            let path = self.safe_path(old)?;
            if path.exists() {
                std::fs::remove_file(path)?;
            }
        }
        for plan in &state.plans {
            let bytes = std::fs::read(self.safe_path(&plan.path)?)
                .with_context(|| format!("missing Plan {}", plan.path))?;
            self.store
                .update_plan_hash(&plan.id, &hash_bytes(&bytes))
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

fn header(title: &str) -> String {
    format!(
        "# {}\n\n> GENERATED — DO NOT EDIT task fields. Use taskcli or an Agent.\n",
        escape(title)
    )
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
