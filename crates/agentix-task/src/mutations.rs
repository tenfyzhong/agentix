use crate::naming::{next_sequence, numbered_name, unique_name};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

use crate::{
    DEFAULT_LEASE_SECONDS, Job, JobStatus, Lease, Plan, Project, Snapshot, Task, TaskPhase,
    TaskStatus, WriteOptions, new_id,
};

pub(crate) fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .with_context(|| format!("invalid: {key} must not be blank"))
}

pub(crate) fn check_revision(revision: i64, options: &WriteOptions) -> Result<()> {
    ensure!(
        options.expected_revision.is_none_or(|r| r == revision),
        "conflict: revision changed (current {revision})"
    );
    Ok(())
}

pub(crate) fn apply(
    state: &mut Snapshot,
    request: &Value,
    options: &WriteOptions,
    now: i64,
) -> Result<Value> {
    let command = required(request, "command")?;
    match command {
        "project.register" => register_project(state, request, now),
        "project.delete" | "job.delete" => crate::deletion::apply(state, request, options),
        "project.archive" | "project.unarchive" => archive_project(state, request, options, now),
        "job.create" => create_job(state, request, now),
        "job.update" | "job.cancel" | "job.archive" | "job.unarchive" => {
            update_job(state, request, options, now)
        }
        "task.add" => add_task(state, request, now),
        "session.start" | "session.end" | "session.interrupt" | "session.heartbeat" => {
            session(state, request, now)
        }
        _ if command.starts_with("task.") || command == "plan.register" => {
            update_task(state, request, options, now)
        }
        _ => bail!("invalid: unknown command {command}"),
    }
}

fn create_job(state: &mut Snapshot, request: &Value, now: i64) -> Result<Value> {
    let project = &state.projects[state.project_index(required(request, "project")?)?];
    ensure!(
        project.archived_at.is_none(),
        "conflict: Project is archived"
    );
    let id = new_id("job");
    let title = required(request, "title")?;
    let name = unique_name(
        request["name"].as_str().unwrap_or(title),
        state
            .jobs
            .iter()
            .filter(|j| j.project_id == project.id)
            .map(|j| j.name.as_str()),
    );
    let sequence = next_sequence(
        now,
        state
            .jobs
            .iter()
            .filter(|j| j.project_id == project.id)
            .map(|j| (j.created_at, j.sequence))
            .chain(std::iter::once((
                now,
                *state
                    .document_sequences
                    .get(&crate::deletion::sequence_key(&project.id, "job", now))
                    .unwrap_or(&0),
            ))),
    )?;
    let filename = numbered_name(&name, now, sequence)?;
    let job = Job {
        document_path: format!("Projects/{}/Jobs/{filename}.md", project.key),
        sequence,
        name,
        id,
        project_id: project.id.clone(),
        title: required(request, "title")?.into(),
        goal: request["goal"].as_str().unwrap_or_default().into(),
        status: JobStatus::Active,
        revision: 1,
        created_at: now,
        updated_at: now,
        started_at: None,
        completed_at: None,
        cancelled_at: None,
        archived_at: None,
    };
    let result = serde_json::to_value(&job)?;
    state.jobs.push(job);
    Ok(result)
}

fn add_task(state: &mut Snapshot, request: &Value, now: i64) -> Result<Value> {
    let job = &state.jobs[state.job_index(required(request, "job")?)?];
    ensure!(
        state.projects[state.project_index(&job.project_id)?]
            .archived_at
            .is_none(),
        "conflict: Project is archived"
    );
    ensure!(
        job.status == JobStatus::Active && job.archived_at.is_none(),
        "conflict: Job is closed or archived; create a new Job for new scope"
    );
    let task = Task {
        sequence: next_sequence(
            now,
            state
                .tasks
                .iter()
                .filter(|t| t.project_id == job.project_id)
                .map(|t| (t.created_at, t.sequence))
                .chain(std::iter::once((
                    now,
                    *state
                        .document_sequences
                        .get(&crate::deletion::sequence_key(&job.project_id, "task", now))
                        .unwrap_or(&0),
                ))),
        )?,
        name: unique_name(
            request["name"]
                .as_str()
                .unwrap_or(required(request, "title")?),
            state
                .tasks
                .iter()
                .filter(|t| t.project_id == job.project_id)
                .map(|t| t.name.as_str()),
        ),
        id: new_id("task"),
        project_id: job.project_id.clone(),
        job_id: job.id.clone(),
        title: required(request, "title")?.into(),
        status: TaskStatus::Todo,
        phase: None,
        revision: 1,
        position: i64::try_from(state.tasks.len())?,
        created_at: now,
        updated_at: now,
        started_at: None,
        completed_at: None,
        reason: None,
        dependencies: Vec::new(),
        current_plan: None,
        last_executor: None,
        last_session: None,
        delegated_by: None,
        system_block: false,
    };
    let result = serde_json::to_value(&task)?;
    state.tasks.push(task);
    Ok(result)
}

fn register_project(state: &mut Snapshot, request: &Value, now: i64) -> Result<Value> {
    let root = required(request, "root")?;
    if let Some(project) = state.projects.iter().find(|p| p.root == root) {
        return Ok(serde_json::to_value(project)?);
    }
    let name = required(request, "name")?;
    let id = new_id("prj");
    let key = unique_name(name, state.projects.iter().map(|p| p.key.as_str()));
    let project = Project {
        id,
        name: key.clone(),
        key,
        root: root.into(),
        remote: request["remote"].as_str().map(str::to_owned),
        revision: 1,
        created_at: now,
        archived_at: None,
    };
    let result = serde_json::to_value(&project)?;
    state.projects.push(project);
    Ok(result)
}

fn archive_project(
    state: &mut Snapshot,
    request: &Value,
    options: &WriteOptions,
    now: i64,
) -> Result<Value> {
    let i = state.project_index(required(request, "project")?)?;
    let project = &state.projects[i];
    check_revision(project.revision, options)?;
    let archive = required(request, "command")? == "project.archive";
    ensure!(
        archive == project.archived_at.is_none(),
        "conflict: Project archive state unchanged"
    );
    if archive {
        ensure!(
            !state
                .jobs
                .iter()
                .any(|j| j.project_id == project.id && j.status == JobStatus::Active),
            "conflict: complete or cancel all Jobs before archiving Project"
        );
    }
    state.projects[i].archived_at = archive.then_some(now);
    state.projects[i].revision += 1;
    Ok(serde_json::to_value(&state.projects[i])?)
}

#[allow(clippy::too_many_lines)] // Keep Job validation and state changes together.
fn update_job(
    state: &mut Snapshot,
    request: &Value,
    options: &WriteOptions,
    now: i64,
) -> Result<Value> {
    let i = state.job_index(required(request, "job")?)?;
    check_revision(state.jobs[i].revision, options)?;
    ensure!(
        state.projects[state.project_index(&state.jobs[i].project_id)?]
            .archived_at
            .is_none(),
        "conflict: Project is archived"
    );
    let command = required(request, "command")?;
    if command == "job.cancel" {
        ensure!(
            state.jobs[i].status == JobStatus::Active,
            "conflict: Job is not active"
        );
        let job_id = state.jobs[i].id.clone();
        ensure!(
            !state.leases.iter().any(|l| state
                .tasks
                .iter()
                .any(|t| t.id == l.task_id && t.job_id == job_id)),
            "conflict: release active Task leases before cancelling Job"
        );
        for task in state
            .tasks
            .iter_mut()
            .filter(|t| t.job_id == job_id && !t.status.terminal())
        {
            task.completed_at = Some(now);
            task.status = TaskStatus::Cancelled;
            task.phase = None;
            task.revision += 1;
            task.updated_at = now;
            task.system_block = false;
        }
        state.jobs[i].status = JobStatus::Cancelled;
        state.jobs[i].cancelled_at = Some(now);
    }
    let renamed = if command == "job.update" && request.get("name").is_some() {
        Some(unique_name(
            required(request, "name")?,
            state
                .jobs
                .iter()
                .filter(|j| j.project_id == state.jobs[i].project_id && j.id != state.jobs[i].id)
                .map(|j| j.name.as_str()),
        ))
    } else {
        None
    };
    let project_key = state.projects[state.project_index(&state.jobs[i].project_id)?]
        .key
        .clone();
    let job = &mut state.jobs[i];
    match command {
        "job.update" => {
            ensure!(
                (job.status == JobStatus::Active
                    || (renamed.is_some()
                        && request.get("title").is_none()
                        && request.get("goal").is_none()))
                    && job.archived_at.is_none(),
                "conflict: Job is closed"
            );
            if let Some(name) = renamed {
                let parent = job
                    .document_path
                    .rsplit_once('/')
                    .context("invalid Job path")?
                    .0;
                let filename = numbered_name(&name, job.created_at, job.sequence)?;
                job.document_path = format!("{parent}/{filename}.md");
                job.name = name;
            }
            if request.get("title").is_some() {
                job.title = required(request, "title")?.into();
            }
            if let Some(goal) = request["goal"].as_str() {
                job.goal = goal.into();
            }
        }
        "job.archive" => {
            ensure!(
                job.status != JobStatus::Active,
                "conflict: complete or cancel Job before archiving"
            );
            ensure!(job.archived_at.is_none(), "conflict: Job already archived");
            job.archived_at = Some(now);
            let filename = numbered_name(&job.name, job.created_at, job.sequence)?;
            job.document_path = format!("Projects/{project_key}/Jobs/Archived/{filename}.md");
        }
        "job.unarchive" => {
            ensure!(job.archived_at.is_some(), "conflict: Job is not archived");
            job.archived_at = None;
            let filename = numbered_name(&job.name, job.created_at, job.sequence)?;
            job.document_path = format!("Projects/{project_key}/Jobs/{filename}.md");
        }
        _ => {}
    }
    job.revision += 1;
    job.updated_at = now;
    Ok(serde_json::to_value(job)?)
}

fn authorize(state: &Snapshot, task: &Task, options: &WriteOptions, now: i64) -> Result<()> {
    if let Some(lease) = state.leases.iter().find(|l| l.task_id == task.id) {
        ensure!(
            options.lease_token.as_deref() == Some(lease.token.as_str())
                && options.session_ref.as_deref() == Some(lease.session_ref.as_str())
                && lease.lease_expires_at > now,
            "conflict: active Task requires the current lease token and session"
        );
    } else {
        ensure!(
            options.lease_token.is_none(),
            "conflict: lease has expired or been released"
        );
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // Keep the transactional state-machine dispatch together.
fn update_task(
    state: &mut Snapshot,
    request: &Value,
    options: &WriteOptions,
    now: i64,
) -> Result<Value> {
    let i = state.task_index(required(request, "task")?)?;
    let task = state.tasks[i].clone();
    let j = state.job_index(&task.job_id)?;
    check_revision(task.revision, options)?;
    ensure!(
        state.projects[state.project_index(&task.project_id)?]
            .archived_at
            .is_none(),
        "conflict: Project is archived"
    );
    ensure!(
        state.jobs[j].archived_at.is_none() && state.jobs[j].status != JobStatus::Cancelled,
        "conflict: Job is archived or cancelled"
    );
    let command = required(request, "command")?;
    if command != "task.claim" {
        authorize(state, &task, options, now)?;
    }
    if matches!(command, "plan.register" | "task.start" | "task.done") {
        ensure!(
            task.status == TaskStatus::InProgress
                && state.leases.iter().any(|l| l.task_id == task.id),
            "conflict: {command} requires an active Task lease"
        );
    }
    match command {
        "task.claim" => claim(state, i, request, now)?,
        "task.start" => {
            ensure!(
                task.phase == Some(TaskPhase::Planning),
                "conflict: Task must be in PLANNING before start"
            );
            ensure!(
                task.current_plan.is_some(),
                "invalid: current Plan is required before start"
            );
            ensure!(
                task.dependencies.iter().all(|d| state
                    .tasks
                    .iter()
                    .any(|t| t.id == *d && t.status == TaskStatus::Done)),
                "conflict: dependencies are incomplete"
            );
            state.tasks[i].phase = Some(TaskPhase::Executing);
            state.tasks[i].started_at.get_or_insert(now);
            state.jobs[j].started_at.get_or_insert(now);
        }
        "task.heartbeat" => {
            let lease = state
                .leases
                .iter_mut()
                .find(|l| l.task_id == task.id)
                .context("conflict: no active lease")?;
            lease.lease_expires_at = now + DEFAULT_LEASE_SECONDS;
        }
        "task.update" => {
            ensure!(
                (state.jobs[j].status == JobStatus::Active && !task.status.terminal())
                    || (request.get("name").is_some()
                        && request.get("title").is_none()
                        && request.get("position").is_none()),
                "conflict: Task is closed"
            );
            if request.get("name").is_some() {
                let name = unique_name(
                    required(request, "name")?,
                    state
                        .tasks
                        .iter()
                        .filter(|t| t.project_id == task.project_id && t.id != task.id)
                        .map(|t| t.name.as_str()),
                );
                let filename = numbered_name(&name, task.created_at, task.sequence)?;
                for plan in state.plans.iter_mut().filter(|p| p.task_id == task.id) {
                    let parent = plan.path.rsplit_once('/').context("invalid Plan path")?.0;
                    plan.path = format!("{parent}/{filename}.md");
                }
                state.tasks[i].name = name;
            }
            if request.get("title").is_some() {
                state.tasks[i].title = required(request, "title")?.into();
            }
            if let Some(position) = request["position"].as_i64() {
                ensure!(position >= 0, "invalid: negative position");
                state.tasks[i].position = position;
            }
        }
        "task.depend" | "task.undepend" => {
            ensure!(
                task.started_at.is_none(),
                "conflict: dependencies cannot change after Task starts"
            );
            let dependency =
                state.tasks[state.task_index(required(request, "dependency")?)?].clone();
            ensure!(
                dependency.project_id == task.project_id,
                "invalid: dependency must belong to the same Project"
            );
            ensure!(
                dependency.id != task.id,
                "invalid: Task cannot depend on itself"
            );
            if command == "task.depend" {
                ensure!(
                    !depends_on(state, &dependency.id, &task.id),
                    "invalid: dependency cycle"
                );
                if !state.tasks[i].dependencies.contains(&dependency.id) {
                    state.tasks[i].dependencies.push(dependency.id);
                }
            } else {
                state.tasks[i].dependencies.retain(|d| *d != dependency.id);
            }
        }
        "plan.register" => {
            ensure!(!task.status.terminal(), "conflict: Task is closed");
            let plan: Plan = serde_json::from_value(request["plan"].clone())?;
            let version = state
                .plans
                .iter()
                .filter(|p| p.task_id == task.id)
                .map(|p| p.version)
                .max()
                .unwrap_or(0)
                + 1;
            ensure!(
                plan.task_id == task.id && plan.version == version,
                "conflict: Plan version changed"
            );
            state.tasks[i].current_plan = Some(plan.id.clone());
            state.plans.retain(|p| p.task_id != task.id);
            state.plans.push(plan.clone());
            state.tasks[i].revision += 1;
            state.tasks[i].updated_at = now;
            return Ok(serde_json::to_value(plan)?);
        }
        "task.retry" | "task.reopen" => {
            ensure!(
                if command == "task.retry" {
                    task.status == TaskStatus::Failed
                } else {
                    matches!(task.status, TaskStatus::Done | TaskStatus::Cancelled)
                },
                "invalid: wrong terminal state for {command}"
            );
            ensure!(
                !state
                    .tasks
                    .iter()
                    .any(|t| t.started_at.is_some() && depends_on(state, &t.id, &task.id)),
                "conflict: a downstream Task has already started"
            );
            state.tasks[i].completed_at = None;
            state.tasks[i].status = TaskStatus::Todo;
            state.tasks[i].phase = None;
            state.tasks[i].reason = None;
            state.tasks[i].system_block = false;
            state.jobs[j].status = JobStatus::Active;
            state.jobs[j].completed_at = None;
        }
        "task.block" | "task.wait" | "task.done" | "task.fail" | "task.cancel" | "task.release" => {
            if command == "task.done" {
                ensure!(
                    task.phase == Some(TaskPhase::Executing),
                    "conflict: Task must be EXECUTING before done; call start first"
                );
            }
            let next = match command {
                "task.done" => TaskStatus::Done,
                "task.fail" => TaskStatus::Failed,
                "task.cancel" => TaskStatus::Cancelled,
                "task.wait" => TaskStatus::WaitingUser,
                _ => TaskStatus::Blocked,
            };
            ensure!(
                task.status.allows(next),
                "invalid: transition {} -> {next}",
                task.status
            );
            let reason = if matches!(
                next,
                TaskStatus::Blocked | TaskStatus::WaitingUser | TaskStatus::Failed
            ) {
                Some(required(request, "reason")?.to_owned())
            } else {
                None
            };
            state.tasks[i].completed_at = next.terminal().then_some(now);
            state.tasks[i].status = next;
            state.tasks[i].phase = None;
            state.tasks[i].reason = reason;
            state.tasks[i].system_block = false;
            state.leases.retain(|l| l.task_id != task.id);
        }
        _ => bail!("invalid: unknown command {command}"),
    }
    state.tasks[i].revision += 1;
    state.tasks[i].updated_at = now;
    aggregate_job(state, j, now);
    state.task_result(&task.id)
}

fn claim(state: &mut Snapshot, i: usize, request: &Value, now: i64) -> Result<()> {
    let task = &state.tasks[i];
    ensure!(
        task.status.allows(TaskStatus::InProgress),
        "conflict: Task cannot be claimed from {}",
        task.status
    );
    let executor = required(request, "executor")?;
    let session = required(request, "session")?;
    ensure!(
        !state
            .leases
            .iter()
            .any(|l| l.task_id == task.id
                || (l.executor_ref == executor && l.session_ref == session)),
        "conflict: Task or executor session already leased"
    );
    let delegated_by = request["delegated_by"].as_str().map(str::to_owned);
    state.leases.push(Lease {
        task_id: task.id.clone(),
        executor_ref: executor.into(),
        session_ref: session.into(),
        delegated_by: delegated_by.clone(),
        token: new_id("lease"),
        lease_expires_at: now + DEFAULT_LEASE_SECONDS,
    });
    let task = &mut state.tasks[i];
    task.status = TaskStatus::InProgress;
    task.phase = Some(TaskPhase::Planning);
    task.system_block = false;
    task.reason = None;
    task.last_executor = Some(executor.into());
    task.last_session = Some(session.into());
    task.delegated_by = delegated_by;
    Ok(())
}

fn depends_on(state: &Snapshot, source: &str, target: &str) -> bool {
    let mut pending = vec![source.to_owned()];
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(task) = state.tasks.iter().find(|t| t.id == id) {
            if task.dependencies.iter().any(|d| d == target) {
                return true;
            }
            pending.extend(task.dependencies.iter().cloned());
        }
    }
    false
}

fn aggregate_job(state: &mut Snapshot, index: usize, now: i64) {
    let job = &mut state.jobs[index];
    let tasks: Vec<_> = state
        .tasks
        .iter()
        .filter(|t| t.job_id == job.id && t.status != TaskStatus::Cancelled)
        .collect();
    if job.status == JobStatus::Active
        && !tasks.is_empty()
        && tasks.iter().all(|t| t.status == TaskStatus::Done)
    {
        job.status = JobStatus::Completed;
        job.completed_at = Some(now);
    }
    job.revision += 1;
    job.updated_at = now;
}

fn session(state: &mut Snapshot, request: &Value, now: i64) -> Result<Value> {
    let session = required(request, "session")?;
    let command = required(request, "command")?;
    let mut changed = Vec::new();
    for i in 0..state.tasks.len() {
        if state.tasks[i].last_session.as_deref() != Some(session) {
            continue;
        }
        if matches!(command, "session.end" | "session.interrupt")
            && state.tasks[i].status == TaskStatus::InProgress
        {
            let reason = if command == "session.interrupt" {
                "session interrupted"
            } else {
                "session ended"
            };
            system_block(state, i, reason, now);
            changed.push(state.tasks[i].id.clone());
        } else if command == "session.start"
            && state.tasks[i].system_block
            && state.tasks[i].status == TaskStatus::Blocked
        {
            let task = state.tasks[i].clone();
            let req = json!({"executor":task.last_executor,"session":session,"delegated_by":task.delegated_by});
            if claim(state, i, &req, now).is_ok() {
                state.tasks[i].revision += 1;
                state.tasks[i].updated_at = now;
                changed.push(task.id);
            }
        } else if command == "session.heartbeat" {
            for lease in state
                .leases
                .iter_mut()
                .filter(|l| l.task_id == state.tasks[i].id && l.session_ref == session)
            {
                lease.lease_expires_at = now + DEFAULT_LEASE_SECONDS;
            }
        }
    }
    Ok(json!({"session_ref":session,"tasks":changed}))
}

pub(crate) fn system_block(state: &mut Snapshot, index: usize, reason: &str, now: i64) {
    let task = &mut state.tasks[index];
    task.status = TaskStatus::Blocked;
    task.phase = None;
    task.reason = Some(reason.into());
    task.system_block = true;
    task.revision += 1;
    task.updated_at = now;
    state.leases.retain(|l| l.task_id != task.id);
}
