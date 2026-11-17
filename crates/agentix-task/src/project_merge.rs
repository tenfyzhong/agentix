//! Merge repository identities and journal document relocation for crash recovery.
use crate::{
    Project, Service, Snapshot, WriteOptions,
    mutations::{check_revision, required},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqliteConnection;
use std::collections::{BTreeMap, BTreeSet};

fn base_key(key: &str) -> &str {
    key.rsplit_once('-')
        .filter(|(_, suffix)| suffix.parse::<u64>().is_ok_and(|n| n >= 2))
        .map_or(key, |(base, _)| base)
}

pub(crate) fn apply(
    state: &mut Snapshot,
    request: &Value,
    options: &WriteOptions,
) -> Result<Value> {
    let a = state.projects[state.project_index(required(request, "project")?)?].clone();
    let b = state.projects[state.project_index(required(request, "source")?)?].clone();
    ensure!(a.id != b.id, "invalid: cannot merge a Project with itself");
    check_revision(a.revision, options)?;
    if let Some(revision) = request["merge_revision"].as_i64() {
        ensure!(a.revision == revision, "conflict: Project revision changed");
    }
    if let Some(revision) = request["source_revision"].as_i64() {
        ensure!(b.revision == revision, "conflict: source revision changed");
    }
    ensure!(
        a.remote
            .as_deref()
            .is_some_and(|r| !r.trim().is_empty() && b.remote.as_deref() == Some(r)),
        "invalid: Projects must have the same nonblank Git remote"
    );
    let (mut keep, duplicate) = if a.key == base_key(&a.key) || b.key != base_key(&b.key) {
        (a, b)
    } else {
        (b, a)
    };
    let remote = keep.remote.as_deref().unwrap();
    let mut candidates = [&keep, &duplicate];
    candidates.sort_by_key(|p| {
        (
            state
                .query_context
                .project_activity
                .get(&p.id)
                .copied()
                .unwrap_or((p.created_at, 0)),
            p.created_at,
        )
    });
    let root = candidates
        .into_iter()
        .rev()
        .find_map(|p| {
            crate::git_identity(std::path::Path::new(&p.root))
                .ok()
                .filter(|(_, r)| r.as_deref() == Some(remote))
                .map(|(root, _)| root)
        })
        .context("invalid: no existing local repository with the same Git remote")?;
    keep.root = root.to_string_lossy().into_owned();
    let key = base_key(&keep.key).to_owned();
    keep.key = key;
    keep.name.clone_from(&keep.key);
    keep.archived_at = keep.archived_at.filter(|_| duplicate.archived_at.is_some());
    let directory = format!(
        "{}/{}",
        if keep.archived_at.is_some() {
            "Archived Projects"
        } else {
            "Projects"
        },
        keep.key
    );
    keep.document_directory = Some(directory.clone());
    keep.revision += 1;
    let members = BTreeSet::from([keep.id.clone(), duplicate.id.clone()]);
    reallocate_sequences(state, &keep, &duplicate)?;
    relocate_work(state, &keep, &members, &directory)?;
    state.projects.retain(|p| !members.contains(&p.id));
    let result = serde_json::to_value(&keep)?;
    state.projects.push(keep);
    Ok(result)
}

fn reallocate_sequences(state: &mut Snapshot, keep: &Project, duplicate: &Project) -> Result<()> {
    // Reallocate incoming sequences to avoid same-day filename collisions.
    for kind in ["job", "task", "issue", "milestone"] {
        let entries: Vec<(String, i64, u64, String)> = match kind {
            "job" => state
                .jobs
                .iter()
                .map(|v| (v.id.clone(), v.created_at, v.sequence, v.project_id.clone()))
                .collect(),
            "task" => state
                .tasks
                .iter()
                .map(|v| (v.id.clone(), v.created_at, v.sequence, v.project_id.clone()))
                .collect(),
            "issue" => state
                .issues
                .iter()
                .map(|v| (v.id.clone(), v.created_at, v.sequence, v.project_id.clone()))
                .collect(),
            _ => state
                .milestones
                .iter()
                .map(|v| (v.id.clone(), v.created_at, v.sequence, v.project_id.clone()))
                .collect(),
        };
        let mut high = BTreeMap::<i64, u64>::new();
        for (_, date, seq, project) in &entries {
            if project == &keep.id {
                high.entry(date.div_euclid(86400))
                    .and_modify(|n| *n = (*n).max(*seq))
                    .or_insert(*seq);
            }
        }
        for (id, date, _, project) in entries {
            if project != duplicate.id {
                continue;
            }
            let key = crate::deletion::sequence_key(&keep.id, kind, date);
            let seq = high.entry(date.div_euclid(86400)).or_default();
            *seq = (*seq)
                .max(state.document_sequences.get(&key).copied().unwrap_or(0))
                .checked_add(1)
                .context("document sequence exhausted")?;
            state.document_sequences.insert(key, *seq);
            match kind {
                "job" => state.jobs.iter_mut().find(|v| v.id == id).unwrap().sequence = *seq,
                "task" => {
                    state
                        .tasks
                        .iter_mut()
                        .find(|v| v.id == id)
                        .unwrap()
                        .sequence = *seq;
                }
                "issue" => {
                    state
                        .issues
                        .iter_mut()
                        .find(|v| v.id == id)
                        .unwrap()
                        .sequence = *seq;
                }
                _ => {
                    state
                        .milestones
                        .iter_mut()
                        .find(|v| v.id == id)
                        .unwrap()
                        .sequence = *seq;
                }
            }
        }
    }
    Ok(())
}

fn relocate_work(
    state: &mut Snapshot,
    keep: &Project,
    members: &BTreeSet<String>,
    directory: &str,
) -> Result<()> {
    for job in state
        .jobs
        .iter_mut()
        .filter(|v| members.contains(&v.project_id))
    {
        job.project_id.clone_from(&keep.id);
        job.revision += 1;
        job.document_path = format!(
            "{directory}/Jobs/{}{}.md",
            if job.archived_at.is_some() {
                "Archived/"
            } else {
                ""
            },
            crate::naming::numbered_name(&job.name, job.created_at, job.sequence)?
        );
    }
    for task in state
        .tasks
        .iter_mut()
        .filter(|v| members.contains(&v.project_id))
    {
        task.project_id.clone_from(&keep.id);
        task.revision += 1;
    }
    for issue in state
        .issues
        .iter_mut()
        .filter(|v| members.contains(&v.project_id))
    {
        issue.project_id.clone_from(&keep.id);
        issue.revision += 1;
        issue.document_path = format!(
            "{directory}/Issues/{}.md",
            crate::naming::numbered_name(&issue.name, issue.created_at, issue.sequence)?
        );
    }
    for milestone in state
        .milestones
        .iter_mut()
        .filter(|v| members.contains(&v.project_id))
    {
        milestone.project_id.clone_from(&keep.id);
        milestone.revision += 1;
        milestone.document_path = format!(
            "{directory}/Milestones/{}.md",
            crate::naming::numbered_name(
                &milestone.name,
                milestone.created_at,
                milestone.sequence
            )?
        );
    }
    for plan in &mut state.plans {
        if let Some(task) = state
            .tasks
            .iter()
            .find(|v| v.id == plan.task_id && v.project_id == keep.id)
        {
            plan.path = crate::naming::task_path_in(keep, task)?;
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct Journal {
    files: BTreeMap<String, String>,
    directories: BTreeMap<String, String>,
}

fn document_paths(state: &Snapshot) -> Result<BTreeMap<String, String>> {
    let mut paths = BTreeMap::new();
    for job in &state.jobs {
        paths.insert(format!("job:{}", job.id), job.document_path.clone());
    }
    for task in &state.tasks {
        paths.insert(
            format!("task:{}", task.id),
            crate::naming::task_path(state, task)?,
        );
    }
    for plan in &state.plans {
        paths.insert(format!("plan:{}", plan.id), plan.path.clone());
    }
    for issue in &state.issues {
        paths.insert(format!("issue:{}", issue.id), issue.document_path.clone());
    }
    for milestone in &state.milestones {
        paths.insert(
            format!("milestone:{}", milestone.id),
            milestone.document_path.clone(),
        );
    }
    Ok(paths)
}

pub(crate) async fn prepare(
    conn: &mut SqliteConnection,
    before: &Snapshot,
    after: &Snapshot,
) -> Result<()> {
    let keep = after.projects.first().context("missing merged Project")?;
    let journal = journal(before, after)?;
    for project in &before.projects {
        if project.id != keep.id {
            sqlx::query("UPDATE project_merges SET target=? WHERE target=?")
                .bind(&keep.id)
                .bind(&project.id)
                .execute(&mut *conn)
                .await?;
            sqlx::query("INSERT INTO project_merges(source,target) VALUES (?,?)")
                .bind(&project.id)
                .bind(&keep.id)
                .execute(&mut *conn)
                .await?;
            for table in [
                "memory_source_heads",
                "memory_source_outbox",
                "memory_job_cancellations",
            ] {
                sqlx::query(&format!(
                    "UPDATE {table} SET project_id=? WHERE project_id=?"
                ))
                .bind(&keep.id)
                .bind(&project.id)
                .execute(&mut *conn)
                .await?;
            }
            // Free the UNIQUE root before updating the retained row; the row is removed below.
            sqlx::query("UPDATE projects SET data=json_set(data,'$.root',?) WHERE id=?")
                .bind(format!("merged:{}", project.id))
                .bind(&project.id)
                .execute(&mut *conn)
                .await?;
        }
    }
    let cleanup = crate::deletion::Cleanup {
        id: crate::new_id("cleanup"),
        files: BTreeSet::new(),
        candidates: BTreeMap::new(),
        directories: journal.directories.keys().cloned().collect(),
    };
    sqlx::query("INSERT INTO document_deletions(id,data) VALUES (?,?)")
        .bind(&cleanup.id)
        .bind(serde_json::to_string(&cleanup)?)
        .execute(&mut *conn)
        .await?;
    sqlx::query("INSERT INTO projection_state(key,value) VALUES (?,?)")
        .bind(format!("project_merge:{}", keep.id))
        .bind(serde_json::to_string(&journal)?)
        .execute(conn)
        .await?;
    Ok(())
}

fn journal(before: &Snapshot, after: &Snapshot) -> Result<Journal> {
    let keep = after.projects.first().context("missing merged Project")?;
    let paths = document_paths(after)?;
    let mut journal = Journal {
        files: BTreeMap::new(),
        directories: BTreeMap::new(),
    };
    for (key, old) in document_paths(before)? {
        let new = &paths[&key];
        if old != *new {
            journal.files.insert(old, new.clone());
        }
    }
    for project in &before.projects {
        if project.document_directory() != keep.document_directory() {
            journal
                .directories
                .insert(project.document_directory(), keep.document_directory());
        }
    }
    Ok(journal)
}

impl Service {
    pub(crate) fn validate_project_merge(
        &self,
        before: &Snapshot,
        request: &Value,
        options: &WriteOptions,
    ) -> Result<()> {
        let mut after = before.clone();
        apply(&mut after, request, options)?;
        let mut journal = journal(before, &after)?;
        for (old, new) in &journal.directories {
            self.collect_merge_files(old, new, &mut journal.files, true)?;
        }
        self.validate_merge_destinations(&journal)
    }

    fn validate_merge_destinations(&self, journal: &Journal) -> Result<()> {
        let mut destinations = BTreeMap::new();
        for (old, new) in &journal.files {
            let source = self.safe_path(old)?;
            let destination = self.safe_path(new)?;
            if source.exists() {
                ensure!(
                    !std::fs::symlink_metadata(&source)?.file_type().is_symlink(),
                    "conflict: merge source contains a symlink"
                );
                let bytes = std::fs::read(&source)?;
                if destination.exists() {
                    ensure!(
                        bytes == std::fs::read(&destination)?,
                        "conflict: merge destination already exists: {new}"
                    );
                }
                if let Some(previous) = destinations.insert(new, bytes.clone()) {
                    ensure!(
                        previous == bytes,
                        "conflict: merge destination collision: {new}"
                    );
                }
            }
        }
        Ok(())
    }
    pub(crate) async fn recover_project_merges_locked(&self) -> Result<()> {
        use sqlx::Row;
        let rows =
            sqlx::query("SELECT key,value FROM projection_state WHERE key LIKE 'project_merge:%'")
                .fetch_all(&self.store().pool)
                .await?;
        for row in rows {
            let mut journal: Journal = serde_json::from_str(&row.get::<String, _>("value"))?;
            for (old, new) in &journal.directories {
                self.collect_merge_files(old, new, &mut journal.files, true)?;
            }
            self.validate_merge_destinations(&journal)?;
            for (old, new) in &journal.files {
                let source = self.safe_path(old)?;
                let destination = self.safe_path(new)?;
                if source.exists() {
                    std::fs::create_dir_all(
                        destination.parent().context("missing document parent")?,
                    )?;
                    if !destination.exists() {
                        let temporary = tempfile::NamedTempFile::new_in(
                            destination.parent().context("missing document parent")?,
                        )?;
                        std::fs::copy(&source, temporary.path())?;
                        temporary.as_file().sync_all()?;
                        temporary.persist_noclobber(&destination)?;
                    }
                }
            }
            let mut tx = self.store().pool.begin().await?;
            for (old, new) in &journal.files {
                sqlx::query("UPDATE document_registry SET path=? WHERE path=?")
                    .bind(new)
                    .bind(old)
                    .execute(&mut *tx)
                    .await?;
            }
            sqlx::query("DELETE FROM projection_state WHERE key=?")
                .bind(row.get::<String, _>("key"))
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        Ok(())
    }

    fn collect_merge_files(
        &self,
        old: &str,
        new: &str,
        files: &mut BTreeMap<String, String>,
        top_level: bool,
    ) -> Result<()> {
        let source = self.safe_path(old)?;
        if !source.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            ensure!(
                !entry.file_type()?.is_symlink(),
                "conflict: merge source contains a symlink"
            );
            let name = entry.file_name().to_string_lossy().into_owned();
            let from = format!("{old}/{name}");
            let to = format!("{new}/{name}");
            if entry.file_type()?.is_dir() {
                self.collect_merge_files(&from, &to, files, false)?;
            } else if !top_level
                || !matches!(
                    name.as_str(),
                    "Board.md"
                        | "Project.md"
                        | "Tasks.base"
                        | "Issue.base"
                        | "Milestone.base"
                        | "Sync.md"
                )
            {
                files.entry(from).or_insert(to);
            }
        }
        Ok(())
    }
}

impl crate::Store {
    /// Durable ownership changes, retained for independent Memory database recovery.
    pub async fn project_merges(&self) -> Result<Vec<(String, String)>> {
        Ok(
            sqlx::query_as("SELECT source,target FROM project_merges ORDER BY source")
                .fetch_all(&self.pool)
                .await?,
        )
    }
}
