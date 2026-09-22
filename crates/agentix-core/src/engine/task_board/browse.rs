use agentix_task::{BrowseScope, JobStatus, TaskListItem, TaskStatus};

use super::{
    ActionButton, ActionStyle, ConversationRef, EngineError, OutboundView, SessionId,
    TaskBoardView, UiAction, error,
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
    LegacyTasks {
        filter: Option<String>,
        page: usize,
    },
    ProjectJobs {
        project: String,
        status: Option<JobStatus>,
        page: usize,
    },
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
            | Self::LegacyTasks { page, .. }
            | Self::ProjectJobs { page, .. }
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
            | Self::LegacyTasks { page: current, .. }
            | Self::ProjectJobs { page: current, .. }
            | Self::Inboxes { page: current, .. }
            | Self::Inbox { page: current, .. }
            | Self::Board { page: current, .. }
            | Self::Job { page: current, .. }
            | Self::Task { page: current, .. } => *current = page,
        }
        target
    }
}

impl TaskBoardView<'_> {
    pub(in crate::engine) async fn open_dashboard(
        &self,
        conversation: &ConversationRef,
        owner: &str,
    ) -> Result<(), EngineError> {
        self.ui
            .update_command_menu_best_effort(
                conversation,
                self.ui.sessions().current(conversation).await.is_some(),
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
            TaskBrowse::ProjectJobs {
                project,
                status,
                page,
            } => {
                self.show_project_jobs(conversation, owner, &project, status, page)
                    .await
            }
            TaskBrowse::LegacyTasks { filter, page } => {
                self.show_legacy_tasks(conversation, owner, filter.as_deref(), page)
                    .await
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
        if !buttons.is_empty() {
            self.add_action_section(
                conversation,
                owner,
                view,
                "Navigation",
                buttons
                    .into_iter()
                    .map(|(label, target)| (label, target, ActionStyle::Primary))
                    .collect(),
            )
            .await;
        }
    }

    pub(super) async fn add_action_section(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        view: &mut OutboundView,
        title: &str,
        buttons: Vec<(String, TaskBrowse, ActionStyle)>,
    ) {
        let index = append_section(view, title, String::new());
        let group = format!("task-browse:{}", uuid::Uuid::new_v4());
        for (label, target, style) in buttons {
            let token = self
                .ui
                .issue_action(conversation, owner, &group, UiAction::TaskBrowse(target))
                .await;
            view.sections[index].action_tokens.push(token.clone());
            view.actions.push(ActionButton {
                disabled: false,
                label: short(&label),
                token,
                style,
            });
        }
    }

    pub(super) async fn add_entry(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        view: &mut OutboundView,
        body: String,
        label: String,
        target: TaskBrowse,
    ) {
        let index = append_section(view, "", body);
        let token = self
            .ui
            .issue_action(
                conversation,
                owner,
                &format!("task-browse:{}", uuid::Uuid::new_v4()),
                UiAction::TaskBrowse(target),
            )
            .await;
        view.sections[index].action_tokens.push(token.clone());
        view.actions.push(ActionButton {
            disabled: false,
            label: short(&label),
            token,
            style: ActionStyle::Default,
        });
    }

    async fn show_dashboard(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        page: usize,
    ) -> Result<(), EngineError> {
        let projects = self
            .tasks_service()?
            .store()
            .project_summaries()
            .await
            .map_err(error)?;
        let pages = page_count(projects.len());
        let page = page.min(pages - 1);
        let mut view = OutboundView::text(
            "Dashboard",
            format!(
                "**Projects ({})**\n\nSelect a project to browse its jobs.",
                projects.len()
            ),
        );
        for summary in projects.iter().skip(page * PAGE_SIZE).take(PAGE_SIZE) {
            let project = &summary.project;
            self.add_entry(
                conversation,
                owner,
                &mut view,
                format!(
                    "**{}**\n{} jobs · {} tasks",
                    escape(&short(&project.name)),
                    summary.job_count,
                    summary.task_count
                ),
                project.name.clone(),
                TaskBrowse::ProjectJobs {
                    project: project.id.clone(),
                    status: None,
                    page: 0,
                },
            )
            .await;
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
            Vec::new(),
        )
        .await;
        self.ui.send_view(conversation, &view).await?;
        Ok(())
    }

    pub(super) async fn require_board_session(
        &self,
        conversation: &ConversationRef,
    ) -> Result<Option<SessionId>, EngineError> {
        self.tasks_service()?;
        let session = self.ui.sessions().current(conversation).await;
        if session.is_none() {
            self.ui.send_view(conversation, &OutboundView::text("Task board", "Attach a session first with /sessions or /attach <thread-id>. Use /dashboard to browse projects.")).await?;
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
            self.ui.sessions().current(conversation).await
        };
        let service = self.tasks_service()?;
        let selection = if let Some(project) = &project {
            BrowseScope::Project(project)
        } else {
            BrowseScope::Session(session.as_ref().expect("required session").as_str())
        };
        let result = service
            .store()
            .browse_task_page(
                selection,
                session.as_ref().map(SessionId::as_str),
                page,
                PAGE_SIZE,
                1,
            )
            .await
            .map_err(error)?;
        let page = result.page;
        let pages = result.pages;
        let heading = result.project.as_ref().map_or_else(
            || {
                format!(
                    "**Session:** `{}`",
                    session.as_ref().expect("required session")
                )
            },
            |project| format!("**Project:** {}", escape(&short(&project.name))),
        );
        let mut view = OutboundView::text(
            "Task board",
            format!(
                "{heading}\n{} jobs · {} tasks",
                result.job_count, result.total
            ),
        );
        view.body
            .push_str(&task_status_counts(&result.status_counts));
        for task in &result.tasks {
            self.add_entry(
                conversation,
                owner,
                &mut view,
                format!(
                    "{}{}",
                    if task.current { "Current · " } else { "" },
                    task_summary(task)
                ),
                task.title.clone(),
                TaskBrowse::Task {
                    id: task.id.clone(),
                    page: 0,
                },
            )
            .await;
        }
        if result.tasks.is_empty() {
            append_section(
                &mut view,
                "",
                "No associated tasks. Use /jobs to browse the attached session's jobs.".into(),
            );
        }
        let mut buttons = vec![("Dashboard".into(), TaskBrowse::Dashboard(0))];
        if let Some(project) = &project {
            buttons.push((
                "Project jobs".into(),
                TaskBrowse::ProjectJobs {
                    project: project.clone(),
                    status: None,
                    page: 0,
                },
            ));
        } else {
            buttons.push(("Session jobs".into(), TaskBrowse::Jobs(0)));
        }
        self.add_browse_actions(
            conversation,
            owner,
            &mut view,
            TaskBrowse::Board { project, page },
            pages,
            buttons,
        )
        .await;
        self.ui.send_view(conversation, &view).await?;
        Ok(())
    }

    async fn show_project_jobs(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        project: &str,
        status: Option<JobStatus>,
        page: usize,
    ) -> Result<(), EngineError> {
        let service = self.tasks_service()?;
        let project = service
            .store()
            .project_result(project)
            .await
            .map_err(error)?;
        let result = service
            .store()
            .project_job_page(&project.id, status, page, PAGE_SIZE)
            .await
            .map_err(error)?;
        let mut view = OutboundView::text(
            "Jobs",
            format!(
                "**Project:** {}\n**Jobs ({})**\nStatus: {}\nMost recently updated first.",
                escape(&short(&project.name)),
                result.total,
                status.map_or_else(|| "ALL".into(), |status| status.to_string())
            ),
        );
        for job in &result.jobs {
            self.add_entry(
                conversation,
                owner,
                &mut view,
                format!(
                    "**{}**\n{} · {} tasks",
                    escape(&short(&job.title)),
                    job.status,
                    job.task_count
                ),
                job.title.clone(),
                TaskBrowse::Job {
                    id: job.id.clone(),
                    page: 0,
                },
            )
            .await;
        }
        if result.jobs.is_empty() {
            append_section(
                &mut view,
                "",
                "No matching jobs. Choose another status below.".into(),
            );
        }
        let buttons = vec![
            ("Dashboard".into(), TaskBrowse::Dashboard(0)),
            (
                "Project inbox".into(),
                TaskBrowse::Inboxes {
                    project: project.id.clone(),
                    page: 0,
                },
            ),
        ];
        self.add_browse_actions(
            conversation,
            owner,
            &mut view,
            TaskBrowse::ProjectJobs {
                project: project.id.clone(),
                status,
                page: result.page,
            },
            result.pages,
            buttons,
        )
        .await;
        self.add_action_section(
            conversation,
            owner,
            &mut view,
            "Filter by status",
            std::iter::once(None)
                .chain(JobStatus::ALL.into_iter().map(Some))
                .map(|filter| {
                    (
                        filter.map_or_else(|| "All statuses".into(), |status| status.to_string()),
                        TaskBrowse::ProjectJobs {
                            project: project.id.clone(),
                            status: filter,
                            page: 0,
                        },
                        if filter == Some(JobStatus::Cancelled) {
                            ActionStyle::Danger
                        } else {
                            ActionStyle::Primary
                        },
                    )
                })
                .collect(),
        )
        .await;
        self.ui.send_view(conversation, &view).await?;
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
        let result = self
            .tasks_service()?
            .store()
            .session_job_page(session.as_str(), page, PAGE_SIZE)
            .await
            .map_err(error)?;
        let jobs = &result.jobs;
        let pages = result.pages;
        let page = result.page;
        let mut view = OutboundView::text(
            "Jobs",
            format!("**Session:** `{session}`\n**Jobs ({})**", result.total),
        );
        for job in jobs {
            self.add_entry(
                conversation,
                owner,
                &mut view,
                format!(
                    "**{}**\n{} · {} tasks",
                    escape(&short(&job.title)),
                    job.status,
                    job.task_count
                ),
                job.title.clone(),
                TaskBrowse::Job {
                    id: job.id.clone(),
                    page: 0,
                },
            )
            .await;
        }
        if jobs.is_empty() {
            append_section(
                &mut view,
                "",
                "No associated jobs. Use Dashboard to browse all projects.".into(),
            );
        }
        let buttons = vec![
            ("Dashboard".into(), TaskBrowse::Dashboard(0)),
            (
                "Session tasks".into(),
                TaskBrowse::Board {
                    project: None,
                    page: 0,
                },
            ),
        ];
        self.add_browse_actions(
            conversation,
            owner,
            &mut view,
            TaskBrowse::Jobs(page),
            pages,
            buttons,
        )
        .await;
        self.ui.send_view(conversation, &view).await?;
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
        let job = service.store().job_record(id).await.map_err(error)?;
        let project = service
            .store()
            .project_result(&job.project_id)
            .await
            .map_err(error)?;
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
        let result = service
            .store()
            .browse_task_page(
                BrowseScope::Job(&job.id),
                None,
                page,
                PAGE_SIZE,
                content.len(),
            )
            .await
            .map_err(error)?;
        let pages = result.pages;
        let page = result.page;
        let mut view = OutboundView::text(
            short(&job.title),
            format!(
                "**Job:** `{}`\n**Project:** {}\n**Status:** {}\n\n{}\n\n**Tasks ({})**",
                job.id,
                escape(&short(&project.name)),
                job.status,
                content.get(page).map_or("", String::as_str),
                result.total
            ),
        );
        for task in &result.tasks {
            self.add_entry(
                conversation,
                owner,
                &mut view,
                task_summary(task),
                task.title.clone(),
                TaskBrowse::Task {
                    id: task.id.clone(),
                    page: 0,
                },
            )
            .await;
        }
        if result.tasks.is_empty() && result.total == 0 {
            append_section(&mut view, "", "No tasks in this job yet.".into());
        }
        let buttons = vec![
            (
                "Project jobs".into(),
                TaskBrowse::ProjectJobs {
                    project: job.project_id.clone(),
                    status: None,
                    page: 0,
                },
            ),
            ("Dashboard".into(), TaskBrowse::Dashboard(0)),
        ];
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
        self.ui.send_view(conversation, &view).await?;
        Ok(())
    }
}

fn task_summary(task: &TaskListItem) -> String {
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

fn task_status_counts(counts: &[(TaskStatus, usize)]) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for status in TaskStatus::ALL {
        let count = counts
            .iter()
            .find(|(value, _)| *value == status)
            .map_or(0, |(_, count)| *count);
        write!(text, "\n{status} ({count})").expect("writing to a String cannot fail");
    }
    text
}

pub(super) fn append_section(view: &mut OutboundView, title: &str, body: String) -> usize {
    if view.sections.is_empty() {
        view.sections.push(agentix_domain::ViewSection {
            body: view.body.clone(),
            ..Default::default()
        });
    }
    if !title.is_empty() {
        view.body.push_str(&format!("\n\n**{title}**"));
    }
    if !body.is_empty() {
        view.body.push_str(&format!("\n\n{body}"));
    }
    view.sections.push(agentix_domain::ViewSection {
        title: title.into(),
        body,
        ..Default::default()
    });
    view.sections.len() - 1
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
