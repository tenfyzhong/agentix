//! Startup restoration and shutdown orchestration.
use super::{
    DeliveryClass, Engine, EngineError, Instant, OutboundView, RestoredBinding,
    RestoredBindingStatus, RestoredBindings, RestoredTurn, ShutdownNotification, TurnBuffer,
    TurnStatus, ViewStatus, live_turn_view,
};

impl Engine {
    async fn reconcile_channel_identities(&self) -> Result<(), EngineError> {
        // Resolve every identity before mutating persistence. Authentication failure
        // must not adopt stale destinations or partially migrate other channels.
        let mut identities = Vec::new();
        for (kind, channel) in &self.channels {
            if let Some(identity) = channel.identity().await? {
                identities.push((*kind, identity));
            }
        }
        for (kind, identity) in identities {
            let detached = self
                .state
                .reconcile_channel_identity(kind, &identity)
                .await?;
            if detached > 0 {
                tracing::warn!(channel = %kind, detached, "bot identity changed or was unknown; old IM bindings disabled; use /sessions to attach again");
            }
        }
        Ok(())
    }

    /// Restores durable conversation bindings and their upstream subscriptions.
    pub async fn restore_bindings(&self) -> Result<usize, EngineError> {
        let updates = self.restore_bindings_deferred().await?;
        let restored = updates.restored_count();
        self.notify_restored_bindings(updates).await?;
        Ok(restored)
    }

    /// Resolve bot identities, then restore bindings without sending IM messages.
    pub async fn restore_bindings_deferred(&self) -> Result<RestoredBindings, EngineError> {
        self.reconcile_channel_identities().await?;
        let persisted = self.state.list_bindings().await?;
        let mut updates = RestoredBindings {
            bindings: Vec::new(),
            turns: Vec::new(),
        };
        for (conversation, session) in &persisted {
            if !self.channels.contains_key(&conversation.channel) {
                continue;
            }
            let status = self
                .sessions
                .restore_binding(self.agent.as_ref(), conversation, session)
                .await?;
            updates.bindings.push(RestoredBinding {
                conversation: conversation.clone(),
                session: session.clone(),
                status,
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
                    process_items: Vec::new(),
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

    /// Present restored bindings and turns. The caller owns cancellation and shutdown.
    pub async fn notify_restored_bindings(
        &self,
        updates: RestoredBindings,
    ) -> Result<(), EngineError> {
        for binding in updates.bindings {
            let RestoredBinding {
                conversation,
                session,
                status,
                epoch,
            } = binding;
            if self.sessions.epoch(&conversation).await != epoch {
                continue;
            }
            if status == RestoredBindingStatus::Attached {
                self.sessions
                    .cache_session_summary(self.agent.as_ref(), &session)
                    .await;
            }
            let session_label = self.session_label(&session).await;
            if self.sessions.epoch(&conversation).await != epoch {
                continue;
            }
            self.update_command_menu_best_effort(
                &conversation,
                status != RestoredBindingStatus::Detached,
            )
            .await;
            if self.sessions.epoch(&conversation).await != epoch {
                // A slow old request may have overwritten the newer binding's menu.
                let attached_now = self.sessions.current(&conversation).await.is_some();
                self.update_command_menu_best_effort(&conversation, attached_now)
                    .await;
                continue;
            }
            let view = if status == RestoredBindingStatus::Offline {
                OutboundView {
                    title: "Agentix serve".into(),
                    subtitle: Some("Online · Waiting for agent".into()),
                    body: format!(
                        "Agentix serve is online. Saved {} session {} is temporarily unavailable. Its binding is retained while waiting for the agent to reconnect.",
                        self.agent.display_name(),
                        session_label
                    ),
                    status: ViewStatus::Warning,
                    actions: Vec::new(),
                }
            } else if status == RestoredBindingStatus::Attached {
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

    /// Checkpoints local state, detaches routes, and prepares IM effects without sending them.
    pub async fn prepare_shutdown_notifications(
        &self,
    ) -> Result<Vec<ShutdownNotification>, EngineError> {
        self.state.checkpoint().await?;
        let persisted = self.state.list_bindings().await?;
        let mut stored_turns: std::collections::HashMap<_, Vec<_>> =
            std::collections::HashMap::new();
        for stored in self.state.list_turn_views().await? {
            stored_turns
                .entry((
                    stored.message.conversation.clone(),
                    stored.session_id.clone(),
                ))
                .or_default()
                .push(stored);
        }
        self.interactions.actions.lock().await.clear();
        self.interactions.pending.lock().await.clear();
        self.interactions.turn_action_groups.lock().await.clear();
        self.interactions.reply_modes.lock().await.clear();
        self.interactions.session_inputs.lock().await.clear();
        self.turns.stop_actions.lock().await.clear();

        let mut notifications = Vec::new();
        for (conversation, session) in persisted {
            if !self.channels.contains_key(&conversation.channel) {
                continue;
            }
            let session_label = self.session_label(&session).await;
            self.sessions
                .bindings
                .lock()
                .await
                .detach(&conversation, false);
            let turn_views = stored_turns
                .remove(&(conversation.clone(), session))
                .unwrap_or_default()
                .into_iter()
                .map(|stored| {
                    let buffer = TurnBuffer {
                        user_text: stored.user_text,
                        agent_text: stored.agent_text,
                        process_items: Vec::new(),
                        status: stored.status,
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
                    (stored.message, view)
                })
                .collect();
            notifications.push(ShutdownNotification {
                conversation,
                turn_views,
                offline_view: OutboundView {
                    title: "Agentix serve".into(),
                    subtitle: Some("Offline · Detached".into()),
                    body: format!(
                        "Saved {} session {} for automatic reattachment. This IM conversation is detached while Agentix serve is offline.",
                        self.agent.display_name(), session_label
                    ),
                    status: ViewStatus::Warning,
                    actions: Vec::new(),
                },
            });
        }
        Ok(notifications)
    }

    pub async fn send_shutdown_notification(
        &self,
        notification: ShutdownNotification,
    ) -> Result<(), EngineError> {
        let conversation = &notification.conversation;
        if let Err(error) = self.update_command_menu(conversation, false).await {
            tracing::warn!(%error, ?conversation, "failed to detach the IM command menu during shutdown");
        }
        for (message, view) in notification.turn_views {
            if let Err(error) = self
                .channel(conversation.channel)?
                .update(conversation, &message, &view)
                .await
            {
                tracing::warn!(%error, ?conversation, "failed to remove live turn controls during shutdown");
            }
        }
        self.send_view(conversation, &notification.offline_view)
            .await?;
        Ok(())
    }
}
