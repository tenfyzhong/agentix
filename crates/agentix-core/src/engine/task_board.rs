use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use tokio::sync::Mutex;

use agentix_task::{JobStatus, Service, Snapshot, Task, TaskPhase, TaskStatus, WriteOptions};

mod browse;
mod inbox;
#[cfg(test)]
mod tests;
pub(super) use browse::TaskBrowse;
use serde_json::json;

use super::{
    ActionButton, ActionStyle, ConversationRef, EngineError, OutboundView, SessionId, UiAction,
};

#[derive(Debug, Clone)]
pub(super) struct TaskAction {
    pub task_id: String,
    pub command: String,
    pub revision: i64,
    pub session_id: SessionId,
}

pub(super) struct PendingTaskInput {
    action: TaskAction,
    owner_id: String,
    generation: u64,
    epoch: u64,
}

fn error(error: impl std::fmt::Display) -> EngineError {
    EngineError::InvalidInput(error.to_string())
}

/// Owns task-board state independently of the IM engine.
pub(super) struct TaskBoardService {
    pub(super) backend: Option<Arc<Service>>,
    pub(super) output: crate::OutputConfig,
    conversations: Mutex<HashMap<(String, String), Vec<serde_json::Value>>>,
    inputs: Mutex<HashMap<ConversationRef, PendingTaskInput>>,
    refresh: Mutex<()>,
    state: crate::SqliteState,
    pub(super) consumer: String,
}

/// The application facilities used by task views. No host or storage implementation
/// is exposed through this port.
#[async_trait]
pub(super) trait TaskBoardUi: Send + Sync {
    fn agent(&self) -> &dyn crate::AgentAdapter;
    fn sessions(&self) -> &super::SessionService;
    fn channels(&self) -> &HashMap<crate::ChannelKind, Arc<dyn crate::ChannelAdapter>>;
    async fn send_view(
        &self,
        conversation: &ConversationRef,
        view: &OutboundView,
    ) -> Result<crate::MessageRef, EngineError>;
    async fn issue_action(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        group: &str,
        action: UiAction,
    ) -> String;
    async fn update_command_menu_best_effort(&self, conversation: &ConversationRef, attached: bool);
}

pub(super) struct TaskBoardView<'a> {
    service: &'a TaskBoardService,
    ui: &'a dyn TaskBoardUi,
}

impl TaskBoardService {
    pub(super) fn new(backend: Option<Arc<Service>>, state: crate::SqliteState) -> Self {
        Self {
            backend,
            output: crate::OutputConfig::default(),
            state,
            conversations: Mutex::new(HashMap::new()),
            inputs: Mutex::new(HashMap::new()),
            refresh: Mutex::new(()),
            consumer: "default".into(),
        }
    }

    pub(super) async fn take_input(
        &self,
        conversation: &ConversationRef,
    ) -> Option<PendingTaskInput> {
        self.inputs.lock().await.remove(conversation)
    }

    pub(super) fn view<'a>(&'a self, ui: &'a dyn TaskBoardUi) -> TaskBoardView<'a> {
        TaskBoardView { service: self, ui }
    }
}

impl TaskBoardView<'_> {
    fn tasks_service(&self) -> Result<&Service, EngineError> {
        self.service
            .backend
            .as_deref()
            .ok_or_else(|| error("Task board is not configured."))
    }

    pub(super) async fn show_tasks(
        &self,
        conversation: &ConversationRef,
        filter: Option<&str>,
    ) -> Result<(), EngineError> {
        let tasks = self
            .tasks_service()?
            .store()
            .legacy_tasks(filter, 50)
            .await
            .map_err(error)?;
        let body = tasks
            .iter()
            .map(|t| format!("{} · {}\n{}", t.id, t.status, t.title))
            .collect::<Vec<_>>()
            .join("\n\n");
        self.ui
            .send_view(
                conversation,
                &OutboundView::text(
                    "Tasks",
                    if body.is_empty() {
                        "No matching tasks.".into()
                    } else {
                        body
                    },
                ),
            )
            .await?;
        Ok(())
    }

    pub(super) async fn show_task(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        id: &str,
    ) -> Result<(), EngineError> {
        self.show_task_page(conversation, owner_id, id, 0).await
    }

    async fn show_task_page(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        id: &str,
        page: usize,
    ) -> Result<(), EngineError> {
        let state = self
            .tasks_service()?
            .store()
            .browse_snapshot(agentix_task::BrowseScope::Task(id))
            .await
            .map_err(error)?;
        let task = &state.tasks[state.task_index(id).map_err(error)?];
        let job = &state.jobs[state.job_index(&task.job_id).map_err(error)?];
        let markdown = self
            .tasks_service()?
            .task_markdown(id)
            .await
            .unwrap_or_else(|_| "Task document is unavailable.".into());
        let mut content = String::new();
        for (label, title) in [("Task title", &task.title), ("Job title", &job.title)] {
            if title.chars().count() > 60 {
                content.push_str(&format!("**{label}:** {}\n\n", browse::escape(title)));
            }
        }
        if let Some(reason) = &task.reason {
            content.push_str(&format!("{}\n\n", browse::escape(reason)));
        }
        content.push_str(&markdown);
        let pages = browse::markdown_pages(&content);
        let page = page.min(pages.len() - 1);
        let mut view = OutboundView::text(
            browse::short(&task.title),
            format!(
                "**Task:** `{}`\n**Status:** {} · {}\n**Job:** {}\n**Revision:** {}\n\n{}",
                task.id,
                task.status,
                task.phase.map_or_else(|| "—".into(), |p| p.to_string()),
                browse::escape(&browse::short(&job.title)),
                task.revision,
                pages[page]
            ),
        );
        self.add_browse_actions(
            conversation,
            owner_id,
            &mut view,
            TaskBrowse::Task {
                id: id.into(),
                page,
            },
            pages.len(),
            vec![(
                "Job".into(),
                TaskBrowse::Job {
                    id: task.job_id.clone(),
                    page: 0,
                },
            )],
        )
        .await;
        self.add_task_mutations(conversation, owner_id, &state, task, &mut view)
            .await;
        self.ui.send_view(conversation, &view).await?;
        Ok(())
    }

    async fn add_task_mutations(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        state: &Snapshot,
        task: &Task,
        view: &mut OutboundView,
    ) {
        let job = &state.jobs[state.job_index(&task.job_id).expect("task Job")];
        let lease = state.leases.iter().find(|l| l.task_id == task.id);
        if let Some(session_id) = self.ui.sessions().current(conversation).await
            && job.status != JobStatus::Cancelled
            && job.archived_at.is_none()
            && lease.is_none_or(|l| session_id.matches_task_lease(&l.session_ref, &l.executor_ref))
        {
            let group = format!("task:{}:{}", task.id, uuid::Uuid::new_v4());
            for (label, command, target) in [
                ("Claim", "task.claim", TaskStatus::InProgress),
                ("Start", "task.start", TaskStatus::InProgress),
                ("Block", "task.block", TaskStatus::Blocked),
                ("Wait", "task.wait", TaskStatus::WaitingUser),
                ("Done", "task.done", TaskStatus::Done),
                ("Fail", "task.fail", TaskStatus::Failed),
                ("Cancel", "task.cancel", TaskStatus::Cancelled),
                ("Retry", "task.retry", TaskStatus::Todo),
                ("Reopen", "task.reopen", TaskStatus::Todo),
            ] {
                let allowed = match command {
                    "task.retry" => task.status == TaskStatus::Failed,
                    "task.reopen" => {
                        matches!(task.status, TaskStatus::Done | TaskStatus::Cancelled)
                    }
                    "task.start" => {
                        lease.is_some()
                            && task.phase == Some(TaskPhase::Planning)
                            && task.current_plan.is_some()
                            && state.dependencies_done(task)
                    }
                    "task.done" => lease.is_some() && task.phase == Some(TaskPhase::Executing),
                    _ => task.status.allows(target),
                };
                if !allowed {
                    continue;
                }
                let token = self
                    .ui
                    .issue_action(
                        conversation,
                        owner_id,
                        &group,
                        UiAction::Task(TaskAction {
                            task_id: task.id.clone(),
                            command: command.into(),
                            revision: task.revision,
                            session_id: session_id.clone(),
                        }),
                    )
                    .await;
                view.actions.push(ActionButton {
                    label: label.into(),
                    token,
                    style: ActionStyle::Default,
                });
            }
        }
    }

    pub(super) async fn run_task_action(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        action: TaskAction,
    ) -> Result<(), EngineError> {
        if self.ui.sessions().current(conversation).await.as_ref() != Some(&action.session_id) {
            return Err(EngineError::InvalidAction);
        }
        if matches!(
            action.command.as_str(),
            "task.block" | "task.wait" | "task.fail"
        ) {
            self.service.inputs.lock().await.insert(
                conversation.clone(),
                PendingTaskInput {
                    action,
                    owner_id: owner_id.into(),
                    generation: self.ui.agent().generation(),
                    epoch: self.ui.sessions().epoch(conversation).await,
                },
            );
            self.ui
                .send_view(
                    conversation,
                    &OutboundView::text("Task reason", "Reply with a reason, or use /cancel."),
                )
                .await?;
            Ok(())
        } else {
            self.apply_task_action(conversation, owner_id, action, None)
                .await
        }
    }

    pub(super) async fn finish_task_input(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        pending: PendingTaskInput,
        reason: &str,
    ) -> Result<(), EngineError> {
        if pending.owner_id != owner_id
            || pending.generation != self.ui.agent().generation()
            || pending.epoch != self.ui.sessions().epoch(conversation).await
        {
            return Err(EngineError::InvalidAction);
        }
        self.apply_task_action(conversation, owner_id, pending.action, Some(reason))
            .await
    }

    async fn apply_task_action(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        action: TaskAction,
        reason: Option<&str>,
    ) -> Result<(), EngineError> {
        if self.ui.sessions().current(conversation).await.as_ref() != Some(&action.session_id) {
            return Err(EngineError::InvalidAction);
        }
        let service = self.tasks_service()?;
        let lease = service
            .store()
            .task_lease(&action.task_id)
            .await
            .map_err(error)?;
        let lease = lease.as_ref();
        if lease.is_some_and(|l| {
            !action
                .session_id
                .matches_task_lease(&l.session_ref, &l.executor_ref)
        }) {
            return Err(EngineError::InvalidAction);
        }
        let options = WriteOptions {
            actor_ref: format!("im:{owner_id}"),
            session_ref: Some(action.session_id.native_str().to_owned()),
            lease_token: lease.map(|l| l.token.clone()),
            expected_revision: Some(action.revision),
            ..WriteOptions::default()
        };
        let result=service.execute(json!({"command":action.command,"task":action.task_id,"reason":reason,"session":action.session_id.native_str(),"executor":action.session_id.task_executor()}),options).await.map_err(error)?;
        if let Some(warning) = result.projection_pending {
            self.ui
                .send_view(
                    conversation,
                    &OutboundView::text("Projection pending", warning),
                )
                .await?;
        }
        self.show_task(conversation, owner_id, &action.task_id)
            .await
    }

    pub(super) async fn task_session_event(&self, command: &str, session: &str) {
        if let Some(service) = &self.service.backend {
            let key = SessionId::new(session);
            let session = key.native_str();
            let executor = crate::SessionKey::decode(&key).map(|_| key.task_executor());
            if let Err(error) = service
                .execute(
                    json!({"command":command,"session":session,"executor":executor}),
                    WriteOptions {
                        actor_ref: "system:agentix".into(),
                        session_ref: Some(session.into()),
                        ..WriteOptions::default()
                    },
                )
                .await
            {
                tracing::warn!(%error, session, command, "task session event failed");
            }
            if command == "session.end"
                && let Err(error) = self.refresh_task_board().await
            {
                tracing::warn!(%error, "task notification refresh failed during session exit");
            }
        }
    }

    pub async fn refresh_task_board(&self) -> Result<(), EngineError> {
        let Some(service) = &self.service.backend else {
            return Ok(());
        };
        let _guard = self.service.refresh.lock().await;
        service.store().reap_expired().await.map_err(error)?;
        let key = format!("agentix:cursor:{}", self.service.consumer);
        let legacy = service
            .store()
            .metadata(&key)
            .await
            .map_err(error)?
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let cursor = self
            .service
            .state
            .notification_cursor(&self.service.consumer, legacy)
            .await?;
        let events = service
            .store()
            .events(None, cursor, 100)
            .await
            .map_err(error)?;
        for event in events {
            let notification = if matches!(
                event.event_type.as_str(),
                "task.waiting_user"
                    | "task.blocked"
                    | "task.failed"
                    | "job.completed"
                    | "job.pending_review"
                    | "job.rejected"
            ) && let Some(session) = event.session_ref.as_deref()
                && let Ok(session) = self
                    .ui
                    .agent()
                    .canonical_session(&SessionId::new(session))
                    .await
                && let Some(conversation) = self.ui.sessions().bound_conversation(&session).await
            {
                let body = format!(
                    "{}\n{}\n{}",
                    event.payload["title"].as_str().unwrap_or(""),
                    event.event_type,
                    event.payload["reason"]
                        .as_str()
                        .or_else(|| event.payload["review_reason"].as_str())
                        .unwrap_or("")
                );
                Some((conversation, OutboundView::text("Task update", body)))
            } else {
                None
            };
            self.service
                .state
                .stage_notification(
                    &self.service.consumer,
                    event.sequence,
                    notification
                        .as_ref()
                        .map(|(conversation, view)| (conversation, view)),
                )
                .await?;
        }
        let rendered = service
            .store()
            .metadata("sequence")
            .await
            .map_err(error)?
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if rendered < service.store().latest_sequence().await.map_err(error)? {
            service.sync().await.map_err(error)?;
        }
        Ok(())
    }
}

impl TaskBoardService {
    pub(in crate::engine) async fn record_job_message(&self, event: &crate::AgentEvent) {
        let Some(service) = &self.backend else {
            return;
        };
        match event {
            crate::AgentEvent::ItemCompleted {
                session_id,
                turn_id,
                item,
            } => {
                let process = self.output.process_text(item);
                let role = match item.kind.as_str() {
                    "userMessage" => "user",
                    "agentMessage" => "assistant",
                    _ if process.is_some() => "assistant",
                    _ => return,
                };
                let Some(text) = process
                    .as_deref()
                    .or(item.text.as_deref())
                    .filter(|text| !text.trim().is_empty())
                else {
                    return;
                };
                let mut conversations = self.conversations.lock().await;
                let messages = conversations
                    .entry((session_id.clone(), turn_id.clone()))
                    .or_default();
                let id = format!("{turn_id}:{}", item.id);
                let message = json!({"id":id,"role":role,"text":text});
                if let Some(existing) = messages.iter_mut().find(|message| message["id"] == id) {
                    *existing = message;
                } else {
                    messages.push(message);
                }
            }
            crate::AgentEvent::TurnCompleted {
                session_id,
                turn_id,
                ..
            } => {
                let key = (session_id.clone(), turn_id.clone());
                let messages = self
                    .conversations
                    .lock()
                    .await
                    .get(&key)
                    .cloned()
                    .unwrap_or_default();
                if messages.is_empty() {
                    return;
                }
                let native = SessionId::new(session_id);
                let job = if crate::SessionKey::decode(&native).is_some() {
                    match service.store().session_job_page(session_id, 0, 1).await {
                        Ok(page) => {
                            if let Some(job) = page.jobs.first() {
                                Some(job.id.clone())
                            } else {
                                self.conversations.lock().await.remove(&key);
                                return;
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, "task association failed");
                            return;
                        }
                    }
                } else {
                    None
                };
                let result = service.execute(json!({"command":"session.record","session":native.native_str(),"job":job,"messages":messages}), WriteOptions {session_ref:Some(native.native_str().to_owned()), ..WriteOptions::default()}).await;
                match result {
                    Ok(result) => {
                        self.conversations.lock().await.remove(&key);
                        if let Some(error) = result.projection_pending {
                            tracing::warn!(%error, "Job conversation projection pending");
                        }
                    }
                    Err(error) => tracing::warn!(%error, "Job conversation recording failed"),
                }
            }
            _ => (),
        }
    }
}
