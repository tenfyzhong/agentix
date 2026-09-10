use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use thiserror::Error;
use tokio::time::Instant;
use uuid::Uuid;

mod cold_turns;
mod coordinator;
mod dispatch;
pub use dispatch::{EngineDispatchSnapshot, EngineResource, EngineWork};
mod interaction_flows;
mod presentation;
pub use presentation::command_menu;
mod session_flows;
mod session_service;
mod startup;
mod task_board;
mod turn_flows;
mod workspace_flows;

use presentation::{
    background_completion_body, completed_input_body, decision_label, display_workspace,
    history_views, input_progress_body, input_questions, input_response, is_shell_command,
    live_turn_view, markdown_quote, multiplexer_root_body, multiplexer_session_body,
    multiplexer_session_contains, multiplexer_window_body, multiplexer_window_contains, plural,
    session_display_label, session_status_label, session_title, short_identifier,
    turn_conversation_body, turn_status_label,
};
use session_service::{RestoredBindingStatus, SessionService};

use task_board::{TaskAction, TaskBoardService, TaskBoardUi, TaskBrowse};

use coordinator::{InteractionCoordinator, RmuxController, TurnCoordinator};

use crate::{
    ActionButton, ActionScope, ActionStyle, AgentAdapter, AgentCommand, AgentError, AgentEvent,
    AttachOutcome, ChannelAdapter, ChannelCommand, ChannelError, ChannelKind, ConversationRef,
    DeliveryClass, EventImportance, HistoryPage, InboundEnvelope, InboundPayload,
    InteractionDecision, InteractionKind, InteractionRequest, ItemSummary, MessageRef,
    MultiplexerMutation, MultiplexerSession, MultiplexerSnapshot, MultiplexerTarget,
    MultiplexerWindow, OutboundView, PaneSplitDirection, ParsedInput, SessionCommand,
    SessionCommandChoice, SessionId, SessionStatus, SqliteState, TurnStatus, TurnSummary,
    ViewStatus, parse_input,
};
use agentix_storage::StoredTurnView;

fn notification_now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error(transparent)]
    Channel(#[from] ChannelError),
    #[error("state storage failed: {0}")]
    State(#[from] agentix_storage::StorageError),
    #[error("no channel adapter is configured for {0}")]
    MissingChannel(ChannelKind),
    #[error("no agent session is attached to this conversation")]
    NoCurrentSession,
    #[error("input is invalid: {0}")]
    InvalidInput(String),
    #[error("the action is invalid, expired, or belongs to a different conversation")]
    InvalidAction,
}

#[derive(Debug, Clone, Default)]
struct TurnBuffer {
    user_text: String,
    agent_text: String,
    process_items: Vec<(String, String)>,
    status: TurnStatus,
    started_at: Option<Instant>,
    rendered_elapsed_seconds: Option<u64>,
}

impl TurnBuffer {
    fn ensure_started(&mut self) {
        self.started_at.get_or_insert_with(Instant::now);
    }

    fn elapsed(&self) -> Option<Duration> {
        self.started_at
            .map(|started_at| Instant::now().saturating_duration_since(started_at))
    }

    fn elapsed_seconds(&self) -> Option<u64> {
        self.elapsed().map(|elapsed| elapsed.as_secs())
    }
}

#[derive(Debug, Clone)]
enum UiAction {
    Task(TaskAction),
    TaskBrowse(TaskBrowse),
    Attach(SessionId),
    QueueControl {
        session_id: SessionId,
        action: String,
    },
    Stop {
        session_id: SessionId,
        turn_id: String,
    },
    Resolve {
        interaction: InteractionKey,
        decision: InteractionDecision,
    },
    BeginInput(InteractionKey),
    SelectInput {
        interaction: InteractionKey,
        answer: String,
    },
    BeginCustomInput(InteractionKey),
    SessionCommand {
        session_id: SessionId,
        command: SessionCommand,
    },
    Multiplexer(Option<crate::AgentKind>, MultiplexerUiAction),
    MultiplexerBackend(crate::AgentKind),
}

impl UiAction {
    fn targets_session(&self, session_id: &SessionId) -> bool {
        match self {
            Self::Attach(target)
            | Self::Stop {
                session_id: target, ..
            }
            | Self::QueueControl {
                session_id: target, ..
            }
            | Self::SessionCommand {
                session_id: target, ..
            } => target == session_id,
            Self::Resolve { interaction, .. }
            | Self::BeginInput(interaction)
            | Self::SelectInput { interaction, .. }
            | Self::BeginCustomInput(interaction) => &interaction.session_id == session_id,
            Self::Multiplexer(..) | Self::MultiplexerBackend(_) | Self::TaskBrowse(_) => false,
            Self::Task(action) => &action.session_id == session_id,
        }
    }
}

#[derive(Debug, Clone)]
enum MultiplexerUiAction {
    ShowRoot,
    ShowSession {
        session_id: String,
    },
    ShowWindow {
        session_id: String,
        window_id: String,
    },
    Mutate(MultiplexerMutation),
}

#[derive(Debug, Clone)]
struct PendingInteractionView {
    rpc_id: Value,
    message: MessageRef,
    view: OutboundView,
    action_group: String,
    input: Option<InputProgress>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct InteractionKey {
    session_id: SessionId,
    request_id: String,
}

#[derive(Debug, Clone)]
struct InputProgress {
    questions: Vec<InputQuestion>,
    answers: Vec<Option<String>>,
    current: usize,
}

#[derive(Debug, Clone)]
struct InputQuestion {
    id: String,
    header: String,
    question: String,
    options: Vec<InputOption>,
    secret: bool,
}

#[derive(Debug, Clone)]
struct InputOption {
    label: String,
    description: String,
}

#[derive(Debug, Clone, Copy)]
enum HistoryPresentation {
    Attached,
    History,
}

#[derive(Debug, Clone)]
enum PendingSessionInput {
    Rename(SessionId),
}

/// IM updates prepared during durable state restoration.
/// Deliver these after starting the service to keep IM latency off the startup path.
pub struct RestoredBindings {
    bindings: Vec<RestoredBinding>,
    turns: Vec<RestoredTurn>,
}

/// Owned IM effects prepared after durable shutdown state has been saved.
pub struct ShutdownNotification {
    conversation: ConversationRef,
    turn_views: Vec<(MessageRef, OutboundView)>,
    offline_view: OutboundView,
}

struct RestoredBinding {
    conversation: ConversationRef,
    session: SessionId,
    status: RestoredBindingStatus,
    epoch: u64,
}

struct RestoredTurn {
    conversation: ConversationRef,
    session: SessionId,
    turn: String,
    epoch: u64,
}

impl RestoredBindings {
    #[must_use]
    pub fn restored_count(&self) -> usize {
        self.bindings
            .iter()
            .filter(|binding| binding.status != RestoredBindingStatus::Detached)
            .count()
    }
}

pub struct Engine {
    tasks: TaskBoardService,
    agent: Arc<dyn AgentAdapter>,
    operations: crate::SessionOperations,
    state: SqliteState,
    channels: HashMap<ChannelKind, Arc<dyn ChannelAdapter>>,
    sessions: SessionService,
    turns: TurnCoordinator,
    interactions: InteractionCoordinator,
    rmux: RmuxController,
    background_turn_notifications: bool,
    output: crate::OutputConfig,
}

impl Engine {
    #[must_use]
    pub fn new(
        agent: Arc<dyn AgentAdapter>,
        state: SqliteState,
        channels: Vec<Arc<dyn ChannelAdapter>>,
    ) -> Self {
        let rmux = RmuxController::new(agent.capabilities().workspace_runtime);
        Self {
            tasks: TaskBoardService::new(None, state.clone()),
            operations: crate::SessionOperations::new(agent.clone()),
            agent,
            state: state.clone(),
            channels: channels
                .into_iter()
                .map(|channel| (channel.kind(), channel))
                .collect(),
            sessions: SessionService::new(state),
            turns: TurnCoordinator::default(),
            interactions: InteractionCoordinator::default(),
            rmux,
            background_turn_notifications: true,
            output: crate::OutputConfig::default(),
        }
    }

    #[must_use]
    pub fn with_output(mut self, output: crate::OutputConfig) -> Self {
        self.output = output;
        self.tasks.output = output;
        self
    }

    /// Enable or disable completion notices for sessions without an IM binding.
    #[must_use]
    pub fn with_background_turn_notifications(mut self, enabled: bool) -> Self {
        self.background_turn_notifications = enabled;
        self
    }

    pub async fn handle_inbound(&self, envelope: InboundEnvelope) -> Result<(), EngineError> {
        if !self
            .state
            .claim_event(envelope.conversation.channel, &envelope.event_id)
            .await?
        {
            return Ok(());
        }
        self.interactions
            .owners
            .lock()
            .await
            .insert(envelope.conversation.clone(), envelope.owner_id.clone());
        let result = match envelope.payload {
            InboundPayload::Text(text) => {
                self.handle_text(
                    &envelope.conversation,
                    &envelope.owner_id,
                    &text,
                    &envelope.event_id,
                )
                .await
            }
            InboundPayload::TextEdited {
                original_event_id,
                version,
                text,
            } => {
                self.tasks
                    .view(self)
                    .edit_inbox_message(
                        &envelope.conversation,
                        &envelope.owner_id,
                        &original_event_id,
                        version,
                        &text,
                    )
                    .await
            }
            InboundPayload::Action { token, message } => {
                self.handle_action(
                    &envelope.conversation,
                    &envelope.owner_id,
                    &token,
                    message.as_ref(),
                )
                .await
            }
        };
        match result {
            Ok(()) => {
                self.state
                    .complete_event(envelope.conversation.channel, &envelope.event_id)
                    .await?;
                Ok(())
            }
            Err(error) => {
                if let Err(release_error) = self
                    .state
                    .release_event(envelope.conversation.channel, &envelope.event_id)
                    .await
                {
                    tracing::warn!(
                        %release_error,
                        event_id = %envelope.event_id,
                        "failed to release a retryable inbound event"
                    );
                }
                Err(error)
            }
        }
    }

    async fn handle_text(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        text: &str,
        event_id: &str,
    ) -> Result<(), EngineError> {
        let is_command = text.trim_start().starts_with('/');
        if !is_command && let Some(pending) = self.tasks.take_input(conversation).await {
            return self
                .tasks
                .view(self)
                .finish_task_input(conversation, owner_id, pending, text)
                .await;
        }
        if !is_command && let Some(input) = self.interactions.take_session_input(conversation).await
        {
            let command = match input {
                PendingSessionInput::Rename(session)
                    if self.sessions.current(conversation).await == Some(session.clone()) =>
                {
                    SessionCommand::Rename(Some(text.trim().into()))
                }
                PendingSessionInput::Rename(_) => {
                    return Err(EngineError::InvalidInput(
                        "the attached session changed before it was renamed".into(),
                    ));
                }
            };
            return self
                .run_session_command(conversation, owner_id, command)
                .await;
        }
        if !is_command
            && let Some(interaction) = self.interactions.take_reply_mode(conversation).await
        {
            return self
                .answer_input(conversation, owner_id, &interaction, text)
                .await;
        }
        let input = match parse_input(text) {
            Ok(input) => input,
            Err(error) => {
                return self
                    .show_invalid_command(conversation, &error.to_string())
                    .await;
            }
        };
        match input {
            ParsedInput::Prompt(prompt) => self.send_prompt(conversation, &prompt).await,
            ParsedInput::Command(command) => {
                self.handle_command(conversation, owner_id, command, event_id)
                    .await
            }
        }
    }

    async fn handle_command(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        command: AgentCommand,
        event_id: &str,
    ) -> Result<(), EngineError> {
        let tasks = self.tasks.view(self);
        match command {
            AgentCommand::Inboxes => {
                tasks.show_current_inboxes(conversation, owner_id).await?;
            }
            AgentCommand::Inbox(content) => {
                tasks
                    .submit_inbox(conversation, owner_id, event_id, &content)
                    .await?;
            }
            AgentCommand::Dashboard => {
                tasks.open_dashboard(conversation, owner_id).await?;
            }
            AgentCommand::Board => {
                tasks
                    .browse_tasks(
                        conversation,
                        owner_id,
                        TaskBrowse::Board {
                            project: None,
                            page: 0,
                        },
                    )
                    .await?;
            }
            AgentCommand::Jobs => {
                tasks
                    .browse_tasks(conversation, owner_id, TaskBrowse::Jobs(0))
                    .await?;
            }
            AgentCommand::Tasks(filter) => {
                tasks.show_tasks(conversation, filter.as_deref()).await?;
            }
            AgentCommand::Task(id) => {
                tasks.show_task(conversation, owner_id, &id).await?;
            }
            AgentCommand::Help => self.show_help(conversation).await?,
            AgentCommand::Sessions => self.show_sessions(conversation, owner_id).await?,
            AgentCommand::SessionsBackend(kind) => {
                let kind = parse_backend(&kind)?;
                self.show_sessions_filtered(conversation, owner_id, Some(kind))
                    .await?;
            }
            AgentCommand::MultiplexerBackend(kind) => {
                self.select_multiplexer_backend(conversation, owner_id, &kind)
                    .await?;
            }
            AgentCommand::Multiplexer => {
                self.show_multiplexer_root(conversation, owner_id).await?;
            }
            AgentCommand::Attach(session_id) => {
                self.attach(conversation, owner_id, SessionId::new(session_id))
                    .await?;
            }
            AgentCommand::Current => self.show_current(conversation).await?,
            AgentCommand::Detach => self.detach(conversation).await?,
            AgentCommand::Stop => self.stop_current(conversation).await?,
            AgentCommand::Queue => self.show_queue(conversation).await?,
            AgentCommand::QueueControl(action) => self.control_queue(conversation, &action).await?,
            AgentCommand::Steer(text) => self.steer_current(conversation, &text).await?,
            AgentCommand::Cancel => self.cancel_pending_reply(conversation, owner_id).await?,
            AgentCommand::HistoryRecent => self.show_history(conversation, None).await?,
            AgentCommand::HistoryOlder => {
                let cursor = self
                    .sessions
                    .history_cursors
                    .lock()
                    .await
                    .get(conversation)
                    .and_then(|cursors| cursors.older.clone());
                self.show_history(conversation, cursor).await?;
            }
            AgentCommand::HistoryNewer => {
                let cursor = self
                    .sessions
                    .history_cursors
                    .lock()
                    .await
                    .get(conversation)
                    .and_then(|cursors| cursors.newer.clone());
                self.show_history(conversation, cursor).await?;
            }
            AgentCommand::Session(command) => {
                self.run_session_command(conversation, owner_id, command)
                    .await?;
            }
        }
        Ok(())
    }

    /// Invalidate stale controls and rebuild bound projections after event loss.
    pub async fn recover_event_gap(&self) -> Result<(), EngineError> {
        self.interactions.actions.lock().await.clear();
        self.interactions.pending.lock().await.clear();
        self.interactions.reply_modes.lock().await.clear();
        self.interactions.session_inputs.lock().await.clear();
        for (conversation, session) in self.state.list_bindings().await? {
            if self.sessions.current(&conversation).await.as_ref() != Some(&session) {
                continue;
            }
            if let Err(error) = self.clear_session_stop_actions(&session).await {
                tracing::warn!(%error, %session, "failed to disable stale session controls");
            }
            if let Err(error) = self
                .reconcile_resumed_session(&conversation, &session)
                .await
            {
                tracing::warn!(%error, %session, "failed to recover session after event loss");
            }
        }
        Ok(())
    }

    pub async fn handle_agent_event(&self, event: AgentEvent) -> Result<(), EngineError> {
        let event = self.output.project_event(event);
        let tasks = self.tasks.view(self);
        self.tasks.record_job_message(&event).await;

        match &event {
            AgentEvent::Connected { .. } => return Ok(()),
            AgentEvent::Disconnected { generation, .. } => {
                self.interactions
                    .actions
                    .lock()
                    .await
                    .invalidate_generation(*generation);
                self.interactions.pending.lock().await.clear();
                self.interactions.reply_modes.lock().await.clear();
                self.turns.stop_actions.lock().await.clear();
                return Ok(());
            }
            AgentEvent::SessionStatusChanged {
                session_id,
                status: SessionStatus::Offline,
            } => {
                self.invalidate_session_actions(session_id).await?;
                return Ok(());
            }
            AgentEvent::SessionExited { session_id } => {
                tasks.task_session_event("session.end", session_id).await;
                return self.handle_session_exit(&SessionId::new(session_id)).await;
            }
            AgentEvent::SessionResumed { session_id } => {
                tasks.task_session_event("session.start", session_id).await;
                return self
                    .handle_session_resume(&SessionId::new(session_id))
                    .await;
            }
            AgentEvent::TurnStarted {
                session_id,
                turn_id,
            } => {
                self.record_turn_started(SessionId::new(session_id), turn_id.clone())
                    .await?;
                return Ok(());
            }
            _ => {}
        }

        let Some(raw_session_id) = event.session_id() else {
            return Ok(());
        };
        let session_id = SessionId::new(raw_session_id);
        let importance = event_importance(&event);
        let Some((conversation, delivery)) = self
            .route_agent_event(&session_id, importance, &event)
            .await?
        else {
            return Ok(());
        };

        match event {
            AgentEvent::AgentMessageDelta { turn_id, delta, .. } => {
                self.handle_message_delta(&conversation, &session_id, &turn_id, &delta, delivery)
                    .await?;
            }
            AgentEvent::ItemCompleted { turn_id, item, .. } => {
                self.handle_completed_item(&conversation, &session_id, &turn_id, &item, delivery)
                    .await?;
            }
            AgentEvent::TurnCompleted {
                turn_id,
                status,
                error,
                ..
            } => {
                self.handle_turn_completed(
                    &conversation,
                    &session_id,
                    turn_id,
                    status,
                    error,
                    delivery,
                )
                .await?;
            }
            AgentEvent::InteractionRequested(request) => {
                self.render_interaction(&conversation, &request, delivery)
                    .await?;
            }
            AgentEvent::InteractionResolved { request_id, .. } => {
                self.resolve_external_request(&conversation, session_id, request_id)
                    .await?;
            }
            AgentEvent::SessionStatusChanged { .. }
            | AgentEvent::SessionExited { .. }
            | AgentEvent::SessionResumed { .. }
            | AgentEvent::QueueChanged { .. }
            | AgentEvent::UserMessage { .. }
            | AgentEvent::ItemStarted { .. }
            | AgentEvent::Connected { .. }
            | AgentEvent::Disconnected { .. }
            | AgentEvent::TurnStarted { .. } => {}
        }
        Ok(())
    }

    async fn send_view(
        &self,
        conversation: &ConversationRef,
        view: &OutboundView,
    ) -> Result<MessageRef, EngineError> {
        self.channel(conversation.channel)?
            .send(conversation, view)
            .await
            .map_err(EngineError::from)
    }

    fn channel(&self, kind: ChannelKind) -> Result<&Arc<dyn ChannelAdapter>, EngineError> {
        self.channels
            .get(&kind)
            .ok_or(EngineError::MissingChannel(kind))
    }
}

fn interaction_key(request: &InteractionRequest) -> InteractionKey {
    InteractionKey {
        session_id: SessionId::new(&request.session_id),
        request_id: match &request.rpc_id {
            Value::String(id) => id.clone(),
            id => id.to_string(),
        },
    }
}

fn event_importance(event: &AgentEvent) -> EventImportance {
    match event {
        AgentEvent::InteractionRequested(_)
        | AgentEvent::InteractionResolved { .. }
        | AgentEvent::TurnCompleted { .. } => EventImportance::Critical,
        _ => EventImportance::Stream,
    }
}

fn parse_backend(kind: &str) -> Result<crate::AgentKind, EngineError> {
    match kind {
        "claude" => Ok(crate::AgentKind::Claude),
        "codex" => Ok(crate::AgentKind::Codex),
        "pi" => Ok(crate::AgentKind::Pi),
        "omp" | "oh-my-pi" => Ok(crate::AgentKind::Omp),
        _ => Err(EngineError::InvalidInput(
            "Choose codex, pi, omp, or claude".into(),
        )),
    }
}

#[async_trait::async_trait]
impl TaskBoardUi for Engine {
    fn agent(&self) -> &dyn AgentAdapter {
        self.agent.as_ref()
    }
    fn sessions(&self) -> &SessionService {
        &self.sessions
    }
    fn channels(&self) -> &HashMap<ChannelKind, Arc<dyn ChannelAdapter>> {
        &self.channels
    }
    async fn send_view(
        &self,
        conversation: &ConversationRef,
        view: &OutboundView,
    ) -> Result<MessageRef, EngineError> {
        Engine::send_view(self, conversation, view).await
    }
    async fn issue_action(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        group: &str,
        action: UiAction,
    ) -> String {
        Engine::issue_action(self, conversation, owner, group, action).await
    }
    async fn update_command_menu_best_effort(
        &self,
        conversation: &ConversationRef,
        attached: bool,
    ) {
        Engine::update_command_menu_best_effort(self, conversation, attached).await;
    }
}

impl Engine {
    #[must_use]
    pub fn with_task_board(mut self, service: Arc<agentix_task::Service>) -> Self {
        self.tasks.backend = Some(service);
        self
    }

    #[must_use]
    pub fn with_task_consumer(mut self, consumer: String) -> Self {
        self.tasks.consumer = consumer;
        self
    }

    pub async fn refresh_task_board(&self) -> Result<(), EngineError> {
        self.tasks.view(self).refresh_task_board().await
    }

    pub async fn claim_task_notifications(
        &self,
        limit: u32,
    ) -> Result<Vec<crate::TaskNotification>, EngineError> {
        if self.tasks.backend.is_none() {
            return Ok(Vec::new());
        }
        Ok(self
            .state
            .claim_notifications(&self.tasks.consumer, notification_now(), 60, limit)
            .await?)
    }

    pub async fn claim_admission_notifications(
        &self,
        limit: u32,
    ) -> Result<Vec<crate::TaskNotification>, EngineError> {
        Ok(self
            .state
            .claim_notifications(
                &format!("admission:{}", self.tasks.consumer),
                notification_now(),
                60,
                limit,
            )
            .await?)
    }

    pub async fn reject_overloaded(
        &self,
        envelope: &InboundEnvelope,
        limit: usize,
    ) -> Result<bool, EngineError> {
        Ok(self
            .state
            .reject_overloaded(
                &format!("admission:{}", self.tasks.consumer),
                &envelope.conversation,
                &envelope.event_id,
                limit,
            )
            .await?)
    }

    pub async fn deliver_task_notification(
        &self,
        notification: crate::TaskNotification,
    ) -> Result<(), EngineError> {
        // The deadline is shorter than the lease, so live workers cannot overlap
        // after lease expiry. Sending and acknowledging cannot be atomic with IM.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            self.send_view(&notification.conversation, &notification.view),
        )
        .await
        .map_err(|_| {
            EngineError::Channel(crate::ChannelError::Transport(
                "task notification timed out".into(),
            ))
        })
        .and_then(|result| result);
        let failure = result.as_ref().err().map(ToString::to_string);
        let acknowledged = self
            .state
            .finish_notification(&notification, notification_now(), failure.as_deref())
            .await?;
        tracing::debug!(target: "agentix::telemetry", sequence = notification.sequence,
            attempt = notification.attempts, acknowledged, success = result.is_ok(), "task notification delivery");
        result.map(|_| ())
    }

    pub async fn refresh_inbox_sources(&self) -> Result<(), EngineError> {
        for envelope in self.poll_inbox_sources().await? {
            if let Err(error) = self.handle_inbound(envelope).await {
                tracing::warn!(%error, "Inbox source synchronization failed");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "engine/render_tests.rs"]
mod render_tests;

#[cfg(test)]
mod architecture_tests {
    use super::*;
    #[tokio::test]
    async fn session_service_commits_exclusive_bindings_without_engine_or_channels() {
        let state = SqliteState::in_memory().await.unwrap();
        let sessions = session_service::SessionService::new(state.clone());
        let first = ConversationRef::new(ChannelKind::Telegram, "one");
        let second = ConversationRef::new(ChannelKind::Feishu, "two");
        let pi = SessionId::new("pi:same");
        let omp = SessionId::new("omp:same");
        sessions.commit_binding(&first, &pi, false).await.unwrap();
        sessions.commit_binding(&second, &omp, false).await.unwrap();
        let moved = sessions.commit_binding(&second, &pi, true).await.unwrap();
        assert_eq!(moved.outcome.displaced_conversation, Some(first.clone()));
        assert_eq!(moved.outcome.previous_session, Some(omp.clone()));
        assert_eq!(sessions.current(&first).await, None);
        assert_eq!(sessions.current(&second).await, Some(pi.clone()));
        assert_eq!(
            state.list_bindings().await.unwrap(),
            vec![(second.clone(), pi)]
        );
        assert!(
            sessions
                .route(&omp, EventImportance::Critical)
                .await
                .is_some()
        );
        sessions.commit_detach(&second, false).await.unwrap();
        assert!(state.list_bindings().await.unwrap().is_empty());
        assert_eq!(sessions.current(&second).await, None);
    }

    #[test]
    fn history_presentation_has_no_runtime_dependency() {
        let views = presentation::history_views(
            "Pi",
            "session",
            &HistoryPage {
                turns: vec![],
                older_cursor: Some("older".into()),
                newer_cursor: None,
            },
            HistoryPresentation::History,
        );
        assert_eq!(views.len(), 1);
        assert!(views[0].body.contains("No conversation history yet."));
        assert!(views[0].body.contains("/history older"));
        assert!(!views[0].body.contains("/history newer"));
    }

    #[test]
    fn claude_backend_can_be_selected() {
        assert_eq!(parse_backend("claude").unwrap(), crate::AgentKind::Claude);
    }
    #[test]
    fn multiplexer_panes_display_their_actual_backend() {
        let pane = crate::MultiplexerPane {
            id: "%1".into(),
            index: "0".into(),
            active: true,
            current_command: "pi".into(),
            cwd: "/tmp".into(),
            agent_session: Some(SessionId::new("pi:native")),
        };
        let body = presentation::multiplexer_pane_body(&pane, None, "Agent");
        assert!(body.contains("Pi"));
        assert!(!body.contains("Codex"));
    }
}
