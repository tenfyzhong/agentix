//! Issue intake, browsing and human acceptance remain independent of sessions.
use super::browse::{PAGE_SIZE, escape, markdown_pages, page_count, short};
use super::labels::issue_status_label;
use super::{
    ActionButton, ActionStyle, ConversationRef, EngineError, OutboundView, SessionId,
    TaskBoardView, TaskBrowse, UiAction, error,
};
use agentix_task::{Issue, IssueFilter, IssueStatus, IssueType, Project, WriteOptions};
use serde_json::json;
use std::{collections::HashSet, path::Path};

#[derive(Debug, Clone)]
pub(crate) enum IssueProjectIntent {
    List {
        status: Option<IssueStatus>,
        milestone: Option<String>,
    },
    Create {
        content: String,
        event: String,
    },
    Milestones,
}

#[derive(Debug, Clone)]
pub(in crate::engine) struct IssueAction {
    pub id: String,
    pub revision: i64,
    pub command: String,
    pub field: Option<String>,
    pub value: Option<String>,
    pub session_id: Option<SessionId>,
    pub job_id: Option<String>,
    pub page: usize,
}

pub(in crate::engine) struct PendingIssueInput {
    pub action: IssueAction,
    pub owner: String,
    pub generation: u64,
    pub epoch: u64,
}

pub(in crate::engine) struct PendingIssueExecution {
    pub action: IssueAction,
    pub owner: String,
    pub generation: u64,
    pub epoch: u64,
    pub workspace: bool,
}

impl TaskBoardView<'_> {
    pub(super) async fn show_issue_jobs(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        id: &str,
        page: usize,
    ) -> Result<(), EngineError> {
        let service = self.tasks_service()?;
        let offset = i64::try_from(
            page.checked_mul(PAGE_SIZE)
                .ok_or(EngineError::InvalidAction)?,
        )
        .map_err(error)?;
        let rows = service
            .store()
            .issue_job_page(
                id,
                None,
                None,
                offset,
                i64::try_from(PAGE_SIZE).map_err(error)?,
            )
            .await
            .map_err(error)?;
        let total = usize::try_from(rows["total"].as_u64().unwrap_or(0)).map_err(error)?;
        let pages = page_count(total);
        let page = page.min(pages - 1);
        if offset > 0 && rows["items"].as_array().is_some_and(Vec::is_empty) {
            return Box::pin(self.show_issue_jobs(conversation, owner, id, page)).await;
        }
        let mut view = OutboundView::text(
            "Issue Jobs",
            format!("**Issue:** `{id}`\n**Jobs ({total})**"),
        );
        for job in rows["items"].as_array().ok_or(EngineError::InvalidAction)? {
            let id = job["id"].as_str().ok_or(EngineError::InvalidAction)?;
            let title = job["title"].as_str().unwrap_or(id);
            self.add_entry(
                conversation,
                owner,
                &mut view,
                format!(
                    "**{}** · {}{}",
                    escape(title),
                    job["status"].as_str().unwrap_or(""),
                    if job["archived_at"].is_null() {
                        ""
                    } else {
                        " · Archived"
                    }
                ),
                title.into(),
                TaskBrowse::Job {
                    id: id.into(),
                    page: 0,
                },
            )
            .await;
        }
        self.add_browse_actions(
            conversation,
            owner,
            &mut view,
            TaskBrowse::IssueJobs {
                id: id.into(),
                page,
            },
            pages,
            vec![(
                "Issue".into(),
                TaskBrowse::Issue {
                    id: id.into(),
                    status: None,
                    milestone: None,
                    page: 0,
                },
            )],
        )
        .await;
        self.ui.send_view(conversation, &view).await?;
        Ok(())
    }
    pub(super) async fn add_issue_notification(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        view: &mut OutboundView,
        id: &str,
    ) -> Result<(), EngineError> {
        let item = match self.tasks_service()?.store().issue_record(id).await {
            Ok(item) => item,
            Err(failure) if failure.to_string().starts_with("not_found:") => return Ok(()),
            Err(failure) => return Err(error(failure)),
        };
        view.body.push_str(&format!("\n\n**Issue:** {} (`{}`)\nIssue remains {}. Job review and Issue acceptance are separate.",escape(&item.title),item.id,issue_status_label(item.status)));
        self.add_action_section(
            conversation,
            owner,
            view,
            "Issue",
            vec![(
                "Open Issue".into(),
                TaskBrowse::Issue {
                    id: item.id.clone(),
                    status: None,
                    milestone: None,
                    page: 0,
                },
                ActionStyle::Primary,
            )],
        )
        .await;
        if item.archived_at.is_none() {
            if item.status == IssueStatus::Active {
                self.add_issue_action(
                    conversation,
                    owner,
                    view,
                    "New round",
                    &item,
                    "new_round",
                    None,
                    None,
                )
                .await;
            } else if item.status == IssueStatus::PendingReview {
                self.add_issue_action(
                    conversation,
                    owner,
                    view,
                    "Accept",
                    &item,
                    "issue.approve",
                    None,
                    None,
                )
                .await;
                self.add_issue_action(
                    conversation,
                    owner,
                    view,
                    "Reject",
                    &item,
                    "issue.reject",
                    None,
                    None,
                )
                .await;
            }
        }
        Ok(())
    }
    async fn current_issue_project(
        &self,
        conversation: &ConversationRef,
    ) -> Result<Option<Project>, EngineError> {
        let Some(session) = self.ui.sessions().current(conversation).await else {
            return Ok(None);
        };
        let mut cursor = None;
        let mut seen = HashSet::new();
        let mut cwd = None;
        loop {
            let page = self.ui.agent().list_sessions(cursor, 100).await?;
            if let Some(summary) = page.sessions.iter().find(|s| s.id == session) {
                cwd = summary.cwd.clone();
                break;
            }
            cursor = page.next_cursor;
            if cursor.as_ref().is_none_or(|c| !seen.insert(c.clone())) {
                break;
            }
        }
        let project = self
            .tasks_service()?
            .project_for_session(cwd.as_deref().map(Path::new), Some(session.native_str()))
            .await
            .map_err(error)?;
        if self.ui.sessions().current(conversation).await.as_ref() != Some(&session) {
            return Err(EngineError::InvalidAction);
        }
        Ok(project.filter(|p| p.archived_at.is_none()))
    }

    pub(in crate::engine) async fn issue_command(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        intent: IssueProjectIntent,
    ) -> Result<(), EngineError> {
        if let Some(project) = self.current_issue_project(conversation).await? {
            self.issue_project(conversation, owner, &project.id, intent)
                .await
        } else {
            self.show_issue_projects(conversation, owner, intent, 0)
                .await
        }
    }

    pub(super) async fn show_issue_projects(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        intent: IssueProjectIntent,
        page: usize,
    ) -> Result<(), EngineError> {
        let projects = self
            .tasks_service()?
            .store()
            .projects()
            .await
            .map_err(error)?
            .into_iter()
            .filter(|p| p.archived_at.is_none())
            .collect::<Vec<_>>();
        let pages = page_count(projects.len());
        let page = page.min(pages - 1);
        let mut view = OutboundView::text(
            "Choose project",
            "Select the Project for this Issue command.",
        );
        for project in projects.iter().skip(page * PAGE_SIZE).take(PAGE_SIZE) {
            self.add_entry(
                conversation,
                owner,
                &mut view,
                escape(&project.name),
                project.name.clone(),
                TaskBrowse::IssueProject {
                    project: project.id.clone(),
                    intent: intent.clone(),
                },
            )
            .await;
        }
        if projects.is_empty() {
            view.body.push_str(
                "\nNo active registered projects. Register one with taskix project register.",
            );
        }
        self.add_browse_actions(
            conversation,
            owner,
            &mut view,
            TaskBrowse::IssueProjects { intent, page },
            pages,
            vec![],
        )
        .await;
        self.ui.send_view(conversation, &view).await?;
        Ok(())
    }

    pub(super) async fn issue_project(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        project: &str,
        intent: IssueProjectIntent,
    ) -> Result<(), EngineError> {
        match intent {
            IssueProjectIntent::List { status, milestone } => {
                self.show_issues(conversation, owner, project, status, milestone, 0)
                    .await
            }
            IssueProjectIntent::Milestones => {
                self.show_milestones(conversation, owner, project, 0).await
            }
            IssueProjectIntent::Create { content, event } => {
                let (issue_type, content) = parse_issue_type(&content)?;
                let title = content.lines().next().unwrap_or("").trim();
                let saved=self.tasks_service()?.execute(json!({"command":"issue.create","project":project,"title":title,"requirement":content,"issue_type":issue_type}),
                    WriteOptions{actor_ref:format!("user:im:{owner}"),idempotency_key:Some(format!("im:issue:{}",json!([conversation.channel,conversation.conversation_id,event]))),..WriteOptions::default()}).await.map_err(error)?;
                let id = saved.result["id"]
                    .as_str()
                    .ok_or_else(|| error("Invalid Issue response"))?;
                self.show_issue(conversation, owner, id, None, None, 0)
                    .await?;
                if let Some(warning) = saved.projection_pending {
                    self.ui
                        .send_view(
                            conversation,
                            &OutboundView::text("Projection pending", warning),
                        )
                        .await?;
                }
                Ok(())
            }
        }
    }

    #[allow(clippy::too_many_lines)] // Keep the filtered page and its navigation/filter controls together.
    pub(super) async fn show_issues(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        project: &str,
        status: Option<IssueStatus>,
        milestone: Option<String>,
        page: usize,
    ) -> Result<(), EngineError> {
        let service = self.tasks_service()?;
        let project = service
            .store()
            .project_result(project)
            .await
            .map_err(error)?;
        let filter = IssueFilter {
            status,
            milestone_id: milestone.clone(),
            archived: Some(false),
        };
        let offset = i64::try_from(
            page.checked_mul(PAGE_SIZE)
                .ok_or(EngineError::InvalidAction)?,
        )
        .map_err(error)?;
        let rows = service
            .store()
            .issue_offset_page(
                Some(&project.id),
                &filter,
                offset,
                i64::try_from(PAGE_SIZE).map_err(error)?,
            )
            .await
            .map_err(error)?;
        let total = usize::try_from(rows["total"].as_u64().unwrap_or(0)).map_err(error)?;
        let pages = page_count(total);
        let page = page.min(pages - 1);
        if offset > 0 && rows["items"].as_array().is_some_and(Vec::is_empty) {
            return Box::pin(self.show_issues(
                conversation,
                owner,
                &project.id,
                status,
                milestone,
                page,
            ))
            .await;
        }
        let mut view = OutboundView::text(
            "Project issues",
            format!(
                "**Project:** {}\n**Issues ({total})**\nStatus: {}\n\nUse /issue new <description> to create a requirement.",
                escape(&short(&project.name)),
                status.map_or("All", issue_status_label)
            ),
        );
        for item in rows["items"]
            .as_array()
            .ok_or_else(|| error("Invalid Issue page"))?
        {
            let id = item["id"]
                .as_str()
                .ok_or_else(|| error("Invalid Issue identity"))?
                .to_owned();
            let title = item["title"].as_str().unwrap_or(&id).to_owned();
            let state: IssueStatus =
                serde_json::from_value(item["status"].clone()).map_err(error)?;
            self.add_entry(
                conversation,
                owner,
                &mut view,
                format!(
                    "**{}**\n{}\n`{}`",
                    escape(&short(&title)),
                    issue_status_label(state),
                    id
                ),
                title,
                TaskBrowse::Issue {
                    id,
                    status,
                    milestone: milestone.clone(),
                    page: 0,
                },
            )
            .await;
        }
        self.add_browse_actions(
            conversation,
            owner,
            &mut view,
            TaskBrowse::Issues {
                project: project.id.clone(),
                status,
                milestone: milestone.clone(),
                page,
            },
            pages,
            vec![
                (
                    "Milestones".into(),
                    TaskBrowse::Milestones {
                        project: project.id.clone(),
                        page: 0,
                    },
                ),
                ("Dashboard".into(), TaskBrowse::Dashboard(0)),
            ],
        )
        .await;
        self.add_action_section(
            conversation,
            owner,
            &mut view,
            "Filter by status",
            [
                None,
                Some(IssueStatus::Todo),
                Some(IssueStatus::Active),
                Some(IssueStatus::PendingReview),
                Some(IssueStatus::Completed),
                Some(IssueStatus::Cancelled),
            ]
            .into_iter()
            .map(|filter| {
                (
                    filter.map_or("All", issue_status_label).into(),
                    TaskBrowse::Issues {
                        project: project.id.clone(),
                        status: filter,
                        milestone: milestone.clone(),
                        page: 0,
                    },
                    ActionStyle::Primary,
                )
            })
            .collect(),
        )
        .await;
        self.ui.send_view(conversation, &view).await?;
        Ok(())
    }

    pub(super) async fn show_milestones(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        project: &str,
        page: usize,
    ) -> Result<(), EngineError> {
        let service = self.tasks_service()?;
        let result = service
            .store()
            .milestone_page(project, page, PAGE_SIZE)
            .await
            .map_err(error)?;
        let pages = usize::try_from(result["pages"].as_u64().unwrap_or(1)).map_err(error)?;
        let page = usize::try_from(result["page"].as_u64().unwrap_or(0)).map_err(error)?;
        let mut view = OutboundView::text("Milestones", "Select a Milestone to filter its Issues.");
        for stage in result["items"].as_array().into_iter().flatten() {
            let progress = &stage["progress"];
            self.add_entry(
                conversation,
                owner,
                &mut view,
                format!(
                    "**{}**\n{}\nAccepted: {} / {} · Cancelled: {}",
                    escape(&short(stage["title"].as_str().unwrap_or_default())),
                    escape(stage["goal"].as_str().unwrap_or_default()),
                    progress["completed"],
                    progress["total"],
                    progress["cancelled"]
                ),
                stage["title"].as_str().unwrap_or_default().to_owned(),
                TaskBrowse::Issues {
                    project: project.into(),
                    status: None,
                    milestone: Some(stage["id"].as_str().unwrap_or_default().to_owned()),
                    page: 0,
                },
            )
            .await;
        }
        self.add_browse_actions(
            conversation,
            owner,
            &mut view,
            TaskBrowse::Milestones {
                project: project.into(),
                page,
            },
            pages,
            vec![(
                "Project issues".into(),
                TaskBrowse::Issues {
                    project: project.into(),
                    status: None,
                    milestone: None,
                    page: 0,
                },
            )],
        )
        .await;
        self.ui.send_view(conversation, &view).await?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // Assemble one Issue card with body, lifecycle controls and related Jobs.
    pub(in crate::engine) async fn show_issue(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        id: &str,
        status: Option<IssueStatus>,
        milestone: Option<String>,
        page: usize,
    ) -> Result<(), EngineError> {
        let service = self.tasks_service()?;
        service.sync_delivery_document(id).await.map_err(error)?;
        let item = service.store().issue_record(id).await.map_err(error)?;
        let milestone_title = if let Some(stage) = &item.milestone_id {
            service
                .store()
                .milestone_record(stage)
                .await
                .map_err(error)?
                .title
        } else {
            "None".into()
        };
        let jobs = service
            .store()
            .issue_job_page(id, None, None, 0, i64::try_from(PAGE_SIZE).map_err(error)?)
            .await
            .map_err(error)?;
        let authored = service.issue_content(id).await.map_err(error)?;
        let body = format!(
            "## Requirement\n{}\n\n## Acceptance criteria\n{}\n\n## Notes\n{}",
            authored.requirement, authored.acceptance, authored.notes
        );
        let content = markdown_pages(&body);
        let page = page.min(content.len() - 1);
        let mut view = OutboundView::text(
            short(&item.title),
            format!(
                "**Issue:** `{}`\n**Status:** {}\n**Revision:** {}\n**Type:** {}\n**Milestone:** {}\n\n{}",
                item.id,
                issue_status_label(item.status),
                item.revision,
                item.issue_type.map_or("None", IssueType::as_str),
                milestone_title,
                content[page]
            ),
        );
        self.add_browse_actions(
            conversation,
            owner,
            &mut view,
            TaskBrowse::Issue {
                id: item.id.clone(),
                status,
                milestone: milestone.clone(),
                page,
            },
            content.len(),
            vec![(
                "Project issues".into(),
                TaskBrowse::Issues {
                    project: item.project_id.clone(),
                    status,
                    milestone,
                    page: 0,
                },
            )],
        )
        .await;
        let closed = item.status.terminal();
        if item.archived_at.is_none() {
            if !closed {
                for (label, field) in [
                    ("Edit requirement", "requirement"),
                    ("Edit acceptance", "acceptance"),
                    ("Edit notes", "notes"),
                ] {
                    self.add_issue_action(
                        conversation,
                        owner,
                        &mut view,
                        label,
                        &item,
                        "edit",
                        Some(field),
                        None,
                    )
                    .await;
                }
                self.add_issue_action(
                    conversation,
                    owner,
                    &mut view,
                    "Type",
                    &item,
                    "type",
                    None,
                    None,
                )
                .await;
                self.add_issue_action(
                    conversation,
                    owner,
                    &mut view,
                    "Milestone",
                    &item,
                    "milestone",
                    None,
                    None,
                )
                .await;
            }
            for (label, command) in match item.status {
                IssueStatus::Todo => vec![
                    ("Start", "execute"),
                    ("Submit", "issue.submit"),
                    ("Cancel", "issue.cancel"),
                ],
                IssueStatus::Active => vec![
                    ("Continue", "execute"),
                    ("New round", "new_round"),
                    ("Submit", "issue.submit"),
                    ("Cancel", "issue.cancel"),
                ],
                IssueStatus::PendingReview => vec![
                    ("Accept", "issue.approve"),
                    ("Reject", "issue.reject"),
                    ("Cancel", "issue.cancel"),
                ],
                IssueStatus::Completed | IssueStatus::Cancelled => vec![("Reopen", "issue.reopen")],
            } {
                self.add_issue_action(
                    conversation,
                    owner,
                    &mut view,
                    label,
                    &item,
                    command,
                    None,
                    None,
                )
                .await;
            }
        }
        self.add_action_section(
            conversation,
            owner,
            &mut view,
            "Related Jobs",
            vec![(
                "All Jobs".into(),
                TaskBrowse::IssueJobs {
                    id: item.id.clone(),
                    page: 0,
                },
                ActionStyle::Primary,
            )],
        )
        .await;
        for job in jobs["items"].as_array().into_iter().flatten() {
            if let Some(id) = job["id"].as_str() {
                self.add_entry(
                    conversation,
                    owner,
                    &mut view,
                    format!(
                        "**Job:** {} · {}",
                        escape(job["title"].as_str().unwrap_or(id)),
                        job["status"].as_str().unwrap_or("")
                    ),
                    job["title"].as_str().unwrap_or(id).into(),
                    TaskBrowse::Job {
                        id: id.into(),
                        page: 0,
                    },
                )
                .await;
            }
        }
        self.ui.send_view(conversation, &view).await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // Specify one control and its guarded Issue mutation in one place.
    async fn add_issue_action(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        view: &mut OutboundView,
        label: &str,
        item: &Issue,
        command: &str,
        field: Option<&str>,
        value: Option<String>,
    ) {
        let token = self
            .ui
            .issue_action(
                conversation,
                owner,
                &format!("issue:{}:{}", item.id, uuid::Uuid::new_v4()),
                UiAction::Issue(IssueAction {
                    id: item.id.clone(),
                    revision: item.revision,
                    command: command.into(),
                    field: field.map(str::to_owned),
                    value,
                    session_id: None,
                    job_id: None,
                    page: 0,
                }),
            )
            .await;
        view.actions.push(ActionButton {
            disabled: false,
            label: label.into(),
            token,
            style: if command == "issue.cancel" {
                ActionStyle::Danger
            } else {
                ActionStyle::Primary
            },
        });
    }

    pub(in crate::engine) async fn run_issue_action(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        action: IssueAction,
    ) -> Result<(), EngineError> {
        if matches!(action.command.as_str(), "edit" | "issue.reject") {
            self.ui.clear_reply_inputs(conversation).await;
            let mut inputs = self.service.issue_inputs.lock().await;
            if inputs.len() >= 128 && !inputs.contains_key(conversation) {
                return Err(error("Too many pending Issue edits"));
            }
            inputs.insert(
                conversation.clone(),
                PendingIssueInput {
                    action,
                    owner: owner.into(),
                    generation: self.ui.agent().generation(),
                    epoch: self.ui.sessions().epoch(conversation).await,
                },
            );
            drop(inputs);
            self.ui
                .send_view(
                    conversation,
                    &OutboundView::text(
                        "Issue input",
                        "Reply with the updated content or rejection reason. Use /cancel to stop.",
                    ),
                )
                .await?;
            return Ok(());
        }
        if action.command == "type" {
            // Keep the picker future out of the shared inbound handler's stack frame.
            return Box::pin(self.show_issue_types(conversation, owner, &action)).await;
        }
        if action.command == "milestone" {
            return self
                .show_issue_milestones(conversation, owner, &action.id, action.revision, 0)
                .await;
        }
        if matches!(
            action.command.as_str(),
            "execute" | "new_round" | "execute.new_session" | "execute.workspace"
        ) {
            return self.ui.execute_issue(conversation, owner, action).await;
        }
        self.apply_issue_action(conversation, owner, action, None)
            .await
    }

    async fn show_issue_types(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        action: &IssueAction,
    ) -> Result<(), EngineError> {
        let item = self
            .tasks_service()?
            .store()
            .issue_record(&action.id)
            .await
            .map_err(error)?;
        if item.revision != action.revision || item.status.terminal() || item.archived_at.is_some()
        {
            return Err(EngineError::InvalidAction);
        }
        let mut view = OutboundView::text("Issue type", "Choose a type label.");
        for kind in std::iter::once(None).chain(IssueType::ALL.into_iter().map(Some)) {
            let value = kind.map(IssueType::as_str);
            self.add_issue_action(
                conversation,
                owner,
                &mut view,
                value.unwrap_or("None"),
                &item,
                "issue.update",
                Some("issue_type"),
                value.map(str::to_owned),
            )
            .await;
        }
        self.ui.send_view(conversation, &view).await?;
        Ok(())
    }

    pub(in crate::engine) async fn finish_issue_input(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        pending: PendingIssueInput,
        text: &str,
    ) -> Result<(), EngineError> {
        if pending.owner != owner
            || pending.generation != self.ui.agent().generation()
            || pending.epoch != self.ui.sessions().epoch(conversation).await
        {
            return Err(EngineError::InvalidAction);
        }
        self.apply_issue_action(conversation, owner, pending.action, Some(text))
            .await
    }

    async fn apply_issue_action(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        action: IssueAction,
        text: Option<&str>,
    ) -> Result<(), EngineError> {
        let mut request = json!({"command":if action.command=="edit"{"issue.update"}else{&action.command},"issue":action.id});
        if let Some(field) = &action.field {
            request[field] = if action.command == "edit" {
                json!(text)
            } else {
                json!(action.value)
            };
        }
        if action.command == "issue.reject" {
            request["reason"] = json!(text);
        }
        let result = self
            .tasks_service()?
            .execute(
                request,
                WriteOptions {
                    actor_ref: format!("user:im:{owner}"),
                    expected_revision: Some(action.revision),
                    idempotency_key: Some(format!("im:issue-action:{}", uuid::Uuid::new_v4())),
                    ..WriteOptions::default()
                },
            )
            .await
            .map_err(error)?;
        self.show_issue(conversation, owner, &action.id, None, None, 0)
            .await?;
        if let Some(warning) = result.projection_pending {
            self.ui
                .send_view(
                    conversation,
                    &OutboundView::text("Projection pending", warning),
                )
                .await?;
        }
        Ok(())
    }

    pub(super) async fn show_issue_milestones(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        id: &str,
        revision: i64,
        page: usize,
    ) -> Result<(), EngineError> {
        let item = self
            .tasks_service()?
            .store()
            .issue_record(id)
            .await
            .map_err(error)?;
        if item.revision != revision {
            return Err(EngineError::InvalidAction);
        }
        let result = self
            .tasks_service()?
            .store()
            .milestone_page(&item.project_id, page, PAGE_SIZE)
            .await
            .map_err(error)?;
        let pages = usize::try_from(result["pages"].as_u64().unwrap_or(1)).map_err(error)?;
        let page = usize::try_from(result["page"].as_u64().unwrap_or(0)).map_err(error)?;
        let mut view = OutboundView::text("Issue milestone", "Choose a Milestone in this Project.");
        self.add_issue_action(
            conversation,
            owner,
            &mut view,
            "None",
            &item,
            "issue.update",
            Some("milestone"),
            None,
        )
        .await;
        for stage in result["items"].as_array().into_iter().flatten() {
            self.add_issue_action(
                conversation,
                owner,
                &mut view,
                stage["title"].as_str().unwrap_or_default(),
                &item,
                "issue.update",
                Some("milestone"),
                Some(stage["id"].as_str().unwrap_or_default().to_owned()),
            )
            .await;
        }
        self.add_browse_actions(
            conversation,
            owner,
            &mut view,
            TaskBrowse::IssueMilestones {
                id: id.into(),
                revision,
                page,
            },
            pages,
            vec![(
                "Issue".into(),
                TaskBrowse::Issue {
                    id: id.into(),
                    status: None,
                    milestone: None,
                    page: 0,
                },
            )],
        )
        .await;
        self.ui.send_view(conversation, &view).await?;
        Ok(())
    }
}

fn parse_issue_type(content: &str) -> Result<(Option<IssueType>, &str), EngineError> {
    let Some(tail) = content
        .strip_prefix("--type")
        .filter(|tail| tail.is_empty() || tail.starts_with(char::is_whitespace))
    else {
        return Ok((None, content));
    };
    let (kind, content) = tail
        .trim_start()
        .split_once(char::is_whitespace)
        .ok_or_else(|| error("Use /issue new --type <type> <description>"))?;
    let kind = serde_json::from_value(json!(kind))
        .map_err(|_| error("Unknown Issue type. Choose feature, bug, enhancement, documentation, question or refactor."))?;
    let content = content.trim();
    if content.is_empty() {
        return Err(error("Issue description is required"));
    }
    Ok((Some(kind), content))
}
