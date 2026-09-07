use std::{path::Path, sync::Arc, time::Duration};

use anyhow::{Context, Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{
    Row, SqliteConnection, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

use crate::{Outcome, Snapshot, TaskEvent, WriteOptions, mutations, new_id};

#[cfg(test)]
#[path = "persist_tests.rs"]
pub(crate) mod persist_tests;

const JOB_EVENTS_QUERY: &str = "SELECT sequence,data FROM task_events WHERE job_id = ? AND sequence > ? ORDER BY sequence LIMIT ?";

#[derive(Clone)]
pub struct Store {
    pub(crate) pool: SqlitePool,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        Self::open_with_clock(
            path,
            Arc::new(|| time::OffsetDateTime::now_utc().unix_timestamp()),
        )
        .await
    }

    pub async fn open_with_clock(
        path: &Path,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true)
                    .foreign_keys(true)
                    .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                    .busy_timeout(Duration::from_secs(10)),
            )
            .await?;
        let store = Self { pool, clock };
        store.migrate().await?;
        Ok(store)
    }

    #[must_use]
    pub fn now(&self) -> i64 {
        (self.clock)()
    }

    async fn migrate(&self) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let application_id: i64 = sqlx::query_scalar("PRAGMA application_id")
            .fetch_one(&mut *tx)
            .await?;
        let table_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'")
                .fetch_one(&mut *tx)
                .await?;
        ensure!(
            application_id == 0x4158_544b || (application_id == 0 && table_count == 0),
            "invalid: task database must be a dedicated taskcli database"
        );
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&mut *tx)
            .await?;
        ensure!(
            version <= 12,
            "unsupported task database schema version {version}"
        );
        sqlx::raw_sql(include_str!("schema.sql"))
            .execute(&mut *tx)
            .await?;
        if version == 1 {
            sqlx::query("UPDATE tasks SET data = json_set(data, '$.phase', CASE WHEN json_extract(data, '$.status') = 'IN_PROGRESS' THEN 'EXECUTING' ELSE NULL END)")
                .execute(&mut *tx)
                .await?;
        }
        if version > 0 && version < 3 {
            migrate_layout(&mut tx).await?;
        }
        if version > 0 && version < 4 {
            migrate_numbered_paths(&mut tx).await?;
        }
        if version > 0 && version < 5 {
            migrate_job_directories(&mut tx).await?;
        }
        if version > 0 && version < 7 {
            migrate_task_notes(&mut tx).await?;
        }
        if version > 0 && version < 10 {
            sqlx::query("UPDATE inbox_entries SET data = json_set(data, '$.status', CASE WHEN json_extract(data, '$.status') = 'DONE' THEN 'COMPLETED' WHEN EXISTS (SELECT 1 FROM jobs WHERE jobs.id = json_extract(inbox_entries.data, '$.job_id') AND json_extract(jobs.data, '$.status') = 'PENDING_REVIEW') THEN 'PENDING_REVIEW' ELSE 'ACTIVE' END, '$.revision', json_extract(data, '$.revision') + 1, '$.updated_at', ?) WHERE json_extract(data, '$.status') IN ('IN_PROGRESS', 'DONE')")
                .bind(self.now()).execute(&mut *tx).await?;
        }
        if version > 0 && version < 11 {
            sqlx::query("UPDATE inbox_entries SET data = json_set(data, '$.source', (SELECT substr(key, 10) FROM idempotency_keys WHERE key LIKE 'im:inbox:%' AND json_extract(result, '$.result.id') = inbox_entries.id LIMIT 1)) WHERE json_extract(data, '$.source') IS NULL")
                .execute(&mut *tx).await?;
            sqlx::query("UPDATE jobs SET data = json_set(data, '$.pending_review_at', COALESCE((SELECT json_extract(task_events.data, '$.occurred_at') FROM task_events WHERE task_events.job_id = jobs.id AND json_extract(task_events.data, '$.event_type') = 'job.pending_review' ORDER BY sequence DESC LIMIT 1), CASE WHEN json_extract(jobs.data, '$.status') = 'PENDING_REVIEW' THEN json_extract(jobs.data, '$.updated_at') END)) WHERE json_extract(data, '$.pending_review_at') IS NULL")
                .execute(&mut *tx).await?;
        }
        if version < 12 {
            sqlx::query("INSERT OR REPLACE INTO document_registry(key,path) SELECT json_each.key,json_each.value FROM projection_state,json_each(projection_state.value) WHERE projection_state.key='documents'")
                .execute(&mut *tx).await?;
            sqlx::query("DELETE FROM projection_state WHERE key='documents'")
                .execute(&mut *tx)
                .await?;
            // Upgrades explicitly rebuild once, including legacy path cleanup.
            sqlx::query(
                "INSERT OR REPLACE INTO pending_documents(key,generation) VALUES ('rebuild',?)",
            )
            .bind(new_id("publication"))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn snapshot(&self) -> Result<Snapshot> {
        let mut tx = self.pool.begin().await?;
        let state = load(&mut tx).await?;
        tx.commit().await?;
        Ok(state)
    }

    /// Read note metadata in entity order, excluding authored bodies and leases.
    pub(crate) async fn obsidian_records(&self) -> Result<Vec<(Value, String)>> {
        let mut tx = self.pool.begin().await?;
        let mut records = Vec::new();
        for (table, fields, filter) in [
            ("tasks", "'$.title'", "1"),
            (
                "jobs",
                "'$.title', '$.goal', '$.prompt', '$.conversation'",
                "1",
            ),
            (
                "inbox_entries",
                "'$.content', '$.lease', '$.source'",
                "json_extract(e.data, '$.published') = 1 AND json_extract(e.data, '$.deleted') IS NOT 1",
            ),
        ] {
            let rows = sqlx::query(&format!(
                "SELECT json_remove(e.data, {fields}) AS entity, json_extract(p.data, '$.key') AS project_key \
                 FROM {table} e LEFT JOIN projects p ON p.id = json_extract(e.data, '$.project_id') \
                 WHERE {filter} ORDER BY e.rowid"
            ))
            .fetch_all(&mut *tx)
            .await?;
            for row in rows {
                let entity = serde_json::from_str(&row.get::<String, _>("entity"))?;
                let key = row
                    .get::<Option<String>, _>("project_key")
                    .context("missing note Project")?;
                records.push((entity, key));
            }
        }
        tx.commit().await?;
        Ok(records)
    }

    /// Read one Task and its lease in the same transaction. Prefix resolution
    /// uses the primary-key index and reads at most two candidate identifiers.
    pub async fn task_result(&self, id: &str) -> Result<Value> {
        ensure!(!id.is_empty(), "invalid: empty identifier");
        let mut tx = self.pool.begin().await?;
        let mut data: Option<String> = sqlx::query_scalar("SELECT data FROM tasks WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        if data.is_none() {
            let candidates: Vec<String> = sqlx::query_scalar(
                "SELECT id FROM tasks WHERE id >= ? AND id < ? ORDER BY id LIMIT 2",
            )
            .bind(id)
            .bind(format!("{id}\u{10ffff}"))
            .fetch_all(&mut *tx)
            .await?;
            ensure!(!candidates.is_empty(), "not_found: {id}");
            ensure!(candidates.len() == 1, "ambiguous identifier: {id}");
            data = sqlx::query_scalar("SELECT data FROM tasks WHERE id = ?")
                .bind(&candidates[0])
                .fetch_optional(&mut *tx)
                .await?;
        }
        let task: crate::Task = serde_json::from_str(&data.context("not_found: task")?)?;
        let lease: Option<String> = sqlx::query_scalar("SELECT data FROM task_leases WHERE id = ?")
            .bind(&task.id)
            .fetch_optional(&mut *tx)
            .await?;
        let lease: Option<crate::Lease> = lease.map(|s| serde_json::from_str(&s)).transpose()?;
        let mut result = serde_json::to_value(task)?;
        result["lease"] = serde_json::to_value(lease)?;
        tx.commit().await?;
        Ok(result)
    }

    /// Fetch just the requested entity and its project key, without authored
    /// content or credentials. Both reads use existing primary keys.
    pub(crate) async fn obsidian_record(&self, id: &str) -> Result<Option<(Value, String)>> {
        let query = match id.split_once('_').map(|(kind, _)| kind) {
            Some("task") => "SELECT json_remove(data, '$.title') FROM tasks WHERE id = ?",
            Some("job") => {
                "SELECT json_remove(data, '$.title', '$.goal', '$.prompt', '$.conversation') FROM jobs WHERE id = ?"
            }
            Some("inbox") => {
                "SELECT json_remove(data, '$.content', '$.lease', '$.source') FROM inbox_entries WHERE id = ?"
            }
            _ => anyhow::bail!("invalid: expected a task, job, or inbox ID"),
        };
        let mut tx = self.pool.begin().await?;
        let data: Option<String> = sqlx::query_scalar(query)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(data) = data else {
            return Ok(None);
        };
        let entity: Value = serde_json::from_str(&data)?;
        if id.starts_with("inbox_") && (entity["deleted"] == true || entity["published"] != true) {
            return Ok(None);
        }
        let project = entity["project_id"]
            .as_str()
            .context("missing project ID")?;
        let key: String =
            sqlx::query_scalar("SELECT json_extract(data, '$.key') FROM projects WHERE id = ?")
                .bind(project)
                .fetch_one(&mut *tx)
                .await?;
        tx.commit().await?;
        Ok(Some((entity, key)))
    }

    pub async fn execute(&self, request: Value, options: WriteOptions) -> Result<Outcome> {
        self.reap_expired().await?;
        self.execute_as(request.clone(), options, request).await
    }

    pub(crate) async fn replay(
        &self,
        request: &Value,
        options: &WriteOptions,
    ) -> Result<Option<Outcome>> {
        let Some(key) = &options.idempotency_key else {
            return Ok(None);
        };
        let fingerprint = hash_bytes(
            serde_json::to_string(&json!({"request":request,"options":options}))?.as_bytes(),
        );
        if let Some(row) =
            sqlx::query("SELECT fingerprint,result FROM idempotency_keys WHERE key = ?")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?
        {
            ensure!(
                row.get::<String, _>("fingerprint") == fingerprint,
                "conflict: idempotency key reused with different input"
            );
            return Ok(Some(serde_json::from_str(&row.get::<String, _>("result"))?));
        }
        Ok(None)
    }

    pub(crate) async fn execute_as(
        &self,
        request: Value,
        options: WriteOptions,
        source: Value,
    ) -> Result<Outcome> {
        let command = mutations::required(&request, "command")?.to_owned();
        let fingerprint = hash_bytes(
            serde_json::to_string(&json!({"request":source,"options":options}))?.as_bytes(),
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(key) = &options.idempotency_key {
            ensure!(!key.trim().is_empty(), "invalid: empty idempotency key");
            if let Some(row) =
                sqlx::query("SELECT fingerprint, result FROM idempotency_keys WHERE key = ?")
                    .bind(key)
                    .fetch_optional(&mut *tx)
                    .await?
            {
                ensure!(
                    row.get::<String, _>("fingerprint") == fingerprint,
                    "conflict: idempotency key reused with different input"
                );
                return Ok(serde_json::from_str(&row.get::<String, _>("result"))?);
            }
        }
        let before = crate::scoped::load_request(&mut tx, &request, self.now()).await?;
        let mut state = before.clone();
        let result = mutations::apply(&mut state, &request, &options, self.now())?;
        crate::inbox::refresh(&mut state, self.now());
        crate::deletion::check_pending_paths(&mut tx, &before, &state).await?;
        persist(&mut tx, &before, &state, &command, &options, self.now()).await?;
        let sequence = max_sequence(&mut tx).await?;
        let outcome = Outcome {
            result,
            sequence,
            projection_pending: None,
        };
        if let Some(key) = &options.idempotency_key {
            sqlx::query("INSERT INTO idempotency_keys(key,fingerprint,result) VALUES (?,?,?)")
                .bind(key)
                .bind(fingerprint)
                .bind(serde_json::to_string(&outcome)?)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn reap_expired(&self) -> Result<usize> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let expired = crate::scoped::ids(&mut tx, "SELECT id FROM task_leases WHERE json_extract(data,'$.lease_expires_at')<=CAST(? AS INTEGER)", &self.now().to_string()).await?;
        let inboxes = crate::scoped::ids(&mut tx, "SELECT id FROM inbox_entries WHERE json_extract(data,'$.lease.lease_expires_at')<=CAST(? AS INTEGER)", &self.now().to_string()).await?;
        if expired.is_empty() && inboxes.is_empty() {
            return Ok(0);
        }
        let mut scope = crate::scoped::Scope {
            tasks: expired.clone(),
            inboxes,
            ..Default::default()
        };
        scope.parents(&mut tx).await?;
        scope.job_tasks(&mut tx).await?;
        scope.inboxes.extend(crate::scoped::ids(&mut tx,"SELECT id FROM inbox_entries WHERE json_extract(data,'$.job_id') IN (SELECT value FROM json_each(?))", &json!(scope.jobs).to_string()).await?);
        let before = scope.load(&mut tx).await?;
        let mut state = before.clone();
        for task in &expired {
            let i = state.task_index(task)?;
            mutations::system_block(&mut state, i, "lease expired", self.now());
        }
        crate::inbox::refresh(&mut state, self.now());
        if before != state {
            persist(
                &mut tx,
                &before,
                &state,
                "lease.expired",
                &WriteOptions {
                    actor_ref: "system:lease".into(),
                    ..WriteOptions::default()
                },
                self.now(),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(expired.len())
    }

    pub async fn events(
        &self,
        job: Option<&str>,
        after: i64,
        limit: i64,
    ) -> Result<Vec<TaskEvent>> {
        ensure!(
            after >= 0 && (1..=1000).contains(&limit),
            "invalid: event cursor or limit"
        );
        let rows = if let Some(id) = job {
            let mut tx = self.pool.begin().await?;
            let id = crate::scoped::resolve(&mut tx, "jobs", id).await?;
            let rows = sqlx::query(JOB_EVENTS_QUERY)
                .bind(id)
                .bind(after)
                .bind(limit)
                .fetch_all(&mut *tx)
                .await?;
            tx.commit().await?;
            rows
        } else {
            sqlx::query(
                "SELECT sequence,data FROM task_events WHERE sequence > ? ORDER BY sequence LIMIT ?",
            )
            .bind(after)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };
        rows.into_iter()
            .map(|row| {
                let mut event: TaskEvent = serde_json::from_str(&row.get::<String, _>("data"))?;
                event.sequence = row.get("sequence");
                Ok(event)
            })
            .collect()
    }

    pub async fn latest_sequence(&self) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT COALESCE(MAX(sequence),0) FROM task_events")
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn metadata(&self, key: &str) -> Result<Option<Value>> {
        if key == "documents" {
            return Ok(Some(serde_json::to_value(
                self.document_paths(None).await?,
            )?));
        }
        let value: Option<String> =
            sqlx::query_scalar("SELECT value FROM projection_state WHERE key = ?")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;
        value
            .map(|v| serde_json::from_str(&v).map_err(Into::into))
            .transpose()
    }
    pub(crate) async fn metadata_batch(
        &self,
        keys: &std::collections::BTreeSet<String>,
    ) -> Result<std::collections::BTreeMap<String, Value>> {
        if keys.is_empty() {
            return Ok(std::collections::BTreeMap::new());
        }
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT key,value FROM projection_state WHERE key IN (SELECT value FROM json_each(?))",
        )
        .bind(serde_json::to_string(keys)?)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(key, value)| Ok((key, serde_json::from_str(&value)?)))
            .collect()
    }

    pub async fn set_metadata(&self, key: &str, value: &Value) -> Result<()> {
        if key == "documents" {
            let paths: std::collections::BTreeMap<String, String> =
                serde_json::from_value(value.clone())?;
            let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
            sqlx::query("DELETE FROM document_registry")
                .execute(&mut *tx)
                .await?;
            for (key, path) in paths {
                sqlx::query("INSERT INTO document_registry(key,path) VALUES (?,?)")
                    .bind(key)
                    .bind(path)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
            return Ok(());
        }
        sqlx::query("INSERT INTO projection_state(key,value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value").bind(key).bind(serde_json::to_string(value)?).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn update_plan_hash(&self, id: &str, hash: &str) -> Result<()> {
        sqlx::query("UPDATE plans SET data = json_set(data, '$.hash', ?) WHERE id = ?")
            .bind(hash)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub(crate) async fn pending_deletions(&self) -> Result<Vec<crate::deletion::Cleanup>> {
        let rows: Vec<String> =
            sqlx::query_scalar("SELECT data FROM document_deletions ORDER BY rowid")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter()
            .map(|row| serde_json::from_str(&row).map_err(Into::into))
            .collect()
    }

    pub(crate) async fn finish_deletion(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM document_deletions WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

async fn migrate_task_notes(conn: &mut SqliteConnection) -> Result<()> {
    let state = load(conn).await?;
    let old: Option<String> =
        sqlx::query_scalar("SELECT value FROM projection_state WHERE key = 'documents'")
            .fetch_optional(&mut *conn)
            .await?;
    let mut documents: Value = old
        .map(|v| serde_json::from_str(&v))
        .transpose()?
        .unwrap_or_else(|| json!({}));
    for plan in &state.plans {
        let task = &state.tasks[state.task_index(&plan.task_id)?];
        let key = format!("plan:{}", plan.id);
        if documents.get(&key).is_none() {
            documents[&key] = json!(plan.path);
        }
        let mut migrated = plan.clone();
        migrated.path = crate::naming::task_path(&state, task)?;
        sqlx::query("UPDATE plans SET data=? WHERE id=?")
            .bind(serde_json::to_string(&migrated)?)
            .bind(&plan.id)
            .execute(&mut *conn)
            .await?;
    }
    sqlx::query("INSERT INTO projection_state(key,value) VALUES ('documents',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value")
        .bind(documents.to_string()).execute(&mut *conn).await?;
    Ok(())
}

/// Keep legacy locations until projection acknowledges all new files. A failed
/// or interrupted migration can then retry without discarding editable content.
#[allow(clippy::too_many_lines)] // All entity paths and recovery locations migrate in one transaction.
async fn migrate_layout(conn: &mut SqliteConnection) -> Result<()> {
    use crate::naming::unique_name;
    let mut state = load(conn).await?;
    let old: Option<String> =
        sqlx::query_scalar("SELECT value FROM projection_state WHERE key = 'documents'")
            .fetch_optional(&mut *conn)
            .await?;
    let mut documents: Value = old
        .map(|v| serde_json::from_str(&v))
        .transpose()?
        .unwrap_or_else(|| json!({}));
    for i in 0..state.projects.len() {
        let project = &state.projects[i];
        for (kind, name) in [
            ("board", "Board.md"),
            ("tasks", "Tasks.md"),
            ("sync", "Sync Status.md"),
        ] {
            let key = format!("{kind}:{}", project.id);
            if documents.get(&key).is_none() {
                documents[key] = json!(format!("Projects/{}/{name}", project.key));
            }
        }
        state.projects[i].key = unique_name(
            &project.name,
            state.projects[..i].iter().map(|p| p.key.as_str()),
        );
        let project = &mut state.projects[i];
        project.name.clone_from(&project.key);
    }
    for i in 0..state.jobs.len() {
        let job = &state.jobs[i];
        let key = format!("job:{}", job.id);
        if documents.get(&key).is_none() {
            documents[key] = json!(job.document_path);
        }
        let project = &state.projects[state.project_index(&job.project_id)?];
        let name = unique_name(
            &job.title,
            state.jobs[..i]
                .iter()
                .filter(|j| j.project_id == job.project_id)
                .map(|j| j.name.as_str()),
        );
        let folder = job
            .document_path
            .split("/Jobs/")
            .nth(1)
            .and_then(|p| p.rsplit_once('/').map(|(folder, _)| folder))
            .unwrap_or("Active");
        state.jobs[i].document_path = format!("Projects/{}/Jobs/{folder}/{name}.md", project.key);
        state.jobs[i].name = name;
        state.jobs[i].started_at = state
            .tasks
            .iter()
            .filter(|t| t.job_id == state.jobs[i].id)
            .filter_map(|t| t.started_at)
            .min();
    }
    for i in 0..state.tasks.len() {
        let task = &state.tasks[i];
        state.tasks[i].name = unique_name(
            &task.title,
            state.tasks[..i]
                .iter()
                .filter(|t| t.project_id == task.project_id)
                .map(|t| t.name.as_str()),
        );
        if state.tasks[i].status.terminal() {
            state.tasks[i].completed_at = Some(state.tasks[i].updated_at);
        }
    }
    let old_plans = state.plans.clone();
    for plan in &old_plans {
        documents[format!("plan:{}", plan.id)] = json!(plan.path);
    }
    state.plans.retain(|p| {
        state
            .tasks
            .iter()
            .any(|t| t.current_plan.as_ref() == Some(&p.id))
    });
    for plan in &mut state.plans {
        let task = state.tasks.iter().find(|t| t.id == plan.task_id).unwrap();
        let project = state
            .projects
            .iter()
            .find(|p| p.id == task.project_id)
            .unwrap();
        plan.path = format!("Projects/{}/Plans/{}.md", project.key, task.name);
        plan.updated_at = plan.created_at;
        plan.created_at = old_plans
            .iter()
            .filter(|p| p.task_id == task.id)
            .map(|p| p.created_at)
            .min()
            .unwrap_or(plan.created_at);
    }
    for project in &state.projects {
        upsert(conn, "projects", &project.id, project).await?;
    }
    for job in &state.jobs {
        upsert(conn, "jobs", &job.id, job).await?;
    }
    for task in &state.tasks {
        upsert(conn, "tasks", &task.id, task).await?;
    }
    sqlx::query("DELETE FROM plans").execute(&mut *conn).await?;
    for plan in &state.plans {
        upsert(conn, "plans", &plan.id, plan).await?;
    }
    sqlx::query("INSERT INTO projection_state(key,value) VALUES ('documents',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value").bind(documents.to_string()).execute(&mut *conn).await?;
    Ok(())
}

async fn migrate_numbered_paths(conn: &mut SqliteConnection) -> Result<()> {
    use crate::naming::{next_sequence, numbered_name};

    let mut state = load(conn).await?;
    let old: Option<String> =
        sqlx::query_scalar("SELECT value FROM projection_state WHERE key = 'documents'")
            .fetch_optional(&mut *conn)
            .await?;
    let mut documents: Value = old
        .map(|v| serde_json::from_str(&v))
        .transpose()?
        .unwrap_or_else(|| json!({}));
    state
        .jobs
        .sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    state
        .tasks
        .sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    for i in 0..state.jobs.len() {
        let job = &state.jobs[i];
        let sequence = next_sequence(
            job.created_at,
            state.jobs[..i]
                .iter()
                .filter(|j| j.project_id == job.project_id)
                .map(|j| (j.created_at, j.sequence)),
        )?;
        let job = &mut state.jobs[i];
        let key = format!("job:{}", job.id);
        if documents.get(&key).is_none() {
            documents[key] = json!(job.document_path);
        }
        job.sequence = sequence;
        let parent = job
            .document_path
            .rsplit_once('/')
            .context("invalid Job path")?
            .0;
        let filename = numbered_name(&job.name, job.created_at, job.sequence)?;
        job.document_path = format!("{parent}/{filename}.md");
        upsert(conn, "jobs", &job.id, job).await?;
    }
    for i in 0..state.tasks.len() {
        let task = &state.tasks[i];
        let sequence = next_sequence(
            task.created_at,
            state.tasks[..i]
                .iter()
                .filter(|t| t.project_id == task.project_id)
                .map(|t| (t.created_at, t.sequence)),
        )?;
        let task = &mut state.tasks[i];
        task.sequence = sequence;
        upsert(conn, "tasks", &task.id, task).await?;
    }
    for plan in &mut state.plans {
        let task = state
            .tasks
            .iter()
            .find(|t| t.id == plan.task_id)
            .context("missing Plan task")?;
        let key = format!("plan:{}", plan.id);
        if documents.get(&key).is_none() {
            documents[key] = json!(plan.path);
        }
        let parent = plan.path.rsplit_once('/').context("invalid Plan path")?.0;
        let filename = numbered_name(&task.name, task.created_at, task.sequence)?;
        plan.path = format!("{parent}/{filename}.md");
        upsert(conn, "plans", &plan.id, plan).await?;
    }
    sqlx::query("INSERT INTO projection_state(key,value) VALUES ('documents',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value").bind(documents.to_string()).execute(&mut *conn).await?;
    Ok(())
}

async fn migrate_job_directories(conn: &mut SqliteConnection) -> Result<()> {
    let state = load(conn).await?;
    let old: Option<String> =
        sqlx::query_scalar("SELECT value FROM projection_state WHERE key = 'documents'")
            .fetch_optional(&mut *conn)
            .await?;
    let mut documents: Value = old
        .map(|v| serde_json::from_str(&v))
        .transpose()?
        .unwrap_or_else(|| json!({}));
    for original in &state.jobs {
        let mut job = original.clone();
        let key = format!("job:{}", job.id);
        if documents.get(&key).is_none() {
            documents[key] = json!(job.document_path);
        }
        let project = &state.projects[state.project_index(&job.project_id)?];
        let filename = crate::naming::numbered_name(&job.name, job.created_at, job.sequence)?;
        let folder = if job.archived_at.is_some() {
            "Archived/"
        } else {
            ""
        };
        job.document_path = format!("Projects/{}/Jobs/{folder}{filename}.md", project.key);
        upsert(conn, "jobs", &job.id, &job).await?;
    }
    sqlx::query("INSERT INTO projection_state(key,value) VALUES ('documents',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value").bind(documents.to_string()).execute(&mut *conn).await?;
    Ok(())
}

async fn load(conn: &mut SqliteConnection) -> Result<Snapshot> {
    let sequences: Option<String> =
        sqlx::query_scalar("SELECT value FROM projection_state WHERE key = 'document_sequences'")
            .fetch_optional(&mut *conn)
            .await?;
    Ok(Snapshot {
        inboxes: read_entities(conn, "inbox_entries").await?,
        document_sequences: sequences
            .map(|v| serde_json::from_str(&v))
            .transpose()?
            .unwrap_or_default(),
        projects: read_entities(conn, "projects").await?,
        jobs: read_entities(conn, "jobs").await?,
        tasks: read_entities(conn, "tasks").await?,
        plans: read_entities(conn, "plans").await?,
        leases: read_entities(conn, "task_leases").await?,
        ..Snapshot::default()
    })
}

async fn read_entities<T: DeserializeOwned>(
    conn: &mut SqliteConnection,
    table: &str,
) -> Result<Vec<T>> {
    let rows: Vec<String> = sqlx::query_scalar(&format!("SELECT data FROM {table} ORDER BY rowid"))
        .fetch_all(conn)
        .await?;
    rows.into_iter()
        .map(|row| serde_json::from_str(&row).map_err(Into::into))
        .collect()
}

#[allow(clippy::too_many_lines)] // Entity writes and their audit events share a transaction.
async fn persist(
    conn: &mut SqliteConnection,
    before: &Snapshot,
    after: &Snapshot,
    command: &str,
    options: &WriteOptions,
    now: i64,
) -> Result<()> {
    let old = crate::state_index::StateIndex::new(before);
    let new = crate::state_index::StateIndex::new(after);
    let recent = crate::state_index::recent_session_tasks(after);
    for project in &after.projects {
        if old.projects.get(project.id.as_str()).copied() == Some(project) {
            continue;
        }
        upsert(conn, "projects", &project.id, project).await?;
        append_event(
            conn,
            TaskEvent {
                project_id: Some(project.id.clone()),
                revision: project.revision,
                payload: serde_json::to_value(project)?,
                ..event(command, options, now)
            },
        )
        .await?;
    }
    for job in &after.jobs {
        if old.jobs.get(job.id.as_str()).copied() == Some(job) {
            continue;
        }
        upsert(conn, "jobs", &job.id, job).await?;
        let event_type = if old
            .jobs
            .get(job.id.as_str())
            .is_some_and(|j| j.status != job.status)
        {
            match job.status {
                crate::JobStatus::Completed => "job.completed",
                crate::JobStatus::PendingReview => "job.pending_review",
                crate::JobStatus::Active
                    if command == "job.reject"
                        || (command == "inbox.set-status"
                            && old.jobs.get(job.id.as_str()).is_some_and(|job| {
                                job.status == crate::JobStatus::PendingReview
                            })) =>
                {
                    "job.rejected"
                }
                _ => command,
            }
        } else {
            command
        };
        let related = recent.get(job.id.as_str()).copied();
        append_event(
            conn,
            TaskEvent {
                project_id: Some(job.project_id.clone()),
                job_id: Some(job.id.clone()),
                revision: job.revision,
                session_ref: options
                    .session_ref
                    .clone()
                    .or_else(|| related.and_then(|t| t.last_session.clone())),
                payload: serde_json::to_value(job)?,
                ..event(event_type, options, now)
            },
        )
        .await?;
    }
    for task in &after.tasks {
        if old.tasks.get(task.id.as_str()).copied() == Some(task) {
            continue;
        }
        upsert(conn, "tasks", &task.id, task).await?;
        let changed_status = old
            .tasks
            .get(task.id.as_str())
            .is_some_and(|t| t.status != task.status);
        let event_type = if changed_status {
            format!("task.{}", task.status.to_string().to_lowercase())
        } else {
            command.into()
        };
        append_event(
            conn,
            TaskEvent {
                project_id: Some(task.project_id.clone()),
                job_id: Some(task.job_id.clone()),
                task_id: Some(task.id.clone()),
                revision: task.revision,
                session_ref: options
                    .session_ref
                    .clone()
                    .or_else(|| task.last_session.clone()),
                delegated_by: task
                    .delegated_by
                    .clone()
                    .or_else(|| options.delegated_by.clone()),
                payload: serde_json::to_value(task)?,
                ..event(&event_type, options, now)
            },
        )
        .await?;
    }
    for plan in &after.plans {
        if old.plans.get(plan.id.as_str()).copied() != Some(plan) {
            upsert(conn, "plans", &plan.id, plan).await?;
        }
    }
    for lease in &before.leases {
        if !new.leases.contains_key(lease.task_id.as_str()) {
            sqlx::query("DELETE FROM task_leases WHERE id=?")
                .bind(&lease.task_id)
                .execute(&mut *conn)
                .await?;
        }
    }
    for lease in &after.leases {
        if old.leases.get(lease.task_id.as_str()).copied() != Some(lease) {
            upsert(conn, "task_leases", &lease.task_id, lease).await?;
        }
    }
    for task in &after.tasks {
        if old
            .tasks
            .get(task.id.as_str())
            .copied()
            .is_some_and(|t| t.dependencies == task.dependencies)
        {
            continue;
        }
        sqlx::query("DELETE FROM task_dependencies WHERE task_id = ?")
            .bind(&task.id)
            .execute(&mut *conn)
            .await?;
        for dependency in &task.dependencies {
            sqlx::query("INSERT INTO task_dependencies(task_id,dependency_id) VALUES (?,?)")
                .bind(&task.id)
                .bind(dependency)
                .execute(&mut *conn)
                .await?;
        }
    }
    for entry in &after.inboxes {
        if old.inboxes.get(entry.id.as_str()).copied() == Some(entry) {
            continue;
        }
        upsert(conn, "inbox_entries", &entry.id, entry).await?;
        append_event(
            conn,
            TaskEvent {
                project_id: Some(entry.project_id.clone()),
                job_id: entry.job_id.clone(),
                revision: entry.revision,
                payload: serde_json::to_value(entry)?,
                ..event(command, options, now)
            },
        )
        .await?;
    }
    for entry in &before.inboxes {
        if !new.inboxes.contains_key(entry.id.as_str()) {
            sqlx::query("DELETE FROM inbox_entries WHERE id = ?")
                .bind(&entry.id)
                .execute(&mut *conn)
                .await?;
        }
    }
    crate::deletion::persist(conn, before, (&old, &new), command, options, now).await?;
    crate::publication::enqueue_changes(conn, before, after, command).await?;
    if before.document_sequences != after.document_sequences {
        sqlx::query("INSERT INTO projection_state(key,value) VALUES ('document_sequences',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value")
            .bind(serde_json::to_string(&after.document_sequences)?).execute(&mut *conn).await?;
    }
    Ok(())
}

async fn upsert(
    conn: &mut SqliteConnection,
    table: &str,
    id: &str,
    data: &impl Serialize,
) -> Result<()> {
    sqlx::query(&format!(
        "INSERT INTO {table}(id,data) VALUES (?,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data"
    ))
    .bind(id)
    .bind(serde_json::to_string(data)?)
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) fn event(command: &str, options: &WriteOptions, now: i64) -> TaskEvent {
    TaskEvent {
        sequence: 0,
        event_id: new_id("evt"),
        project_id: None,
        job_id: None,
        task_id: None,
        actor_ref: options.actor_ref.clone(),
        session_ref: options.session_ref.clone(),
        delegated_by: options.delegated_by.clone(),
        event_type: command.into(),
        revision: 0,
        occurred_at: now,
        payload: Value::Null,
    }
}

pub(crate) async fn append_event(conn: &mut SqliteConnection, event: TaskEvent) -> Result<()> {
    sqlx::query("INSERT INTO task_events(event_id,job_id,data) VALUES (?,?,?)")
        .bind(&event.event_id)
        .bind(&event.job_id)
        .bind(serde_json::to_string(&event)?)
        .execute(conn)
        .await?;
    Ok(())
}

async fn max_sequence(conn: &mut SqliteConnection) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COALESCE(MAX(sequence),0) FROM task_events")
            .fetch_one(conn)
            .await?,
    )
}

pub(crate) fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn job_event_pages_use_the_job_sequence_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("tasks.sqlite3"))
            .await
            .unwrap();
        let rows = sqlx::query(&format!("EXPLAIN QUERY PLAN {JOB_EVENTS_QUERY}"))
            .bind("job_target")
            .bind(100)
            .bind(10)
            .fetch_all(&store.pool)
            .await
            .unwrap();
        let details: Vec<String> = rows.iter().map(|row| row.get("detail")).collect();
        assert!(
            details.iter().any(|detail| {
                detail.contains("events_by_job") && detail.contains("job_id=? AND sequence>?")
            }),
            "Job pages must seek by Job and cursor: {details:?}"
        );
        assert!(
            details.iter().all(|detail| !detail.contains("TEMP B-TREE")),
            "Job pages must stream in index order: {details:?}"
        );
    }
}
