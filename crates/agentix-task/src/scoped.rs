//! Indexed reads for a mutation's relationship scope. The existing state machine
//! still owns authorization and lifecycle rules; absent unrelated rows must never
//! be interpreted as deletions, so the same scope is used for both sides of diff.
use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sqlx::{Row, SqliteConnection};

use crate::{Snapshot, Store, mutations::required};

/// Optional Job predicates, applied before loading authored content.
/// Timestamp lower bounds are inclusive; upper bounds are exclusive.
#[derive(Debug, Default)]
pub struct JobFilter {
    pub issue_id: Option<String>,
    pub unassociated: bool,
    pub status: Option<crate::JobStatus>,
    pub archived: Option<bool>,
    pub created_from: Option<i64>,
    pub created_before: Option<i64>,
    pub archived_from: Option<i64>,
    pub archived_before: Option<i64>,
}

const SESSION_PROJECTS_QUERY: &str = "SELECT DISTINCT project_id FROM (
    SELECT json_extract(data,'$.project_id') AS project_id FROM tasks WHERE json_extract(data,'$.last_session')=?1
    UNION ALL SELECT project_id FROM jobs WHERE json_extract(data,'$.session_id')=?1
) LIMIT 2";

pub(crate) async fn ids(
    conn: &mut SqliteConnection,
    query: &str,
    value: &str,
) -> Result<BTreeSet<String>> {
    Ok(sqlx::query_scalar::<_, String>(query)
        .bind(value)
        .fetch_all(conn)
        .await?
        .into_iter()
        .collect())
}

pub(crate) async fn resolve(conn: &mut SqliteConnection, table: &str, id: &str) -> Result<String> {
    ensure!(!id.is_empty(), "invalid: empty identifier");
    if let Some(exact) =
        sqlx::query_scalar::<_, String>(&format!("SELECT id FROM {table} WHERE id=?"))
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
    {
        return Ok(exact);
    }
    // Never resolve a prefix against a subset of the database.
    let rows: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT id FROM {table} WHERE id>=? AND id<? ORDER BY id LIMIT 2"
    ))
    .bind(id)
    .bind(format!("{id}\u{10ffff}"))
    .fetch_all(&mut *conn)
    .await?;
    let mut rows: BTreeSet<_> = rows.into_iter().collect();
    if table == "projects" {
        rows.extend(
            ids(
                conn,
                "SELECT id FROM projects WHERE json_extract(data,'$.name')=? LIMIT 2",
                id,
            )
            .await?,
        );
    }
    ensure!(!rows.is_empty(), "not_found: {id}");
    ensure!(rows.len() == 1, "ambiguous identifier: {id}");
    Ok(rows.into_iter().next().unwrap())
}

pub(crate) async fn entities<T: DeserializeOwned>(
    conn: &mut SqliteConnection,
    table: &str,
    selected: &BTreeSet<String>,
) -> Result<Vec<T>> {
    let rows: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT data FROM {table} WHERE id IN (SELECT value FROM json_each(?)) ORDER BY rowid"
    ))
    .bind(serde_json::to_string(selected)?)
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|v| crate::stored_paths::from_str(&v))
        .collect()
}

#[derive(Default)]
pub(crate) struct Scope {
    pub projects: BTreeSet<String>,
    pub jobs: BTreeSet<String>,
    pub tasks: BTreeSet<String>,
    pub issues: BTreeSet<String>,
    pub milestones: BTreeSet<String>,
    pub extra_leases: BTreeSet<String>,
}

impl Scope {
    pub async fn job_tasks(&mut self, conn: &mut SqliteConnection) -> Result<()> {
        self.tasks.extend(
            ids(
                conn,
                "SELECT id FROM tasks WHERE job_id IN (SELECT value FROM json_each(?))",
                &json!(self.jobs).to_string(),
            )
            .await?,
        );
        Ok(())
    }

    pub async fn parents(&mut self, conn: &mut SqliteConnection) -> Result<()> {
        self.jobs.extend(
            ids(
                conn,
                "SELECT job_id FROM tasks WHERE id IN (SELECT value FROM json_each(?))",
                &json!(self.tasks).to_string(),
            )
            .await?,
        );
        self.issues.extend(ids(conn, "SELECT json_extract(data,'$.issue_id') FROM jobs WHERE id IN (SELECT value FROM json_each(?)) AND json_extract(data,'$.issue_id') IS NOT NULL", &json!(self.jobs).to_string()).await?);
        self.milestones.extend(ids(conn, "SELECT json_extract(data,'$.milestone_id') FROM issues WHERE id IN (SELECT value FROM json_each(?)) AND json_extract(data,'$.milestone_id') IS NOT NULL", &json!(self.issues).to_string()).await?);
        for (table, selected) in [("issues", &self.issues), ("milestones", &self.milestones)] {
            self.projects.extend(ids(conn, &format!("SELECT project_id FROM {table} WHERE id IN (SELECT value FROM json_each(?))"), &json!(selected).to_string()).await?);
        }
        self.projects.extend(
            ids(
                conn,
                "SELECT project_id FROM jobs WHERE id IN (SELECT value FROM json_each(?))",
                &json!(self.jobs).to_string(),
            )
            .await?,
        );
        Ok(())
    }

    pub async fn dependencies(&mut self, conn: &mut SqliteConnection, reverse: bool) -> Result<()> {
        let query = if reverse {
            "WITH RECURSIVE related(id) AS (SELECT value FROM json_each(?) UNION SELECT task_id FROM task_dependencies JOIN related ON dependency_id=related.id) SELECT id FROM related"
        } else {
            "WITH RECURSIVE related(id) AS (SELECT value FROM json_each(?) UNION SELECT dependency_id FROM task_dependencies JOIN related ON task_id=related.id) SELECT id FROM related"
        };
        self.tasks
            .extend(ids(conn, query, &json!(self.tasks).to_string()).await?);
        self.parents(conn).await
    }

    pub async fn load(&self, conn: &mut SqliteConnection) -> Result<Snapshot> {
        let plans = ids(
            conn,
            "SELECT id FROM plans WHERE task_id IN (SELECT value FROM json_each(?))",
            &json!(self.tasks).to_string(),
        )
        .await?;
        let mut leases = self.tasks.clone();
        leases.extend(self.extra_leases.iter().cloned());
        Ok(Snapshot {
            projects: entities(conn, "projects", &self.projects).await?,
            jobs: entities(conn, "jobs", &self.jobs).await?,
            tasks: entities(conn, "tasks", &self.tasks).await?,
            issues: entities(conn, "issues", &self.issues).await?,
            milestones: entities(conn, "milestones", &self.milestones).await?,
            plans: entities(conn, "plans", &plans).await?,
            leases: entities(conn, "task_leases", &leases).await?,
            ..Snapshot::default()
        })
    }
}

#[allow(clippy::too_many_lines)] // Keep the command-to-relationship boundary auditable in one dispatch.
pub(crate) async fn request_scope(conn: &mut SqliteConnection, request: &Value) -> Result<Scope> {
    let command = required(request, "command")?;
    let mut scope = Scope::default();
    for (field, table) in [
        ("project", "projects"),
        ("job", "jobs"),
        ("task", "tasks"),
        ("dependency", "tasks"),
        ("issue", "issues"),
        ("milestone", "milestones"),
    ] {
        if let Some(id) = request[field].as_str() {
            let id = resolve(conn, table, id).await?;
            match field {
                "project" => {
                    scope.projects.insert(id);
                }
                "job" => {
                    scope.jobs.insert(id);
                }
                "issue" => {
                    scope.issues.insert(id);
                }
                "milestone" => {
                    scope.milestones.insert(id);
                }
                _ => {
                    scope.tasks.insert(id);
                }
            }
        }
    }
    if command == "project.register" {
        scope.projects = ids(
            conn,
            crate::project_lookup::BY_ROOT,
            &crate::project_lookup::canonical_root(required(request, "root")?)?,
        )
        .await?;
    }
    if command.starts_with("session.") {
        session_scope(conn, &mut scope, request).await?;
    }
    scope.parents(conn).await?;
    let project_wide = matches!(
        command,
        "project.archive"
            | "project.unarchive"
            | "project.delete"
            | "project.rename"
            | "project.relocate"
    );
    if project_wide {
        for (table, selected) in [
            ("issues", &mut scope.issues),
            ("milestones", &mut scope.milestones),
        ] {
            selected.extend(ids(conn,&format!("SELECT id FROM {table} WHERE project_id IN (SELECT value FROM json_each(?))"),&json!(scope.projects).to_string()).await?);
        }
    }
    if command == "issue.cancel" {
        scope.jobs.extend(ids(conn,"SELECT id FROM jobs WHERE json_extract(data,'$.issue_id') IN (SELECT value FROM json_each(?))",&json!(scope.issues).to_string()).await?);
    }
    if command == "milestone.delete"
        || (command == "milestone.update" && request.get("name").is_some())
    {
        scope.issues.extend(ids(conn,"SELECT id FROM issues WHERE json_extract(data,'$.milestone_id') IN (SELECT value FROM json_each(?))",&json!(scope.milestones).to_string()).await?);
    }
    if project_wide {
        scope.jobs.extend(
            ids(
                conn,
                "SELECT id FROM jobs WHERE project_id IN (SELECT value FROM json_each(?))",
                &json!(scope.projects).to_string(),
            )
            .await?,
        );
        scope.parents(conn).await?;
    }
    // Whole-Job mutations and human acceptance inspect all Task records.
    // Other lifecycle checks use SQL summaries of the unmaterialized siblings.
    if matches!(
        command,
        "job.cancel"
            | "job.approve"
            | "job.followup"
            | "job.delete"
            | "project.delete"
            | "project.rename"
            | "project.relocate"
    ) || command == "issue.cancel"
    {
        scope.job_tasks(conn).await?;
    }
    if matches!(
        command,
        "task.retry" | "task.reopen" | "job.delete" | "project.delete"
    ) {
        scope.dependencies(conn, true).await?;
    }
    if !command.starts_with("session.") {
        for job in &scope.jobs {
            scope.tasks.extend(ids(conn,"SELECT id FROM tasks WHERE job_id=? AND json_extract(data,'$.last_session') IS NOT NULL ORDER BY json_extract(data,'$.updated_at') DESC,rowid DESC LIMIT 1",job).await?);
        }
    }
    if matches!(
        command,
        "task.start" | "task.depend" | "task.retry" | "task.reopen"
    ) {
        scope.dependencies(conn, false).await?;
    }
    if command == "task.claim" {
        scope.extra_leases.extend(
            sqlx::query_scalar::<_, String>(
                "SELECT id FROM task_leases WHERE executor_ref=? AND session_ref=?",
            )
            .bind(request["executor"].as_str())
            .bind(request["session"].as_str())
            .fetch_all(&mut *conn)
            .await?,
        );
    }
    Ok(scope)
}

async fn session_scope(
    conn: &mut SqliteConnection,
    scope: &mut Scope,
    request: &Value,
) -> Result<()> {
    let session = required(request, "session")?;
    let command = required(request, "command")?;
    if command == "session.stage"
        || (command == "session.record"
            && request.get("turn_id").is_some()
            && request["job"].is_null())
    {
        return Ok(());
    }
    if command == "session.record" {
        if scope.jobs.is_empty() {
            scope.jobs.extend(ids(conn,"WITH activity(job_id,stamp,token) AS (SELECT id,json_extract(data,'$.created_at'),substr(id,instr(id,'_')+1) FROM jobs WHERE json_extract(data,'$.session_id')=?1 UNION ALL SELECT id,json_extract(data,'$.followup_at'),substr(COALESCE(json_extract(data,'$.followup_id'),id),instr(COALESCE(json_extract(data,'$.followup_id'),id),'_')+1) FROM jobs WHERE json_extract(data,'$.followup_session_id')=?1 UNION ALL SELECT job_id,json_extract(data,'$.updated_at'),substr(id,instr(id,'_')+1) FROM tasks WHERE json_extract(data,'$.last_session')=?1) SELECT jobs.id FROM activity JOIN jobs ON jobs.id=activity.job_id WHERE json_extract(jobs.data,'$.archived_at') IS NULL ORDER BY MAX(stamp,json_extract(jobs.data,'$.created_at')) DESC,token DESC,jobs.id DESC LIMIT 1",session).await?);
        }
        for job in &scope.jobs {
            let task: Option<String>=sqlx::query_scalar("SELECT id FROM tasks WHERE json_extract(data,'$.last_session')=? AND job_id=? ORDER BY json_extract(data,'$.updated_at') DESC,rowid DESC LIMIT 1")
                .bind(session).bind(job).fetch_optional(&mut *conn).await?;
            scope.tasks.extend(task);
        }
        return Ok(());
    }
    let query = match command {
        "session.heartbeat" => "SELECT id FROM task_leases WHERE session_ref=?",
        "session.start" => {
            "SELECT id FROM tasks WHERE json_extract(data,'$.last_session')=? AND json_extract(data,'$.status')='BLOCKED' AND json_extract(data,'$.system_block')=1"
        }
        _ => {
            "SELECT id FROM tasks WHERE json_extract(data,'$.last_session')=? AND json_extract(data,'$.status')='IN_PROGRESS'"
        }
    };
    scope.tasks.extend(ids(conn, query, session).await?);
    scope.extra_leases.extend(
        ids(
            conn,
            "SELECT id FROM task_leases WHERE session_ref=?",
            session,
        )
        .await?,
    );
    Ok(())
}

pub(crate) async fn load_request(
    conn: &mut SqliteConnection,
    request: &Value,
    now: i64,
) -> Result<Snapshot> {
    let scope = request_scope(conn, request).await?;
    let mut state = scope.load(conn).await?;
    load_query_context(conn, &mut state, request, now).await?;
    // Only creation/deletion uses the historical filename high-water marks.
    if matches!(
        required(request, "command")?,
        "job.create"
            | "task.add"
            | "job.delete"
            | "project.delete"
            | "issue.create"
            | "milestone.create"
            | "milestone.delete"
    ) && let Some(data) = sqlx::query_scalar::<_, String>(
        "SELECT value FROM projection_state WHERE key='document_sequences'",
    )
    .fetch_optional(&mut *conn)
    .await?
    {
        state.document_sequences = serde_json::from_str(&data)?;
    }
    Ok(state)
}

#[allow(clippy::too_many_lines)] // Keep indexed mutation summaries and naming reads auditable together.
async fn load_query_context(
    conn: &mut SqliteConnection,
    state: &mut Snapshot,
    request: &Value,
    now: i64,
) -> Result<()> {
    let command = required(request, "command")?;
    if command == "project.register" {
        state.query_context.registered_project = state.projects.first().map(|p| p.id.clone());
        if state.query_context.registered_project.is_none() {
            state.query_context.available_project_key =
                Some(crate::project_lookup::available_key(conn, required(request, "name")?).await?);
        }
    }
    if command == "project.rename" {
        let project = &state.projects[state.project_index(required(request, "project")?)?];
        let collision: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM project_lookup WHERE folded_key=? AND project_id<>?)",
        )
        .bind(required(request, "name")?.to_lowercase())
        .bind(&project.id)
        .fetch_one(&mut *conn)
        .await?;
        ensure!(
            !collision,
            "conflict: Project folder name is already registered"
        );
    }
    if command == "project.move" {
        let project = &state.projects[state.project_index(required(request, "project")?)?];
        let collision: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM project_lookup WHERE canonical_root=? AND project_id<>?)",
        )
        .bind(crate::project_lookup::canonical_root(required(
            request, "root",
        )?)?)
        .bind(&project.id)
        .fetch_one(&mut *conn)
        .await?;
        ensure!(!collision, "conflict: Project root is already registered");
    }
    if matches!(command, "issue.submit" | "issue.approve") {
        for issue in &state.issues {
            let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE json_extract(data,'$.issue_id')=? AND json_extract(data,'$.status') NOT IN ('COMPLETED','CANCELLED'))")
                .bind(&issue.id).fetch_one(&mut *conn).await?;
            state
                .query_context
                .issue_active_jobs
                .insert(issue.id.clone(), active);
        }
    }
    let selected = json!(state.tasks.iter().map(|t| &t.id).collect::<Vec<_>>()).to_string();
    for job in &state.jobs {
        let row=sqlx::query("SELECT COUNT(*) AS eligible, COALESCE(SUM(json_extract(data,'$.status') IS NOT 'DONE'),0) AS incomplete FROM tasks WHERE job_id=? AND json_extract(data,'$.status') IS NOT 'CANCELLED' AND id NOT IN (SELECT value FROM json_each(?))")
            .bind(&job.id).bind(&selected).fetch_one(&mut *conn).await?;
        state.query_context.job_counts.insert(
            job.id.clone(),
            (
                row.get::<i64, _>("eligible").try_into()?,
                row.get::<i64, _>("incomplete").try_into()?,
            ),
        );
    }
    let kind = if matches!(command, "issue.create" | "issue.update") {
        Some("issue")
    } else if matches!(command, "milestone.create" | "milestone.update") {
        Some("milestone")
    } else if matches!(command, "task.add" | "task.update") {
        Some("task")
    } else if matches!(command, "job.create" | "job.update") {
        Some("job")
    } else {
        None
    };
    if let Some(kind) = kind
        && (request.get("name").is_some()
            || matches!(
                command,
                "task.add" | "job.create" | "issue.create" | "milestone.create"
            ))
    {
        let table = match kind {
            "task" => "tasks",
            "issue" => "issues",
            "milestone" => "milestones",
            _ => "jobs",
        };
        let project_column = if kind == "task" {
            "json_extract(data,'$.project_id')"
        } else {
            "project_id"
        };
        for project in &state.projects {
            let rows=sqlx::query(&format!("SELECT id,COALESCE(json_extract(data,'$.name'),'') AS name FROM {table} WHERE {project_column}=?"))
                    .bind(&project.id).fetch_all(&mut *conn).await?;
            state.query_context.names.insert(
                (project.id.clone(), kind.into()),
                rows.into_iter()
                    .map(|r| (r.get("id"), r.get("name")))
                    .collect(),
            );
            let sequence: i64=sqlx::query_scalar(&format!("SELECT COALESCE(MAX(json_extract(data,'$.sequence')),0) FROM {table} WHERE {project_column}=? AND CAST(json_extract(data,'$.created_at')/86400 AS INTEGER)=?"))
                    .bind(&project.id).bind(now.div_euclid(86400)).fetch_one(&mut *conn).await?;
            state
                .query_context
                .sequences
                .insert((project.id.clone(), kind.into()), sequence.try_into()?);
        }
    }
    if command == "task.add" {
        state.query_context.task_count = Some(
            sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
                .fetch_one(&mut *conn)
                .await?,
        );
    }
    Ok(())
}

impl Store {
    pub(crate) async fn session_project(&self, session: &str) -> Result<Option<crate::Project>> {
        let mut tx = self.pool.begin().await?;
        // Two distinct IDs are enough to reject an ambiguous association.
        let projects = ids(&mut tx, SESSION_PROJECTS_QUERY, session).await?;
        ensure!(
            projects.len() <= 1,
            "ambiguous Project for this session; select a registered project directory"
        );
        Ok(entities(&mut tx, "projects", &projects).await?.pop())
    }

    pub async fn job_record(&self, id: &str) -> Result<crate::Job> {
        let mut tx = self.pool.begin().await?;
        let id = resolve(&mut tx, "jobs", id).await?;
        Ok(entities(&mut tx, "jobs", &BTreeSet::from([id]))
            .await?
            .remove(0))
    }

    pub(crate) async fn current_plan(&self, id: &str) -> Result<crate::Plan> {
        let mut tx = self.pool.begin().await?;
        let id = resolve(&mut tx, "tasks", id).await?;
        let task: crate::Task = entities(&mut tx, "tasks", &BTreeSet::from([id]))
            .await?
            .remove(0);
        entities(&mut tx, "plans", &task.current_plan.into_iter().collect())
            .await?
            .pop()
            .context("not_found: current Plan")
    }

    /// A Task's document path needs only its own record and parent Project.
    pub(crate) async fn task_document_snapshot(&self, id: &str) -> Result<Snapshot> {
        let mut tx = self.pool.begin().await?;
        let id = resolve(&mut tx, "tasks", id).await?;
        let tasks: Vec<crate::Task> = entities(&mut tx, "tasks", &BTreeSet::from([id])).await?;
        let projects = entities(
            &mut tx,
            "projects",
            &BTreeSet::from([tasks[0].project_id.clone()]),
        )
        .await?;
        Ok(Snapshot {
            projects,
            tasks,
            ..Snapshot::default()
        })
    }

    pub async fn project_result(&self, id: &str) -> Result<crate::Project> {
        let mut tx = self.pool.begin().await?;
        let id = resolve(&mut tx, "projects", id).await?;
        Ok(entities(&mut tx, "projects", &BTreeSet::from([id]))
            .await?
            .remove(0))
    }

    /// Indexed lookup by canonical directory identity, without filesystem access.
    pub async fn project_by_root(&self, root: &str) -> Result<Option<crate::Project>> {
        let data: Option<String> = sqlx::query_scalar("SELECT data FROM projects WHERE id=(SELECT p.id FROM project_lookup l JOIN projects p ON p.id=l.project_id WHERE l.canonical_root IN (?,?) ORDER BY p.rowid LIMIT 1)")
            .bind(crate::stored_paths::abbreviate_home(&crate::stored_paths::expand_home(root)?)?)
            .bind(crate::stored_paths::expand_home(root)?)
            .fetch_optional(&self.pool)
            .await?;
        data.map(|v| crate::stored_paths::from_str(&v)).transpose()
    }

    pub(crate) async fn request_snapshot(&self, request: &Value) -> Result<Snapshot> {
        let mut tx = self.pool.begin().await?;
        let state = load_request(&mut tx, request, self.now()).await?;
        tx.commit().await?;
        Ok(state)
    }

    /// List Jobs without loading their Tasks, Plans, or Issue bodies.
    pub async fn jobs(&self, project: Option<&str>) -> Result<Vec<crate::Job>> {
        self.filtered_jobs(project, &JobFilter::default()).await
    }

    /// Filter Jobs in `SQLite` while retaining insertion order and full results.
    pub async fn filtered_jobs(
        &self,
        project: Option<&str>,
        filter: &JobFilter,
    ) -> Result<Vec<crate::Job>> {
        let mut tx = self.pool.begin().await?;
        let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new("SELECT data FROM jobs WHERE 1");
        if let Some(project) = project {
            let project = resolve(&mut tx, "projects", project).await?;
            query.push(" AND project_id=").push_bind(project);
        }
        if let Some(status) = filter.status {
            query
                .push(" AND json_extract(data,'$.status')=")
                .push_bind(status.to_string());
        }
        if let Some(issue) = &filter.issue_id {
            query
                .push(" AND json_extract(data,'$.issue_id')=")
                .push_bind(resolve(&mut tx, "issues", issue).await?);
        }
        if filter.unassociated {
            query.push(" AND json_extract(data,'$.issue_id') IS NULL");
        }
        if let Some(archived) = filter.archived {
            query.push(if archived {
                " AND json_extract(data,'$.archived_at') IS NOT NULL"
            } else {
                " AND json_extract(data,'$.archived_at') IS NULL"
            });
        }
        for (condition, bound) in [
            (
                " AND json_extract(data,'$.created_at')>=",
                filter.created_from,
            ),
            (
                " AND json_extract(data,'$.created_at')<",
                filter.created_before,
            ),
            (
                " AND json_extract(data,'$.archived_at')>=",
                filter.archived_from,
            ),
            (
                " AND json_extract(data,'$.archived_at')<",
                filter.archived_before,
            ),
        ] {
            if let Some(bound) = bound {
                query.push(condition).push_bind(bound);
            }
        }
        query.push(" ORDER BY rowid");
        let rows: Vec<String> = query.build_query_scalar().fetch_all(&mut *tx).await?;
        rows.into_iter()
            .map(|row| serde_json::from_str(&row).map_err(Into::into))
            .collect()
    }

    /// Filter Tasks in `SQLite`, including dependencies outside the selected scope.
    pub async fn tasks(
        &self,
        job: Option<&str>,
        project: Option<&str>,
        status: Option<&str>,
        ready: bool,
    ) -> Result<Vec<crate::Task>> {
        let mut tx = self.pool.begin().await?;
        let job = match job {
            Some(id) => Some(resolve(&mut tx, "jobs", id).await?),
            None => None,
        };
        let project = match project {
            Some(id) => Some(resolve(&mut tx, "projects", id).await?),
            None => None,
        };
        if let Some(status) = status {
            ensure!(
                crate::TaskStatus::ALL
                    .iter()
                    .any(|s| s.to_string() == status),
                "invalid task status"
            );
        }
        read_tasks(&mut tx, job, project, status, ready, None).await
    }

    /// The legacy IM list gives Job identifiers precedence over Project identifiers.
    pub async fn legacy_tasks(&self, filter: Option<&str>, limit: u32) -> Result<Vec<crate::Task>> {
        let mut tx = self.pool.begin().await?;
        let (job, project) = match filter {
            Some(filter) => match resolve(&mut tx, "jobs", filter).await {
                Ok(id) => (Some(id), None),
                Err(_) => (None, Some(resolve(&mut tx, "projects", filter).await?)),
            },
            None => (None, None),
        };
        read_tasks(&mut tx, job, project, None, false, Some(limit)).await
    }

    /// Read an exact Task ID's lease for action ownership checks.
    pub async fn task_lease(&self, id: &str) -> Result<Option<crate::Lease>> {
        let row: Option<String> = sqlx::query_scalar("SELECT data FROM task_leases WHERE id=?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| serde_json::from_str(&row).map_err(Into::into))
            .transpose()
    }

    pub async fn projects(&self) -> Result<Vec<crate::Project>> {
        let mut tx = self.pool.begin().await?;
        let selected = ids(&mut tx, "SELECT id FROM projects WHERE ? IS NOT NULL", "").await?;
        entities(&mut tx, "projects", &selected).await
    }
}

async fn read_tasks(
    conn: &mut SqliteConnection,
    job: Option<String>,
    project: Option<String>,
    status: Option<&str>,
    ready: bool,
    limit: Option<u32>,
) -> Result<Vec<crate::Task>> {
    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new("SELECT t.data FROM tasks t WHERE 1=1");
    if let Some(job) = job {
        query.push(" AND t.job_id=").push_bind(job);
    }
    if let Some(project) = project {
        query
            .push(" AND json_extract(t.data,'$.project_id')=")
            .push_bind(project);
    }
    if let Some(status) = status {
        query
            .push(" AND json_extract(t.data,'$.status')=")
            .push_bind(status);
    }
    if ready {
        query.push(" AND json_extract(t.data,'$.status')='TODO' AND NOT EXISTS (SELECT 1 FROM json_each(t.data,'$.dependencies') d WHERE NOT EXISTS (SELECT 1 FROM tasks dependency WHERE dependency.id=d.value AND json_extract(dependency.data,'$.status')='DONE'))");
    }
    query.push(" ORDER BY t.rowid");
    if let Some(limit) = limit {
        query.push(" LIMIT ").push_bind(i64::from(limit));
    }
    let rows: Vec<String> = query.build_query_scalar().fetch_all(conn).await?;
    rows.into_iter()
        .map(|row| serde_json::from_str(&row).map_err(Into::into))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn issue_mutations_do_not_hydrate_related_job_bodies() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("tasks.sqlite3"))
            .await
            .unwrap();
        let project = store
            .execute(
                json!({"command":"project.register","name":"Scope","root":dir.path()}),
                crate::WriteOptions::default(),
            )
            .await
            .unwrap()
            .result;
        let issue = store.execute(json!({"command":"issue.create","project":project["id"],"title":"Requirement","requirement":"Build it","acceptance":"Tests pass"}), crate::WriteOptions::default()).await.unwrap().result;
        for n in 0..12 {
            store.execute(json!({"command":"job.create","project":project["id"],"title":format!("Round {n}"),"issue":issue["id"],"goal":"x".repeat(100_000)}), crate::WriteOptions::default()).await.unwrap();
        }
        let mut tx = store.pool.begin().await.unwrap();
        for command in [
            "issue.update",
            "issue.submit",
            "issue.approve",
            "issue.archive",
            "issue.reopen",
        ] {
            let state = load_request(
                &mut tx,
                &json!({"command":command,"issue":issue["id"]}),
                store.now(),
            )
            .await
            .unwrap();
            assert!(
                state.jobs.is_empty(),
                "{command} hydrated {} Jobs",
                state.jobs.len()
            );
            assert!(state.tasks.is_empty());
        }
        tx.rollback().await.unwrap();
        assert!(
            store
                .execute(
                    json!({"command":"issue.submit","issue":issue["id"]}),
                    crate::WriteOptions::default()
                )
                .await
                .unwrap_err()
                .to_string()
                .contains("terminal")
        );
        let mut tx = store.pool.begin().await.unwrap();
        let cancel = load_request(
            &mut tx,
            &json!({"command":"issue.cancel","issue":issue["id"]}),
            store.now(),
        )
        .await
        .unwrap();
        assert_eq!(cancel.jobs.len(), 12);
    }

    #[tokio::test]
    async fn filtered_jobs_preserve_scope_order_and_full_records() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("tasks.sqlite3"))
            .await
            .unwrap();
        let project = store
            .execute(
                json!({"command":"project.register","name":"Scope","root":dir.path()}),
                crate::WriteOptions::default(),
            )
            .await
            .unwrap()
            .result["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let job = store.execute(
            json!({"command":"job.create","project":project,"title":"Body preserved","prompt":"Full prompt"}),
            crate::WriteOptions::default(),
        ).await.unwrap().result;
        let mut expected = Vec::new();
        for id in ["job_z", "job_a"] {
            let mut record = job.clone();
            record["id"] = json!(id);
            record["created_at"] = json!(100);
            record["archived_at"] = json!(200);
            record["status"] = json!("COMPLETED");
            sqlx::query("INSERT INTO jobs(id,data) VALUES(?,?)")
                .bind(id)
                .bind(record.to_string())
                .execute(&store.pool)
                .await
                .unwrap();
            expected.push(serde_json::from_value::<crate::Job>(record).unwrap());
        }
        let filter = JobFilter {
            issue_id: None,
            unassociated: false,
            status: Some(crate::JobStatus::Completed),
            archived: Some(true),
            created_from: Some(100),
            created_before: Some(101),
            archived_from: Some(200),
            archived_before: Some(201),
        };
        assert_eq!(
            store
                .filtered_jobs(Some(&project[..project.len() - 1]), &filter)
                .await
                .unwrap(),
            expected
        );
        assert_eq!(store.filtered_jobs(None, &filter).await.unwrap(), expected);
        assert!(
            store
                .filtered_jobs(Some("prj_missing"), &filter)
                .await
                .unwrap_err()
                .to_string()
                .contains("not_found")
        );
        assert!(
            store
                .filtered_jobs(
                    None,
                    &JobFilter {
                        created_before: Some(100),
                        ..filter
                    }
                )
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn session_project_history_uses_both_session_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("tasks.sqlite3"))
            .await
            .unwrap();
        let rows = sqlx::query(&format!("EXPLAIN QUERY PLAN {SESSION_PROJECTS_QUERY}"))
            .bind("target")
            .fetch_all(&store.pool)
            .await
            .unwrap();
        let details: Vec<String> = rows.iter().map(|row| row.get("detail")).collect();
        for index in ["tasks_by_session", "jobs_by_session"] {
            assert!(
                details
                    .iter()
                    .any(|detail| detail.contains("SEARCH") && detail.contains(index)),
                "Session history must seek through {index}: {details:?}"
            );
        }
    }
}
