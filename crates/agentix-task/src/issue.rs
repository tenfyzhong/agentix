//! Project deliverables are independent from individual execution rounds.
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

use crate::{
    Issue, IssueStatus, JobStatus, Milestone, MilestoneStatus, Snapshot, TaskStatus, WriteOptions,
    mutations::{check_revision, entity_name, required},
    naming::numbered_name,
    new_id,
};

pub(crate) fn validate_public_hashes(request: &Value) -> Result<()> {
    if matches!(
        request["command"].as_str(),
        Some("issue.create" | "issue.update")
    ) {
        ensure!(
            request.get("content_hash").is_none() && request.get("review_content_hash").is_none(),
            "invalid: Issue content hashes are reserved for document synchronization"
        );
    }
    Ok(())
}

pub(crate) fn apply_content_hashes(
    issue: &mut Issue,
    hash: &str,
    review_hash: &str,
) -> Result<bool> {
    let changed = issue.content_hash != hash;
    ensure!(
        !issue.status.terminal() || !changed,
        "conflict: reopen terminal Issue before updating content"
    );
    if issue.status == IssueStatus::PendingReview && issue.review_content_hash != review_hash {
        issue.status = IssueStatus::Active;
        issue.pending_review_at = None;
        issue.review_reason = None;
    }
    issue.content_hash = hash.into();
    issue.review_content_hash = review_hash.into();
    Ok(changed)
}

fn human(options: &WriteOptions) -> Result<()> {
    ensure!(
        options.actor_ref.starts_with("user:"),
        "forbidden: Issue acceptance requires a human actor"
    );
    Ok(())
}

fn optional_id(state: &Snapshot, request: &Value, key: &str) -> Result<Option<String>> {
    match request.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(id)) => Ok(Some(
            state.milestones[state.milestone_index(id)?].id.clone(),
        )),
        _ => bail!("invalid: {key} must be a single identifier or null"),
    }
}

fn milestone_project(state: &Snapshot, id: Option<&str>, project: &str) -> Result<()> {
    if let Some(id) = id {
        ensure!(
            state.milestones[state.milestone_index(id)?].project_id == project,
            "invalid: Milestone must belong to the same Project"
        );
    }
    Ok(())
}

fn sequence(state: &Snapshot, project: &str, kind: &str, now: i64) -> Result<u64> {
    let current = state
        .query_context
        .sequences
        .get(&(project.into(), kind.into()))
        .copied()
        .unwrap_or(0);
    let retained = state
        .document_sequences
        .get(&crate::deletion::sequence_key(project, kind, now))
        .copied()
        .unwrap_or(0);
    current
        .max(retained)
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("invalid: document sequence exhausted"))
}

#[allow(clippy::too_many_lines)] // Keep lifecycle validation and atomic transitions together.
pub(crate) fn apply(
    state: &mut Snapshot,
    request: &Value,
    options: &WriteOptions,
    now: i64,
) -> Result<Value> {
    let command = required(request, "command")?;
    ensure!(
        ["requirement", "acceptance", "notes"]
            .iter()
            .all(|key| request.get(key).is_none()),
        "invalid: Issue content must be written through the document service"
    );
    if command == "issue.create" {
        let project = &state.projects[state.project_index(required(request, "project")?)?];
        ensure!(
            project.archived_at.is_none(),
            "conflict: Project is archived"
        );
        let title = required(request, "title")?;
        let name = entity_name(
            state,
            &project.id,
            "issue",
            None,
            request["name"].as_str().unwrap_or(title),
        );
        let sequence = sequence(state, &project.id, "issue", now)?;
        let milestone_id = optional_id(state, request, "milestone")?;
        milestone_project(state, milestone_id.as_deref(), &project.id)?;
        let issue = Issue {
            issue_type: serde_json::from_value(request["issue_type"].clone())
                .map_err(|_| anyhow::anyhow!("invalid: unknown Issue type"))?,
            id: request["id"]
                .as_str()
                .map_or_else(|| new_id("issue"), str::to_owned),
            project_id: project.id.clone(),
            milestone_id,
            title: title.into(),
            document_path: format!(
                "{}/Issues/{}.md",
                project.document_directory(),
                numbered_name(&name, now, sequence)?
            ),
            name,
            sequence,
            content_hash: request["content_hash"].as_str().unwrap_or_default().into(),
            review_content_hash: request["review_content_hash"]
                .as_str()
                .unwrap_or_default()
                .into(),
            status: IssueStatus::Todo,
            review_reason: None,
            revision: 1,
            created_at: now,
            updated_at: now,
            started_at: None,
            pending_review_at: None,
            completed_at: None,
            cancelled_at: None,
            archived_at: None,
        };
        let result = json!(issue);
        state.issues.push(issue);
        return Ok(result);
    }
    let index = state.issue_index(required(request, "issue")?)?;
    let original = state.issues[index].clone();
    check_revision(original.revision, options)?;
    let project = &state.projects[state.project_index(&original.project_id)?];
    ensure!(
        project.archived_at.is_none(),
        "conflict: Project is archived"
    );
    if command != "issue.unarchive" {
        ensure!(
            original.archived_at.is_none(),
            "conflict: Issue is archived"
        );
    }
    let renamed = if request.get("name").is_some() {
        Some(entity_name(
            state,
            &original.project_id,
            "issue",
            Some(&original.id),
            required(request, "name")?,
        ))
    } else {
        None
    };
    let milestone = optional_id(state, request, "milestone")?;
    if request.get("milestone").is_some() {
        milestone_project(state, milestone.as_deref(), &original.project_id)?;
    }
    match command {
        "issue.update" => {
            let issue = &mut state.issues[index];
            if let Some(name) = renamed {
                let parent = issue
                    .document_path
                    .rsplit_once('/')
                    .ok_or_else(|| anyhow::anyhow!("invalid: Issue path"))?
                    .0;
                issue.document_path = format!(
                    "{parent}/{}.md",
                    numbered_name(&name, issue.created_at, issue.sequence)?
                );
                issue.name = name;
            }
            if request.get("title").is_some() {
                issue.title = required(request, "title")?.into();
            }
            apply_content_hashes(
                issue,
                request["content_hash"]
                    .as_str()
                    .unwrap_or(&original.content_hash),
                request["review_content_hash"]
                    .as_str()
                    .unwrap_or(&original.review_content_hash),
            )?;
            if request.get("milestone").is_some() {
                issue.milestone_id = milestone;
            }
            if request.get("issue_type").is_some() {
                let issue_type = serde_json::from_value(request["issue_type"].clone())
                    .map_err(|_| anyhow::anyhow!("invalid: unknown Issue type"))?;
                ensure!(
                    !issue.status.terminal() || issue.issue_type == issue_type,
                    "conflict: reopen terminal Issue before updating type"
                );
                issue.issue_type = issue_type;
            }
        }
        "issue.start" => {
            human(options)?;
            ensure!(
                original.status == IssueStatus::Todo,
                "conflict: only a TODO Issue can be started"
            );
            activate(state, index, now);
            return Ok(json!(state.issues[index]));
        }
        "issue.submit" => {
            ensure!(
                matches!(original.status, IssueStatus::Todo | IssueStatus::Active),
                "conflict: Issue must be TODO or ACTIVE to submit"
            );
            terminal_jobs(state, &original.id)?;
            state.issues[index].status = IssueStatus::PendingReview;
            state.issues[index].pending_review_at = Some(now);
            state.issues[index].review_reason = None;
        }
        "issue.approve" => {
            human(options)?;
            ensure!(
                original.status == IssueStatus::PendingReview,
                "conflict: Issue must be PENDING_REVIEW"
            );
            terminal_jobs(state, &original.id)?;
            state.issues[index].status = IssueStatus::Completed;
            state.issues[index].completed_at = Some(now);
        }
        "issue.reject" => {
            human(options)?;
            ensure!(
                original.status == IssueStatus::PendingReview,
                "conflict: Issue must be PENDING_REVIEW"
            );
            let reason = required(request, "reason")?;
            state.issues[index].status = IssueStatus::Active;
            state.issues[index].review_reason = Some(reason.into());
            state.issues[index].pending_review_at = None;
        }
        "issue.cancel" => {
            human(options)?;
            ensure!(
                !original.status.terminal(),
                "conflict: reopen terminal Issue before cancelling"
            );
            cancel_jobs(state, &original.id, now);
            state.issues[index].status = IssueStatus::Cancelled;
            state.issues[index].cancelled_at = Some(now);
        }
        "issue.reopen" => {
            human(options)?;
            ensure!(
                original.status.terminal(),
                "conflict: only terminal Issues can be reopened"
            );
            let issue = &mut state.issues[index];
            issue.status = IssueStatus::Todo;
            issue.started_at = None;
            issue.pending_review_at = None;
            issue.completed_at = None;
            issue.cancelled_at = None;
            issue.review_reason = None;
        }
        "issue.archive" | "issue.unarchive" => {
            let archive = command == "issue.archive";
            ensure!(
                archive == original.archived_at.is_none(),
                "conflict: Issue archive state unchanged"
            );
            ensure!(
                original.status.terminal(),
                "conflict: complete or cancel Issue before archiving"
            );
            let issue = &mut state.issues[index];
            issue.archived_at = archive.then_some(now);
            issue.document_path = format!(
                "{}/Issues/{}{}.md",
                project.document_directory(),
                if archive { "Archived/" } else { "" },
                numbered_name(&issue.name, issue.created_at, issue.sequence)?
            );
        }
        _ => bail!("invalid: unknown command {command}"),
    }
    state.issues[index].revision += 1;
    state.issues[index].updated_at = now;
    Ok(json!(state.issues[index]))
}

fn terminal_jobs(state: &Snapshot, issue: &str) -> Result<()> {
    ensure!(
        !state
            .query_context
            .issue_active_jobs
            .get(issue)
            .copied()
            .unwrap_or(false)
            && state
                .jobs
                .iter()
                .filter(|job| job.issue_id.as_deref() == Some(issue))
                .all(|job| job.status.terminal()),
        "conflict: all associated Jobs must be terminal before Issue acceptance"
    );
    Ok(())
}

fn cancel_jobs(state: &mut Snapshot, issue: &str, now: i64) {
    let jobs: std::collections::BTreeSet<_> = state
        .jobs
        .iter()
        .filter(|j| j.issue_id.as_deref() == Some(issue) && !j.status.terminal())
        .map(|j| j.id.clone())
        .collect();
    let tasks: std::collections::BTreeSet<_> = state
        .tasks
        .iter()
        .filter(|t| jobs.contains(&t.job_id))
        .map(|t| t.id.clone())
        .collect();
    state.leases.retain(|lease| !tasks.contains(&lease.task_id));
    for task in state
        .tasks
        .iter_mut()
        .filter(|t| tasks.contains(&t.id) && !t.status.terminal())
    {
        task.status = TaskStatus::Cancelled;
        task.phase = None;
        task.completed_at = Some(now);
        task.reason = Some("Associated Issue cancelled by a human".into());
        task.system_block = false;
        task.updated_at = now;
        task.revision += 1;
    }
    for job in state.jobs.iter_mut().filter(|j| jobs.contains(&j.id)) {
        job.status = JobStatus::Cancelled;
        job.cancelled_at = Some(now);
        job.updated_at = now;
        job.revision += 1;
    }
}

pub(crate) fn writable(state: &Snapshot, issue: &str) -> Result<usize> {
    let index = state.issue_index(issue)?;
    ensure!(
        matches!(
            state.issues[index].status,
            IssueStatus::Todo | IssueStatus::Active
        ) && state.issues[index].archived_at.is_none(),
        "conflict: reject pending review or reopen terminal Issue before new work"
    );
    Ok(index)
}

pub(crate) fn activate(state: &mut Snapshot, index: usize, now: i64) {
    if state.issues[index].status == IssueStatus::Todo {
        let issue = &mut state.issues[index];
        issue.status = IssueStatus::Active;
        issue.started_at = Some(now);
        issue.updated_at = now;
        issue.revision += 1;
    }
}

pub(crate) fn associate(state: &mut Snapshot, job: &str, request: &Value, now: i64) -> Result<()> {
    let Some(value) = request.get("issue") else {
        return Ok(());
    };
    let j = state.job_index(job)?;
    let id = match value {
        Value::Null => None,
        Value::String(id) => Some(state.issues[state.issue_index(id)?].id.clone()),
        _ => bail!("invalid: issue must be a single identifier or null"),
    };
    if let Some(id) = &id {
        let i = state.issue_index(id)?;
        if let Some(revision) = request.get("issue_revision").filter(|v| !v.is_null()) {
            ensure!(
                revision.as_i64() == Some(state.issues[i].revision),
                "conflict: selected Issue revision changed"
            );
        }
        if let Some(hash) = request.get("issue_content_hash").filter(|v| !v.is_null()) {
            ensure!(
                hash.as_str() == Some(state.issues[i].content_hash.as_str()),
                "conflict: selected Issue requirement changed"
            );
        }
        ensure!(
            state.issues[i].project_id == state.jobs[j].project_id,
            "invalid: Issue must belong to the same Project"
        );
        if !state.jobs[j].status.terminal() {
            writable(state, id)?;
            if state.jobs[j].started_at.is_some() {
                activate(state, i, now);
            }
        }
    }
    state.jobs[j].issue_id = id;
    Ok(())
}

fn due_date(request: &Value) -> Result<Option<String>> {
    match request.get("due_date") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(date)) => {
            let format = time::format_description::parse_borrowed::<2>("[year]-[month]-[day]")?;
            time::Date::parse(date, &format)
                .map_err(|_| anyhow::anyhow!("invalid: due_date must be YYYY-MM-DD"))?;
            Ok(Some(date.clone()))
        }
        _ => bail!("invalid: due_date must be YYYY-MM-DD or null"),
    }
}

#[allow(clippy::too_many_lines)] // Basic Milestone create/update/delete share one guarded dispatch.
pub(crate) fn milestone(
    state: &mut Snapshot,
    request: &Value,
    options: &WriteOptions,
    now: i64,
) -> Result<Value> {
    let command = required(request, "command")?;
    if command == "milestone.create" {
        let project = &state.projects[state.project_index(required(request, "project")?)?];
        ensure!(
            project.archived_at.is_none(),
            "conflict: Project is archived"
        );
        let title = required(request, "title")?;
        let name = entity_name(
            state,
            &project.id,
            "milestone",
            None,
            request["name"].as_str().unwrap_or(title),
        );
        let sequence = sequence(state, &project.id, "milestone", now)?;
        let milestone = Milestone {
            status: MilestoneStatus::Planned,
            started_at: None,
            completed_at: None,
            cancelled_at: None,
            archived_at: None,
            id: new_id("milestone"),
            project_id: project.id.clone(),
            title: title.into(),
            document_path: format!(
                "{}/Milestones/{}.md",
                project.document_directory(),
                numbered_name(&name, now, sequence)?
            ),
            name,
            sequence,
            goal: request["goal"].as_str().unwrap_or_default().into(),
            order: match request.get("order") {
                None => 0,
                Some(order) => order
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("invalid: order must be an integer"))?,
            },
            due_date: due_date(request)?,
            revision: 1,
            created_at: now,
            updated_at: now,
        };
        let result = json!(milestone);
        state.milestones.push(milestone);
        return Ok(result);
    }
    let i = state.milestone_index(required(request, "milestone")?)?;
    let original = &state.milestones[i];
    check_revision(original.revision, options)?;
    ensure!(
        state.projects[state.project_index(&original.project_id)?]
            .archived_at
            .is_none(),
        "conflict: Project is archived"
    );
    if command != "milestone.unarchive" {
        ensure!(
            original.archived_at.is_none(),
            "conflict: Milestone is archived"
        );
    }
    if !matches!(command, "milestone.update" | "milestone.delete") {
        return milestone_transition(state, i, command, now);
    }
    if command == "milestone.delete" {
        ensure!(
            !state
                .issues
                .iter()
                .any(|issue| issue.milestone_id.as_deref() == Some(original.id.as_str())),
            "conflict: unlink or reassign Issues before deleting Milestone"
        );
        let removed = state.milestones.remove(i);
        let key =
            crate::deletion::sequence_key(&removed.project_id, "milestone", removed.created_at);
        let number = state.document_sequences.entry(key).or_default();
        *number = (*number).max(removed.sequence);
        return Ok(json!({"id":removed.id,"deleted":true}));
    }
    ensure!(
        command == "milestone.update",
        "invalid: unknown command {command}"
    );
    let name = request.get("name").map(|_| {
        entity_name(
            state,
            &original.project_id,
            "milestone",
            Some(&original.id),
            request["name"].as_str().unwrap_or_default(),
        )
    });
    let date = due_date(request)?;
    let milestone = &mut state.milestones[i];
    if request.get("title").is_some() {
        milestone.title = required(request, "title")?.into();
    }
    if let Some(name) = name {
        let parent = milestone
            .document_path
            .rsplit_once('/')
            .ok_or_else(|| anyhow::anyhow!("invalid: Milestone path"))?
            .0;
        milestone.document_path = format!(
            "{parent}/{}.md",
            numbered_name(&name, milestone.created_at, milestone.sequence)?
        );
        milestone.name = name;
    }
    if let Some(goal) = request["goal"].as_str() {
        milestone.goal = goal.into();
    }
    if request.get("order").is_some() {
        milestone.order = request["order"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("invalid: order must be an integer"))?;
    }
    if request.get("due_date").is_some() {
        milestone.due_date = date;
    }
    milestone.revision += 1;
    milestone.updated_at = now;
    Ok(json!(milestone))
}

fn milestone_transition(
    state: &mut Snapshot,
    index: usize,
    command: &str,
    now: i64,
) -> Result<Value> {
    let original = &state.milestones[index];
    let directory = state.projects[state.project_index(&original.project_id)?].document_directory();
    match command {
        "milestone.start" => {
            ensure!(
                original.status == MilestoneStatus::Planned,
                "conflict: only a PLANNED Milestone can be started"
            );
            state.milestones[index].status = MilestoneStatus::Active;
            state.milestones[index].started_at = Some(now);
        }
        "milestone.complete" | "milestone.cancel" => {
            ensure!(
                !original.status.terminal(),
                "conflict: reopen terminal Milestone first"
            );
            if command == "milestone.complete" {
                ensure!(
                    !state
                        .query_context
                        .milestone_active_issues
                        .get(&original.id)
                        .copied()
                        .unwrap_or(false)
                        && !state
                            .issues
                            .iter()
                            .any(|issue| issue.milestone_id.as_deref()
                                == Some(original.id.as_str())
                                && !issue.status.terminal()),
                    "conflict: Milestone has unfinished Issues; complete, cancel or reassign them first"
                );
                state.milestones[index].status = MilestoneStatus::Completed;
                state.milestones[index].completed_at = Some(now);
            } else {
                state.milestones[index].status = MilestoneStatus::Cancelled;
                state.milestones[index].cancelled_at = Some(now);
            }
        }
        "milestone.reopen" => {
            ensure!(
                original.status.terminal(),
                "conflict: only terminal Milestones can be reopened"
            );
            let milestone = &mut state.milestones[index];
            milestone.status = MilestoneStatus::Planned;
            milestone.started_at = None;
            milestone.completed_at = None;
            milestone.cancelled_at = None;
        }
        "milestone.archive" | "milestone.unarchive" => {
            let archive = command == "milestone.archive";
            ensure!(
                archive == original.archived_at.is_none(),
                "conflict: Milestone archive state unchanged"
            );
            ensure!(
                original.status.terminal(),
                "conflict: complete or cancel Milestone before archiving"
            );
            let milestone = &mut state.milestones[index];
            milestone.archived_at = archive.then_some(now);
            milestone.document_path = format!(
                "{directory}/Milestones/{}{}.md",
                if archive { "Archived/" } else { "" },
                numbered_name(&milestone.name, milestone.created_at, milestone.sequence)?
            );
        }
        _ => bail!("invalid: unknown command {command}"),
    }
    let milestone = &mut state.milestones[index];
    milestone.revision += 1;
    milestone.updated_at = now;
    Ok(json!(milestone))
}
