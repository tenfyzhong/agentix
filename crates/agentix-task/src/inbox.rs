use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

use crate::mutations::{check_revision, required};
use crate::{
    DEFAULT_LEASE_SECONDS, InboxEntry, InboxLease, InboxStatus, JobStatus, Snapshot, TaskStatus,
    WriteOptions, new_id,
};

impl Snapshot {
    #[must_use]
    pub fn cancelled_inboxes_for_session(&self, session: &str) -> Vec<&InboxEntry> {
        self.inboxes
            .iter()
            .filter(|entry| {
                entry.status == InboxStatus::Cancelled
                    && (entry.last_session.as_deref() == Some(session)
                        || self
                            .query_context
                            .cancelled_inboxes
                            .get(session)
                            .is_some_and(|ids| ids.contains(&entry.id))
                        || self.tasks.iter().any(|task| {
                            Some(&task.job_id) == entry.job_id.as_ref()
                                && task.last_session.as_deref() == Some(session)
                        }))
            })
            .collect()
    }
}

pub(crate) fn index(state: &Snapshot, id: &str) -> Result<usize> {
    if let Some(i) = state.inboxes.iter().position(|e| e.id == id) {
        return Ok(i);
    }
    let matches: Vec<_> = state
        .inboxes
        .iter()
        .enumerate()
        .filter(|(_, e)| !id.is_empty() && e.id.starts_with(id))
        .map(|(i, _)| i)
        .collect();
    ensure!(
        matches.len() == 1,
        "not_found or ambiguous: Inbox entry {id}"
    );
    Ok(matches[0])
}

fn changed(entry: &mut InboxEntry, now: i64) {
    entry.revision += 1;
    entry.updated_at = now;
}

/// Only authored Job prompts authorize matching Inbox intake.
pub(crate) fn has_job_prompt(request: &Value) -> bool {
    matches!(
        request["command"].as_str(),
        Some("job.create" | "job.update" | "job.followup")
    ) && request["prompt"]
        .as_str()
        .is_some_and(|prompt| !prompt.trim().is_empty())
}

pub(crate) fn link_prompt(
    state: &mut Snapshot,
    job_id: &str,
    prompt: &str,
    options: &WriteOptions,
    now: i64,
) -> Result<()> {
    let job = &state.jobs[state.job_index(job_id)?];
    for entry in &mut state.inboxes {
        if entry.project_id != job.project_id
            || entry.deleted
            || !entry.published
            || entry.content_pending
            || entry.status != InboxStatus::Todo
            || entry.job_id.is_some()
            || entry.lease.is_some()
            || entry.content.trim().is_empty()
            || !prompt.contains(entry.content.trim())
        {
            continue;
        }
        entry.job_id = Some(job.id.clone());
        entry.status = InboxStatus::Active;
        entry.last_session.clone_from(&options.session_ref);
        if options.actor_ref.starts_with("agent:") {
            entry.lease = options.session_ref.as_ref().map(|session| InboxLease {
                executor_ref: options.actor_ref.clone(),
                session_ref: session.clone(),
                token: new_id("lease"),
                lease_expires_at: now + DEFAULT_LEASE_SECONDS,
            });
        }
        changed(entry, now);
    }
    Ok(())
}

fn insert(
    state: &mut Snapshot,
    project: &str,
    content: &str,
    actor: &str,
    id: String,
    now: i64,
) -> InboxEntry {
    let entry = InboxEntry {
        id,
        project_id: project.into(),
        content: content.trim().into(),
        source: None,
        source_version: 0,
        content_pending: false,
        position: state
            .inboxes
            .iter()
            .filter(|e| e.project_id == project)
            .map(|e| e.position)
            .max()
            .unwrap_or(-1)
            + 1,
        status: InboxStatus::Todo,
        job_id: None,
        lease: None,
        actor_ref: actor.into(),
        last_session: None,
        revision: 1,
        created_at: now,
        updated_at: now,
        deleted: false,
        published: false,
    };
    state.inboxes.push(entry.clone());
    entry
}

pub(crate) fn apply(
    state: &mut Snapshot,
    request: &Value,
    options: &WriteOptions,
    now: i64,
) -> Result<Value> {
    let command = required(request, "command")?;
    if command == "inbox.edit" {
        return edit(state, request, options, now);
    }
    if matches!(
        command,
        "inbox.cancel" | "inbox.release" | "inbox.set-status"
    ) {
        let i = index(state, required(request, "inbox")?)?;
        check_revision(state.inboxes[i].revision, options)?;
        if command == "inbox.set-status" {
            set_status(state, i, required(request, "status")?, now)?;
        } else if command == "inbox.cancel" {
            cancel(state, i, false, now);
        } else {
            let lease = state.inboxes[i]
                .lease
                .as_ref()
                .context("conflict: Inbox has no lease")?;
            ensure!(
                options.session_ref.as_ref() == Some(&lease.session_ref)
                    && options.lease_token.as_ref() == Some(&lease.token)
                    && lease.lease_expires_at > now,
                "conflict: invalid Inbox lease"
            );
            release(state, i, now);
        }
        return Ok(serde_json::to_value(&state.inboxes[i])?);
    }
    let project = &state.projects[state.project_index(required(request, "project")?)?];
    let project_id = project.id.clone();
    if !matches!(
        command,
        "inbox.list" | "inbox.sync" | "inbox.import" | "inbox.publish"
    ) {
        ensure!(
            project.archived_at.is_none(),
            "conflict: Project is archived"
        );
    }
    match command {
        "inbox.add" => {
            let content = required(request, "content")?;
            ensure!(
                !content.contains("<!-- taskcli:"),
                "invalid: Inbox content contains reserved taskcli control markers"
            );
            let mut entry = insert(
                state,
                &project_id,
                content,
                &options.actor_ref,
                new_id("inbox"),
                now,
            );
            entry.source = request["source"]
                .as_str()
                .or_else(|| {
                    options
                        .idempotency_key
                        .as_deref()
                        .and_then(|key| key.strip_prefix("im:inbox:"))
                })
                .map(str::to_owned);
            *state.inboxes.last_mut().unwrap() = entry.clone();
            Ok(serde_json::to_value(entry)?)
        }
        "inbox.list" | "inbox.sync" => {
            let mut entries: Vec<_> = state
                .inboxes
                .iter()
                .filter(|e| e.project_id == project_id && !e.deleted)
                .collect();
            entries.sort_by_key(|e| e.position);
            Ok(serde_json::to_value(entries)?)
        }
        "inbox.claim-next" => claim_next(state, &project_id, options, now),
        "inbox.import" => import(state, &project_id, request, now),
        "inbox.publish" => {
            for id in request["ids"]
                .as_array()
                .context("invalid: published IDs")?
            {
                let i = index(state, id.as_str().context("invalid: Inbox ID")?)?;
                ensure!(
                    state.inboxes[i].project_id == project_id,
                    "conflict: Inbox Project mismatch"
                );
                state.inboxes[i].published = true;
                state.inboxes[i].content_pending = false;
            }
            Ok(json!({"published":true}))
        }
        _ => bail!("invalid: unknown command {command}"),
    }
}

fn import(state: &mut Snapshot, project: &str, request: &Value, now: i64) -> Result<Value> {
    let rows = request["entries"]
        .as_array()
        .context("invalid: Inbox entries")?;
    let mut seen = std::collections::BTreeSet::new();
    for (position, row) in rows.iter().enumerate() {
        let id = required(row, "id")?;
        ensure!(seen.insert(id), "conflict: duplicate Inbox ID");
        let content = required(row, "content")?;
        let i = if let Some(i) = state.inboxes.iter().position(|e| e.id == id) {
            i
        } else {
            insert(state, project, content, "user:inbox", id.into(), now);
            state.inboxes.len() - 1
        };
        let entry = &mut state.inboxes[i];
        ensure!(
            entry.project_id == project,
            "conflict: Inbox Project mismatch"
        );
        // Tombstones are sticky, including when an editor restores an old buffer.
        if entry.deleted {
            continue;
        }
        let old = entry.clone();
        entry.position = i64::try_from(position)?;
        if !entry.content_pending && entry.status == InboxStatus::Todo && entry.job_id.is_none() {
            entry.content = content.into();
        }
        entry.published = true;
        if *entry != old {
            changed(entry, now);
        }
        if row["cancelled"] == true {
            cancel(state, i, false, now);
        }
    }
    for i in 0..state.inboxes.len() {
        let entry = &state.inboxes[i];
        if entry.project_id == project
            && entry.published
            && !entry.deleted
            && !seen.contains(entry.id.as_str())
        {
            cancel(state, i, true, now);
        }
    }
    Ok(json!({"synced":true}))
}

fn claim_next(
    state: &mut Snapshot,
    project: &str,
    options: &WriteOptions,
    now: i64,
) -> Result<Value> {
    let session = options
        .session_ref
        .as_deref()
        .context("invalid: Inbox claim requires --session")?;
    ensure!(
        options.actor_ref.starts_with("agent:"),
        "invalid: Inbox claim requires --executor agent:HOST"
    );
    let candidates: Vec<_> = state
        .inboxes
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.project_id == project
                && !e.deleted
                && e.published
                && e.lease.is_none()
                && matches!(e.status, InboxStatus::Todo | InboxStatus::Active)
        })
        .map(|(i, e)| (i, e.job_id.is_none(), e.position))
        .collect();
    let Some((i, _, _)) = candidates
        .into_iter()
        .min_by_key(|(_, fresh, position)| (*fresh, *position))
    else {
        return Ok(json!({"claimed":false,"reason":"empty"}));
    };
    let linked = state.inboxes[i].job_id.as_deref();
    if state.jobs.iter().any(|j| {
        j.project_id == project && j.status == JobStatus::Active && Some(j.id.as_str()) != linked
    }) {
        return Ok(json!({"claimed":false,"reason":"active_jobs"}));
    }
    if linked.is_some_and(|job| {
        state
            .tasks
            .iter()
            .any(|t| t.job_id == job && state.leases.iter().any(|l| l.task_id == t.id))
    }) {
        return Ok(json!({"claimed":false,"reason":"leased_tasks"}));
    }
    let job = if let Some(id) = linked {
        serde_json::to_value(&state.jobs[state.job_index(id)?])?
    } else {
        let entry = &state.inboxes[i];
        let request = json!({"project":project,"title":entry.title(),"goal":entry.content});
        crate::mutations::create_job(state, &request, options, now)?
    };
    let entry = &mut state.inboxes[i];
    entry.job_id = job["id"].as_str().map(str::to_owned);
    entry.status = InboxStatus::Active;
    entry.last_session = Some(session.into());
    entry.lease = Some(InboxLease {
        executor_ref: options.actor_ref.clone(),
        session_ref: session.into(),
        token: new_id("lease"),
        lease_expires_at: now + DEFAULT_LEASE_SECONDS,
    });
    changed(entry, now);
    Ok(json!({"claimed":true,"entry":entry,"job":job}))
}

fn release(state: &mut Snapshot, i: usize, now: i64) {
    let job = state.inboxes[i].job_id.clone();
    let session = state.inboxes[i]
        .lease
        .as_ref()
        .map(|l| l.session_ref.clone());
    for t in 0..state.tasks.len() {
        if job.as_ref() == Some(&state.tasks[t].job_id)
            && state.tasks[t].status == TaskStatus::InProgress
            && state
                .leases
                .iter()
                .any(|l| l.task_id == state.tasks[t].id && Some(&l.session_ref) == session.as_ref())
        {
            crate::mutations::system_block(state, t, "Inbox ownership released", now);
        }
    }
    let entry = &mut state.inboxes[i];
    entry.lease = None;
    entry.status = InboxStatus::Todo;
    changed(entry, now);
}

fn set_status(state: &mut Snapshot, i: usize, target: &str, now: i64) -> Result<()> {
    let target = match target {
        "TODO" => InboxStatus::Todo,
        "ACTIVE" | "IN_PROGRESS" => InboxStatus::Active,
        "PENDING_REVIEW" => InboxStatus::PendingReview,
        "COMPLETED" | "DONE" => InboxStatus::Completed,
        "CANCELLED" => InboxStatus::Cancelled,
        _ => bail!("invalid: use TODO, ACTIVE, PENDING_REVIEW, COMPLETED or CANCELLED"),
    };
    let entry = &state.inboxes[i];
    ensure!(!entry.deleted, "conflict: Inbox entry was withdrawn");
    let project = &state.projects[state.project_index(&entry.project_id)?];
    ensure!(
        project.archived_at.is_none(),
        "conflict: Project is archived"
    );
    let job_index = entry
        .job_id
        .as_ref()
        .map(|id| state.job_index(id))
        .transpose()?;
    if let Some(j) = job_index {
        ensure!(
            state.jobs[j].archived_at.is_none(),
            "conflict: Job is archived"
        );
    }
    if entry.status == target {
        return Ok(());
    }
    match target {
        InboxStatus::Todo => {
            ensure!(
                entry.lease.is_none()
                    && !state
                        .tasks
                        .iter()
                        .any(|t| Some(&t.job_id) == entry.job_id.as_ref()
                            && state.leases.iter().any(|l| l.task_id == t.id)),
                "conflict: release the active Inbox and Task leases before returning to TODO"
            );
            if let Some(j) = job_index {
                activate_job(state, j, now)?;
            }
        }
        InboxStatus::Active => {
            if let Some(j) = job_index {
                activate_job(state, j, now)?;
            }
        }
        InboxStatus::PendingReview => {
            if let Some(j) = job_index {
                crate::mutations::review_job(state, j, &json!({"command":"job.submit"}), now)?;
            }
        }
        InboxStatus::Completed => {
            ensure!(
                entry.status != InboxStatus::Cancelled,
                "conflict: reopen the cancelled entry first"
            );
            if let Some(j) = job_index {
                crate::mutations::review_job(state, j, &json!({"command":"job.approve"}), now)?;
            }
        }
        InboxStatus::Cancelled => {
            ensure!(
                entry.status != InboxStatus::Completed,
                "conflict: reopen the completed entry before cancelling"
            );
            cancel(state, i, false, now);
            return Ok(());
        }
    }
    let entry = &mut state.inboxes[i];
    entry.status = target;
    entry.lease = None;
    changed(entry, now);
    Ok(())
}

fn activate_job(state: &mut Snapshot, j: usize, now: i64) -> Result<()> {
    if state.jobs[j].status == JobStatus::PendingReview {
        crate::mutations::review_job(
            state,
            j,
            &json!({"command":"job.reject","reason":"Inbox status changed: verification rejected"}),
            now,
        )?;
    } else if state.jobs[j].status.terminal() {
        let job = &mut state.jobs[j];
        job.status = JobStatus::Active;
        job.completed_at = None;
        job.cancelled_at = None;
        job.review_reason = Some("Inbox entry reopened".into());
        job.revision += 1;
        job.updated_at = now;
    }
    Ok(())
}

fn cancel(state: &mut Snapshot, i: usize, deleted: bool, now: i64) {
    let entry = &mut state.inboxes[i];
    let old = entry.clone();
    entry.deleted |= deleted;
    if !matches!(
        entry.status,
        InboxStatus::Completed | InboxStatus::Cancelled
    ) {
        entry.status = InboxStatus::Cancelled;
        entry.lease = None;
        if let Some(job_id) = &entry.job_id {
            let tasks: std::collections::BTreeSet<_> = state
                .tasks
                .iter()
                .filter(|t| &t.job_id == job_id)
                .map(|t| t.id.clone())
                .collect();
            state.leases.retain(|l| !tasks.contains(&l.task_id));
            for task in state
                .tasks
                .iter_mut()
                .filter(|t| &t.job_id == job_id && !t.status.terminal())
            {
                task.status = TaskStatus::Cancelled;
                task.phase = None;
                task.reason = Some(
                    if deleted {
                        "Inbox entry withdrawn"
                    } else {
                        "Inbox entry cancelled"
                    }
                    .into(),
                );
                task.system_block = false;
                task.completed_at = Some(now);
                task.updated_at = now;
                task.revision += 1;
            }
            if let Some(job) = state
                .jobs
                .iter_mut()
                .find(|j| &j.id == job_id && !j.status.terminal())
            {
                job.status = JobStatus::Cancelled;
                job.cancelled_at = Some(now);
                job.updated_at = now;
                job.revision += 1;
            }
        }
    }
    if *entry != old {
        changed(entry, now);
    }
}

pub(crate) fn refresh(state: &mut Snapshot, now: i64) {
    for i in 0..state.inboxes.len() {
        let entry = &state.inboxes[i];
        if matches!(
            entry.status,
            InboxStatus::Completed | InboxStatus::Cancelled
        ) {
            continue;
        }
        let job = entry
            .job_id
            .as_ref()
            .and_then(|id| state.jobs.iter().find(|j| &j.id == id));
        if job.is_some_and(|j| j.status == JobStatus::Completed) {
            let entry = &mut state.inboxes[i];
            entry.status = InboxStatus::Completed;
            entry.lease = None;
            changed(entry, now);
        } else if (entry.job_id.is_some() && job.is_none())
            || job.is_some_and(|j| j.status == JobStatus::Cancelled)
        {
            cancel(state, i, false, now);
        } else if job.is_some_and(|j| j.status == JobStatus::PendingReview) {
            let entry = &mut state.inboxes[i];
            if entry.lease.is_some() || entry.status != InboxStatus::PendingReview {
                entry.lease = None;
                entry.status = InboxStatus::PendingReview;
                changed(entry, now);
            }
        } else if job.is_some_and(|j| j.status == JobStatus::Active)
            && entry.status == InboxStatus::PendingReview
        {
            let entry = &mut state.inboxes[i];
            entry.status = InboxStatus::Active;
            changed(entry, now);
        } else if entry
            .lease
            .as_ref()
            .is_some_and(|l| l.lease_expires_at <= now)
        {
            release(state, i, now);
        }
    }
}

pub(crate) fn session(state: &mut Snapshot, session: &str, command: &str, now: i64) {
    for i in 0..state.inboxes.len() {
        if state.inboxes[i]
            .lease
            .as_ref()
            .is_none_or(|l| l.session_ref != session)
        {
            continue;
        }
        if command == "session.heartbeat" {
            if let Some(lease) = &mut state.inboxes[i].lease {
                lease.lease_expires_at = now + DEFAULT_LEASE_SECONDS;
            }
        } else if matches!(command, "session.end" | "session.interrupt") {
            release(state, i, now);
        }
    }
}

fn edit(state: &mut Snapshot, request: &Value, options: &WriteOptions, now: i64) -> Result<Value> {
    let i = index(state, required(request, "inbox")?)?;
    let entry = &state.inboxes[i];
    check_revision(entry.revision, options)?;
    ensure!(
        entry.actor_ref == options.actor_ref
            && entry.source.as_deref() == Some(required(request, "source")?),
        "conflict: Inbox source owner mismatch"
    );
    ensure!(
        !entry.deleted
            && state.projects[state.project_index(&entry.project_id)?]
                .archived_at
                .is_none(),
        "conflict: Inbox entry is withdrawn or archived"
    );
    let content = required(request, "content")?;
    ensure!(
        !content.contains("<!-- taskcli:"),
        "invalid: Inbox content contains reserved taskcli control markers"
    );
    let version = request["version"]
        .as_i64()
        .filter(|v| *v > 0)
        .context("invalid: source version")?;
    if version <= entry.source_version {
        return Ok(serde_json::to_value(entry)?);
    }
    let entry = &mut state.inboxes[i];
    entry.content = content.trim().into();
    entry.source_version = version;
    entry.content_pending = true;
    changed(entry, now);
    Ok(serde_json::to_value(entry)?)
}
