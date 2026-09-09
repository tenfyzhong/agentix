//! Workspace browser and mutation orchestration.
use super::{
    ActionButton, ActionStyle, ConversationRef, Engine, EngineError, MultiplexerMutation,
    MultiplexerSession, MultiplexerSnapshot, MultiplexerTarget, MultiplexerUiAction,
    MultiplexerWindow, OutboundView, PaneSplitDirection, SessionId, UiAction, Uuid, ViewStatus,
    is_shell_command, multiplexer_root_body, multiplexer_session_body,
    multiplexer_session_contains, multiplexer_window_body, multiplexer_window_contains, plural,
};

impl Engine {
    pub(super) async fn select_multiplexer_backend(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        kind: &str,
    ) -> Result<(), EngineError> {
        let kind = match kind {
            "claude" => crate::AgentKind::Claude,
            "codex" => crate::AgentKind::Codex,
            "pi" => crate::AgentKind::Pi,
            "omp" => crate::AgentKind::Omp,
            _ => {
                return Err(EngineError::InvalidInput(
                    "Use /rmux codex, /rmux pi, /rmux omp, or /rmux claude".into(),
                ));
            }
        };
        if !self.agent.workspace_backends().contains(&kind) {
            return Err(EngineError::InvalidInput(
                "Backend is not configured for rmux".into(),
            ));
        }
        self.rmux
            .selected
            .lock()
            .await
            .insert(conversation.clone(), kind);
        self.show_multiplexer_root(conversation, owner_id).await
    }

    pub(super) async fn multiplexer_agent_name(
        &self,
        conversation: &ConversationRef,
    ) -> &'static str {
        self.multiplexer_backend(conversation)
            .await
            .map_or(self.agent.display_name(), crate::AgentKind::display_name)
    }

    pub(super) async fn multiplexer_backend(
        &self,
        conversation: &ConversationRef,
    ) -> Option<crate::AgentKind> {
        self.rmux.selected.lock().await.get(conversation).copied()
    }

    pub(super) async fn ensure_multiplexer_backend(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
    ) -> Result<bool, EngineError> {
        let backends = self.agent.workspace_backends();
        if self.multiplexer_backend(conversation).await.is_none() {
            let attached = self
                .sessions
                .current(conversation)
                .await
                .and_then(|id| crate::SessionKey::decode(&id))
                .map(|key| key.agent);
            if let Some(kind) = attached
                .filter(|kind| backends.contains(kind))
                .or_else(|| (backends.len() == 1).then(|| backends[0]))
            {
                self.rmux
                    .selected
                    .lock()
                    .await
                    .insert(conversation.clone(), kind);
            } else if backends.len() > 1 {
                let mut view = OutboundView::text("Terminal · rmux", "Choose the agent to launch.");
                for kind in backends {
                    let token = self
                        .issue_action(
                            conversation,
                            owner_id,
                            "rmux-backends",
                            UiAction::MultiplexerBackend(kind),
                        )
                        .await;
                    view.actions.push(ActionButton {
                        label: kind.display_name().into(),
                        token,
                        style: ActionStyle::Default,
                    });
                }
                self.send_view(conversation, &view).await?;
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) async fn show_multiplexer_root(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
    ) -> Result<(), EngineError> {
        if !self
            .ensure_multiplexer_backend(conversation, owner_id)
            .await?
        {
            return Ok(());
        }
        let Some(workspace) = self.rmux.runtime(self.agent.as_ref(), conversation).await else {
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

    pub(super) async fn multiplexer_root_actions(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        snapshot: &MultiplexerSnapshot,
        current: Option<&SessionId>,
    ) -> Vec<ActionButton> {
        let mut actions = Vec::new();
        let action_group = Uuid::new_v4().simple().to_string();
        let default_directory = self
            .rmux
            .default_directory(self.agent.as_ref(), conversation)
            .await;
        for session in &snapshot.sessions {
            let attached = multiplexer_session_contains(session, current);
            let token = self
                .issue_action(
                    conversation,
                    owner_id,
                    &action_group,
                    UiAction::Multiplexer(
                        self.multiplexer_backend(conversation).await,
                        MultiplexerUiAction::ShowSession {
                            session_id: session.id.clone(),
                        },
                    ),
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
                        name: self
                            .multiplexer_backend(conversation)
                            .await
                            .map_or("codex", |kind| kind.as_str())
                            .into(),
                        cwd: default_directory,
                    },
                    launch_agent: true,
                }),
            ),
            ("Refresh", MultiplexerUiAction::ShowRoot),
        ] {
            let token = self
                .issue_action(
                    conversation,
                    owner_id,
                    &action_group,
                    UiAction::Multiplexer(self.multiplexer_backend(conversation).await, action),
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

    pub(super) async fn show_multiplexer_session(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        session_id: &str,
    ) -> Result<(), EngineError> {
        let snapshot = self.required_multiplexer_snapshot(conversation).await?;
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

    pub(super) async fn multiplexer_session_actions(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        session: &MultiplexerSession,
        current: Option<&SessionId>,
    ) -> Vec<ActionButton> {
        let mut actions = Vec::new();
        let action_group = Uuid::new_v4().simple().to_string();
        let default_directory = self
            .rmux
            .default_directory(self.agent.as_ref(), conversation)
            .await;
        for window in &session.windows {
            let attached = multiplexer_window_contains(window, current);
            let token = self
                .issue_action(
                    conversation,
                    owner_id,
                    &action_group,
                    UiAction::Multiplexer(
                        self.multiplexer_backend(conversation).await,
                        MultiplexerUiAction::ShowWindow {
                            session_id: session.id.clone(),
                            window_id: window.id.clone(),
                        },
                    ),
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
                        name: self
                            .multiplexer_backend(conversation)
                            .await
                            .map_or("codex", |kind| kind.as_str())
                            .into(),
                        cwd: default_directory,
                    },
                    launch_agent: true,
                }),
            ),
            ("← Back", MultiplexerUiAction::ShowRoot),
        ] {
            let token = self
                .issue_action(
                    conversation,
                    owner_id,
                    &action_group,
                    UiAction::Multiplexer(self.multiplexer_backend(conversation).await, action),
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

    pub(super) async fn show_multiplexer_window(
        &self,
        conversation: &ConversationRef,
        owner_id: &str,
        session_id: &str,
        window_id: &str,
    ) -> Result<(), EngineError> {
        let snapshot = self.required_multiplexer_snapshot(conversation).await?;
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
        let body = multiplexer_window_body(&window, current.as_ref(), self.agent.display_name());
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
                UiAction::Multiplexer(
                    self.multiplexer_backend(conversation).await,
                    MultiplexerUiAction::ShowSession {
                        session_id: session.id.clone(),
                    },
                ),
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

    pub(super) async fn multiplexer_pane_actions(
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
                current.is_some_and(|current| pane.agent_session.as_ref() == Some(current));
            let (label, action) = if let Some(codex_session) = &pane.agent_session {
                if attached {
                    continue;
                }
                (
                    format!("{} · Attach", pane.index),
                    UiAction::Attach(codex_session.clone()),
                )
            } else if is_shell_command(&pane.current_command) {
                (
                    format!(
                        "{} · Run {}",
                        pane.index,
                        self.multiplexer_agent_name(conversation).await
                    ),
                    UiAction::Multiplexer(
                        self.multiplexer_backend(conversation).await,
                        MultiplexerUiAction::Mutate(MultiplexerMutation {
                            target: MultiplexerTarget::ExistingPane {
                                pane_id: pane.id.clone(),
                            },
                            launch_agent: true,
                        }),
                    ),
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

    pub(super) async fn multiplexer_split_actions(
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
        let default_directory = self
            .rmux
            .default_directory(self.agent.as_ref(), conversation)
            .await;
        for (label, direction) in [
            (
                format!(
                    "Split ↔ + {}",
                    self.multiplexer_agent_name(conversation).await
                ),
                PaneSplitDirection::Horizontal,
            ),
            (
                format!(
                    "Split ↕ + {}",
                    self.multiplexer_agent_name(conversation).await
                ),
                PaneSplitDirection::Vertical,
            ),
        ] {
            let action = UiAction::Multiplexer(
                self.multiplexer_backend(conversation).await,
                MultiplexerUiAction::Mutate(MultiplexerMutation {
                    target: MultiplexerTarget::SplitPane {
                        pane_id: pane_id.clone(),
                        direction,
                        cwd: default_directory.clone(),
                    },
                    launch_agent: true,
                }),
            );
            let token = self
                .issue_action(conversation, owner_id, action_group, action)
                .await;
            actions.push(ActionButton {
                label,
                token,
                style: ActionStyle::Default,
            });
        }
        Ok(actions)
    }

    pub(super) async fn required_multiplexer_snapshot(
        &self,
        conversation: &ConversationRef,
    ) -> Result<MultiplexerSnapshot, EngineError> {
        self.rmux
            .runtime(self.agent.as_ref(), conversation)
            .await
            .ok_or_else(|| EngineError::InvalidInput("workspace runtime is unavailable".into()))?
            .snapshot()
            .await?
            .ok_or_else(|| EngineError::InvalidInput("rmux is no longer running".into()))
    }

    pub(super) async fn handle_multiplexer_action(
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

    pub(super) async fn execute_multiplexer_mutation(
        &self,
        conversation: &ConversationRef,
        mutation: MultiplexerMutation,
    ) -> Result<(), EngineError> {
        let result = match self
            .rmux
            .runtime(self.agent.as_ref(), conversation)
            .await
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
}
