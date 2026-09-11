//! Live turn projection and lifecycle orchestration.
use super::{
    ActionButton, ActionStyle, AgentEvent, ConversationRef, DeliveryClass, Duration, Engine,
    EngineError, EventImportance, HashSet, Instant, ItemSummary, OutboundView, SessionId,
    StoredTurnView, TurnBuffer, TurnStatus, TurnSummary, UiAction, Uuid, ViewStatus,
    background_completion_body, cold_turns, live_turn_view, short_identifier,
    turn_conversation_body, turn_status_label,
};

impl Engine {
    pub(super) async fn handle_routed_event(
        &self,
        conversation: ConversationRef,
        session_id: SessionId,
        event: AgentEvent,
        delivery: DeliveryClass,
    ) -> Result<(), EngineError> {
        match event {
            AgentEvent::AgentMessageDelta {
                turn_id,
                item_id,
                delta,
                ..
            } => {
                self.handle_message_delta(
                    &conversation,
                    &session_id,
                    &turn_id,
                    &item_id,
                    &delta,
                    delivery,
                )
                .await?;
            }
            AgentEvent::ItemStarted {
                turn_id,
                item_id,
                kind,
                ..
            } if kind == "commentary" => {
                self.restore_cold_turn(&session_id, &turn_id).await?;
                self.turns
                    .buffers
                    .lock()
                    .await
                    .entry((session_id, turn_id))
                    .or_default()
                    .record_output(Some(&item_id), "", true, false);
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

    pub(super) async fn hydrate_running_turn(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        turn: &TurnSummary,
    ) -> Result<(), EngineError> {
        let key = (session_id.clone(), turn.id.clone());
        self.record_turn_started(session_id.clone(), turn.id.clone())
            .await?;
        self.turns.cold.remove(session_id, &turn.id).await?;
        self.turns.buffers.lock().await.insert(
            key.clone(),
            TurnBuffer {
                user_text: turn.user_text.clone().unwrap_or_default(),
                agent_text: turn.agent_text.clone().unwrap_or_default(),
                output_items: Vec::new(),
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

    pub(super) async fn route_agent_event(
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

    pub(super) async fn handle_turn_completed(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        turn_id: String,
        status: TurnStatus,
        error: Option<String>,
        delivery: DeliveryClass,
    ) -> Result<(), EngineError> {
        self.restore_cold_turn(session_id, &turn_id).await?;
        let key = (session_id.clone(), turn_id.clone());
        let mut buffers = self.turns.buffers.lock().await;
        let buffer = buffers.entry(key).or_default();
        buffer.ensure_started();
        buffer.status = status;
        if let Some(error) = error {
            buffer.record_output(None, &format!("Error: {error}"), false, false);
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

    pub(super) async fn notify_unattached_turn_completion(
        &self,
        session_id: &SessionId,
        turn_id: &str,
        status: &TurnStatus,
        error: Option<&str>,
    ) -> Result<(), EngineError> {
        self.turns.remove_active(session_id).await;
        self.archive_turn(session_id, turn_id).await?;
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
        if self.agent.is_subagent(session_id).await? {
            return Ok(());
        }
        let content = self.background_turn_content(session_id, turn_id).await;
        let body = format!("{}\n\n{content}", background_completion_body(status, error));
        self.sessions
            .cache_session_summary(self.agent.as_ref(), session_id)
            .await;
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
                    sections: Vec::new(),
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

    pub(super) async fn background_turn_content(
        &self,
        session: &SessionId,
        turn_id: &str,
    ) -> String {
        let mut cursor = None;
        let mut visited = HashSet::new();
        loop {
            let page = match self.operations.history(session, cursor, 20).await {
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

    pub(super) async fn handle_message_delta(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        turn_id: &str,
        item_id: &str,
        delta: &str,
        delivery: DeliveryClass,
    ) -> Result<(), EngineError> {
        let was_cold = self.restore_cold_turn(session_id, turn_id).await?;
        let key = (session_id.clone(), turn_id.to_owned());
        let mut buffers = self.turns.buffers.lock().await;
        let buffer = buffers.entry(key).or_default();
        buffer.ensure_started();
        let commentary = buffer
            .output_items
            .iter()
            .any(|item| item.id.as_deref() == Some(item_id) && item.process);
        if commentary {
            if self.output.show_reasoning {
                buffer.append_commentary(item_id, delta);
            }
        } else {
            buffer.record_output(Some(item_id), delta, false, true);
        }
        drop(buffers);
        self.render_turn(conversation, session_id, turn_id, delivery, false)
            .await?;
        if was_cold {
            self.archive_turn(session_id, turn_id).await?;
        }
        Ok(())
    }

    pub(super) async fn handle_completed_item(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        turn_id: &str,
        item: &ItemSummary,
        delivery: DeliveryClass,
    ) -> Result<(), EngineError> {
        let was_cold = self.restore_cold_turn(session_id, turn_id).await?;
        if self.apply_completed_item(session_id, turn_id, item).await {
            self.render_turn(conversation, session_id, turn_id, delivery, false)
                .await?;
        }
        if was_cold {
            self.archive_turn(session_id, turn_id).await?;
        }
        Ok(())
    }

    pub(super) async fn record_turn_started(
        &self,
        session_id: SessionId,
        turn_id: String,
    ) -> Result<(), EngineError> {
        self.restore_cold_turn(&session_id, &turn_id).await?;
        if let Some(previous) = self.turns.active_turn(&session_id).await
            && previous != turn_id
        {
            self.clear_turn_stop_action(&(session_id.clone(), previous.clone()))
                .await?;
            self.archive_turn(&session_id, &previous).await?;
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

    pub(super) async fn handle_session_exit(
        &self,
        session_id: &SessionId,
    ) -> Result<(), EngineError> {
        let Some(conversation) = self.sessions.bound_conversation(session_id).await else {
            self.turns.cold.remove_session(session_id).await?;
            self.turns.active.lock().await.remove(session_id);
            self.turns
                .buffers
                .lock()
                .await
                .retain(|(session, _), _| session != session_id);
            self.turns
                .views
                .lock()
                .await
                .retain(|(session, _), _| session != session_id);
            self.turns
                .last_renders
                .lock()
                .await
                .retain(|(session, _), _| session != session_id);
            self.turns
                .stop_actions
                .lock()
                .await
                .retain(|(session, _), _| session != session_id);
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
        turn_ids.extend(self.turns.cold.session_turns(session_id).await?);
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

    pub(super) async fn reconcile_resumed_session(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
    ) -> Result<(), EngineError> {
        let history = self.operations.history(session, None, 1).await?;
        let Some(turn) = history.turns.last() else {
            self.turns.remove_active(session).await;
            return Ok(());
        };
        if matches!(turn.status, TurnStatus::InProgress | TurnStatus::Unknown) {
            return self.hydrate_running_turn(conversation, session, turn).await;
        }
        let active = self.turns.active.lock().await.get(session).cloned();
        if active.as_deref() == Some(turn.id.as_str()) {
            let item = ItemSummary {
                id: format!("{}:recovered", turn.id),
                kind: "agentMessage".into(),
                text: turn.agent_text.clone(),
                status: None,
            };
            self.handle_completed_item(conversation, session, &turn.id, &item, DeliveryClass::Live)
                .await?;
            self.handle_turn_completed(
                conversation,
                session,
                turn.id.clone(),
                turn.status.clone(),
                None,
                DeliveryClass::Live,
            )
            .await?;
        } else {
            self.turns.remove_active(session).await;
        }
        Ok(())
    }

    pub(super) async fn handle_session_resume(
        &self,
        session_id: &SessionId,
    ) -> Result<(), EngineError> {
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
            return self
                .reconcile_resumed_session(&conversation, session_id)
                .await;
        }

        self.sessions
            .cache_session_summary(self.agent.as_ref(), session_id)
            .await;
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
                    sections: Vec::new(),
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

    pub(super) async fn cleanup_exited_turn(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        turn_id: &str,
    ) {
        if let Err(error) = self.restore_cold_turn(session_id, turn_id).await {
            tracing::warn!(%error, %session_id, %turn_id, "failed to restore exited turn");
            return;
        }
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
                    "failed to mark an exited agent turn as interrupted"
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
                "failed to delete an exited agent turn checkpoint"
            );
        }
        self.turns.buffers.lock().await.remove(&key);
        self.turns.views.lock().await.remove(&key);
        self.turns.last_renders.lock().await.remove(&key);
        self.turns.stop_actions.lock().await.remove(&key);
        if let Err(error) = self.turns.cold.remove(session_id, turn_id).await {
            tracing::warn!(%error, %session_id, %turn_id, "failed to remove exited turn cache");
        }
    }

    pub(super) async fn notify_session_exit(
        &self,
        conversation: &ConversationRef,
        session_label: &str,
    ) -> Result<(), EngineError> {
        self.send_view(
            conversation,
            &OutboundView {
                sections: Vec::new(),
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

    pub(super) async fn apply_completed_item(
        &self,
        session_id: &SessionId,
        turn_id: &str,
        item: &ItemSummary,
    ) -> bool {
        let process = self.output.process_text(item);
        if !matches!(
            item.kind.as_str(),
            "agentMessage" | "userMessage" | "commentary"
        ) && process.is_none()
        {
            return false;
        }
        let mut buffers = self.turns.buffers.lock().await;
        let buffer = buffers
            .entry((session_id.clone(), turn_id.to_owned()))
            .or_default();
        buffer.ensure_started();
        match item.kind.as_str() {
            "agentMessage" => buffer.record_output(
                Some(&item.id),
                item.text.as_deref().unwrap_or_default(),
                false,
                false,
            ),
            "commentary" => buffer.record_output(
                Some(&item.id),
                process.as_deref().unwrap_or_default(),
                true,
                false,
            ),
            "userMessage" => buffer.user_text = item.text.clone().unwrap_or_default(),
            _ => {
                if let Some(text) = process {
                    buffer.record_output(Some(&item.id), &text, true, false);
                }
            }
        }
        true
    }

    pub(super) async fn render_turn(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        turn_id: &str,
        delivery: DeliveryClass,
        force: bool,
    ) -> Result<(), EngineError> {
        let key = (session_id.clone(), turn_id.to_owned());
        let interval = self
            .channel(conversation.channel)?
            .streaming_update_interval();
        if !self.turns.should_render(&key, force, interval).await {
            return Ok(());
        }
        let session_label = self.session_label(session_id).await;
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
            && self.agent.session_access(session_id).await.can_write()
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
        if !is_running {
            self.archive_turn(session_id, turn_id).await?;
        }
        Ok(())
    }

    pub(super) async fn restore_cold_turn(
        &self,
        session: &SessionId,
        turn: &str,
    ) -> Result<bool, EngineError> {
        let key = (session.clone(), turn.to_owned());
        if self.turns.buffers.lock().await.contains_key(&key) {
            return Ok(false);
        }
        if let Some(cold) = self.turns.cold.load(session, turn).await? {
            self.turns
                .buffers
                .lock()
                .await
                .insert(key.clone(), cold.buffer);
            if let Some(message) = cold.message {
                self.turns.views.lock().await.insert(key.clone(), message);
            }
            if let Some(last_render) = cold.last_render {
                self.turns
                    .last_renders
                    .lock()
                    .await
                    .insert(key, last_render);
            }
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) async fn archive_turn(
        &self,
        session: &SessionId,
        turn: &str,
    ) -> Result<(), EngineError> {
        let key = (session.clone(), turn.to_owned());
        let Some(buffer) = self.turns.buffers.lock().await.get(&key).cloned() else {
            return Ok(());
        };
        let message = self.turns.views.lock().await.get(&key).cloned();
        let last_render = self.turns.last_renders.lock().await.get(&key).copied();
        self.turns
            .cold
            .store(
                session,
                turn,
                cold_turns::ColdTurn {
                    buffer,
                    message,
                    last_render,
                },
            )
            .await?;
        // Keep the hot state until the complete cold record has been written.
        self.turns.buffers.lock().await.remove(&key);
        self.turns.views.lock().await.remove(&key);
        self.turns.last_renders.lock().await.remove(&key);
        Ok(())
    }

    pub(super) async fn clear_session_stop_actions(
        &self,
        session_id: &SessionId,
    ) -> Result<(), EngineError> {
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

    pub(super) async fn clear_turn_stop_action(
        &self,
        key: &(SessionId, String),
    ) -> Result<(), EngineError> {
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

    pub(super) async fn replace_stop_action(
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

    /// Refreshes visible running turns so their working duration advances without agent output.
    pub async fn refresh_working_turns(&self) -> usize {
        let mut refreshed = 0;
        for (session, turn) in self.working_turns().await {
            refreshed += self.refresh_working_turn(&session, &turn).await;
        }
        refreshed
    }

    pub(super) async fn refresh_working_turn(
        &self,
        session_id: &SessionId,
        turn_id: &str,
    ) -> usize {
        if self.turns.active_turn(session_id).await.as_deref() != Some(turn_id) {
            return 0;
        }
        let key = (session_id.clone(), turn_id.to_owned());
        let Some((conversation, delivery)) = self
            .sessions
            .route(session_id, EventImportance::Stream)
            .await
        else {
            return 0;
        };
        let message = self.turns.views.lock().await.get(&key).cloned();
        let Some(buffer) = self.turns.buffers.lock().await.get(&key).cloned() else {
            return 0;
        };
        if !matches!(buffer.status, TurnStatus::InProgress | TurnStatus::Unknown) {
            return 0;
        }
        let Some(elapsed_seconds) = buffer.elapsed_seconds() else {
            return 0;
        };
        if buffer.rendered_elapsed_seconds == Some(elapsed_seconds) {
            return 0;
        }
        let Some(message) = message else {
            match self
                .render_turn(&conversation, session_id, turn_id, delivery, true)
                .await
            {
                Ok(()) => return 1,
                Err(error) => tracing::warn!(
                    %error,
                    session = %session_id,
                    turn = %turn_id,
                    "failed to create the IM working state"
                ),
            }
            return 0;
        };
        let interval = self
            .channel(conversation.channel)
            .map_or(Duration::from_secs(1), |channel| {
                channel.streaming_update_interval()
            });
        if !self.turns.should_render(&key, false, interval).await {
            return 0;
        }
        let session_label = self.session_label(session_id).await;
        let mut view = live_turn_view(
            self.agent.display_name(),
            &session_label,
            turn_id,
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
                return 0;
            }
        };
        match result {
            Ok(()) => {
                self.turns
                    .mark_elapsed_rendered(&key, Some(elapsed_seconds))
                    .await;
                return 1;
            }
            Err(error) => tracing::warn!(
                %error,
                session = %session_id,
                turn = %turn_id,
                "failed to refresh the IM working duration"
            ),
        }
        0
    }
}
