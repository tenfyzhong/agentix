//! Live turn projection and lifecycle orchestration.
use super::{
    ActionButton, ActionStyle, AgentEvent, ConversationRef, DeliveryClass, Duration, Engine,
    EngineError, EventImportance, Instant, ItemSummary, MessageRef, OutboundView, SessionId,
    StoredTurnView, TurnBuffer, TurnStatus, TurnSummary, UiAction, Uuid, ViewStatus, cold_turns,
    live_turn_view,
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
                let mut buffers = self.turns.buffers.lock().await;
                let buffer = buffers.entry((session_id, turn_id)).or_default();
                if !buffer
                    .output_items
                    .iter()
                    .any(|item| item.id.as_deref() == Some(&item_id))
                {
                    buffer.record_output(Some(&item_id), "", true, false);
                }
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
            AgentEvent::SessionSwitchStarted { .. }
            | AgentEvent::SessionReplaced { .. }
            | AgentEvent::SessionSwitchFailed { .. }
            | AgentEvent::SessionStatusChanged { .. }
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
                started_at: Some(Instant::now()),
                ..TurnBuffer::from_summary(turn, self.output)
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
        buffer.set_status(status.clone());
        if let Some(error) = &error {
            buffer.record_output(None, &format!("Error: {error}"), false, false);
        }
        drop(buffers);
        // Remote completion is authoritative even if the final IM edit fails.
        // Retain the completed buffer for recovery, but never steer new input
        // into a turn that the backend has already finished.
        if self.turns.active_turn(session_id).await.as_deref() == Some(&turn_id) {
            self.turns.remove_active(session_id).await;
        }
        if delivery == DeliveryClass::Draining {
            self.clear_turn_stop_action(&(session_id.clone(), turn_id.clone()))
                .await?;
            self.state.delete_turn_view(session_id, &turn_id).await?;
            let notification = self
                .prepare_background_completion(
                    session_id,
                    &turn_id,
                    &status,
                    None,
                    Some(conversation),
                )
                .await;
            self.archive_turn(session_id, &turn_id).await?;
            self.sessions.finish_draining(session_id).await;
            self.sessions
                .cleanup
                .enqueue(self.agent.clone(), session_id)
                .await;
            if let Some(ready) = notification {
                let _ = ready.send(());
            }
        } else {
            self.render_turn(conversation, session_id, &turn_id, delivery, true)
                .await?;
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
        if self.turns.active_turn(session_id).await.as_deref() == Some(turn_id) {
            self.turns.remove_active(session_id).await;
        }
        self.restore_cold_turn(session_id, turn_id).await?;
        if let Some(buffer) = self
            .turns
            .buffers
            .lock()
            .await
            .get_mut(&(session_id.clone(), turn_id.to_owned()))
        {
            buffer.set_status(status.clone());
        }
        self.clear_turn_stop_action(&(session_id.clone(), turn_id.to_owned()))
            .await?;
        let notification = self
            .prepare_background_completion(session_id, turn_id, status, error, None)
            .await;
        self.archive_turn(session_id, turn_id).await?;
        if let Some(ready) = notification {
            let _ = ready.send(());
        }
        Ok(())
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
        self.adopt_pending_prompt(&session_id, &turn_id).await;
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
            .entry((session_id.clone(), turn_id.clone()))
            .or_default()
            .ensure_started();
        if self.turns.pending_prompts.take_stop(&session_id) {
            self.operations.stop(&session_id, &turn_id).await?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(super) async fn handle_session_exit(
        &self,
        session_id: &SessionId,
    ) -> Result<(), EngineError> {
        self.cancel_session_attachments(session_id);
        self.turns.input_recovery.cancel_session(session_id);
        self.freeze_exited_cards(session_id).await;
        self.turns.pending_prompts.invalidate(session_id);
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
        if self
            .retain_switch_on_exit(&conversation, session_id)
            .await?
        {
            return Ok(());
        }
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
        if let Err(error) = self
            .notify_session_exit(&conversation, &session_label)
            .await
        {
            tracing::warn!(%error, ?conversation, "failed to notify an exited session");
        }
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

        let cleanup_deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        for turn_id in turn_ids {
            self.cleanup_exited_turn_until(&conversation, session_id, &turn_id, cleanup_deadline)
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
                    saved_session == session_id
                        && self.transports.contains_key(&conversation.channel)
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
            .cache_session_summary(self.agent.clone(), session_id)
            .await;
        let epoch = self.state.binding_epoch(&conversation).await?;
        self.sessions
            .attach_at_epoch(conversation.clone(), session_id.clone(), false, epoch)
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
        self.update_command_menu_best_effort(&conversation, true)
            .await;
        Ok(())
    }

    pub(super) async fn cleanup_exited_turn(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        turn_id: &str,
    ) {
        self.cleanup_exited_turn_until(
            conversation,
            session_id,
            turn_id,
            tokio::time::Instant::now() + Duration::from_secs(1),
        )
        .await;
    }

    async fn cleanup_exited_turn_until(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        turn_id: &str,
        deadline: tokio::time::Instant,
    ) {
        if let Err(error) = self.restore_cold_turn(session_id, turn_id).await {
            tracing::warn!(%error, %session_id, %turn_id, "failed to restore exited turn");
            return;
        }
        let key = (session_id.clone(), turn_id.to_owned());
        if self.turns.views.lock().await.contains_key(&key) {
            {
                let mut buffers = self.turns.buffers.lock().await;
                let buffer = buffers.entry(key.clone()).or_default();
                if matches!(buffer.status, TurnStatus::InProgress | TurnStatus::Unknown) {
                    buffer.set_status(TurnStatus::Interrupted);
                }
            }
            match tokio::time::timeout_at(
                deadline,
                self.render_turn_view(conversation, session_id, turn_id, DeliveryClass::Live),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, %session_id, %turn_id, "failed to finalize an exited agent turn");
                }
                Err(_) => {
                    tracing::warn!(%session_id, %turn_id, "exited turn card cleanup exceeded its deadline");
                }
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
        let mut buffers = self.turns.buffers.lock().await;
        buffers
            .entry((session_id.clone(), turn_id.to_owned()))
            .or_default()
            .apply_item(item, self.output)
    }

    pub(super) async fn render_turn(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        turn_id: &str,
        delivery: DeliveryClass,
        force: bool,
    ) -> Result<(), EngineError> {
        self.adopt_pending_prompt(session_id, turn_id).await;
        // Waiting for a card ID is not a render and must not consume the
        // channel update interval before the first visible output.
        if self
            .hold_pending_card(conversation, session_id, turn_id)
            .await
        {
            return Ok(());
        }
        let key = (session_id.clone(), turn_id.to_owned());
        let interval = self
            .channel(conversation.channel)?
            .streaming_update_interval();
        if !self.turns.should_render(&key, force, interval).await {
            return Ok(());
        }
        self.restore_turn_input(conversation, session_id, turn_id, delivery)
            .await;
        self.render_turn_view(conversation, session_id, turn_id, delivery)
            .await
    }

    // Exit finalization renders only known content and must not restart recovery.
    async fn render_turn_view(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
        turn_id: &str,
        delivery: DeliveryClass,
    ) -> Result<(), EngineError> {
        if self
            .hold_pending_card(conversation, session_id, turn_id)
            .await
        {
            return Ok(());
        }
        let key = (session_id.clone(), turn_id.to_owned());
        let existing = self.turns.views.lock().await.get(&key).cloned();
        let revision = existing
            .as_ref()
            .map(|message| self.card_writes.reserve(message));
        let session_label = self.session_label(session_id).await;
        let (mut view, is_running, snapshot) = {
            let buffers = self.turns.buffers.lock().await;
            let Some(buffer) = buffers.get(&key) else {
                // A deferred startup refresh can race with removal of a finished turn.
                return Ok(());
            };
            (
                live_turn_view(
                    self.agent.session_display_name(session_id),
                    &session_label,
                    turn_id,
                    buffer,
                    delivery,
                ),
                matches!(buffer.status, TurnStatus::InProgress | TurnStatus::Unknown),
                buffer.clone(),
            )
        };
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
        let Some(message) = self
            .deliver_turn_view(conversation, &key, existing.zip(revision), &view)
            .await?
        else {
            return Ok(());
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

    async fn deliver_turn_view(
        &self,
        conversation: &ConversationRef,
        key: &(SessionId, String),
        existing: Option<(MessageRef, super::card_writes::Revision)>,
        view: &OutboundView,
    ) -> Result<Option<MessageRef>, EngineError> {
        if let Some((message, revision)) = existing {
            return Ok(self
                .update_card_revision(conversation, &revision, view)
                .await?
                .then_some(message));
        }
        let message = self.send_view(conversation, view).await?;
        self.turns
            .views
            .lock()
            .await
            .insert(key.clone(), message.clone());
        Ok(Some(message))
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
            let revision = self.card_writes.reserve(&message);
            let session_label = self.session_label(&key.0).await;
            let view = live_turn_view(
                self.agent.session_display_name(&key.0),
                &session_label,
                &key.1,
                &buffer,
                DeliveryClass::Live,
            );
            // Revoke the action independently of its visual projection. A failed
            // edit must not preserve a usable stale button or block a new binding.
            self.replace_stop_action(key, &message.conversation, None, false)
                .await;
            self.state.delete_turn_view(&key.0, &key.1).await?;
            match tokio::time::timeout(
                Duration::from_millis(250),
                self.update_card_revision(&message.conversation, &revision, &view),
            )
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, session = %key.0, turn = %key.1,
                        "failed to remove the revoked stop button from its message");
                }
                Err(_) => {
                    tracing::warn!(session = %key.0, turn = %key.1,
                        "revoked stop button edit exceeded its navigation budget");
                }
            }
        }
        // Terminal buffers still own action tokens until their group is revoked.
        if let Some(group) = self
            .interactions
            .turn_action_groups
            .lock()
            .await
            .remove(key)
        {
            self.revoke_action_group(&group).await;
        }
        self.turns.stop_actions.lock().await.remove(key);
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
            disabled: false,
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
        let revision = self.card_writes.reserve(&message);
        let session_label = self.session_label(session_id).await;
        let mut view = live_turn_view(
            self.agent.session_display_name(session_id),
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
        let result = self
            .update_card_revision(&conversation, &revision, &view)
            .await;
        match result {
            Ok(true) => {
                self.turns
                    .mark_elapsed_rendered(&key, Some(elapsed_seconds))
                    .await;
                return 1;
            }
            Ok(false) => return 0,
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
