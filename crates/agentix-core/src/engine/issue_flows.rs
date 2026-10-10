//! Explicit Issue execution reuses the existing session and workspace facilities.
use super::task_board::PendingIssueExecution;
use super::{
    ActionButton, ActionStyle, ConversationRef, Engine, EngineError, IssueAction,
    MultiplexerMutation, MultiplexerTarget, OutboundView, SessionId, UiAction,
};
use agentix_task::{IssueStatus, WriteOptions};
use serde_json::json;
use std::{collections::HashSet, path::Path};

impl Engine {
    pub(super) async fn execute_issue_round(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        action: IssueAction,
    ) -> Result<(), EngineError> {
        let service = self
            .tasks
            .backend
            .as_deref()
            .ok_or_else(|| EngineError::InvalidInput("Task board is not configured".into()))?;
        service
            .sync_delivery_document(&action.id)
            .await
            .map_err(issue_error)?;
        let item = service
            .store()
            .issue_record(&action.id)
            .await
            .map_err(issue_error)?;
        if item.revision != action.revision
            || item.archived_at.is_some()
            || !matches!(item.status, IssueStatus::Todo | IssueStatus::Active)
        {
            return Err(EngineError::InvalidAction);
        }
        let project = service
            .store()
            .project_result(&item.project_id)
            .await
            .map_err(issue_error)?;
        if project.archived_at.is_some() {
            return Err(EngineError::InvalidAction);
        }
        if action.command == "execute.workspace" {
            if !self.multiplexer_enabled || !self.agent.capabilities().workspace_runtime {
                return Err(EngineError::InvalidAction);
            }
            self.remember_issue_execution(conversation, owner, action, true)
                .await?;
            return self
                .begin_directory_draft_at(
                    conversation,
                    owner,
                    MultiplexerMutation {
                        target: MultiplexerTarget::NewSession {
                            name: format!("issue-{}", uuid::Uuid::new_v4().simple()),
                            cwd: project.root.clone(),
                        },
                        launch_agent: false,
                    },
                    Some(project.root),
                )
                .await;
        }
        if action.command == "execute.new_session" {
            let session = action
                .session_id
                .as_ref()
                .ok_or(EngineError::InvalidAction)?;
            if self.sessions.current(conversation).await.as_ref() != Some(session)
                || !self.agent.session_access(session).await.can_write()
            {
                return Err(EngineError::InvalidAction);
            }
            let prompt = issue_prompt(&item, None);
            self.request_new_session(conversation, session).await?;
            if !self
                .enqueue_switch_prompt(conversation, &uuid::Uuid::new_v4().to_string(), &prompt)
                .await?
            {
                return Err(EngineError::InvalidInput("No new-session request is pending. Choose an existing writable session or a workspace.".into()));
            }
            return Ok(());
        }
        if let Some(session) = &action.session_id {
            return self
                .execute_issue_session(conversation, owner, &item, &action, session)
                .await;
        }
        self.show_issue_sessions(conversation, owner, &item, &action)
            .await
    }

    async fn execute_issue_session(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        item: &agentix_task::Issue,
        action: &IssueAction,
        session: &SessionId,
    ) -> Result<(), EngineError> {
        let service = self
            .tasks
            .backend
            .as_deref()
            .ok_or(EngineError::InvalidAction)?;
        let summary = self.issue_session_project(session).await?;
        if summary.as_deref() != Some(&item.project_id)
            || !self.agent.session_access(session).await.can_write()
        {
            return Err(EngineError::InvalidAction);
        }
        let assignment = service
            .store()
            .routing_assignment(session.native_str())
            .await
            .map_err(issue_error)?;
        if assignment["job_id"].as_str().is_some() && assignment["issue_id"] != item.id {
            return Err(EngineError::InvalidInput("This session owns another Job. Finish or release that Task, or choose another session.".into()));
        }
        let assigned_job = assignment["job_id"].as_str();
        if assigned_job.is_some()
            && (action.command == "new_round"
                || action
                    .job_id
                    .as_deref()
                    .is_some_and(|id| Some(id) != assigned_job))
        {
            return Err(EngineError::InvalidInput("Finish or release the current Task before starting another Job round in this session.".into()));
        }
        if self.sessions.current(conversation).await.as_ref() != Some(session) {
            self.remember_issue_execution(conversation, owner, action.clone(), false)
                .await?;
            return self.attach(conversation, owner, session.clone()).await;
        }
        let offset = i64::try_from(
            action
                .page
                .checked_mul(6)
                .ok_or(EngineError::InvalidAction)?,
        )
        .map_err(issue_error)?;
        let rows = service
            .store()
            .issue_job_page(
                &item.id,
                Some(agentix_task::JobStatus::Active),
                Some(false),
                offset,
                6,
            )
            .await
            .map_err(issue_error)?;
        let jobs = rows["items"].as_array().ok_or(EngineError::InvalidAction)?;
        let total = rows["total"].as_u64().ok_or(EngineError::InvalidAction)?;
        if action.command != "new_round"
            && action.job_id.is_none()
            && assigned_job.is_none()
            && total > 1
        {
            return self
                .show_issue_job_choices(conversation, owner, action, &rows)
                .await;
        }
        let existing = if action.command == "new_round" {
            None
        } else if let Some(id) = action.job_id.as_deref().or(assigned_job) {
            let job = service.store().job_record(id).await.map_err(issue_error)?;
            if job.issue_id.as_deref() != Some(&item.id)
                || job.status != agentix_task::JobStatus::Active
                || job.archived_at.is_some()
            {
                return Err(EngineError::InvalidAction);
            }
            Some(serde_json::to_value(job).map_err(issue_error)?)
        } else {
            jobs.first().cloned()
        };
        let job = if let Some(job) = existing {
            job.clone()
        } else {
            service.execute(json!({"command":"job.create","project":item.project_id,"executor":session.task_executor(),"session":session.native_str(),"title":format!("{} execution",item.title),"prompt":issue_prompt(item,None),"issue":item.id,"issue_revision":item.revision,"issue_content_hash":item.content_hash}),
                    WriteOptions{actor_ref:format!("user:im:{owner}"),session_ref:Some(session.native_str().into()),idempotency_key:Some(format!("im:issue-execution:{}",uuid::Uuid::new_v4())),..WriteOptions::default()}).await.map_err(issue_error)?.result
        };
        return self
            .send_prompt(conversation, &issue_prompt(item, job["id"].as_str()))
            .await;
    }

    async fn show_issue_job_choices(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        action: &IssueAction,
        rows: &serde_json::Value,
    ) -> Result<(), EngineError> {
        let jobs = rows["items"].as_array().ok_or(EngineError::InvalidAction)?;
        let total = rows["total"].as_u64().ok_or(EngineError::InvalidAction)?;
        let offset = action
            .page
            .checked_mul(6)
            .ok_or(EngineError::InvalidAction)?;
        if jobs.is_empty() && action.page > 0 {
            let mut target = action.clone();
            target.page = 0;
            return Box::pin(self.execute_issue_round(conversation, owner, target)).await;
        }
        let mut view = OutboundView::text(
            "Choose Job",
            "Several execution rounds remain active. Choose the Job to continue.",
        );
        for job in jobs {
            let mut target = action.clone();
            target.job_id = job["id"].as_str().map(str::to_owned);
            target.page = 0;
            self.issue_execution_button(
                conversation,
                owner,
                &mut view,
                job["title"].as_str().unwrap_or("Job"),
                target,
            )
            .await;
        }
        if action.page > 0 {
            let mut target = action.clone();
            target.page -= 1;
            self.issue_execution_button(conversation, owner, &mut view, "Previous Jobs", target)
                .await;
        }
        if u64::try_from(offset).map_err(issue_error)? + 6 < total {
            let mut target = action.clone();
            target.page += 1;
            self.issue_execution_button(conversation, owner, &mut view, "Next Jobs", target)
                .await;
        }
        self.send_view(conversation, &view).await?;
        Ok(())
    }

    async fn show_issue_sessions(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        item: &agentix_task::Issue,
        action: &IssueAction,
    ) -> Result<(), EngineError> {
        let service = self
            .tasks
            .backend
            .as_deref()
            .ok_or(EngineError::InvalidAction)?;
        let mut view = OutboundView::text(
            "Choose execution session",
            "Select a writable session in this Issue's Project. Browse, edit and acceptance do not require a session.",
        );
        let mut cursor = None;
        let mut seen = HashSet::new();
        let mut shown = 0;
        let offset = action
            .page
            .checked_mul(6)
            .ok_or(EngineError::InvalidAction)?;
        let mut matched = 0;
        let mut more = false;
        loop {
            let page = self.agent.list_sessions(cursor, 100).await?;
            for summary in page.sessions {
                if self.agent.session_access(&summary.id).await.can_write()
                    && service
                        .project_for_session(
                            summary.cwd.as_deref().map(Path::new),
                            Some(summary.id.native_str()),
                        )
                        .await
                        .map_err(issue_error)?
                        .is_some_and(|p| p.id == item.project_id)
                {
                    matched += 1;
                    if matched <= offset {
                        continue;
                    }
                    if shown >= 6 {
                        more = true;
                        break;
                    }
                    self.sessions
                        .cache
                        .lock()
                        .await
                        .insert(summary.id.clone(), summary.clone());
                    let mut target = action.clone();
                    target.session_id = Some(summary.id.clone());
                    target.page = 0;
                    self.issue_execution_button(
                        conversation,
                        owner,
                        &mut view,
                        &summary.id.to_string(),
                        target,
                    )
                    .await;
                    shown += 1;
                }
            }
            cursor = page.next_cursor;
            if more || cursor.as_ref().is_none_or(|c| !seen.insert(c.clone())) {
                break;
            }
        }
        if action.page > 0 {
            let mut target = action.clone();
            target.page -= 1;
            self.issue_execution_button(
                conversation,
                owner,
                &mut view,
                "Previous sessions",
                target,
            )
            .await;
        }
        if more {
            let mut target = action.clone();
            target.page += 1;
            self.issue_execution_button(conversation, owner, &mut view, "Next sessions", target)
                .await;
        }
        self.add_issue_execution_targets(conversation, owner, item, action, &mut view)
            .await?;
        if shown == 0 {
            view.body.push_str("\nNo writable session is available in this Project. Choose New workspace when available.");
        }
        self.send_view(conversation, &view).await?;
        Ok(())
    }

    async fn add_issue_execution_targets(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        item: &agentix_task::Issue,
        action: &IssueAction,
        view: &mut OutboundView,
    ) -> Result<(), EngineError> {
        if let Some(current) = self.sessions.current(conversation).await
            && self.agent.session_access(&current).await.can_write()
            && self.agent.capabilities().session_control
            && self.agent.supports_command(&current, "new").await
            && self.agent.session_client_id(&current).await.is_some()
            && self.issue_session_project(&current).await?.as_deref() == Some(&item.project_id)
        {
            let mut target = action.clone();
            target.command = "execute.new_session".into();
            target.session_id = Some(current);
            self.issue_execution_button(conversation, owner, view, "New session", target)
                .await;
        }
        if self.multiplexer_enabled && self.agent.capabilities().workspace_runtime {
            let mut target = action.clone();
            target.command = "execute.workspace".into();
            self.issue_execution_button(conversation, owner, view, "New workspace", target)
                .await;
        }
        Ok(())
    }

    async fn remember_issue_execution(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        action: IssueAction,
        workspace: bool,
    ) -> Result<(), EngineError> {
        let mut pending = self.tasks.issue_executions.lock().await;
        if pending.len() >= 128 && !pending.contains_key(conversation) {
            return Err(EngineError::InvalidInput(
                "Too many pending Issue executions".into(),
            ));
        }
        pending.insert(
            conversation.clone(),
            PendingIssueExecution {
                action,
                owner: owner.into(),
                generation: self.agent.generation(),
                epoch: self.sessions.epoch(conversation).await,
                workspace,
            },
        );
        Ok(())
    }

    pub(super) async fn resume_issue_execution(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
        workspace: bool,
    ) -> Result<(), EngineError> {
        let pending = {
            let mut map = self.tasks.issue_executions.lock().await;
            let Some(pending) = map.get(conversation) else {
                return Ok(());
            };
            if pending.workspace != workspace
                || (!workspace && pending.action.session_id.as_ref() != Some(session))
            {
                return Ok(());
            }
            map.remove(conversation).expect("pending execution exists")
        };
        if pending.generation != self.agent.generation()
            || pending.epoch.checked_add(1) != Some(self.sessions.epoch(conversation).await)
        {
            return Err(EngineError::InvalidAction);
        }
        let mut action = pending.action;
        action.session_id = Some(session.clone());
        if workspace {
            action.command = "new_round".into();
        }
        Box::pin(self.execute_issue_round(conversation, &pending.owner, action)).await
    }

    async fn issue_execution_button(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        view: &mut OutboundView,
        label: &str,
        action: IssueAction,
    ) {
        let token = self
            .issue_action(
                conversation,
                owner,
                &format!("issue-execution:{}", uuid::Uuid::new_v4()),
                UiAction::Issue(action),
            )
            .await;
        view.actions.push(ActionButton {
            disabled: false,
            label: label.into(),
            token,
            style: ActionStyle::Primary,
        });
    }

    async fn issue_session_project(
        &self,
        session: &SessionId,
    ) -> Result<Option<String>, EngineError> {
        let service = self
            .tasks
            .backend
            .as_deref()
            .ok_or(EngineError::InvalidAction)?;
        let mut cursor = None;
        let mut seen = HashSet::new();
        loop {
            let page = self.agent.list_sessions(cursor, 100).await?;
            if let Some(summary) = page.sessions.iter().find(|s| s.id == *session) {
                return service
                    .project_for_session(
                        summary.cwd.as_deref().map(Path::new),
                        Some(session.native_str()),
                    )
                    .await
                    .map(|p| p.map(|p| p.id))
                    .map_err(issue_error);
            }
            cursor = page.next_cursor;
            if cursor.as_ref().is_none_or(|c| !seen.insert(c.clone())) {
                if let Some(summary) = self.sessions.cache.lock().await.get(session).cloned() {
                    return service
                        .project_for_session(
                            summary.cwd.as_deref().map(Path::new),
                            Some(session.native_str()),
                        )
                        .await
                        .map(|p| p.map(|p| p.id))
                        .map_err(issue_error);
                }
                return Ok(None);
            }
        }
    }
}

fn issue_error(error: impl std::fmt::Display) -> EngineError {
    EngineError::InvalidInput(error.to_string())
}

fn issue_prompt(item: &agentix_task::Issue, job: Option<&str>) -> String {
    format!(
        "Taskix Issue request: {}\n{}Implement this Issue's requirement and verify its acceptance criteria. Read issue show {} and its Milestone goal before execution. Preserve the Issue association using its current revision and content hash. Follow the normal Task claim, Plan, start and verification workflow. Job completion leaves this Issue active; human Issue acceptance is separate.\n\nRequirement:\n{}\n\nAcceptance criteria:\n{}",
        item.id,
        job.map_or_else(String::new, |id| format!(
            "Continue ACTIVE Job {id} for this Issue. "
        )),
        item.id,
        item.requirement,
        item.acceptance
    )
}
