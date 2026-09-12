//! IM session commands: coordinate session services and presentation.
use super::{
    ActionButton, ActionStyle, AgentCommand, AgentError, AttachOutcome, ChannelCommand,
    ConversationRef, DeliveryClass, Engine, EngineError, HistoryPage, HistoryPresentation, Instant,
    OutboundView, ParsedInput, PendingSessionInput, SessionCommand, SessionCommandChoice,
    SessionId, TurnBuffer, TurnStatus, UiAction, Uuid, ViewStatus, command_menu, display_workspace,
    history_views, markdown_quote, session_display_label, session_status_label, session_title,
};

const SESSION_COMMAND_HELP: &[(&str, &str)] = &[
    (
        "/fast [on|off]",
        "Show or change fast mode for subsequent turns.",
    ),
    (
        "/clear [name]",
        "Start a fresh session, optionally with a name.",
    ),
    ("/exit", "End the IM connection to the current session."),
    ("/diff", "Show workspace changes."),
    ("/rename <name>", "Change the current session name."),
    ("/compact", "Compact the current session context."),
    (
        "/fork",
        "Create a new session from the current conversation.",
    ),
    ("/model [id]", "Show available models or select a model."),
    ("/reasoning [effort]", "Show or change reasoning effort."),
    ("/skills", "List skills available to the agent."),
    (
        "/plan [prompt|off]",
        "Enter plan mode with an optional prompt, or leave plan mode.",
    ),
    (
        "/goal [objective|pause|resume|clear]",
        "Show, set, pause, resume, or clear the session goal.",
    ),
    ("/review", "Start a review of workspace changes."),
    ("/status", "Show session settings and usage."),
    ("/mcp", "Show MCP server status."),
];

impl Engine {
    pub(super) async fn steer_current(
        &self,
        conversation: &ConversationRef,
        text: &str,
    ) -> Result<(), EngineError> {
        let session = self.current_session(conversation).await?;
        if self.agent.session_access(&session).await.can_write() {
            let turn = self.turns.active_turn(&session).await.ok_or_else(|| {
                EngineError::InvalidInput(
                    "No active turn; send an ordinary message instead.".into(),
                )
            })?;
            self.operations.send(&session, text, Some(&turn)).await?;
        } else {
            self.show_read_only_notice(conversation).await?;
        }
        Ok(())
    }

    pub(super) async fn show_help(
        &self,
        conversation: &ConversationRef,
    ) -> Result<(), EngineError> {
        let body = self.available_commands(conversation).await;
        let body = if self.tasks.backend.is_some() {
            let mut body = format!(
                "{body}\n**/dashboard** — Browse projects; click a project to open its board."
            );
            if self.sessions.current(conversation).await.is_some() {
                body.push_str("\n**/board** — Current session's task board\n**/jobs** — Current session's jobs\n**/tasks [job-id]** — List tasks for the current session or a job.\n**/task <id>** — Read a task and its Markdown details.\n**/inboxes** — Current project's human queue\n**/inbox <content>** — Append a human requirement");
            }
            body
        } else {
            body.clone()
        };
        self.send_view(conversation, &OutboundView::text("Agentix commands", body))
            .await?;
        Ok(())
    }

    pub(super) async fn show_invalid_command(
        &self,
        conversation: &ConversationRef,
        error: &str,
    ) -> Result<(), EngineError> {
        let commands = self.available_commands(conversation).await;
        self.send_view(
            conversation,
            &OutboundView {
                sections: Vec::new(),
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

    pub(super) async fn available_commands(&self, conversation: &ConversationRef) -> String {
        let session = self.sessions.current(conversation).await;
        let read_only = if let Some(session) = &session {
            !self.agent.session_access(session).await.can_write()
        } else {
            false
        };
        let mut commands = vec![
            ("/help", "Show available commands and their purpose."),
            (
                "/sessions [backend]",
                "List existing sessions, optionally filtered by backend.",
            ),
            (
                "/rmux [backend]",
                "Browse terminal sessions and create or attach an agent session.",
            ),
            (
                "/attach <thread-id>",
                "Connect this conversation to an existing session.",
            ),
            ("/cancel", "Cancel the pending command input."),
        ];
        if session.is_some() {
            commands.extend([
                (
                    "/current",
                    "Show the session attached to this conversation.",
                ),
                (
                    "/history [recent|older|newer]",
                    "Browse the attached session history.",
                ),
                ("/detach", "Disconnect this conversation from its session."),
            ]);
            if !read_only {
                commands.extend([
                    (
                        "/queue [resume|clear]",
                        "Show, resume, or clear queued prompts.",
                    ),
                    ("/stop", "Interrupt the active turn."),
                    (
                        "/steer <text>",
                        "Steer the active turn with additional instructions.",
                    ),
                ]);
                if self.agent.capabilities().session_control {
                    commands.extend_from_slice(SESSION_COMMAND_HELP);
                }
            }
        }
        let mut lines = Vec::new();
        for (usage, description) in commands {
            let name = usage
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_start_matches('/');
            if let Some(session) = &session
                && matches!(
                    crate::parse_input(&format!("/{name}")),
                    Ok(ParsedInput::Command(AgentCommand::Session(_)))
                )
                && name != "exit"
                && !self.agent.supports_command(session, name).await
            {
                continue;
            }
            lines.push(format!("**{usage}** — {description}"));
        }
        if read_only {
            lines.push("\nThis session is connected read-only.".into());
        }
        lines.join("\n")
    }

    pub(super) async fn show_sessions(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
    ) -> Result<(), EngineError> {
        self.show_sessions_filtered(conversation, owner_id, None)
            .await
    }

    pub(super) async fn show_sessions_filtered(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        kind: Option<crate::AgentKind>,
    ) -> Result<(), EngineError> {
        let page = self
            .sessions
            .filtered_sessions(self.agent.as_ref(), kind)
            .await?;
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
        for (index, session) in page.into_iter().enumerate() {
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
                sections: Vec::new(),
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

    pub(super) async fn attach(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        session_id: SessionId,
    ) -> Result<(), EngineError> {
        let session_id = self.agent.canonical_session(&session_id).await?;
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
        self.sessions
            .cache_session_summary(self.agent.as_ref(), &session_id)
            .await;
        let history = match self.operations.history(&session_id, None, 1).await {
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
        self.sessions
            .remember_history_cursors(conversation, &history)
            .await;
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

    pub(super) async fn show_attach_failure(
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
                sections: Vec::new(),
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

    pub(super) async fn show_read_only_notice(
        &self,
        conversation: &ConversationRef,
    ) -> Result<(), EngineError> {
        let session = self.current_session(conversation).await?;
        let name = self.agent.session_display_name(&session);
        let body = if matches!(
            self.agent.session_access(&session).await,
            crate::SessionAccess::ReadOnly(crate::ReadOnlyReason::OwnedByOtherProcess)
        ) {
            format!(
                "This session is connected read-only because another {name} process owns it. Use /history to read its latest content, or the original {name} session to send messages and make changes."
            )
        } else {
            format!(
                "The {name} session is currently unavailable for changes. Check the original {name} terminal and reconnect."
            )
        };
        self.send_view(
            conversation,
            &OutboundView {
                sections: Vec::new(),
                title: format!("{name} · Read-only session"),
                subtitle: None,
                body,
                status: ViewStatus::Info,
                actions: Vec::new(),
            },
        )
        .await?;
        Ok(())
    }

    pub(super) async fn show_current(
        &self,
        conversation: &ConversationRef,
    ) -> Result<(), EngineError> {
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
                sections: Vec::new(),
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

    pub(super) async fn show_history(
        &self,
        conversation: &ConversationRef,
        cursor: Option<String>,
    ) -> Result<(), EngineError> {
        let session = self.current_session(conversation).await?;
        let history = self.operations.history(&session, cursor, 5).await?;
        self.sessions
            .remember_history_cursors(conversation, &history)
            .await;
        self.send_history_views(
            conversation,
            &session,
            &history,
            HistoryPresentation::History,
        )
        .await?;
        Ok(())
    }

    pub(super) async fn send_history_views(
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
                && !self.agent.session_access(session_id).await.can_write()
            {
                if matches!(
                    self.agent.session_access(session_id).await,
                    crate::SessionAccess::ReadOnly(crate::ReadOnlyReason::OwnedByOtherProcess)
                ) {
                    let name = self.agent.session_display_name(session_id);
                    overview.body.push_str(&format!("\n\nConnected read-only: another {name} process owns this session. Latest content is checked every 10 seconds; sending messages, stopping turns, and changing settings require the original {name} session."));
                } else {
                    overview.body.push_str(
                        "\n\nThe original terminal is currently unavailable for changes.",
                    );
                }
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

    pub(super) async fn detach(&self, conversation: &ConversationRef) -> Result<(), EngineError> {
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
        let session = self.sessions.commit_detach(conversation, active).await?;
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
    pub(super) async fn run_session_command(
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
                        sections: Vec::new(),
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
        if !self.agent.session_access(&session).await.can_write() {
            return self.show_read_only_notice(conversation).await;
        }
        if !self
            .agent
            .session_capabilities(&session)
            .await
            .supports(command.capability())
        {
            return self
                .send_view(
                    conversation,
                    &OutboundView::text(
                        "Command unavailable",
                        "This command is not supported by the attached session.",
                    ),
                )
                .await
                .map(|_| ());
        }
        if matches!(command, SessionCommand::Rename(None)) {
            self.interactions.session_inputs.lock().await.insert(
                conversation.clone(),
                PendingSessionInput::Rename(session.clone()),
            );
            self.send_view(
                conversation,
                &OutboundView::text(
                    format!("{} · Rename", self.agent.session_display_name(&session)),
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
                    sections: Vec::new(),
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
        let Some(_session_control) = self.agent.session_control() else {
            self.send_view(
                conversation,
                &OutboundView {
                    sections: Vec::new(),
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
        let result = match self.operations.command(&session, command).await {
            Ok(result) => result,
            Err(error) => {
                self.send_view(
                    conversation,
                    &OutboundView {
                        sections: Vec::new(),
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
                sections: Vec::new(),
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

    pub(super) async fn session_command_actions(
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

    pub(super) async fn bind_subscribed_session(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        old_active: bool,
    ) -> Result<(), EngineError> {
        if let Some(previous) = self.sessions.current(conversation).await {
            self.clear_session_stop_actions(&previous).await?;
        }
        self.clear_session_stop_actions(session_id).await?;
        let transition = self
            .sessions
            .commit_binding(conversation, session_id, old_active)
            .await?;
        let outcome = transition.outcome;
        let persisted_previous = transition.persisted_previous;
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

    pub(super) async fn apply_binding_effects(
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
                        sections: Vec::new(),
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
        self.sync_command_menu_best_effort(conversation, true).await;
    }

    pub(super) async fn update_command_menu_best_effort(
        &self,
        conversation: &ConversationRef,
        attached: bool,
    ) {
        if let Err(error) = self.update_command_menu(conversation, attached).await {
            tracing::warn!(%error, ?conversation, attached, "failed to update the IM command menu");
        }
    }

    pub(super) async fn update_command_menu(
        &self,
        conversation: &ConversationRef,
        attached: bool,
    ) -> Result<(), EngineError> {
        let channel = self.channel(conversation.channel)?;
        let menu = self.conversation_command_menu(conversation, attached).await;
        channel.set_command_menu(conversation, &menu).await?;
        Ok(())
    }

    pub(super) async fn sync_command_menu_best_effort(
        &self,
        conversation: &ConversationRef,
        attached: bool,
    ) {
        let result = async {
            let channel = self.channel(conversation.channel)?;
            let menu = self.conversation_command_menu(conversation, attached).await;
            channel.sync_command_menu(conversation, &menu).await?;
            Ok::<(), EngineError>(())
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, ?conversation, attached, "failed to synchronize the IM command menu");
        }
    }

    async fn conversation_command_menu(
        &self,
        conversation: &ConversationRef,
        attached: bool,
    ) -> crate::CommandMenu {
        let mut menu = command_menu(attached && self.agent.capabilities().session_control);
        if attached && let Some(session) = self.sessions.current(conversation).await {
            let mut commands = Vec::new();
            for command in menu.commands.drain(..) {
                let session_command = crate::parse_input(&format!("/{}", command.name)).ok();
                if !matches!(
                    session_command,
                    Some(crate::ParsedInput::Command(crate::AgentCommand::Session(_)))
                ) || command.name == "exit"
                    || self.agent.supports_command(&session, &command.name).await
                {
                    commands.push(command);
                }
            }
            menu.commands = commands;
        }
        if attached
            && let Some(session) = self.sessions.current(conversation).await
            && !self.agent.session_access(&session).await.can_write()
        {
            menu.commands.retain(|command| {
                matches!(
                    command.name.as_str(),
                    "sessions" | "rmux" | "current" | "history" | "detach" | "cancel" | "help"
                )
            });
        }
        if self.tasks.backend.is_some() {
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
        menu
    }

    pub(super) async fn stop_current(
        &self,
        conversation: &ConversationRef,
    ) -> Result<(), EngineError> {
        let session = self.current_session(conversation).await?;
        if !self.agent.session_access(&session).await.can_write() {
            return self.show_read_only_notice(conversation).await;
        }
        let turn = self
            .turns
            .active_turn(&session)
            .await
            .ok_or_else(|| EngineError::InvalidInput("the current session is idle".into()))?;
        self.operations.stop(&session, &turn).await?;
        Ok(())
    }

    pub(super) async fn send_prompt(
        &self,
        conversation: &ConversationRef,
        prompt: &str,
    ) -> Result<(), EngineError> {
        let session = self.current_session(conversation).await?;
        if !self.agent.session_access(&session).await.can_write() {
            return self.show_read_only_notice(conversation).await;
        }
        let active = self.turns.active_turn(&session).await;
        if let Some(turn) = active {
            if self
                .agent
                .session_capabilities(&session)
                .await
                .supports(crate::SessionCapability::Queue)
                && let Some(queue) = self.agent.queued_prompts()
            {
                return self
                    .queue_prompt(queue, conversation, &session, prompt)
                    .await;
            }
            let turn_id = self.operations.send(&session, prompt, Some(&turn)).await?;
            self.turns.set_active(session, turn_id).await;
            return Ok(());
        }
        let turn_id = self.operations.send(&session, prompt, None).await?;
        self.turns
            .set_active(session.clone(), turn_id.clone())
            .await;
        self.turns.buffers.lock().await.insert(
            (session.clone(), turn_id.clone()),
            TurnBuffer {
                user_text: prompt.to_owned(),
                agent_text: String::new(),
                output_items: Vec::new(),
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

    pub(super) async fn queue_prompt(
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
                sections: Vec::new(),
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

    pub(super) async fn show_queue(
        &self,
        conversation: &ConversationRef,
    ) -> Result<(), EngineError> {
        let session = self.current_session(conversation).await?;
        if !self.agent.session_access(&session).await.can_write() {
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
        let mut body = if prompts.is_empty() {
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
        if let Some(status) = queue.queue_status(&session).await? {
            body = format!("{status}\n\n{body}");
        }
        let mut actions = Vec::new();
        if self
            .agent
            .session_capabilities(&session)
            .await
            .supports(crate::SessionCapability::QueueControl)
            && let Some(owner) = self
                .interactions
                .owners
                .lock()
                .await
                .get(conversation)
                .cloned()
        {
            let group = uuid::Uuid::new_v4().to_string();
            for (label, action) in [("Resume queue", "resume"), ("Clear queue", "clear")] {
                let token = self
                    .issue_action(
                        conversation,
                        &owner,
                        &group,
                        UiAction::QueueControl {
                            session_id: session.clone(),
                            action: action.into(),
                        },
                    )
                    .await;
                actions.push(ActionButton {
                    label: label.into(),
                    token,
                    style: ActionStyle::Default,
                });
            }
        }
        self.send_view(
            conversation,
            &OutboundView {
                sections: Vec::new(),
                title: format!("{} · {session_label}", self.agent.display_name()),
                subtitle: Some(format!(
                    "Queue · {count} {}",
                    if count == 1 { "message" } else { "messages" }
                )),
                body,
                status: ViewStatus::Info,
                actions,
            },
        )
        .await?;
        Ok(())
    }

    pub(super) async fn control_queue(
        &self,
        conversation: &ConversationRef,
        action: &str,
    ) -> Result<(), EngineError> {
        let session = self.current_session(conversation).await?;
        if !self.agent.session_access(&session).await.can_write() {
            return self.show_read_only_notice(conversation).await;
        }
        let queue = self
            .agent
            .queued_prompts()
            .ok_or_else(|| EngineError::InvalidInput("Queue unavailable".into()))?;
        queue.control_queue(&session, action).await?;
        self.show_queue(conversation).await
    }

    pub(super) async fn current_session(
        &self,
        conversation: &ConversationRef,
    ) -> Result<SessionId, EngineError> {
        self.sessions
            .current(conversation)
            .await
            .ok_or(EngineError::NoCurrentSession)
    }

    pub(super) async fn session_label(&self, session_id: &SessionId) -> String {
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

    pub(super) async fn attach_action(
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
}
