use agentix_task::{Job, Snapshot, Task, TaskStatus};

use super::{
    ActionButton, ActionStyle, ConversationRef, Engine, EngineError, OutboundView, SessionId,
    UiAction, error,
};

pub(super) const PAGE_SIZE: usize = 6;
const MARKDOWN_PAGE_BYTES: usize = 1200;

#[derive(Debug, Clone)]
pub(crate) enum TaskBrowse {
    Dashboard(usize),
    Board {
        project: Option<String>,
        page: usize,
    },
    Jobs(usize),
    Inboxes {
        project: String,
        page: usize,
    },
    Inbox {
        id: String,
        page: usize,
    },
    Job {
        id: String,
        page: usize,
    },
    Task {
        id: String,
        page: usize,
    },
}

impl TaskBrowse {
    fn page(&self) -> usize {
        match self {
            Self::Dashboard(page)
            | Self::Jobs(page)
            | Self::Inboxes { page, .. }
            | Self::Inbox { page, .. }
            | Self::Board { page, .. }
            | Self::Job { page, .. }
            | Self::Task { page, .. } => *page,
        }
    }

    fn at_page(&self, page: usize) -> Self {
        let mut target = self.clone();
        match &mut target {
            Self::Dashboard(current)
            | Self::Jobs(current)
            | Self::Inboxes { page: current, .. }
            | Self::Inbox { page: current, .. }
            | Self::Board { page: current, .. }
            | Self::Job { page: current, .. }
            | Self::Task { page: current, .. } => *current = page,
        }
        target
    }
}

impl Engine {
    pub(in crate::engine) async fn open_dashboard(
        &self,
        conversation: &ConversationRef,
        owner: &str,
    ) -> Result<(), EngineError> {
        self.update_command_menu_best_effort(
            conversation,
            self.sessions.current(conversation).await.is_some(),
        )
        .await;
        self.show_dashboard(conversation, owner, 0).await
    }

    pub(in crate::engine) async fn browse_tasks(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        target: TaskBrowse,
    ) -> Result<(), EngineError> {
        match target {
            TaskBrowse::Dashboard(page) => self.show_dashboard(conversation, owner, page).await,
            TaskBrowse::Board { project, page } => {
                self.show_board(conversation, owner, project, page).await
            }
            TaskBrowse::Jobs(page) => self.show_session_jobs(conversation, owner, page).await,
            TaskBrowse::Inboxes { project, page } => {
                self.show_inboxes(conversation, owner, &project, page).await
            }
            TaskBrowse::Inbox { id, page } => self.show_inbox(conversation, owner, &id, page).await,
            TaskBrowse::Job { id, page } => self.show_job(conversation, owner, &id, page).await,
            TaskBrowse::Task { id, page } => {
                self.show_task_page(conversation, owner, &id, page).await
            }
        }
    }

    pub(super) async fn add_browse_actions(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        view: &mut OutboundView,
        target: TaskBrowse,
        pages: usize,
        mut buttons: Vec<(String, TaskBrowse)>,
    ) {
        let page = target.page();
        if page > 0 {
            buttons.push(("Previous".into(), target.at_page(page - 1)));
        }
        if page + 1 < pages {
            buttons.push(("Next".into(), target.at_page(page + 1)));
        }
        if pages > 1 {
            view.subtitle = Some(format!("Page {} / {pages}", page + 1));
        }
        let group = format!("task-browse:{}", uuid::Uuid::new_v4());
        for (label, target) in buttons {
            let token = self
                .issue_action(conversation, owner, &group, UiAction::TaskBrowse(target))
                .await;
            view.actions.push(ActionButton {
                label: short(&label),
                token,
                style: ActionStyle::Default,
            });
        }
    }

    async fn show_dashboard(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        page: usize,
    ) -> Result<(), EngineError> {
        let state = self
            .tasks_service()?
            .store()
            .snapshot()
            .await
            .map_err(error)?;
        let projects: Vec<_> = state
            .projects
            .iter()
            .filter(|p| p.archived_at.is_none())
            .collect();
        let pages = page_count(projects.len());
        let page = page.min(pages - 1);
        let mut view = OutboundView::text(
            "Dashboard",
            format!(
                "**Projects ({})**\n\nSelect a project to open its board.",
                projects.len()
            ),
        );
        let mut buttons = Vec::new();
        for project in projects.iter().skip(page * PAGE_SIZE).take(PAGE_SIZE) {
            let jobs: Vec<_> = visible_jobs(&state)
                .into_iter()
                .filter(|j| j.project_id == project.id)
                .collect();
            let tasks = state
                .tasks
                .iter()
                .filter(|t| jobs.iter().any(|j| j.id == t.job_id))
                .count();
            view.body.push_str(&format!(
                "\n\n**{}**\n{} jobs · {tasks} tasks",
                escape(&short(&project.name)),
                jobs.len()
            ));
            buttons.push((
                project.name.clone(),
                TaskBrowse::Board {
                    project: Some(project.id.clone()),
                    page: 0,
                },
            ));
        }
        if projects.is_empty() {
            view.body.push_str("\n\nNo matching projects.");
        }
        self.add_browse_actions(
            conversation,
            owner,
            &mut view,
            TaskBrowse::Dashboard(page),
            pages,
            buttons,
        )
        .await;
        self.send_view(conversation, &view).await?;
        Ok(())
    }

    pub(super) async fn require_board_session(
        &self,
        conversation: &ConversationRef,
    ) -> Result<Option<SessionId>, EngineError> {
        self.tasks_service()?;
        let session = self.sessions.current(conversation).await;
        if session.is_none() {
            self.send_view(conversation, &OutboundView::text("Task board", "Attach a session first with /sessions or /attach <thread-id>. Use /dashboard to browse projects.")).await?;
        }
        Ok(session)
    }

    async fn show_board(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        project: Option<String>,
        page: usize,
    ) -> Result<(), EngineError> {
        let session = if project.is_none() {
            let Some(session) = self.require_board_session(conversation).await? else {
                return Ok(());
            };
            Some(session)
        } else {
            self.sessions.current(conversation).await
        };
        let service = self.tasks_service()?;
        let state = service.store().snapshot().await.map_err(error)?;
        let jobs = if let Some(project) = &project {
            let project = &state.projects[state.project_index(project).map_err(error)?];
            visible_jobs(&state)
                .into_iter()
                .filter(|j| j.project_id == project.id)
                .collect()
        } else {
            session_jobs(&state, session.as_ref().expect("required session"))
        };
        let mut tasks: Vec<_> = state
            .tasks
            .iter()
            .filter(|t| jobs.iter().any(|j| j.id == t.job_id))
            .collect();
        let current: Vec<_> = state
            .leases
            .iter()
            .filter(|l| {
                session
                    .as_ref()
                    .is_some_and(|s| s.as_str() == l.session_ref)
                    && l.lease_expires_at > service.store().now()
            })
            .map(|l| l.task_id.as_str())
            .collect();
        tasks.sort_by_key(|t| {
            (
                status_order(t.status),
                !current.contains(&t.id.as_str()),
                t.position,
                &t.id,
            )
        });
        let pages = page_count(tasks.len());
        let page = page.min(pages - 1);
        let heading = board_heading(&state, project.as_deref(), session.as_ref());
        let mut view = OutboundView::text(
            "Task board",
            format!("{heading}\n{} jobs · {} tasks", jobs.len(), tasks.len()),
        );
        for status in TaskStatus::ALL {
            view.body.push_str(&format!(
                "\n{} ({})",
                status,
                tasks.iter().filter(|t| t.status == status).count()
            ));
        }
        let selected: Vec<_> = tasks
            .into_iter()
            .skip(page * PAGE_SIZE)
            .take(PAGE_SIZE)
            .collect();
        let mut buttons = task_buttons(&selected);
        for task in &selected {
            view.body.push_str(&format!(
                "\n\n{}{}",
                if current.contains(&task.id.as_str()) {
                    "Current · "
                } else {
                    ""
                },
                task_summary(task)
            ));
        }
        if selected.is_empty() {
            view.body.push_str("\n\nNo associated tasks.");
        }
        buttons.push(("Dashboard".into(), TaskBrowse::Dashboard(0)));
        self.add_browse_actions(
            conversation,
            owner,
            &mut view,
            TaskBrowse::Board { project, page },
            pages,
            buttons,
        )
        .await;
        self.send_view(conversation, &view).await?;
        Ok(())
    }

    async fn show_session_jobs(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        page: usize,
    ) -> Result<(), EngineError> {
        let Some(session) = self.require_board_session(conversation).await? else {
            return Ok(());
        };
        let state = self
            .tasks_service()?
            .store()
            .snapshot()
            .await
            .map_err(error)?;
        let jobs = session_jobs(&state, &session);
        let pages = page_count(jobs.len());
        let page = page.min(pages - 1);
        let mut view = OutboundView::text(
            "Jobs",
            format!("**Session:** `{session}`\n**Jobs ({})**", jobs.len()),
        );
        let mut buttons = Vec::new();
        for job in jobs.iter().skip(page * PAGE_SIZE).take(PAGE_SIZE) {
            let count = state.tasks.iter().filter(|t| t.job_id == job.id).count();
            view.body.push_str(&format!(
                "\n\n**{}**\n{} · {count} tasks",
                escape(&short(&job.title)),
                job.status
            ));
            buttons.push((
                job.title.clone(),
                TaskBrowse::Job {
                    id: job.id.clone(),
                    page: 0,
                },
            ));
        }
        if jobs.is_empty() {
            view.body.push_str("\n\nNo associated jobs.");
        }
        self.add_browse_actions(
            conversation,
            owner,
            &mut view,
            TaskBrowse::Jobs(page),
            pages,
            buttons,
        )
        .await;
        self.send_view(conversation, &view).await?;
        Ok(())
    }

    async fn show_job(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        id: &str,
        page: usize,
    ) -> Result<(), EngineError> {
        let service = self.tasks_service()?;
        let state = service.store().snapshot().await.map_err(error)?;
        let job = &state.jobs[state.job_index(id).map_err(error)?];
        let project = &state.projects[state.project_index(&job.project_id).map_err(error)?];
        let markdown = service
            .job_markdown(id)
            .await
            .unwrap_or_else(|_| format!("## Goal\n\n{}\n\nJob document is unavailable.", job.goal));
        let markdown = if job.title.chars().count() > 60 {
            format!("**Job title:** {}\n\n{markdown}", escape(&job.title))
        } else {
            markdown
        };
        let content = markdown_pages(&markdown);
        let mut tasks: Vec<_> = state.tasks.iter().filter(|t| t.job_id == job.id).collect();
        tasks.sort_by_key(|t| (t.position, &t.id));
        let pages = content.len().max(page_count(tasks.len()));
        let page = page.min(pages - 1);
        let mut view = OutboundView::text(
            short(&job.title),
            format!(
                "**Job:** `{}`\n**Project:** {}\n**Status:** {}\n\n{}\n\n**Tasks ({})**",
                job.id,
                escape(&short(&project.name)),
                job.status,
                content.get(page).map_or("", String::as_str),
                tasks.len()
            ),
        );
        let selected: Vec<_> = tasks
            .into_iter()
            .skip(page * PAGE_SIZE)
            .take(PAGE_SIZE)
            .collect();
        let mut buttons = task_buttons(&selected);
        // Task titles are on the buttons; keep the body available for authored Markdown.
        buttons.push((
            "Project board".into(),
            TaskBrowse::Board {
                project: Some(job.project_id.clone()),
                page: 0,
            },
        ));
        self.add_browse_actions(
            conversation,
            owner,
            &mut view,
            TaskBrowse::Job {
                id: id.into(),
                page,
            },
            pages,
            buttons,
        )
        .await;
        self.send_view(conversation, &view).await?;
        Ok(())
    }
}

fn visible_jobs(state: &Snapshot) -> Vec<&Job> {
    state
        .jobs
        .iter()
        .filter(|j| {
            j.archived_at.is_none()
                && state
                    .projects
                    .iter()
                    .any(|p| p.id == j.project_id && p.archived_at.is_none())
        })
        .collect()
}

fn session_jobs<'a>(state: &'a Snapshot, session: &SessionId) -> Vec<&'a Job> {
    visible_jobs(state)
        .into_iter()
        .filter(|j| {
            state.tasks.iter().any(|t| {
                t.job_id == j.id
                    && (t.last_session.as_deref() == Some(session.as_str())
                        || state
                            .leases
                            .iter()
                            .any(|l| l.task_id == t.id && l.session_ref == session.as_str()))
            })
        })
        .collect()
}

fn task_buttons(tasks: &[&Task]) -> Vec<(String, TaskBrowse)> {
    tasks
        .iter()
        .map(|t| {
            (
                t.title.clone(),
                TaskBrowse::Task {
                    id: t.id.clone(),
                    page: 0,
                },
            )
        })
        .collect()
}

fn task_summary(task: &Task) -> String {
    let phase = task.phase.map_or_else(String::new, |p| format!(" · {p}"));
    let reason = task
        .reason
        .as_deref()
        .map_or_else(String::new, |r| format!("\n{}", escape(&short(r))));
    format!(
        "**{}**\n{}{phase}{reason}",
        escape(&short(&task.title)),
        task.status
    )
}

fn status_order(status: TaskStatus) -> usize {
    match status {
        TaskStatus::InProgress => 0,
        TaskStatus::Blocked => 1,
        TaskStatus::WaitingUser => 2,
        TaskStatus::Todo => 3,
        TaskStatus::Failed => 4,
        TaskStatus::Done => 5,
        TaskStatus::Cancelled => 6,
    }
}

pub(super) fn page_count(count: usize) -> usize {
    count.div_ceil(PAGE_SIZE).max(1)
}

pub(super) fn short(text: &str) -> String {
    text.chars().take(60).collect()
}

pub(super) fn escape(text: &str) -> String {
    let mut result = String::new();
    for c in text.chars() {
        if "\\`*_{}[]<>()#+-.!|~>".contains(c) {
            result.push('\\');
        }
        result.push(c);
    }
    result
}

/// Keep fenced code blocks valid across bounded IM pages.
pub(super) fn markdown_pages(markdown: &str) -> Vec<String> {
    let mut pages = Vec::new();
    let mut page = String::new();
    let mut fence: Option<String> = None;
    for line in markdown.split_inclusive('\n') {
        for part in crate::chunk_text(line, MARKDOWN_PAGE_BYTES / 2) {
            if page.len() + part.len() > MARKDOWN_PAGE_BYTES && !page.is_empty() {
                if let Some(opening) = &fence {
                    page.push_str(&format!("\n{}\n", fence_marker(opening)));
                }
                pages.push(std::mem::take(&mut page));
                if let Some(opening) = &fence {
                    page.push_str(opening);
                    page.push('\n');
                }
            }
            let trimmed = part.trim();
            if let Some(opening) = &fence {
                let marker = fence_marker(opening);
                if trimmed.starts_with(&marker) && trimmed.chars().all(|c| marker.starts_with(c)) {
                    fence = None;
                }
            } else if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                fence = Some(trimmed.into());
            }
            page.push_str(&part);
        }
    }
    if let Some(opening) = &fence {
        page.push_str(&format!("\n{}\n", fence_marker(opening)));
    }
    if !page.is_empty() || pages.is_empty() {
        pages.push(page);
    }
    pages
}

fn fence_marker(opening: &str) -> String {
    opening
        .chars()
        .take_while(|c| opening.starts_with(*c))
        .collect()
}

fn board_heading(state: &Snapshot, project: Option<&str>, session: Option<&SessionId>) -> String {
    if let Some(id) = project {
        format!(
            "**Project:** {}",
            escape(&short(
                &state.projects[state.project_index(id).expect("resolved project")].name
            ))
        )
    } else {
        format!("**Session:** `{}`", session.expect("required session"))
    }
}
