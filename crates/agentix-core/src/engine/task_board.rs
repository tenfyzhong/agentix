use std::sync::Arc;

use agentix_task::{JobStatus, Service, TaskPhase, TaskStatus, WriteOptions};
use serde_json::json;

use super::{
    ActionButton, ActionStyle, ConversationRef, Engine, EngineError, OutboundView, SessionId,
    UiAction,
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

impl Engine {
    #[must_use]
    pub fn with_task_board(mut self, service: Arc<Service>) -> Self {
        self.task_board = Some(service);
        self
    }

    #[must_use]
    pub fn with_task_consumer(mut self, consumer: String) -> Self {
        self.task_consumer = consumer;
        self
    }

    fn tasks_service(&self) -> Result<&Service, EngineError> {
        self.task_board
            .as_deref()
            .ok_or_else(|| error("Task board is not configured."))
    }

    pub(super) async fn show_task_jobs(
        &self,
        conversation: &ConversationRef,
        project: Option<&str>,
    ) -> Result<(), EngineError> {
        let service = self.tasks_service()?;
        let state = service.store().snapshot().await.map_err(error)?;
        let project = project
            .map(|p| state.project_index(p).map(|i| state.projects[i].id.clone()))
            .transpose()
            .map_err(error)?;
        let body = state
            .jobs
            .iter()
            .filter(|j| {
                j.archived_at.is_none() && project.as_ref().is_none_or(|p| *p == j.project_id)
            })
            .take(50)
            .map(|j| format!("{} · {}\n{}", j.id, j.status, j.title))
            .collect::<Vec<_>>()
            .join("\n\n");
        self.send_view(
            conversation,
            &OutboundView::text(
                "Jobs",
                if body.is_empty() {
                    "No matching jobs.".into()
                } else {
                    body
                },
            ),
        )
        .await?;
        Ok(())
    }

    pub(super) async fn show_tasks(
        &self,
        conversation: &ConversationRef,
        filter: Option<&str>,
    ) -> Result<(), EngineError> {
        let state = self
            .tasks_service()?
            .store()
            .snapshot()
            .await
            .map_err(error)?;
        let mut project = None;
        let mut job = None;
        if let Some(filter) = filter {
            if let Ok(i) = state.job_index(filter) {
                job = Some(state.jobs[i].id.clone());
            } else {
                project = Some(
                    state.projects[state.project_index(filter).map_err(error)?]
                        .id
                        .clone(),
                );
            }
        }
        let body = state
            .tasks
            .iter()
            .filter(|t| {
                job.as_ref().is_none_or(|j| *j == t.job_id)
                    && project.as_ref().is_none_or(|p| *p == t.project_id)
            })
            .take(50)
            .map(|t| format!("{} · {}\n{}", t.id, t.status, t.title))
            .collect::<Vec<_>>()
            .join("\n\n");
        self.send_view(
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
        let state = self
            .tasks_service()?
            .store()
            .snapshot()
            .await
            .map_err(error)?;
        let task = &state.tasks[state.task_index(id).map_err(error)?];
        let job = &state.jobs[state.job_index(&task.job_id).map_err(error)?];
        let lease = state.leases.iter().find(|l| l.task_id == task.id);
        let mut view = OutboundView::text(
            &task.title,
            format!(
                "{}\nStatus: {}\nPhase: {}\nJob: {}\nRevision: {}\n{}",
                task.id,
                task.status,
                task.phase.map_or_else(|| "—".into(), |p| p.to_string()),
                job.title,
                task.revision,
                task.reason.as_deref().unwrap_or("")
            ),
        );
        if let Some(session_id) = self.sessions.current(conversation).await
            && job.status != JobStatus::Cancelled
            && job.archived_at.is_none()
            && lease.is_none_or(|l| l.session_ref == session_id.as_str())
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
                            && task.dependencies.iter().all(|d| {
                                state
                                    .tasks
                                    .iter()
                                    .any(|t| t.id == *d && t.status == TaskStatus::Done)
                            })
                    }
                    "task.done" => lease.is_some() && task.phase == Some(TaskPhase::Executing),
                    _ => task.status.allows(target),
                };
                if !allowed {
                    continue;
                }
                let token = self
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
        self.send_view(conversation, &view).await?;
        Ok(())
    }

    pub(super) async fn run_task_action(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        action: TaskAction,
    ) -> Result<(), EngineError> {
        if self.sessions.current(conversation).await.as_ref() != Some(&action.session_id) {
            return Err(EngineError::InvalidAction);
        }
        if matches!(
            action.command.as_str(),
            "task.block" | "task.wait" | "task.fail"
        ) {
            self.task_inputs.lock().await.insert(
                conversation.clone(),
                PendingTaskInput {
                    action,
                    owner_id: owner_id.into(),
                    generation: self.agent.generation(),
                    epoch: self.sessions.epoch(conversation).await,
                },
            );
            self.send_view(
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
            || pending.generation != self.agent.generation()
            || pending.epoch != self.sessions.epoch(conversation).await
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
        if self.sessions.current(conversation).await.as_ref() != Some(&action.session_id) {
            return Err(EngineError::InvalidAction);
        }
        let service = self.tasks_service()?;
        let state = service.store().snapshot().await.map_err(error)?;
        let lease = state.leases.iter().find(|l| l.task_id == action.task_id);
        if lease.is_some_and(|l| l.session_ref != action.session_id.as_str()) {
            return Err(EngineError::InvalidAction);
        }
        let options = WriteOptions {
            actor_ref: format!("im:{owner_id}"),
            session_ref: Some(action.session_id.to_string()),
            lease_token: lease.map(|l| l.token.clone()),
            expected_revision: Some(action.revision),
            ..WriteOptions::default()
        };
        let result=service.execute(json!({"command":action.command,"task":action.task_id,"reason":reason,"session":action.session_id.to_string(),"executor":format!("agent:{}",action.session_id)}),options).await.map_err(error)?;
        if let Some(warning) = result.projection_pending {
            self.send_view(
                conversation,
                &OutboundView::text("Projection pending", warning),
            )
            .await?;
        }
        self.show_task(conversation, owner_id, &action.task_id)
            .await
    }

    pub(super) async fn task_session_event(&self, command: &str, session: &str) {
        if let Some(service) = &self.task_board {
            if let Err(error) = service
                .execute(
                    json!({"command":command,"session":session}),
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
        let Some(service) = &self.task_board else {
            return Ok(());
        };
        let _guard = self.task_refresh.lock().await;
        service.store().reap_expired().await.map_err(error)?;
        let key = format!("agentix:cursor:{}", self.task_consumer);
        let cursor = service
            .store()
            .metadata(&key)
            .await
            .map_err(error)?
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let events = service
            .store()
            .events(None, cursor, 100)
            .await
            .map_err(error)?;
        for event in events {
            if matches!(
                event.event_type.as_str(),
                "task.waiting_user" | "task.blocked" | "task.failed" | "job.completed"
            ) && let Some(session) = event.session_ref.as_deref()
                && let Some(conversation) = self
                    .sessions
                    .bound_conversation(&SessionId::new(session))
                    .await
            {
                let body = format!(
                    "{}\n{}\n{}",
                    event.payload["title"].as_str().unwrap_or(""),
                    event.event_type,
                    event.payload["reason"].as_str().unwrap_or("")
                );
                self.send_view(&conversation, &OutboundView::text("Task update", body))
                    .await?;
            }
            service
                .store()
                .set_metadata(&key, &json!(event.sequence))
                .await
                .map_err(error)?;
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
