use std::{path::Path, sync::Arc, time::Duration};

use anyhow::{Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{
    Row, SqliteConnection, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

use crate::{Outcome, Snapshot, TaskEvent, WriteOptions, mutations, new_id};

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
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
            version <= 1,
            "unsupported task database schema version {version}"
        );
        sqlx::raw_sql(include_str!("schema.sql"))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn snapshot(&self) -> Result<Snapshot> {
        let mut tx = self.pool.begin().await?;
        let state = load(&mut tx).await?;
        tx.commit().await?;
        Ok(state)
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
        let before = load(&mut tx).await?;
        let mut state = before.clone();
        let result = mutations::apply(&mut state, &request, &options, self.now())?;
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
        let before = load(&mut tx).await?;
        let mut state = before.clone();
        let expired: Vec<_> = state
            .leases
            .iter()
            .filter(|l| l.lease_expires_at <= self.now())
            .map(|l| l.task_id.clone())
            .collect();
        for task in &expired {
            let i = state.task_index(task)?;
            mutations::system_block(&mut state, i, "lease expired", self.now());
        }
        if !expired.is_empty() {
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
        let job = if let Some(id) = job {
            let state = self.snapshot().await?;
            Some(state.jobs[state.job_index(id)?].id.clone())
        } else {
            None
        };
        let rows=sqlx::query("SELECT sequence,data FROM task_events WHERE sequence > ? AND (? IS NULL OR job_id = ?) ORDER BY sequence LIMIT ?").bind(after).bind(&job).bind(&job).bind(limit).fetch_all(&self.pool).await?;
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
        let value: Option<String> =
            sqlx::query_scalar("SELECT value FROM projection_state WHERE key = ?")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;
        value
            .map(|v| serde_json::from_str(&v).map_err(Into::into))
            .transpose()
    }
    pub async fn set_metadata(&self, key: &str, value: &Value) -> Result<()> {
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
}

async fn load(conn: &mut SqliteConnection) -> Result<Snapshot> {
    Ok(Snapshot {
        projects: read_entities(conn, "projects").await?,
        jobs: read_entities(conn, "jobs").await?,
        tasks: read_entities(conn, "tasks").await?,
        plans: read_entities(conn, "plans").await?,
        leases: read_entities(conn, "task_leases").await?,
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
    for project in &after.projects {
        if before.projects.iter().find(|p| p.id == project.id) == Some(project) {
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
        if before.jobs.iter().find(|j| j.id == job.id) == Some(job) {
            continue;
        }
        upsert(conn, "jobs", &job.id, job).await?;
        let event_type = if job.status == crate::JobStatus::Completed
            && before
                .jobs
                .iter()
                .any(|j| j.id == job.id && j.status != job.status)
        {
            "job.completed"
        } else {
            command
        };
        let related = after
            .tasks
            .iter()
            .filter(|t| t.job_id == job.id && t.last_session.is_some())
            .max_by_key(|t| t.updated_at);
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
        if before.tasks.iter().find(|t| t.id == task.id) == Some(task) {
            continue;
        }
        upsert(conn, "tasks", &task.id, task).await?;
        let changed_status = before
            .tasks
            .iter()
            .any(|t| t.id == task.id && t.status != task.status);
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
        if before.plans.iter().find(|p| p.id == plan.id) != Some(plan) {
            upsert(conn, "plans", &plan.id, plan).await?;
        }
    }
    if before.leases != after.leases {
        sqlx::query("DELETE FROM task_leases")
            .execute(&mut *conn)
            .await?;
        for lease in &after.leases {
            upsert(conn, "task_leases", &lease.task_id, lease).await?;
        }
    }
    for task in &after.tasks {
        if before
            .tasks
            .iter()
            .find(|t| t.id == task.id)
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

fn event(command: &str, options: &WriteOptions, now: i64) -> TaskEvent {
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

async fn append_event(conn: &mut SqliteConnection, event: TaskEvent) -> Result<()> {
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
