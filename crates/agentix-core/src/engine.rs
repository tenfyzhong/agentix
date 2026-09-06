use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use thiserror::Error;
use tokio::time::Instant;
use uuid::Uuid;

mod coordinator;
mod task_board;

use task_board::{PendingTaskInput, TaskAction, TaskBrowse};

use coordinator::{InteractionCoordinator, RmuxController, SessionCoordinator, TurnCoordinator};

use crate::state::StoredTurnView;
use crate::{
    ActionButton, ActionScope, ActionStyle, AgentAdapter, AgentCommand, AgentError, AgentEvent,
    AttachOutcome, ChannelAdapter, ChannelCommand, ChannelError, ChannelKind, CommandMenu,
    ConversationRef, DeliveryClass, EventImportance, HistoryPage, InboundEnvelope, InboundPayload,
    InteractionDecision, InteractionKind, InteractionRequest, ItemSummary, MessageRef,
    MultiplexerMutation, MultiplexerPane, MultiplexerSession, MultiplexerSnapshot,
    MultiplexerTarget, MultiplexerWindow, OutboundView, PaneSplitDirection, ParsedInput,
    SessionCommand, SessionCommandChoice, SessionId, SessionStatus, SessionSummary, SqliteState,
    TurnStatus, TurnSummary, ViewStatus, parse_input,
};

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error(transparent)]
    Channel(#[from] ChannelError),
    #[error("state storage failed: {0}")]
    State(#[from] sqlx::Error),
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
    Multiplexer(MultiplexerUiAction),
}

impl UiAction {
    fn targets_session(&self, session_id: &SessionId) -> bool {
        match self {
            Self::Attach(target)
            | Self::Stop {
                session_id: target, ..
            }
            | Self::SessionCommand {
                session_id: target, ..
            } => target == session_id,
            Self::Resolve { interaction, .. }
            | Self::BeginInput(interaction)
            | Self::SelectInput { interaction, .. }
            | Self::BeginCustomInput(interaction) => &interaction.session_id == session_id,
            Self::Multiplexer(_) | Self::TaskBrowse(_) => false,
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

#[derive(Debug, Clone, Default)]
struct HistoryCursors {
    older: Option<String>,
    newer: Option<String>,
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

struct RestoredBinding {
    conversation: ConversationRef,
    session: SessionId,
    attached: bool,
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
            .filter(|binding| binding.attached)
            .count()
    }
}

pub struct Engine {
    task_board: Option<Arc<agentix_task::Service>>,
    task_inputs: tokio::sync::Mutex<HashMap<ConversationRef, PendingTaskInput>>,
    task_refresh: tokio::sync::Mutex<()>,
    task_consumer: String,
    agent: Arc<dyn AgentAdapter>,
    state: SqliteState,
    channels: HashMap<ChannelKind, Arc<dyn ChannelAdapter>>,
    sessions: SessionCoordinator,
    turns: TurnCoordinator,
    interactions: InteractionCoordinator,
    rmux: RmuxController,
    background_turn_notifications: bool,
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
            task_board: None,
            task_inputs: tokio::sync::Mutex::new(HashMap::new()),
            task_refresh: tokio::sync::Mutex::new(()),
            task_consumer: "default".into(),
            agent,
            state,
            channels: channels
                .into_iter()
                .map(|channel| (channel.kind(), channel))
                .collect(),
            sessions: SessionCoordinator::default(),
            turns: TurnCoordinator::default(),
            interactions: InteractionCoordinator::default(),
            rmux,
            background_turn_notifications: true,
        }
    }

    /// Enable or disable completion notices for sessions without an IM binding.
    #[must_use]
    pub fn with_background_turn_notifications(mut self, enabled: bool) -> Self {
        self.background_turn_notifications = enabled;
        self
    }

    /// Restores durable conversation bindings and their upstream subscriptions.
    pub async fn restore_bindings(&self) -> Result<usize, EngineError> {
        let updates = self.restore_bindings_deferred().await?;
        let restored = updates.restored_count();
        self.notify_restored_bindings(updates).await?;
        Ok(restored)
    }

    /// Restore bindings and turn state without making any IM requests.
    pub async fn restore_bindings_deferred(&self) -> Result<RestoredBindings, EngineError> {
        let persisted = self.state.list_bindings().await?;
        let mut updates = RestoredBindings {
            bindings: Vec::new(),
            turns: Vec::new(),
        };
        for (conversation, session) in &persisted {
            if !self.channels.contains_key(&conversation.channel) {
                continue;
            }
            let attached = self.restore_binding(conversation, session).await?;
            updates.bindings.push(RestoredBinding {
                conversation: conversation.clone(),
                session: session.clone(),
                attached,
                epoch: self.sessions.epoch(conversation).await,
            });
        }
        for stored in self.state.list_turn_views().await? {
            if !self
                .channels
                .contains_key(&stored.message.conversation.channel)
            {
                continue;
            }
            let is_current = self
                .sessions
                .bindings
                .lock()
                .await
                .current_session(&stored.message.conversation)
                == Some(&stored.session_id);
            if !is_current {
                self.state
                    .delete_turn_view(&stored.session_id, &stored.turn_id)
                    .await?;
                continue;
            }
            let key = (stored.session_id.clone(), stored.turn_id.clone());
            if let Some(owner_id) = stored.owner_id {
                self.interactions
                    .owners
                    .lock()
                    .await
                    .insert(stored.message.conversation.clone(), owner_id);
            }
            if matches!(stored.status, TurnStatus::InProgress | TurnStatus::Unknown) {
                self.turns
                    .active
                    .lock()
                    .await
                    .insert(stored.session_id.clone(), stored.turn_id.clone());
            }
            self.turns.buffers.lock().await.insert(
                key.clone(),
                TurnBuffer {
                    user_text: stored.user_text,
                    agent_text: stored.agent_text,
                    started_at: matches!(
                        &stored.status,
                        TurnStatus::InProgress | TurnStatus::Unknown
                    )
                    .then(Instant::now),
                    rendered_elapsed_seconds: None,
                    status: stored.status,
                },
            );
            self.turns
                .views
                .lock()
                .await
                .insert(key, stored.message.clone());
            updates.turns.push(RestoredTurn {
                epoch: self.sessions.epoch(&stored.message.conversation).await,
                conversation: stored.message.conversation,
                session: stored.session_id,
                turn: stored.turn_id,
            });
        }
        Ok(updates)
    }

    async fn restore_binding(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
    ) -> Result<bool, EngineError> {
        match self.agent.attach(session).await {
            Ok(()) => {
                let epoch = self.state.binding_epoch(conversation).await?;
                self.sessions
                    .attach_at_epoch(conversation.clone(), session.clone(), false, epoch)
                    .await;
                Ok(true)
            }
            Err(AgentError::Rejected(reason)) => {
                tracing::warn!(%reason, ?conversation, %session, "saved agent session is no longer attachable");
                self.state.detach(conversation).await?;
                Ok(false)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Present restored bindings and turns. The caller owns cancellation and shutdown.
    pub async fn notify_restored_bindings(
        &self,
        updates: RestoredBindings,
    ) -> Result<(), EngineError> {
        for binding in updates.bindings {
            let RestoredBinding {
                conversation,
                session,
                attached,
                epoch,
            } = binding;
            if self.sessions.epoch(&conversation).await != epoch {
                continue;
            }
            if attached {
                self.cache_session_summary(&session).await;
            }
            let session_label = self.session_label(&session).await;
            if self.sessions.epoch(&conversation).await != epoch {
                continue;
            }
            self.update_command_menu_best_effort(&conversation, attached)
                .await;
            if self.sessions.epoch(&conversation).await != epoch {
                // A slow old request may have overwritten the newer binding's menu.
                let attached_now = self.sessions.current(&conversation).await.is_some();
                self.update_command_menu_best_effort(&conversation, attached_now)
                    .await;
                continue;
            }
            let view = if attached {
                OutboundView {
                    title: "Agentix serve".into(),
                    subtitle: Some("Online · Reattached".into()),
                    body: format!(
                        "Agentix serve is online. Reattached to {} session {}.",
                        self.agent.display_name(),
                        session_label
                    ),
                    status: ViewStatus::Success,
                    actions: Vec::new(),
                }
            } else {
                OutboundView {
                    title: "Agentix serve".into(),
                    subtitle: Some("Online · Detached".into()),
                    body: format!(
                        "Agentix serve is online. Saved {} session {} is no longer running, so this IM conversation remains detached.",
                        self.agent.display_name(),
                        session_label
                    ),
                    status: ViewStatus::Warning,
                    actions: Vec::new(),
                }
            };
            if let Err(error) = self.send_view(&conversation, &view).await {
                tracing::warn!(%error, ?conversation, "failed to notify a restored conversation");
            }
        }
        for RestoredTurn {
            conversation,
            session,
            turn,
            epoch,
        } in updates.turns
        {
            if self.sessions.epoch(&conversation).await != epoch
                || self.sessions.current(&conversation).await.as_ref() != Some(&session)
                || !self
                    .turns
                    .buffers
                    .lock()
                    .await
                    .contains_key(&(session.clone(), turn.clone()))
            {
                continue;
            }
            self.render_turn(&conversation, &session, &turn, DeliveryClass::Live, true)
                .await?;
        }
        Ok(())
    }

    /// Checkpoints durable bindings and presents every bound IM conversation as detached.
    pub async fn prepare_shutdown(&self) -> Result<usize, EngineError> {
        self.state.checkpoint().await?;
        let persisted = self.state.list_bindings().await?;
        let stored_turns = self.state.list_turn_views().await?;
        self.interactions.actions.lock().await.clear();
        self.interactions.pending.lock().await.clear();
        self.interactions.turn_action_groups.lock().await.clear();
        self.interactions.reply_modes.lock().await.clear();
        self.interactions.session_inputs.lock().await.clear();
        self.turns.stop_actions.lock().await.clear();

        let mut notified = 0;
        for (conversation, session) in &persisted {
            if !self.channels.contains_key(&conversation.channel) {
                continue;
            }
            let session_label = self.session_label(session).await;
            self.sessions
                .bindings
                .lock()
                .await
                .detach(conversation, false);
            if let Err(error) = self.update_command_menu(conversation, false).await {
                tracing::warn!(
                    %error,
                    ?conversation,
                    "failed to detach the IM command menu during shutdown"
                );
            }
            for stored in stored_turns.iter().filter(|stored| {
                &stored.message.conversation == conversation && &stored.session_id == session
            }) {
                let buffer = TurnBuffer {
                    user_text: stored.user_text.clone(),
                    agent_text: stored.agent_text.clone(),
                    status: stored.status.clone(),
                    started_at: None,
                    rendered_elapsed_seconds: None,
                };
                let view = live_turn_view(
                    self.agent.display_name(),
                    &session_label,
                    &stored.turn_id,
                    &buffer,
                    DeliveryClass::Live,
                );
                if let Err(error) = self
                    .channel(conversation.channel)?
                    .update(conversation, &stored.message, &view)
                    .await
                {
                    tracing::warn!(
                        %error,
                        ?conversation,
                        turn = %stored.turn_id,
                        "failed to remove live turn controls during shutdown"
                    );
                }
            }
            let view = OutboundView {
                title: "Agentix serve".into(),
                subtitle: Some("Offline · Detached".into()),
                body: format!(
                    "Saved {} session {} for automatic reattachment. This IM conversation is detached while Agentix serve is offline.",
                    self.agent.display_name(),
                    session_label
                ),
                status: ViewStatus::Warning,
                actions: Vec::new(),
            };
            match self.send_view(conversation, &view).await {
                Ok(_) => notified += 1,
                Err(error) => tracing::warn!(
                    %error,
                    ?conversation,
                    "failed to notify IM conversation during shutdown"
                ),
            }
        }
        Ok(notified)
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
        if !is_command && let Some(pending) = self.task_inputs.lock().await.remove(conversation) {
            return self
                .finish_task_input(conversation, owner_id, pending, text)
                .await;
        }
        if !is_command
            && let Some(input) = self
                .interactions
                .session_inputs
                .lock()
                .await
                .remove(conversation)
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
            && let Some(interaction) = self
                .interactions
                .reply_modes
                .lock()
                .await
                .remove(conversation)
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
        match command {
            AgentCommand::Inboxes => {
                self.show_current_inboxes(conversation, owner_id).await?;
            }
            AgentCommand::Inbox(content) => {
                self.submit_inbox(conversation, owner_id, event_id, &content)
                    .await?;
            }
            AgentCommand::Dashboard => {
                self.open_dashboard(conversation, owner_id).await?;
            }
            AgentCommand::Board => {
                self.browse_tasks(
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
                self.browse_tasks(conversation, owner_id, TaskBrowse::Jobs(0))
                    .await?;
            }
            AgentCommand::Tasks(filter) => self.show_tasks(conversation, filter.as_deref()).await?,
            AgentCommand::Task(id) => self.show_task(conversation, owner_id, &id).await?,
            AgentCommand::Help => self.show_help(conversation).await?,
            AgentCommand::Sessions => self.show_sessions(conversation, owner_id).await?,
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
            AgentCommand::Cancel => {
                self.task_inputs.lock().await.remove(conversation);
                self.interactions
                    .session_inputs
                    .lock()
                    .await
                    .remove(conversation);
                if let Some(interaction) = self
                    .interactions
                    .reply_modes
                    .lock()
                    .await
                    .remove(conversation)
                {
                    self.show_input_question(conversation, owner_id, &interaction)
                        .await?;
                }
                self.send_view(
                    conversation,
                    &OutboundView::text("Agentix", "Pending reply cancelled."),
                )
                .await?;
            }
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

    async fn show_help(&self, conversation: &ConversationRef) -> Result<(), EngineError> {
        let body = self.available_commands(conversation).await;
        let body = if self.task_board.is_some() {
            let mut body = format!(
                "{body}\n\n/dashboard — Browse projects; click a project to open its board."
            );
            if self.sessions.current(conversation).await.is_some() {
                body.push_str("\n/board — Current session's task board\n/jobs — Current session's jobs\n/inboxes — Current project's human queue\n/inbox <content> — Append a human requirement\nClick tasks and jobs to read their Markdown details.");
            }
            body
        } else {
            body.to_owned()
        };
        self.send_view(conversation, &OutboundView::text("Agentix commands", body))
            .await?;
        Ok(())
    }

    async fn show_invalid_command(
        &self,
        conversation: &ConversationRef,
        error: &str,
    ) -> Result<(), EngineError> {
        let commands = self.available_commands(conversation).await;
        self.send_view(
            conversation,
            &OutboundView {
                title: "Invalid command".into(),
                subtitle: None,
                body: format!("**Error:** {error}\n\n**Available commands**\n\n{commands}"),
                status: ViewStatus::Warning,
                actions: Vec::new(),
            },
        )
        .await?;
        Ok(())
    }

    async fn available_commands(&self, conversation: &ConversationRef) -> &'static str {
        if let Some(session) = self.sessions.current(conversation).await
            && self.agent.is_read_only(&session).await
        {
            return "/sessions · /rmux · /current · /history · /detach · /help · /cancel\n\nThis session is connected read-only.";
        }
        let attached = self
            .sessions
            .bindings
            .lock()
            .await
            .current_session(conversation)
            .is_some();
        if attached && self.agent.capabilities().session_control {
            "/sessions · /rmux · /current · /history · /queue · /stop · /detach\n\n/fast [on|off] · /clear [name] · /exit · /diff · /rename <name> · /compact · /fork · /model [id] · /reasoning [effort] · /skills · /plan [prompt|off] · /goal [objective|pause|resume|clear] · /review · /status · /mcp"
        } else {
            "/sessions · /rmux · /attach <thread-id>"
        }
    }

    async fn show_sessions(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
    ) -> Result<(), EngineError> {
        let page = self.agent.list_sessions(None, 25).await?;
        let current_session = self
            .sessions
            .bindings
            .lock()
            .await
            .current_session(conversation)
            .cloned();
        let mut body = String::new();
        let mut actions = Vec::new();
        let action_group = Uuid::new_v4().simple().to_string();
        let mut sessions = self.sessions.cache.lock().await;
        for (index, session) in page.sessions.into_iter().enumerate() {
            let title = session_title(&session);
            let is_current = current_session.as_ref() == Some(&session.id);
            let attached_marker = if is_current {
                " · 📎 **Attached**"
            } else {
                ""
            };
            let (status_icon, status_label) = session_status_label(&session.status);
            if !body.is_empty() {
                body.push_str("\n\n");
            }
            let mut item = format!(
                "**{} · {title}**{attached_marker}\n{status_icon} **Status:** {status_label}\n📁 **Workspace:** `{}`",
                index + 1,
                display_workspace(session.cwd.as_deref())
            );
            if let Some(terminal) = &session.terminal {
                item.push_str(&format!(
                    "\n🖥️ **rmux** · `{}` · `{}` (`{}`) · `{}`",
                    terminal.session,
                    terminal.window_index,
                    terminal.window_name,
                    terminal.pane_index
                ));
            }
            body.push_str(&markdown_quote(&item));
            if !is_current {
                let token = self
                    .issue_action(
                        conversation,
                        owner_id,
                        &action_group,
                        UiAction::Attach(session.id.clone()),
                    )
                    .await;
                actions.push(ActionButton {
                    label: format!("{} · {title}", index + 1),
                    token,
                    style: ActionStyle::Default,
                });
            }
            sessions.insert(session.id.clone(), session);
        }
        drop(sessions);
        self.send_view(
            conversation,
            &OutboundView {
                title: format!("Existing {} sessions", self.agent.display_name()),
                subtitle: None,
                body,
                status: ViewStatus::Info,
                actions,
            },
        )
        .await?;
        Ok(())
    }

    async fn show_multiplexer_root(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
    ) -> Result<(), EngineError> {
        let Some(workspace) = self.rmux.runtime(self.agent.as_ref()) else {
            self.send_view(
                conversation,
                &OutboundView {
                    title: "Terminal multiplexer".into(),
                    subtitle: Some("Unsupported".into()),
                    body: format!(
                        "{} does not support terminal multiplexer management.",
                        self.agent.display_name()
                    ),
                    status: ViewStatus::Warning,
                    actions: Vec::new(),
                },
            )
            .await?;
            return Ok(());
        };
        let snapshot = workspace.snapshot().await?;
        let Some(snapshot) = snapshot else {
            self.send_view(
                conversation,
                &OutboundView {
                    title: "Terminal multiplexer".into(),
                    subtitle: Some("Not running".into()),
                    body: "The rmux server is unavailable.".into(),
                    status: ViewStatus::Muted,
                    actions: Vec::new(),
                },
            )
            .await?;
            return Ok(());
        };
        let current = self
            .sessions
            .bindings
            .lock()
            .await
            .current_session(conversation)
            .cloned();
        let (body, window_count, pane_count) = multiplexer_root_body(&snapshot, current.as_ref());
        let actions = self
            .multiplexer_root_actions(conversation, owner_id, &snapshot, current.as_ref())
            .await;
        self.send_view(
            conversation,
            &OutboundView {
                title: "Terminal · rmux".into(),
                subtitle: Some(format!(
                    "{} {} · {window_count} {} · {pane_count} {}",
                    snapshot.sessions.len(),
                    plural(snapshot.sessions.len(), "session", "sessions"),
                    plural(window_count, "window", "windows"),
                    plural(pane_count, "pane", "panes")
                )),
                body,
                status: ViewStatus::Info,
                actions,
            },
        )
        .await?;
        Ok(())
    }

    async fn multiplexer_root_actions(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        snapshot: &MultiplexerSnapshot,
        current: Option<&SessionId>,
    ) -> Vec<ActionButton> {
        let mut actions = Vec::new();
        let action_group = Uuid::new_v4().simple().to_string();
        let default_directory = self.rmux.default_directory(self.agent.as_ref());
        for session in &snapshot.sessions {
            let attached = multiplexer_session_contains(session, current);
            let token = self
                .issue_action(
                    conversation,
                    owner_id,
                    &action_group,
                    UiAction::Multiplexer(MultiplexerUiAction::ShowSession {
                        session_id: session.id.clone(),
                    }),
                )
                .await;
            actions.push(ActionButton {
                label: session.name.clone(),
                token,
                style: if attached {
                    ActionStyle::Primary
                } else {
                    ActionStyle::Default
                },
            });
        }
        for (label, action) in [
            (
                "+ Session",
                MultiplexerUiAction::Mutate(MultiplexerMutation {
                    target: MultiplexerTarget::NewSession {
                        name: "codex".into(),
                        cwd: default_directory,
                    },
                    launch_codex: true,
                }),
            ),
            ("Refresh", MultiplexerUiAction::ShowRoot),
        ] {
            let token = self
                .issue_action(
                    conversation,
                    owner_id,
                    &action_group,
                    UiAction::Multiplexer(action),
                )
                .await;
            actions.push(ActionButton {
                label: label.into(),
                token,
                style: ActionStyle::Default,
            });
        }
        actions
    }

    async fn show_multiplexer_session(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        session_id: &str,
    ) -> Result<(), EngineError> {
        let snapshot = self.required_multiplexer_snapshot().await?;
        let session = snapshot
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .cloned()
            .ok_or_else(|| {
                EngineError::InvalidInput("multiplexer session no longer exists".into())
            })?;
        let current = self
            .sessions
            .bindings
            .lock()
            .await
            .current_session(conversation)
            .cloned();
        let body = multiplexer_session_body(&session, current.as_ref());
        let actions = self
            .multiplexer_session_actions(conversation, owner_id, &session, current.as_ref())
            .await;
        self.send_view(
            conversation,
            &OutboundView {
                title: format!("rmux · {}", session.name),
                subtitle: Some(format!(
                    "{} {}",
                    session.windows.len(),
                    plural(session.windows.len(), "window", "windows")
                )),
                body,
                status: ViewStatus::Info,
                actions,
            },
        )
        .await?;
        Ok(())
    }

    async fn multiplexer_session_actions(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        session: &MultiplexerSession,
        current: Option<&SessionId>,
    ) -> Vec<ActionButton> {
        let mut actions = Vec::new();
        let action_group = Uuid::new_v4().simple().to_string();
        let default_directory = self.rmux.default_directory(self.agent.as_ref());
        for window in &session.windows {
            let attached = multiplexer_window_contains(window, current);
            let token = self
                .issue_action(
                    conversation,
                    owner_id,
                    &action_group,
                    UiAction::Multiplexer(MultiplexerUiAction::ShowWindow {
                        session_id: session.id.clone(),
                        window_id: window.id.clone(),
                    }),
                )
                .await;
            actions.push(ActionButton {
                label: format!("{} · {}", window.index, window.name),
                token,
                style: if attached {
                    ActionStyle::Primary
                } else {
                    ActionStyle::Default
                },
            });
        }
        for (label, action) in [
            (
                "+ Window",
                MultiplexerUiAction::Mutate(MultiplexerMutation {
                    target: MultiplexerTarget::NewWindow {
                        session_id: session.id.clone(),
                        name: "codex".into(),
                        cwd: default_directory,
                    },
                    launch_codex: true,
                }),
            ),
            ("← Back", MultiplexerUiAction::ShowRoot),
        ] {
            let token = self
                .issue_action(
                    conversation,
                    owner_id,
                    &action_group,
                    UiAction::Multiplexer(action),
                )
                .await;
            actions.push(ActionButton {
                label: label.into(),
                token,
                style: ActionStyle::Default,
            });
        }
        actions
    }

    async fn show_multiplexer_window(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        session_id: &str,
        window_id: &str,
    ) -> Result<(), EngineError> {
        let snapshot = self.required_multiplexer_snapshot().await?;
        let session = snapshot
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .cloned()
            .ok_or_else(|| {
                EngineError::InvalidInput("multiplexer session no longer exists".into())
            })?;
        let window = session
            .windows
            .iter()
            .find(|window| window.id == window_id)
            .cloned()
            .ok_or_else(|| {
                EngineError::InvalidInput("multiplexer window no longer exists".into())
            })?;
        let current = self
            .sessions
            .bindings
            .lock()
            .await
            .current_session(conversation)
            .cloned();
        let body = multiplexer_window_body(&window, current.as_ref());
        let action_group = Uuid::new_v4().simple().to_string();
        let mut actions = self
            .multiplexer_pane_actions(
                conversation,
                owner_id,
                &window,
                current.as_ref(),
                &action_group,
            )
            .await;
        actions.extend(
            self.multiplexer_split_actions(conversation, owner_id, &window, &action_group)
                .await?,
        );
        let back_token = self
            .issue_action(
                conversation,
                owner_id,
                &action_group,
                UiAction::Multiplexer(MultiplexerUiAction::ShowSession {
                    session_id: session.id.clone(),
                }),
            )
            .await;
        actions.push(ActionButton {
            label: "← Back".into(),
            token: back_token,
            style: ActionStyle::Default,
        });
        self.send_view(
            conversation,
            &OutboundView {
                title: format!(
                    "rmux · {} · {} ({})",
                    session.name, window.index, window.name
                ),
                subtitle: Some(format!(
                    "{} {}",
                    window.panes.len(),
                    plural(window.panes.len(), "pane", "panes")
                )),
                body,
                status: ViewStatus::Info,
                actions,
            },
        )
        .await?;
        Ok(())
    }

    async fn multiplexer_pane_actions(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        window: &MultiplexerWindow,
        current: Option<&SessionId>,
        action_group: &str,
    ) -> Vec<ActionButton> {
        let mut actions = Vec::new();
        for pane in &window.panes {
            let attached =
                current.is_some_and(|current| pane.codex_session.as_ref() == Some(current));
            let (label, action) = if let Some(codex_session) = &pane.codex_session {
                if attached {
                    continue;
                }
                (
                    format!("{} · Attach", pane.index),
                    UiAction::Attach(codex_session.clone()),
                )
            } else if is_shell_command(&pane.current_command) {
                (
                    format!("{} · Run Codex", pane.index),
                    UiAction::Multiplexer(MultiplexerUiAction::Mutate(MultiplexerMutation {
                        target: MultiplexerTarget::ExistingPane {
                            pane_id: pane.id.clone(),
                        },
                        launch_codex: true,
                    })),
                )
            } else {
                continue;
            };
            let token = self
                .issue_action(conversation, owner_id, action_group, action)
                .await;
            actions.push(ActionButton {
                label,
                token,
                style: ActionStyle::Primary,
            });
        }
        actions
    }

    async fn multiplexer_split_actions(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        window: &MultiplexerWindow,
        action_group: &str,
    ) -> Result<Vec<ActionButton>, EngineError> {
        let pane_id = window
            .panes
            .iter()
            .find(|pane| pane.active)
            .or_else(|| window.panes.first())
            .map(|pane| pane.id.clone())
            .ok_or_else(|| EngineError::InvalidInput("multiplexer window has no panes".into()))?;
        let mut actions = Vec::new();
        let default_directory = self.rmux.default_directory(self.agent.as_ref());
        for (label, direction) in [
            ("Split ↔ + Codex", PaneSplitDirection::Horizontal),
            ("Split ↕ + Codex", PaneSplitDirection::Vertical),
        ] {
            let action = UiAction::Multiplexer(MultiplexerUiAction::Mutate(MultiplexerMutation {
                target: MultiplexerTarget::SplitPane {
                    pane_id: pane_id.clone(),
                    direction,
                    cwd: default_directory.clone(),
                },
                launch_codex: true,
            }));
            let token = self
                .issue_action(conversation, owner_id, action_group, action)
                .await;
            actions.push(ActionButton {
                label: label.into(),
                token,
                style: ActionStyle::Default,
            });
        }
        Ok(actions)
    }

    async fn required_multiplexer_snapshot(&self) -> Result<MultiplexerSnapshot, EngineError> {
        self.rmux
            .runtime(self.agent.as_ref())
            .ok_or_else(|| EngineError::InvalidInput("workspace runtime is unavailable".into()))?
            .snapshot()
            .await?
            .ok_or_else(|| EngineError::InvalidInput("rmux is no longer running".into()))
    }

    async fn handle_multiplexer_action(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        action: MultiplexerUiAction,
    ) -> Result<(), EngineError> {
        match action {
            MultiplexerUiAction::ShowRoot => {
                self.show_multiplexer_root(conversation, owner_id).await
            }
            MultiplexerUiAction::ShowSession { session_id } => {
                self.show_multiplexer_session(conversation, owner_id, &session_id)
                    .await
            }
            MultiplexerUiAction::ShowWindow {
                session_id,
                window_id,
            } => {
                self.show_multiplexer_window(conversation, owner_id, &session_id, &window_id)
                    .await
            }
            MultiplexerUiAction::Mutate(mutation) => {
                self.execute_multiplexer_mutation(conversation, mutation)
                    .await
            }
        }
    }

    async fn execute_multiplexer_mutation(
        &self,
        conversation: &ConversationRef,
        mutation: MultiplexerMutation,
    ) -> Result<(), EngineError> {
        let result = match self
            .rmux
            .runtime(self.agent.as_ref())
            .ok_or_else(|| EngineError::InvalidInput("workspace runtime is unavailable".into()))?
            .mutate(mutation)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                self.send_view(
                    conversation,
                    &OutboundView {
                        title: "Terminal · rmux".into(),
                        subtitle: Some("Operation failed".into()),
                        body: error.to_string(),
                        status: ViewStatus::Error,
                        actions: Vec::new(),
                    },
                )
                .await?;
                return Ok(());
            }
        };
        let subtitle = if let Some(session) = result.session {
            let session_id = session.id.clone();
            let old = self
                .sessions
                .bindings
                .lock()
                .await
                .current_session(conversation)
                .cloned();
            let old_active = if let Some(old) = &old {
                self.turns.active.lock().await.contains_key(old)
            } else {
                false
            };
            self.sessions
                .cache
                .lock()
                .await
                .insert(session_id.clone(), session);
            self.bind_subscribed_session(conversation, &session_id, old_active)
                .await?;
            format!("Attached · {}", self.session_label(&session_id).await)
        } else {
            "Created".into()
        };
        self.send_view(
            conversation,
            &OutboundView {
                title: "Terminal · rmux".into(),
                subtitle: Some(subtitle),
                body: result.message,
                status: ViewStatus::Success,
                actions: Vec::new(),
            },
        )
        .await?;
        Ok(())
    }

    async fn attach(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        session_id: SessionId,
    ) -> Result<(), EngineError> {
        let already_attached = self
            .sessions
            .bindings
            .lock()
            .await
            .current_session(conversation)
            .is_some_and(|current| current == &session_id);
        if already_attached {
            let session_label = self.session_label(&session_id).await;
            self.send_view(
                conversation,
                &OutboundView::text(
                    format!("{} · {session_label}", self.agent.display_name()),
                    "This session is already attached.",
                ),
            )
            .await?;
            return Ok(());
        }
        if let Err(error) = self.agent.attach(&session_id).await {
            return self
                .show_attach_failure(conversation, owner_id, &session_id, &error)
                .await;
        }
        self.cache_session_summary(&session_id).await;
        let history = match self.agent.read_history(&session_id, None, 1).await {
            Ok(history) => history,
            Err(error) => {
                if self
                    .sessions
                    .bound_conversation(&session_id)
                    .await
                    .is_none()
                    && let Err(cleanup) = self.agent.unsubscribe(&session_id).await
                {
                    tracing::warn!(%cleanup, session = %session_id, "failed to release incomplete attachment");
                }
                return self
                    .show_attach_failure(conversation, owner_id, &session_id, &error)
                    .await;
            }
        };
        self.remember_history_cursors(conversation, &history).await;
        let old = self
            .sessions
            .bindings
            .lock()
            .await
            .current_session(conversation)
            .cloned();
        let old_active = if let Some(old_session) = &old {
            self.turns.active.lock().await.contains_key(old_session)
        } else {
            false
        };
        self.bind_subscribed_session(conversation, &session_id, old_active)
            .await?;
        self.send_history_views(
            conversation,
            &session_id,
            &history,
            HistoryPresentation::Attached,
        )
        .await?;
        Ok(())
    }

    async fn show_attach_failure(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        session: &SessionId,
        error: &AgentError,
    ) -> Result<(), EngineError> {
        tracing::warn!(%error, %session, "failed to attach IM session");
        let mut retry = self.attach_action(conversation, owner_id, session).await;
        retry.label = "Retry attach".into();
        self.send_view(
            conversation,
            &OutboundView {
                title: format!("{} · Attach failed", self.agent.display_name()),
                subtitle: Some(session.to_string()),
                body: format!("{error}\n\nRetry below or use /sessions to choose another session."),
                status: ViewStatus::Error,
                actions: vec![retry],
            },
        )
        .await?;
        Ok(())
    }

    async fn show_read_only_notice(
        &self,
        conversation: &ConversationRef,
    ) -> Result<(), EngineError> {
        self.send_view(conversation, &OutboundView {
            title: format!("{} · Read-only session", self.agent.display_name()),
            subtitle: None,
            body: "This session is connected read-only because another Codex process owns it. Use /history to read its latest content, or the original Codex session to send messages and make changes.".into(),
            status: ViewStatus::Info,
            actions: Vec::new(),
        }).await?;
        Ok(())
    }

    async fn show_current(&self, conversation: &ConversationRef) -> Result<(), EngineError> {
        let session = self
            .sessions
            .bindings
            .lock()
            .await
            .current_session(conversation)
            .cloned()
            .ok_or(EngineError::NoCurrentSession)?;
        let active = self.turns.active.lock().await.get(&session).cloned();
        let session_label = self.session_label(&session).await;
        self.send_view(
            conversation,
            &OutboundView {
                title: format!("{} · {session_label}", self.agent.display_name()),
                subtitle: active.as_ref().map(|turn| format!("Turn {turn} · running")),
                body: active.map_or_else(
                    || "Session is idle.".into(),
                    |_| "Session is active.".into(),
                ),
                status: ViewStatus::Info,
                actions: Vec::new(),
            },
        )
        .await?;
        Ok(())
    }

    async fn show_history(
        &self,
        conversation: &ConversationRef,
        cursor: Option<String>,
    ) -> Result<(), EngineError> {
        let session = self.current_session(conversation).await?;
        let history = self.agent.read_history(&session, cursor, 5).await?;
        self.remember_history_cursors(conversation, &history).await;
        self.send_history_views(
            conversation,
            &session,
            &history,
            HistoryPresentation::History,
        )
        .await?;
        Ok(())
    }

    async fn send_history_views(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        history: &HistoryPage,
        presentation: HistoryPresentation,
    ) -> Result<(), EngineError> {
        let session_label = self.session_label(session_id).await;
        let mut views = history_views(
            self.agent.display_name(),
            &session_label,
            history,
            presentation,
        )
        .into_iter();
        if let Some(mut overview) = views.next() {
            if matches!(presentation, HistoryPresentation::Attached)
                && self.agent.is_read_only(session_id).await
            {
                overview.body.push_str("\n\nConnected read-only: another Codex process owns this session. Latest content is checked every 10 seconds; sending messages, stopping turns, and changing settings require the original Codex session.");
            }
            self.send_view(conversation, &overview).await?;
        }
        let running_turn_id = match presentation {
            HistoryPresentation::Attached => history
                .turns
                .last()
                .filter(|turn| matches!(turn.status, TurnStatus::InProgress | TurnStatus::Unknown))
                .map(|turn| turn.id.as_str()),
            HistoryPresentation::History => None,
        };
        if matches!(presentation, HistoryPresentation::Attached) && running_turn_id.is_none() {
            self.turns.remove_active(session_id).await;
        }
        for (turn, view) in history.turns.iter().zip(views) {
            if running_turn_id == Some(turn.id.as_str()) {
                self.hydrate_running_turn(conversation, session_id, turn)
                    .await?;
            } else {
                self.send_view(conversation, &view).await?;
            }
        }
        Ok(())
    }

    async fn hydrate_running_turn(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        turn: &TurnSummary,
    ) -> Result<(), EngineError> {
        let key = (session_id.clone(), turn.id.clone());
        self.turns
            .active
            .lock()
            .await
            .insert(session_id.clone(), turn.id.clone());
        self.turns.buffers.lock().await.insert(
            key.clone(),
            TurnBuffer {
                user_text: turn.user_text.clone().unwrap_or_default(),
                agent_text: turn.agent_text.clone().unwrap_or_default(),
                status: turn.status.clone(),
                started_at: Some(Instant::now()),
                rendered_elapsed_seconds: None,
            },
        );
        self.turns.views.lock().await.remove(&key);
        self.turns.last_renders.lock().await.remove(&key);
        self.render_turn(
            conversation,
            session_id,
            &turn.id,
            DeliveryClass::Live,
            true,
        )
        .await
    }

    async fn remember_history_cursors(
        &self,
        conversation: &ConversationRef,
        history: &HistoryPage,
    ) {
        self.sessions.history_cursors.lock().await.insert(
            conversation.clone(),
            HistoryCursors {
                older: history.older_cursor.clone(),
                newer: history.newer_cursor.clone(),
            },
        );
    }

    async fn detach(&self, conversation: &ConversationRef) -> Result<(), EngineError> {
        self.interactions
            .session_inputs
            .lock()
            .await
            .remove(conversation);
        let current = self
            .sessions
            .current(conversation)
            .await
            .ok_or(EngineError::NoCurrentSession)?;
        let active = self.turns.is_active(&current).await;
        self.clear_session_stop_actions(&current).await?;
        self.state.detach(conversation).await?;
        let session = self
            .sessions
            .detach(conversation, active)
            .await
            .ok_or(EngineError::NoCurrentSession)?;
        let session_label = self.session_label(&session).await;
        if !active && let Err(error) = self.agent.unsubscribe(&session).await {
            tracing::warn!(%error, %session, "failed to unsubscribe a detached session");
        }
        self.update_command_menu_best_effort(conversation, false)
            .await;
        if let Err(error) = self
            .send_view(
                conversation,
                &OutboundView::text("Agentix", format!("Detached from {session_label}.")),
            )
            .await
        {
            tracing::warn!(%error, ?conversation, "failed to notify a detached conversation");
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn run_session_command(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        command: SessionCommand,
    ) -> Result<(), EngineError> {
        let session = match self.current_session(conversation).await {
            Ok(session) => session,
            Err(EngineError::NoCurrentSession) => {
                self.send_view(
                    conversation,
                    &OutboundView {
                        title: "Agentix · Session command".into(),
                        subtitle: Some("Not attached".into()),
                        body: "Attach a session with `/sessions` before using this command.".into(),
                        status: ViewStatus::Warning,
                        actions: Vec::new(),
                    },
                )
                .await?;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if matches!(command, SessionCommand::Exit) {
            return self.detach(conversation).await;
        }
        if self.agent.is_read_only(&session).await {
            return self.show_read_only_notice(conversation).await;
        }
        if matches!(command, SessionCommand::Rename(None)) {
            self.interactions.session_inputs.lock().await.insert(
                conversation.clone(),
                PendingSessionInput::Rename(session.clone()),
            );
            self.send_view(
                conversation,
                &OutboundView::text(
                    "Codex · Rename",
                    "Reply with the new session name. Use `/cancel` to stop.",
                ),
            )
            .await?;
            return Ok(());
        }
        let inline_plan_prompt = match &command {
            SessionCommand::Plan { prompt, .. } => prompt.clone(),
            _ => None,
        };
        let renamed_to = match &command {
            SessionCommand::Rename(Some(name)) => Some(name.clone()),
            _ => None,
        };
        if matches!(
            command,
            SessionCommand::Clear(_) | SessionCommand::Plan { .. }
        ) && self.turns.active_turn(&session).await.is_some()
        {
            self.send_view(
                conversation,
                &OutboundView {
                    title: format!("{} · Command unavailable", self.agent.display_name()),
                    subtitle: Some(self.session_label(&session).await),
                    body: "Wait for the active turn to finish, or use `/stop` first.".into(),
                    status: ViewStatus::Warning,
                    actions: Vec::new(),
                },
            )
            .await?;
            return Ok(());
        }
        let Some(session_control) = self.agent.session_control() else {
            self.send_view(
                conversation,
                &OutboundView {
                    title: "Agentix · Session command".into(),
                    subtitle: Some("Unsupported".into()),
                    body: format!(
                        "{} does not support attached-session commands.",
                        self.agent.display_name()
                    ),
                    status: ViewStatus::Warning,
                    actions: Vec::new(),
                },
            )
            .await?;
            return Ok(());
        };
        let session_label = self.session_label(&session).await;
        let result = match session_control.run_session_command(&session, command).await {
            Ok(result) => result,
            Err(error) => {
                self.send_view(
                    conversation,
                    &OutboundView {
                        title: format!("{} · Command failed", self.agent.display_name()),
                        subtitle: Some(session_label),
                        body: error.to_string(),
                        status: ViewStatus::Error,
                        actions: Vec::new(),
                    },
                )
                .await?;
                return Ok(());
            }
        };
        let target_session = if let Some(replacement) = result.replacement_session {
            let replacement_id = replacement.id.clone();
            let old_active = self.turns.active.lock().await.contains_key(&session);
            self.sessions
                .cache
                .lock()
                .await
                .insert(replacement_id.clone(), replacement);
            self.bind_subscribed_session(conversation, &replacement_id, old_active)
                .await?;
            replacement_id
        } else {
            session
        };
        if let Some(name) = renamed_to
            && let Some(summary) = self.sessions.cache.lock().await.get_mut(&target_session)
        {
            summary.name = Some(name);
        }
        if let Some(turn_id) = &result.active_turn {
            self.turns
                .active
                .lock()
                .await
                .insert(target_session.clone(), turn_id.clone());
        }
        let target_label = self.session_label(&target_session).await;
        let actions = self
            .session_command_actions(conversation, owner_id, &target_session, result.choices)
            .await;
        self.send_view(
            conversation,
            &OutboundView {
                title: result.title,
                subtitle: Some(target_label),
                body: result.body,
                status: if result.active_turn.is_some() {
                    ViewStatus::Running
                } else {
                    ViewStatus::Info
                },
                actions,
            },
        )
        .await?;
        if let Some(prompt) = inline_plan_prompt {
            self.send_prompt(conversation, &prompt).await?;
        }
        Ok(())
    }

    async fn session_command_actions(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        session_id: &SessionId,
        choices: Vec<SessionCommandChoice>,
    ) -> Vec<ActionButton> {
        let action_group = Uuid::new_v4().simple().to_string();
        let mut actions = Vec::with_capacity(choices.len());
        for choice in choices {
            let token = self
                .issue_action(
                    conversation,
                    owner_id,
                    &action_group,
                    UiAction::SessionCommand {
                        session_id: session_id.clone(),
                        command: choice.command,
                    },
                )
                .await;
            actions.push(ActionButton {
                label: choice.label,
                token,
                style: ActionStyle::Default,
            });
        }
        actions
    }

    async fn bind_subscribed_session(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        old_active: bool,
    ) -> Result<(), EngineError> {
        if let Some(previous) = self.sessions.current(conversation).await {
            self.clear_session_stop_actions(&previous).await?;
        }
        self.clear_session_stop_actions(session_id).await?;
        let persistent = self.state.attach(conversation, session_id).await?;
        let persisted_previous = persistent.previous_session.clone();
        let outcome = self
            .sessions
            .attach_at_epoch(
                conversation.clone(),
                session_id.clone(),
                old_active,
                persistent.epoch,
            )
            .await;
        debug_assert_eq!(persistent.epoch, outcome.epoch);
        let live_previous = outcome.previous_session.clone();

        self.apply_binding_effects(conversation, session_id, old_active, outcome)
            .await;
        if let Some(previous) = persisted_previous
            && live_previous.as_ref() != Some(&previous)
            && let Err(error) = self.agent.unsubscribe(&previous).await
        {
            tracing::warn!(%error, session = %previous, "failed to stop watching the replaced session");
        }
        Ok(())
    }

    async fn apply_binding_effects(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        old_active: bool,
        outcome: AttachOutcome,
    ) {
        if let Some(previous) = outcome.previous_session
            && !old_active
            && let Err(error) = self.agent.unsubscribe(&previous).await
        {
            tracing::warn!(%error, session = %previous, "failed to unsubscribe the previous session");
        }
        if let Some(displaced) = outcome.displaced_conversation {
            let session_label = self.session_label(session_id).await;
            self.update_command_menu_best_effort(&displaced, false)
                .await;
            if let Err(error) = self
                .send_view(
                    &displaced,
                    &OutboundView {
                        title: format!("{} session moved", self.agent.display_name()),
                        subtitle: Some(session_label),
                        body: "This session was attached from another IM conversation.".into(),
                        status: ViewStatus::Muted,
                        actions: Vec::new(),
                    },
                )
                .await
            {
                tracing::warn!(%error, ?displaced, "failed to notify a displaced conversation");
            }
        }
        self.update_command_menu_best_effort(conversation, true)
            .await;
    }

    async fn update_command_menu_best_effort(
        &self,
        conversation: &ConversationRef,
        attached: bool,
    ) {
        if let Err(error) = self.update_command_menu(conversation, attached).await {
            tracing::warn!(%error, ?conversation, attached, "failed to update the IM command menu");
        }
    }

    async fn update_command_menu(
        &self,
        conversation: &ConversationRef,
        attached: bool,
    ) -> Result<(), EngineError> {
        let channel = self
            .channels
            .get(&conversation.channel)
            .ok_or(EngineError::MissingChannel(conversation.channel))?;
        let mut menu = command_menu(attached && self.agent.capabilities().session_control);
        if attached
            && let Some(session) = self.sessions.current(conversation).await
            && self.agent.is_read_only(&session).await
        {
            menu.commands.retain(|command| {
                matches!(
                    command.name.as_str(),
                    "sessions" | "rmux" | "current" | "history" | "detach" | "cancel" | "help"
                )
            });
        }
        if self.task_board.is_some() {
            menu.commands.push(ChannelCommand::new(
                "dashboard",
                "Browse projects and task boards",
            ));
            if attached {
                menu.commands.extend([
                    ChannelCommand::new("board", "Show this session's task board").contextual(),
                    ChannelCommand::new("jobs", "Browse this session's jobs").contextual(),
                    ChannelCommand::new("inboxes", "Browse this project's inbox").contextual(),
                    ChannelCommand::new("inbox", "Append a requirement to this project's inbox")
                        .contextual(),
                ]);
            }
        }
        let primary = ["sessions", "dashboard", "cancel", "rmux", "help"];
        menu.commands.sort_by(|left, right| {
            let rank = |command: &ChannelCommand| {
                if command.contextual {
                    primary.len()
                } else {
                    primary
                        .iter()
                        .position(|name| *name == command.name)
                        .unwrap_or(primary.len())
                }
            };
            rank(left)
                .cmp(&rank(right))
                .then_with(|| left.name.cmp(&right.name))
        });
        channel.set_command_menu(conversation, &menu).await?;
        Ok(())
    }

    async fn stop_current(&self, conversation: &ConversationRef) -> Result<(), EngineError> {
        let session = self.current_session(conversation).await?;
        if self.agent.is_read_only(&session).await {
            return self.show_read_only_notice(conversation).await;
        }
        let turn = self
            .turns
            .active_turn(&session)
            .await
            .ok_or_else(|| EngineError::InvalidInput("the current session is idle".into()))?;
        self.agent.interrupt(&session, &turn).await?;
        Ok(())
    }

    async fn send_prompt(
        &self,
        conversation: &ConversationRef,
        prompt: &str,
    ) -> Result<(), EngineError> {
        let session = self.current_session(conversation).await?;
        if self.agent.is_read_only(&session).await {
            return self.show_read_only_notice(conversation).await;
        }
        let active = self.turns.active_turn(&session).await;
        if let Some(turn) = active {
            if let Some(queue) = self.agent.queued_prompts() {
                return self
                    .queue_prompt(queue, conversation, &session, prompt)
                    .await;
            }
            let turn_id = self.agent.steer(&session, &turn, prompt).await?;
            self.turns.set_active(session, turn_id).await;
            return Ok(());
        }
        let turn_id = self.agent.start_turn(&session, prompt).await?;
        self.turns
            .set_active(session.clone(), turn_id.clone())
            .await;
        self.turns.buffers.lock().await.insert(
            (session.clone(), turn_id.clone()),
            TurnBuffer {
                user_text: prompt.to_owned(),
                agent_text: String::new(),
                status: TurnStatus::InProgress,
                started_at: Some(Instant::now()),
                rendered_elapsed_seconds: None,
            },
        );
        if let Err(error) = self
            .render_turn(conversation, &session, &turn_id, DeliveryClass::Live, true)
            .await
        {
            tracing::warn!(
                %error,
                session = %session,
                turn = %turn_id,
                "failed to show the initial IM working state"
            );
        }
        Ok(())
    }

    async fn queue_prompt(
        &self,
        queue: &dyn crate::QueuedPromptPort,
        conversation: &ConversationRef,
        session: &SessionId,
        prompt: &str,
    ) -> Result<(), EngineError> {
        let client_message_id = Uuid::new_v4().to_string();
        let queued = queue
            .queue_prompt(session, prompt, &client_message_id)
            .await?;
        let position = queue
            .list_queued_prompts(session)
            .await
            .ok()
            .and_then(|prompts| prompts.iter().position(|item| item.id == queued.id))
            .map(|index| index + 1);
        self.send_view(
            conversation,
            &OutboundView {
                title: format!("{} · Queued", self.agent.display_name()),
                subtitle: position.map(|position| format!("Position #{position}")),
                body: format!("**👤 You**\n\n{}", markdown_quote(prompt)),
                status: ViewStatus::Info,
                actions: Vec::new(),
            },
        )
        .await?;
        Ok(())
    }

    async fn show_queue(&self, conversation: &ConversationRef) -> Result<(), EngineError> {
        let session = self.current_session(conversation).await?;
        if self.agent.is_read_only(&session).await {
            return self.show_read_only_notice(conversation).await;
        }
        let session_label = self.session_label(&session).await;
        let Some(queue) = self.agent.queued_prompts() else {
            self.send_view(
                conversation,
                &OutboundView::text(
                    format!("{} · Queue", self.agent.display_name()),
                    "Persistent queued prompts are not supported by this agent.",
                ),
            )
            .await?;
            return Ok(());
        };
        let prompts = queue.list_queued_prompts(&session).await?;
        let count = prompts.len();
        let body = if prompts.is_empty() {
            "The queue is empty.".to_owned()
        } else {
            prompts
                .iter()
                .enumerate()
                .map(|(index, prompt)| {
                    markdown_quote(&format!("**{}**\n{}", index + 1, prompt.text))
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        };
        self.send_view(
            conversation,
            &OutboundView {
                title: format!("{} · {session_label}", self.agent.display_name()),
                subtitle: Some(format!(
                    "Queue · {count} {}",
                    if count == 1 { "message" } else { "messages" }
                )),
                body,
                status: ViewStatus::Info,
                actions: Vec::new(),
            },
        )
        .await?;
        Ok(())
    }

    async fn current_session(
        &self,
        conversation: &ConversationRef,
    ) -> Result<SessionId, EngineError> {
        self.sessions
            .current(conversation)
            .await
            .ok_or(EngineError::NoCurrentSession)
    }

    async fn cache_session_summary(&self, session_id: &SessionId) {
        if self.sessions.cache.lock().await.contains_key(session_id) {
            return;
        }
        match self.agent.list_sessions(None, 100).await {
            Ok(page) => {
                let mut sessions = self.sessions.cache.lock().await;
                sessions.extend(
                    page.sessions
                        .into_iter()
                        .map(|session| (session.id.clone(), session)),
                );
            }
            Err(error) => {
                tracing::debug!(%error, session = %session_id, "failed to load session title");
            }
        }
    }

    async fn session_label(&self, session_id: &SessionId) -> String {
        self.sessions
            .cache
            .lock()
            .await
            .get(session_id)
            .map_or_else(
                || format!("Untitled · {}", session_id.short()),
                session_display_label,
            )
    }

    pub async fn handle_agent_event(&self, event: AgentEvent) -> Result<(), EngineError> {
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
            AgentEvent::SessionExited { session_id } => {
                self.task_session_event("session.end", session_id).await;
                return self.handle_session_exit(&SessionId::new(session_id)).await;
            }
            AgentEvent::SessionResumed { session_id } => {
                self.task_session_event("session.start", session_id).await;
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
                let key = (session_id.clone(), turn_id.clone());
                let mut buffers = self.turns.buffers.lock().await;
                let buffer = buffers.entry(key.clone()).or_default();
                buffer.ensure_started();
                buffer.agent_text.push_str(&delta);
                drop(buffers);
                self.render_turn(&conversation, &session_id, &turn_id, delivery, false)
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

    async fn route_agent_event(
        &self,
        session_id: &SessionId,
        importance: EventImportance,
        event: &AgentEvent,
    ) -> Result<Option<(ConversationRef, DeliveryClass)>, EngineError> {
        let route = self.sessions.route(session_id, importance).await;
        if route.is_some() {
            return Ok(route);
        }
        if let AgentEvent::TurnCompleted {
            turn_id,
            status,
            error,
            ..
        } = event
        {
            self.notify_unattached_turn_completion(session_id, turn_id, status, error.as_deref())
                .await?;
        }
        Ok(None)
    }

    async fn handle_turn_completed(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        turn_id: String,
        status: TurnStatus,
        error: Option<String>,
        delivery: DeliveryClass,
    ) -> Result<(), EngineError> {
        let key = (session_id.clone(), turn_id.clone());
        let mut buffers = self.turns.buffers.lock().await;
        let buffer = buffers.entry(key).or_default();
        buffer.ensure_started();
        buffer.status = status;
        if let Some(error) = error {
            buffer.agent_text.push_str(&format!("\n\nError: {error}"));
        }
        drop(buffers);
        self.render_turn(conversation, session_id, &turn_id, delivery, true)
            .await?;
        if self.turns.active_turn(session_id).await.as_deref() == Some(&turn_id) {
            self.turns.remove_active(session_id).await;
        }
        if delivery == DeliveryClass::Draining {
            self.turns
                .record_background_notification(conversation, session_id, &turn_id)
                .await;
            self.sessions.finish_draining(session_id).await;
            self.agent.unsubscribe(session_id).await?;
        }
        Ok(())
    }

    async fn notify_unattached_turn_completion(
        &self,
        session_id: &SessionId,
        turn_id: &str,
        status: &TurnStatus,
        error: Option<&str>,
    ) -> Result<(), EngineError> {
        self.turns.remove_active(session_id).await;
        if !self.background_turn_notifications {
            return Ok(());
        }
        let recipients = self
            .interactions
            .owners
            .lock()
            .await
            .iter()
            .map(|(conversation, owner)| (conversation.clone(), owner.clone()))
            .collect::<Vec<_>>();
        if recipients.is_empty() {
            return Ok(());
        }

        let delivered = self.turns.background_notifications.lock().await;
        let recipients = recipients
            .into_iter()
            .filter(|(conversation, _)| {
                !delivered.get(session_id).is_some_and(|notice| {
                    notice.turn_id == turn_id && notice.recipients.contains(conversation)
                })
            })
            .collect::<Vec<_>>();
        drop(delivered);
        if recipients.is_empty() {
            return Ok(());
        }
        let content = self.background_turn_content(session_id, turn_id).await;
        let body = format!("{}\n\n{content}", background_completion_body(status, error));
        self.cache_session_summary(session_id).await;
        let session_label = self.session_label(session_id).await;
        for (conversation, owner_id) in recipients {
            if self
                .turns
                .background_notification_delivered(&conversation, session_id, turn_id)
                .await
            {
                continue;
            }
            let action = self
                .attach_action(&conversation, &owner_id, session_id)
                .await;
            self.send_view(
                &conversation,
                &OutboundView {
                    title: format!("{} · {session_label}", self.agent.display_name()),
                    subtitle: Some(format!(
                        "Background turn {} · {}",
                        short_identifier(turn_id),
                        turn_status_label(status)
                    )),
                    body: body.clone(),
                    status: ViewStatus::Background,
                    actions: vec![action],
                },
            )
            .await?;
            self.turns
                .record_background_notification(&conversation, session_id, turn_id)
                .await;
        }
        Ok(())
    }

    async fn background_turn_content(&self, session: &SessionId, turn_id: &str) -> String {
        let mut cursor = None;
        let mut visited = HashSet::new();
        loop {
            let page = match self.agent.read_history(session, cursor, 20).await {
                Ok(page) => page,
                Err(error) => {
                    tracing::warn!(%error, %session, %turn_id, "failed to read background turn content");
                    break;
                }
            };
            if let Some(turn) = page.turns.iter().find(|turn| turn.id == turn_id) {
                return turn_conversation_body(
                    self.agent.display_name(),
                    turn.user_text.as_deref(),
                    turn.agent_text.as_deref(),
                );
            }
            match page.older_cursor {
                Some(next) if visited.insert(next.clone()) => cursor = Some(next),
                _ => break,
            }
        }
        "Turn content is unavailable.".into()
    }

    async fn handle_completed_item(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        turn_id: &str,
        item: &ItemSummary,
        delivery: DeliveryClass,
    ) -> Result<(), EngineError> {
        if self.apply_completed_item(session_id, turn_id, item).await {
            self.render_turn(conversation, session_id, turn_id, delivery, false)
                .await?;
        }
        Ok(())
    }

    async fn record_turn_started(
        &self,
        session_id: SessionId,
        turn_id: String,
    ) -> Result<(), EngineError> {
        if let Some(previous) = self.turns.active_turn(&session_id).await
            && previous != turn_id
        {
            self.clear_turn_stop_action(&(session_id.clone(), previous))
                .await?;
        }
        self.turns
            .set_active(session_id.clone(), turn_id.clone())
            .await;
        self.turns
            .buffers
            .lock()
            .await
            .entry((session_id, turn_id))
            .or_default()
            .ensure_started();
        Ok(())
    }

    async fn handle_session_exit(&self, session_id: &SessionId) -> Result<(), EngineError> {
        let Some(conversation) = self.sessions.bound_conversation(session_id).await else {
            return Ok(());
        };
        let session_label = self.session_label(session_id).await;

        let epoch = self.state.suspend(&conversation).await?;
        self.sessions
            .bindings
            .lock()
            .await
            .detach(&conversation, false);
        debug_assert_eq!(self.sessions.epoch(&conversation).await, epoch);
        self.interactions
            .actions
            .lock()
            .await
            .retain(|action| !action.targets_session(session_id));
        self.interactions
            .pending
            .lock()
            .await
            .retain(|key, _| &key.session_id != session_id);
        let active_turn = self.turns.active.lock().await.remove(session_id);
        let mut turn_ids = self
            .turns
            .views
            .lock()
            .await
            .keys()
            .filter(|(session, _)| session == session_id)
            .map(|(_, turn_id)| turn_id.clone())
            .collect::<Vec<_>>();
        turn_ids.extend(
            self.interactions
                .turn_action_groups
                .lock()
                .await
                .keys()
                .filter(|(session, _)| session == session_id)
                .map(|(_, turn_id)| turn_id.clone()),
        );
        if let Some(turn_id) = active_turn {
            turn_ids.push(turn_id);
        }
        turn_ids.sort();
        turn_ids.dedup();

        for turn_id in turn_ids {
            self.cleanup_exited_turn(&conversation, session_id, &turn_id)
                .await;
        }

        self.sessions.cache.lock().await.remove(session_id);
        self.interactions
            .reply_modes
            .lock()
            .await
            .remove(&conversation);
        self.interactions
            .session_inputs
            .lock()
            .await
            .remove(&conversation);
        self.sessions
            .history_cursors
            .lock()
            .await
            .remove(&conversation);
        self.update_command_menu_best_effort(&conversation, false)
            .await;
        if let Err(error) = self
            .notify_session_exit(&conversation, &session_label)
            .await
        {
            tracing::warn!(%error, ?conversation, "failed to notify an exited session");
        }
        Ok(())
    }

    async fn handle_session_resume(&self, session_id: &SessionId) -> Result<(), EngineError> {
        let Some((conversation, _)) =
            self.state
                .list_bindings()
                .await?
                .into_iter()
                .find(|(conversation, saved_session)| {
                    saved_session == session_id && self.channels.contains_key(&conversation.channel)
                })
        else {
            return Ok(());
        };
        if self.sessions.current(&conversation).await.as_ref() == Some(session_id) {
            return Ok(());
        }

        self.cache_session_summary(session_id).await;
        let epoch = self.state.binding_epoch(&conversation).await?;
        self.sessions
            .attach_at_epoch(conversation.clone(), session_id.clone(), false, epoch)
            .await;
        self.update_command_menu_best_effort(&conversation, true)
            .await;
        let session_label = self.session_label(session_id).await;
        if let Err(error) = self
            .send_view(
                &conversation,
                &OutboundView {
                    title: format!("{} session resumed", self.agent.display_name()),
                    subtitle: Some("Automatically reattached".into()),
                    body: format!(
                        "{} session {session_label} is running again. This IM conversation was reattached automatically.",
                        self.agent.display_name()
                    ),
                    status: ViewStatus::Success,
                    actions: Vec::new(),
                },
            )
            .await
        {
            tracing::warn!(%error, ?conversation, "failed to notify a resumed session");
        }
        Ok(())
    }

    async fn cleanup_exited_turn(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        turn_id: &str,
    ) {
        let key = (session_id.clone(), turn_id.to_owned());
        if self.turns.views.lock().await.contains_key(&key) {
            self.turns
                .buffers
                .lock()
                .await
                .entry(key.clone())
                .or_default()
                .status = TurnStatus::Interrupted;
            if let Err(error) = self
                .render_turn(conversation, session_id, turn_id, DeliveryClass::Live, true)
                .await
            {
                tracing::warn!(
                    %error,
                    session = %session_id,
                    turn = %turn_id,
                    "failed to mark an exited Codex turn as interrupted"
                );
            }
        } else if let Some(group_id) = self
            .interactions
            .turn_action_groups
            .lock()
            .await
            .remove(&key)
        {
            self.revoke_action_group(&group_id).await;
        }
        if let Err(error) = self.state.delete_turn_view(session_id, turn_id).await {
            tracing::warn!(
                %error,
                session = %session_id,
                turn = %turn_id,
                "failed to delete an exited Codex turn checkpoint"
            );
        }
        self.turns.buffers.lock().await.remove(&key);
        self.turns.views.lock().await.remove(&key);
        self.turns.last_renders.lock().await.remove(&key);
        self.turns.stop_actions.lock().await.remove(&key);
    }

    async fn notify_session_exit(
        &self,
        conversation: &ConversationRef,
        session_label: &str,
    ) -> Result<(), EngineError> {
        self.send_view(
            conversation,
            &OutboundView {
                title: format!("{} session exited", self.agent.display_name()),
                subtitle: Some("Automatically detached".into()),
                body: format!(
                    "{} session {session_label} is no longer running. This IM conversation was detached automatically and will reattach if the same session is resumed.",
                    self.agent.display_name()
                ),
                status: ViewStatus::Warning,
                actions: Vec::new(),
            },
        )
        .await?;
        Ok(())
    }

    async fn apply_completed_item(
        &self,
        session_id: &SessionId,
        turn_id: &str,
        item: &ItemSummary,
    ) -> bool {
        if !matches!(item.kind.as_str(), "agentMessage" | "userMessage") {
            return false;
        }
        let mut buffers = self.turns.buffers.lock().await;
        let buffer = buffers
            .entry((session_id.clone(), turn_id.to_owned()))
            .or_default();
        buffer.ensure_started();
        match item.kind.as_str() {
            "agentMessage" => buffer.agent_text = item.text.clone().unwrap_or_default(),
            "userMessage" => buffer.user_text = item.text.clone().unwrap_or_default(),
            _ => {}
        }
        true
    }

    async fn render_turn(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        turn_id: &str,
        delivery: DeliveryClass,
        force: bool,
    ) -> Result<(), EngineError> {
        let key = (session_id.clone(), turn_id.to_owned());
        let session_label = self.session_label(session_id).await;
        let interval = self
            .channel(conversation.channel)?
            .streaming_update_interval();
        if !self.turns.should_render(&key, force, interval).await {
            return Ok(());
        }
        let (mut view, is_running, snapshot) = {
            let buffers = self.turns.buffers.lock().await;
            let Some(buffer) = buffers.get(&key) else {
                // A deferred startup refresh can race with removal of a finished turn.
                return Ok(());
            };
            (
                live_turn_view(
                    self.agent.display_name(),
                    &session_label,
                    turn_id,
                    buffer,
                    delivery,
                ),
                matches!(buffer.status, TurnStatus::InProgress | TurnStatus::Unknown),
                buffer.clone(),
            )
        };
        let existing = self.turns.views.lock().await.get(&key).cloned();
        let can_stop = is_running
            && !self.agent.is_read_only(session_id).await
            && delivery == DeliveryClass::Live
            && self.sessions.current(conversation).await.as_ref() == Some(session_id)
            && {
                // Some adapters deliver output before a turn-started event.
                let mut active = self.turns.active.lock().await;
                if existing.is_none() {
                    active
                        .entry(session_id.clone())
                        .or_insert_with(|| turn_id.to_owned());
                }
                active
                    .get(session_id)
                    .is_some_and(|active_turn| active_turn == turn_id)
            };
        let owner_id = self
            .interactions
            .owners
            .lock()
            .await
            .get(conversation)
            .cloned();
        if let Some(stop_action) = self
            .replace_stop_action(&key, conversation, owner_id.as_deref(), can_stop)
            .await
        {
            view.actions.push(stop_action);
        }
        if delivery == DeliveryClass::Draining
            && !is_running
            && let Some(owner_id) = owner_id.as_deref()
        {
            view.actions
                .push(self.attach_action(conversation, owner_id, session_id).await);
        }
        let message = if let Some(message) = existing {
            self.channel(conversation.channel)?
                .update(conversation, &message, &view)
                .await?;
            message
        } else {
            let message = self.send_view(conversation, &view).await?;
            self.turns
                .views
                .lock()
                .await
                .insert(key.clone(), message.clone());
            message
        };
        self.turns
            .mark_elapsed_rendered(&key, snapshot.elapsed_seconds())
            .await;
        if can_stop {
            self.state
                .save_turn_view(&StoredTurnView {
                    session_id: session_id.clone(),
                    turn_id: turn_id.to_owned(),
                    message,
                    owner_id,
                    user_text: snapshot.user_text,
                    agent_text: snapshot.agent_text,
                    status: snapshot.status,
                })
                .await?;
        } else {
            self.state.delete_turn_view(session_id, turn_id).await?;
        }
        Ok(())
    }

    async fn clear_session_stop_actions(&self, session_id: &SessionId) -> Result<(), EngineError> {
        let keys: Vec<_> = self
            .interactions
            .turn_action_groups
            .lock()
            .await
            .keys()
            .filter(|(session, _)| session == session_id)
            .cloned()
            .collect();
        for key in keys {
            self.clear_turn_stop_action(&key).await?;
        }
        Ok(())
    }

    async fn clear_turn_stop_action(&self, key: &(SessionId, String)) -> Result<(), EngineError> {
        if !self
            .interactions
            .turn_action_groups
            .lock()
            .await
            .contains_key(key)
        {
            return Ok(());
        }
        let message = self.turns.views.lock().await.get(key).cloned();
        let buffer = self.turns.buffers.lock().await.get(key).cloned();
        if let (Some(message), Some(buffer)) = (message, buffer)
            && matches!(buffer.status, TurnStatus::InProgress | TurnStatus::Unknown)
        {
            let session_label = self.session_label(&key.0).await;
            let view = live_turn_view(
                self.agent.display_name(),
                &session_label,
                &key.1,
                &buffer,
                DeliveryClass::Live,
            );
            self.channel(message.conversation.channel)?
                .update(&message.conversation, &message, &view)
                .await?;
            self.replace_stop_action(key, &message.conversation, None, false)
                .await;
            self.state.delete_turn_view(&key.0, &key.1).await?;
        }
        Ok(())
    }

    async fn replace_stop_action(
        &self,
        key: &(SessionId, String),
        conversation: &ConversationRef,
        owner_id: Option<&str>,
        can_stop: bool,
    ) -> Option<ActionButton> {
        let previous_group = self
            .interactions
            .turn_action_groups
            .lock()
            .await
            .remove(key);
        if let Some(previous_group) = previous_group {
            self.revoke_action_group(&previous_group).await;
        }
        self.turns.stop_actions.lock().await.remove(key);
        let owner_id = owner_id.filter(|_| can_stop)?;
        let action_group = Uuid::new_v4().simple().to_string();
        let token = self
            .issue_action(
                conversation,
                owner_id,
                &action_group,
                UiAction::Stop {
                    session_id: key.0.clone(),
                    turn_id: key.1.clone(),
                },
            )
            .await;
        self.interactions
            .turn_action_groups
            .lock()
            .await
            .insert(key.clone(), action_group);
        let action = ActionButton {
            label: "Stop".into(),
            token,
            style: ActionStyle::Danger,
        };
        self.turns
            .stop_actions
            .lock()
            .await
            .insert(key.clone(), action.clone());
        Some(action)
    }

    async fn attach_action(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        session_id: &SessionId,
    ) -> ActionButton {
        let action_group = Uuid::new_v4().simple().to_string();
        let token = self
            .issue_action(
                conversation,
                owner_id,
                &action_group,
                UiAction::Attach(session_id.clone()),
            )
            .await;
        ActionButton {
            label: "Attach".into(),
            token,
            style: ActionStyle::Primary,
        }
    }

    /// Refreshes visible running turns so their working duration advances without agent output.
    pub async fn refresh_working_turns(&self) -> usize {
        let active = self.turns.active.lock().await.clone();
        let mut refreshed = 0;
        for (session_id, turn_id) in active {
            let key = (session_id.clone(), turn_id.clone());
            let Some((conversation, delivery)) = self
                .sessions
                .route(&session_id, EventImportance::Stream)
                .await
            else {
                continue;
            };
            let message = self.turns.views.lock().await.get(&key).cloned();
            let Some(buffer) = self.turns.buffers.lock().await.get(&key).cloned() else {
                continue;
            };
            if !matches!(buffer.status, TurnStatus::InProgress | TurnStatus::Unknown) {
                continue;
            }
            let Some(elapsed_seconds) = buffer.elapsed_seconds() else {
                continue;
            };
            if buffer.rendered_elapsed_seconds == Some(elapsed_seconds) {
                continue;
            }
            let Some(message) = message else {
                match self
                    .render_turn(&conversation, &session_id, &turn_id, delivery, true)
                    .await
                {
                    Ok(()) => refreshed += 1,
                    Err(error) => tracing::warn!(
                        %error,
                        session = %session_id,
                        turn = %turn_id,
                        "failed to create the IM working state"
                    ),
                }
                continue;
            };
            let interval = self
                .channel(conversation.channel)
                .map_or(Duration::from_secs(1), |channel| {
                    channel.streaming_update_interval()
                });
            if !self.turns.should_render(&key, false, interval).await {
                continue;
            }
            let session_label = self.session_label(&session_id).await;
            let mut view = live_turn_view(
                self.agent.display_name(),
                &session_label,
                &turn_id,
                &buffer,
                delivery,
            );
            if delivery == DeliveryClass::Live
                && let Some(action) = self.turns.stop_actions.lock().await.get(&key).cloned()
            {
                view.actions.push(action);
            }
            let result = match self.channel(conversation.channel) {
                Ok(channel) => channel.update(&conversation, &message, &view).await,
                Err(error) => {
                    tracing::warn!(
                        %error,
                        session = %session_id,
                        turn = %turn_id,
                        "failed to refresh the IM working duration"
                    );
                    continue;
                }
            };
            match result {
                Ok(()) => {
                    self.turns
                        .mark_elapsed_rendered(&key, Some(elapsed_seconds))
                        .await;
                    refreshed += 1;
                }
                Err(error) => tracing::warn!(
                    %error,
                    session = %session_id,
                    turn = %turn_id,
                    "failed to refresh the IM working duration"
                ),
            }
        }
        refreshed
    }

    async fn render_interaction(
        &self,
        conversation: &ConversationRef,
        request: &InteractionRequest,
        delivery: DeliveryClass,
    ) -> Result<(), EngineError> {
        let session_id = SessionId::new(&request.session_id);
        let interaction = interaction_key(request);
        let session_label = self.session_label(&session_id).await;
        let owner_id = self
            .interactions
            .owners
            .lock()
            .await
            .get(conversation)
            .cloned()
            .ok_or(EngineError::InvalidAction)?;
        let mut actions = Vec::new();
        let action_group = Uuid::new_v4().simple().to_string();
        let input = if request.kind == InteractionKind::UserInput {
            let questions = input_questions(request);
            let progress = InputProgress {
                answers: vec![None; questions.len()],
                questions,
                current: 0,
            };
            if delivery == DeliveryClass::Live {
                actions = self
                    .input_actions(
                        conversation,
                        &owner_id,
                        &action_group,
                        &interaction,
                        &progress,
                    )
                    .await;
            } else {
                let token = self
                    .issue_action(
                        conversation,
                        &owner_id,
                        &action_group,
                        UiAction::BeginInput(interaction.clone()),
                    )
                    .await;
                actions.push(ActionButton {
                    label: "Answer".into(),
                    token,
                    style: ActionStyle::Primary,
                });
            }
            Some(progress)
        } else {
            actions = self
                .approval_actions(
                    conversation,
                    &owner_id,
                    &action_group,
                    &interaction,
                    request,
                )
                .await;
            None
        };
        let view = OutboundView {
            title: format!("{} · {}", request.title, session_label),
            subtitle: Some(format!(
                "Turn {} · {}",
                request.turn_id,
                if delivery == DeliveryClass::Live {
                    "current session"
                } else {
                    "background session"
                }
            )),
            body: input.as_ref().map_or_else(
                || request.detail.clone(),
                |progress| input_progress_body(progress, false),
            ),
            status: ViewStatus::Waiting,
            actions,
        };
        let message = self.send_view(conversation, &view).await?;
        self.interactions.pending.lock().await.insert(
            interaction,
            PendingInteractionView {
                rpc_id: request.rpc_id.clone(),
                message,
                view,
                action_group,
                input,
            },
        );
        Ok(())
    }

    async fn approval_actions(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        group_id: &str,
        interaction: &InteractionKey,
        request: &InteractionRequest,
    ) -> Vec<ActionButton> {
        let mut actions = Vec::new();
        for decision in &request.available_decisions {
            let token = self
                .issue_action(
                    conversation,
                    owner_id,
                    group_id,
                    UiAction::Resolve {
                        interaction: interaction.clone(),
                        decision: InteractionDecision {
                            rpc_id: request.rpc_id.clone(),
                            response: json!({"decision": decision}),
                        },
                    },
                )
                .await;
            actions.push(ActionButton {
                label: decision_label(decision),
                token,
                style: if decision == "accept" || decision == "acceptForSession" {
                    ActionStyle::Primary
                } else if decision == "decline" {
                    ActionStyle::Danger
                } else {
                    ActionStyle::Default
                },
            });
        }
        actions
    }

    async fn handle_action(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        token: &str,
        message: Option<&MessageRef>,
    ) -> Result<(), EngineError> {
        let generation = self.agent.generation();
        let binding_epoch = self.sessions.epoch(conversation).await;
        let action = self
            .interactions
            .actions
            .lock()
            .await
            .consume(token, conversation, owner_id, generation, binding_epoch)
            .map_err(|_| EngineError::InvalidAction)?;
        if let Some(message) = message.filter(|message| &message.conversation == conversation)
            && let Err(error) = self
                .channel(conversation.channel)?
                .disable_actions(message)
                .await
        {
            tracing::warn!(%error, message = %message.message_id, "failed to disable consumed IM actions");
        }
        match action {
            UiAction::Task(action) => self.run_task_action(conversation, owner_id, action).await?,
            UiAction::TaskBrowse(action) => {
                self.browse_tasks(conversation, owner_id, action).await?;
            }
            UiAction::Attach(session) => self.attach(conversation, owner_id, session).await?,
            UiAction::Stop {
                session_id,
                turn_id,
            } => {
                self.turns
                    .stop_actions
                    .lock()
                    .await
                    .remove(&(session_id.clone(), turn_id.clone()));
                self.agent.interrupt(&session_id, &turn_id).await?;
            }
            UiAction::Resolve {
                interaction,
                decision,
            } => {
                let selected = decision
                    .response
                    .get("decision")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned();
                let pending = self.interactions.pending.lock().await.remove(&interaction);
                self.agent.resolve_interaction(decision).await?;
                if let Some(pending) = pending {
                    self.show_local_approval_resolution(conversation, pending, &selected)
                        .await?;
                }
            }
            UiAction::BeginInput(interaction) => {
                self.show_input_question(conversation, owner_id, &interaction)
                    .await?;
            }
            UiAction::SelectInput {
                interaction,
                answer,
            } => {
                self.answer_input(conversation, owner_id, &interaction, &answer)
                    .await?;
            }
            UiAction::BeginCustomInput(interaction) => {
                self.begin_custom_input(conversation, &interaction).await?;
            }
            UiAction::SessionCommand {
                session_id,
                command,
            } => {
                if self.current_session(conversation).await? != session_id {
                    return Err(EngineError::InvalidAction);
                }
                self.run_session_command(conversation, owner_id, command)
                    .await?;
            }
            UiAction::Multiplexer(action) => {
                self.handle_multiplexer_action(conversation, owner_id, action)
                    .await?;
            }
        }
        Ok(())
    }

    async fn show_local_approval_resolution(
        &self,
        conversation: &ConversationRef,
        mut pending: PendingInteractionView,
        decision: &str,
    ) -> Result<(), EngineError> {
        let label = decision_label(decision);
        pending.view.body = if pending.view.body.is_empty() {
            format!("**Selected:** {label}")
        } else {
            format!("{}\n\n**Selected:** {label}", pending.view.body)
        };
        pending.view.status = if decision == "accept" || decision == "acceptForSession" {
            ViewStatus::Success
        } else {
            ViewStatus::Warning
        };
        pending.view.actions.clear();
        self.channel(conversation.channel)?
            .update(conversation, &pending.message, &pending.view)
            .await?;
        Ok(())
    }

    async fn input_actions(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        group_id: &str,
        interaction: &InteractionKey,
        progress: &InputProgress,
    ) -> Vec<ActionButton> {
        let Some(question) = progress.questions.get(progress.current) else {
            return Vec::new();
        };
        let mut actions = Vec::with_capacity(question.options.len() + 1);
        for option in &question.options {
            let token = self
                .issue_action(
                    conversation,
                    owner_id,
                    group_id,
                    UiAction::SelectInput {
                        interaction: interaction.clone(),
                        answer: option.label.clone(),
                    },
                )
                .await;
            actions.push(ActionButton {
                label: option.label.clone(),
                token,
                style: ActionStyle::Primary,
            });
        }
        let token = self
            .issue_action(
                conversation,
                owner_id,
                group_id,
                UiAction::BeginCustomInput(interaction.clone()),
            )
            .await;
        actions.push(ActionButton {
            label: if question.options.is_empty() {
                "Answer…".into()
            } else {
                "Other…".into()
            },
            token,
            style: ActionStyle::Default,
        });
        actions
    }

    async fn show_input_question(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        interaction: &InteractionKey,
    ) -> Result<(), EngineError> {
        let Some(mut pending) = self.interactions.pending.lock().await.remove(interaction) else {
            return Err(EngineError::InvalidAction);
        };
        let Some(progress) = pending.input.as_ref() else {
            return Err(EngineError::InvalidAction);
        };
        let action_group = Uuid::new_v4().simple().to_string();
        pending.view.body = input_progress_body(progress, false);
        pending.view.actions = self
            .input_actions(conversation, owner_id, &action_group, interaction, progress)
            .await;
        pending.view.status = ViewStatus::Waiting;
        self.channel(conversation.channel)?
            .update(conversation, &pending.message, &pending.view)
            .await?;
        pending.action_group = action_group;
        self.interactions
            .pending
            .lock()
            .await
            .insert(interaction.clone(), pending);
        Ok(())
    }

    async fn begin_custom_input(
        &self,
        conversation: &ConversationRef,
        interaction: &InteractionKey,
    ) -> Result<(), EngineError> {
        let Some(mut pending) = self.interactions.pending.lock().await.remove(interaction) else {
            return Err(EngineError::InvalidAction);
        };
        let Some(progress) = pending.input.as_ref() else {
            return Err(EngineError::InvalidAction);
        };
        pending.view.body = input_progress_body(progress, true);
        pending.view.actions.clear();
        self.channel(conversation.channel)?
            .update(conversation, &pending.message, &pending.view)
            .await?;
        self.interactions
            .pending
            .lock()
            .await
            .insert(interaction.clone(), pending);
        self.interactions
            .reply_modes
            .lock()
            .await
            .insert(conversation.clone(), interaction.clone());
        Ok(())
    }

    async fn answer_input(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        interaction: &InteractionKey,
        answer: &str,
    ) -> Result<(), EngineError> {
        let Some(mut pending) = self.interactions.pending.lock().await.remove(interaction) else {
            return Err(EngineError::InvalidAction);
        };
        let Some(progress) = pending.input.as_mut() else {
            return Err(EngineError::InvalidAction);
        };
        let Some(slot) = progress.answers.get_mut(progress.current) else {
            return Err(EngineError::InvalidAction);
        };
        *slot = Some(answer.to_owned());
        progress.current += 1;
        if progress.current < progress.questions.len() {
            let action_group = Uuid::new_v4().simple().to_string();
            pending.view.body = input_progress_body(progress, false);
            pending.view.actions = self
                .input_actions(conversation, owner_id, &action_group, interaction, progress)
                .await;
            self.channel(conversation.channel)?
                .update(conversation, &pending.message, &pending.view)
                .await?;
            pending.action_group = action_group;
            self.interactions
                .pending
                .lock()
                .await
                .insert(interaction.clone(), pending);
            return Ok(());
        }

        let response = input_response(progress);
        self.agent
            .resolve_interaction(InteractionDecision {
                rpc_id: pending.rpc_id.clone(),
                response,
            })
            .await?;
        pending.view.body = completed_input_body(progress);
        pending.view.status = ViewStatus::Success;
        pending.view.actions.clear();
        self.channel(conversation.channel)?
            .update(conversation, &pending.message, &pending.view)
            .await?;
        Ok(())
    }

    async fn resolve_external_interaction(
        &self,
        conversation: &ConversationRef,
        interaction: &InteractionKey,
    ) -> Result<(), EngineError> {
        let Some(mut pending) = self.interactions.pending.lock().await.remove(interaction) else {
            return Ok(());
        };
        self.revoke_action_group(&pending.action_group).await;
        self.interactions
            .reply_modes
            .lock()
            .await
            .retain(|_, pending| pending != interaction);
        pending.view.body = if pending.view.body.is_empty() {
            "**Resolved:** Outside Telegram".into()
        } else {
            format!("{}\n\n**Resolved:** Outside Telegram", pending.view.body)
        };
        pending.view.status = ViewStatus::Muted;
        pending.view.actions.clear();
        self.channel(conversation.channel)?
            .update(conversation, &pending.message, &pending.view)
            .await?;
        Ok(())
    }

    async fn resolve_external_request(
        &self,
        conversation: &ConversationRef,
        session_id: SessionId,
        request_id: String,
    ) -> Result<(), EngineError> {
        self.resolve_external_interaction(
            conversation,
            &InteractionKey {
                session_id,
                request_id,
            },
        )
        .await
    }

    async fn issue_action(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        group_id: &str,
        action: UiAction,
    ) -> String {
        let generation = self.agent.generation();
        let binding_epoch = self.sessions.epoch(conversation).await;
        self.interactions.actions.lock().await.issue(
            ActionScope::new(
                conversation.clone(),
                owner_id,
                generation,
                binding_epoch,
                group_id,
            ),
            action,
        )
    }

    async fn revoke_action_group(&self, group_id: &str) {
        self.interactions
            .actions
            .lock()
            .await
            .revoke_group(group_id);
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

fn multiplexer_session_contains(session: &MultiplexerSession, current: Option<&SessionId>) -> bool {
    session
        .windows
        .iter()
        .any(|window| multiplexer_window_contains(window, current))
}

fn multiplexer_window_contains(window: &MultiplexerWindow, current: Option<&SessionId>) -> bool {
    window
        .panes
        .iter()
        .any(|pane| current.is_some_and(|current| pane.codex_session.as_ref() == Some(current)))
}

fn multiplexer_root_body(
    snapshot: &MultiplexerSnapshot,
    current: Option<&SessionId>,
) -> (String, usize, usize) {
    let window_count = snapshot
        .sessions
        .iter()
        .map(|session| session.windows.len())
        .sum();
    let pane_count = snapshot
        .sessions
        .iter()
        .flat_map(|session| &session.windows)
        .map(|window| window.panes.len())
        .sum();
    let body = snapshot
        .sessions
        .iter()
        .map(|session| {
            let panes = session
                .windows
                .iter()
                .map(|window| window.panes.len())
                .sum::<usize>();
            markdown_quote(&format!(
                "**{}**{}\n{} {} · {panes} {}",
                session.name,
                if multiplexer_session_contains(session, current) {
                    " · 📎 **Attached**"
                } else {
                    ""
                },
                session.windows.len(),
                plural(session.windows.len(), "window", "windows"),
                plural(panes, "pane", "panes")
            ))
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    (body, window_count, pane_count)
}

fn multiplexer_session_body(session: &MultiplexerSession, current: Option<&SessionId>) -> String {
    session
        .windows
        .iter()
        .map(|window| {
            markdown_quote(&format!(
                "**{} · {}**{}\n{} {}",
                window.index,
                window.name,
                if multiplexer_window_contains(window, current) {
                    " · 📎 **Attached**"
                } else {
                    ""
                },
                window.panes.len(),
                plural(window.panes.len(), "pane", "panes")
            ))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn multiplexer_window_body(window: &MultiplexerWindow, current: Option<&SessionId>) -> String {
    window
        .panes
        .iter()
        .map(|pane| multiplexer_pane_body(pane, current))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn multiplexer_pane_body(pane: &MultiplexerPane, current: Option<&SessionId>) -> String {
    let attached = current.is_some_and(|current| pane.codex_session.as_ref() == Some(current));
    let state = if attached {
        "📎 Attached"
    } else if pane.codex_session.is_some() {
        "Codex"
    } else if is_shell_command(&pane.current_command) {
        "Idle shell"
    } else {
        "Busy"
    };
    markdown_quote(&format!(
        "**{} · {}**{} · {state}\n📁 `{}`",
        pane.index,
        pane.current_command,
        if pane.active { " · Active" } else { "" },
        display_workspace(Some(&pane.cwd))
    ))
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

fn is_shell_command(command: &str) -> bool {
    Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                "bash" | "dash" | "elvish" | "fish" | "ksh" | "nu" | "sh" | "tcsh" | "zsh"
            )
        })
}

fn command_menu(attached: bool) -> CommandMenu {
    let mut commands = [
        ("sessions", "Browse running sessions"),
        ("rmux", "Manage rmux workspaces"),
        ("cancel", "Cancel pending input"),
        ("help", "Show available commands"),
    ]
    .into_iter()
    .map(|(name, description)| ChannelCommand::new(name, description))
    .collect::<Vec<_>>();
    if attached {
        commands.splice(
            2..2,
            [
                ("current", "Show the attached session"),
                ("history", "Show recent conversation history"),
                ("queue", "Show queued follow-up messages"),
                ("stop", "Stop the active turn"),
                ("detach", "Detach the current session"),
                ("compact", "Compact the session context"),
                ("fork", "Fork and attach a copy"),
                ("fast", "Toggle Fast mode"),
                ("clear", "Start a fresh session"),
                ("exit", "Detach the session"),
                ("diff", "Show Git changes"),
                ("rename", "Rename the session"),
                ("model", "Show or change the model"),
                ("reasoning", "Show or change reasoning effort"),
                ("skills", "List available skills"),
                ("plan", "Enter plan mode"),
                ("goal", "Show or manage the goal"),
                ("review", "Review uncommitted changes"),
                ("status", "Show detailed session status"),
                ("mcp", "Show MCP server status"),
            ]
            .into_iter()
            .map(|(name, description)| ChannelCommand::new(name, description).contextual()),
        );
    }
    CommandMenu::new(commands)
}

fn history_views(
    agent_name: &str,
    session_label: &str,
    history: &HistoryPage,
    presentation: HistoryPresentation,
) -> Vec<OutboundView> {
    let turn_count = history.turns.len();
    let mut body = if turn_count == 0 {
        "No conversation history yet.".to_owned()
    } else {
        format!(
            "Showing {turn_count} recent {}.",
            if turn_count == 1 { "turn" } else { "turns" }
        )
    };
    if history.older_cursor.is_some() {
        body.push_str("\n\nEarlier turns: /history older");
    }
    if history.newer_cursor.is_some() {
        body.push_str("\nNewer turns: /history newer");
    }

    let subtitle = match presentation {
        HistoryPresentation::Attached => "Attached",
        HistoryPresentation::History => "History",
    };
    let mut views = vec![OutboundView {
        title: format!("{agent_name} · {session_label}"),
        subtitle: Some(subtitle.into()),
        body,
        status: ViewStatus::Info,
        actions: Vec::new(),
    }];
    views.extend(
        history
            .turns
            .iter()
            .map(|turn| history_turn_view(agent_name, turn)),
    );
    views
}

fn history_turn_view(agent_name: &str, turn: &TurnSummary) -> OutboundView {
    let body = turn_conversation_body(
        agent_name,
        turn.user_text.as_deref(),
        turn.agent_text.as_deref(),
    );

    OutboundView {
        title: format!("{agent_name} · Turn {}", short_identifier(&turn.id)),
        subtitle: Some(turn_status_label(&turn.status).into()),
        body,
        status: match turn.status {
            TurnStatus::Completed => ViewStatus::Success,
            TurnStatus::Failed => ViewStatus::Error,
            TurnStatus::Interrupted => ViewStatus::Warning,
            TurnStatus::InProgress | TurnStatus::Unknown => ViewStatus::Running,
        },
        actions: Vec::new(),
    }
}

fn live_turn_body(agent_name: &str, buffer: &TurnBuffer, delivery: DeliveryClass) -> String {
    let mut body = turn_conversation_body(
        agent_name,
        Some(&buffer.user_text),
        Some(&buffer.agent_text),
    );
    if delivery == DeliveryClass::Draining {
        body.push_str("\n\nThis is a background session after switching.");
    }
    body
}

fn live_turn_view(
    agent_name: &str,
    session_label: &str,
    turn_id: &str,
    buffer: &TurnBuffer,
    delivery: DeliveryClass,
) -> OutboundView {
    OutboundView {
        title: format!("{agent_name} · {session_label}"),
        subtitle: Some(format!(
            "{} {} · {}",
            if delivery == DeliveryClass::Draining {
                "Background turn"
            } else {
                "Turn"
            },
            short_identifier(turn_id),
            live_turn_status_label(buffer)
        )),
        body: live_turn_body(agent_name, buffer, delivery),
        status: if delivery == DeliveryClass::Draining {
            ViewStatus::Background
        } else {
            turn_view_status(&buffer.status)
        },
        actions: Vec::new(),
    }
}

fn background_completion_body(status: &TurnStatus, error: Option<&str>) -> String {
    let outcome = match status {
        TurnStatus::Completed => "completed",
        TurnStatus::Failed => "failed",
        TurnStatus::Interrupted => "was interrupted",
        TurnStatus::InProgress | TurnStatus::Unknown => "finished",
    };
    let mut body =
        format!("This turn {outcome} in a session that is not attached to this IM conversation.");
    if let Some(error) = error.filter(|error| !error.trim().is_empty()) {
        body.push_str(&format!("\n\n**Error**\n\n{}", markdown_quote(error)));
    }
    body
}

const fn turn_view_status(status: &TurnStatus) -> ViewStatus {
    match status {
        TurnStatus::Completed => ViewStatus::Success,
        TurnStatus::Failed => ViewStatus::Error,
        TurnStatus::Interrupted => ViewStatus::Warning,
        TurnStatus::InProgress | TurnStatus::Unknown => ViewStatus::Running,
    }
}

fn turn_conversation_body(
    agent_name: &str,
    user_text: Option<&str>,
    agent_text: Option<&str>,
) -> String {
    let mut sections = Vec::new();
    if let Some(user_text) = user_text.filter(|text| !text.trim().is_empty()) {
        sections.push(format!("**👤 You**\n\n{}", markdown_quote(user_text)));
    }
    if let Some(agent_text) = agent_text.filter(|text| !text.trim().is_empty()) {
        sections.push(format!(
            "**🤖 {agent_name}**\n\n{}",
            markdown_quote(agent_text)
        ));
    }
    if sections.is_empty() {
        "No text content.".to_owned()
    } else {
        sections.join("\n\n")
    }
}

fn markdown_quote(text: &str) -> String {
    text.trim()
        .lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn short_identifier(value: &str) -> &str {
    &value[..value.floor_char_boundary(8)]
}

fn turn_status_label(status: &TurnStatus) -> &'static str {
    match status {
        TurnStatus::InProgress => "In progress",
        TurnStatus::Completed => "Completed",
        TurnStatus::Interrupted => "Interrupted",
        TurnStatus::Failed => "Failed",
        TurnStatus::Unknown => "Unknown",
    }
}

fn live_turn_status_label(buffer: &TurnBuffer) -> String {
    let elapsed = buffer.elapsed().map(format_elapsed);
    match (&buffer.status, elapsed) {
        (TurnStatus::InProgress | TurnStatus::Unknown, Some(elapsed)) => {
            format!("Working {elapsed}")
        }
        (TurnStatus::Completed, Some(elapsed)) => format!("Completed in {elapsed}"),
        (TurnStatus::Interrupted, Some(elapsed)) => format!("Interrupted after {elapsed}"),
        (TurnStatus::Failed, Some(elapsed)) => format!("Failed after {elapsed}"),
        (status, None) => turn_status_label(status).to_owned(),
    }
}

fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn display_workspace(workspace: Option<&str>) -> String {
    let Some(workspace) = workspace else {
        return "unknown workspace".to_owned();
    };
    let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) else {
        return workspace.to_owned();
    };
    let Ok(relative) = Path::new(workspace).strip_prefix(Path::new(&home)) else {
        return workspace.to_owned();
    };
    if relative.as_os_str().is_empty() {
        "~".to_owned()
    } else {
        format!("~/{}", relative.display())
    }
}

fn session_status_label(status: &SessionStatus) -> (&'static str, &'static str) {
    match status {
        SessionStatus::Active => ("🟢", "Active"),
        SessionStatus::Idle => ("🟡", "Idle"),
        SessionStatus::NotLoaded => ("⚫", "Not loaded"),
        SessionStatus::SystemError => ("⚠️", "System error"),
        SessionStatus::Offline => ("🔴", "Offline"),
        SessionStatus::Unknown => ("⚪", "Unknown"),
    }
}

fn session_title(session: &SessionSummary) -> &str {
    session
        .name
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .or_else(|| {
            session
                .preview
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
        })
        .unwrap_or("Untitled")
}

fn session_display_label(session: &SessionSummary) -> String {
    format!("{} · {}", session_title(session), session.id.short())
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

fn input_questions(request: &InteractionRequest) -> Vec<InputQuestion> {
    let mut questions = request
        .payload
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, question)| {
            let id = question.get("id").and_then(Value::as_str)?.to_owned();
            let header = question
                .get("header")
                .and_then(Value::as_str)
                .filter(|header| !header.trim().is_empty())
                .map_or_else(|| format!("Question {}", index + 1), str::to_owned);
            let prompt = question
                .get("question")
                .and_then(Value::as_str)
                .filter(|prompt| !prompt.trim().is_empty())
                .unwrap_or(&request.detail)
                .to_owned();
            let options = question
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|option| {
                    Some(InputOption {
                        label: option.get("label")?.as_str()?.to_owned(),
                        description: option
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    })
                })
                .collect();
            Some(InputQuestion {
                id,
                header,
                question: prompt,
                options,
                secret: question
                    .get("isSecret")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect::<Vec<_>>();
    if questions.is_empty() {
        questions.push(InputQuestion {
            id: "value".into(),
            header: "Input".into(),
            question: if request.detail.trim().is_empty() {
                "Please provide an answer.".into()
            } else {
                request.detail.clone()
            },
            options: Vec::new(),
            secret: false,
        });
    }
    questions
}

fn input_progress_body(progress: &InputProgress, awaiting_custom: bool) -> String {
    let mut sections = Vec::new();
    let answered = progress
        .questions
        .iter()
        .zip(&progress.answers)
        .enumerate()
        .filter_map(|(index, (question, answer))| {
            answer.as_ref().map(|answer| {
                format!(
                    "{}. **{}:** {}",
                    index + 1,
                    question.header,
                    displayed_answer(question, answer)
                )
            })
        })
        .collect::<Vec<_>>();
    if !answered.is_empty() {
        sections.push(format!("**Answers**\n{}", answered.join("\n")));
    }
    if let Some(question) = progress.questions.get(progress.current) {
        let mut current = format!(
            "**Question {} of {} · {}**\n{}",
            progress.current + 1,
            progress.questions.len(),
            question.header,
            question.question
        );
        if !question.options.is_empty() {
            let options = question
                .options
                .iter()
                .map(|option| {
                    if option.description.trim().is_empty() {
                        format!("- **{}**", option.label)
                    } else {
                        format!("- **{}** — {}", option.label, option.description)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            current.push_str(&format!("\n\n{options}"));
        }
        current.push_str(if awaiting_custom {
            "\n\nReply with your custom answer. Use `/cancel` to return to the choices."
        } else if question.options.is_empty() {
            "\n\nChoose Answer… to type your response."
        } else {
            "\n\nChoose an option below, or choose Other… to type a custom answer."
        });
        sections.push(current);
    }
    sections.join("\n\n")
}

fn completed_input_body(progress: &InputProgress) -> String {
    let answers = progress
        .questions
        .iter()
        .zip(&progress.answers)
        .enumerate()
        .filter_map(|(index, (question, answer))| {
            answer.as_ref().map(|answer| {
                format!(
                    "{}. **{}:** {}",
                    index + 1,
                    question.header,
                    displayed_answer(question, answer)
                )
            })
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("**Answered**\n{answers}")
}

fn displayed_answer<'a>(question: &InputQuestion, answer: &'a str) -> &'a str {
    if question.secret { "[hidden]" } else { answer }
}

fn input_response(progress: &InputProgress) -> Value {
    let answers = progress
        .questions
        .iter()
        .zip(&progress.answers)
        .filter_map(|(question, answer)| {
            answer
                .as_ref()
                .map(|answer| (question.id.clone(), json!({"answers": [answer]})))
        })
        .collect::<serde_json::Map<_, _>>();
    json!({"answers": answers})
}

fn decision_label(decision: &str) -> String {
    match decision {
        "accept" => "Allow once",
        "acceptForSession" => "Allow for session",
        "decline" => "Decline",
        "cancel" => "Cancel",
        other => other,
    }
    .to_owned()
}
