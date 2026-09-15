//! Native client handoffs keep the conversation and its FIFO alive across session exit.
use super::{
    ConversationRef, Engine, EngineError, OutboundView, SessionCommand, SessionId, Uuid,
    notification_now,
};
use agentix_storage::SessionSwitch;

impl Engine {
    pub(super) async fn retain_switch_on_exit(
        &self,
        conversation: &ConversationRef,
        session_id: &SessionId,
    ) -> Result<bool, EngineError> {
        if let Some(switch) = self.state.session_switch(conversation).await?
            && switch.old_session == *session_id
            && switch.target.is_none()
            && switch.paused.is_none()
        {
            self.invalidate_session_actions(session_id.as_str()).await?;
            let active = self.turns.active.lock().await.remove(session_id);
            if let Some(turn) = active {
                self.cleanup_exited_turn(conversation, session_id, &turn)
                    .await;
            }
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) async fn handle_native_session_switch_started(
        &self,
        session_id: &str,
        client_id: &str,
    ) -> Result<(), EngineError> {
        let session = SessionId::new(session_id);
        if self
            .agent
            .session_client_id(&session)
            .await
            .as_deref()
            .is_none_or(|id| id == client_id)
        {
            self.cancel_session_attachments(&session);
            if let Some(conversation) = self.sessions.bound_conversation(&session).await {
                self.begin_session_switch(&conversation, &session, client_id)
                    .await?;
            }
        }
        Ok(())
    }

    pub(super) async fn begin_session_switch(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
        client: &str,
    ) -> Result<bool, EngineError> {
        self.cancel_reattachment(conversation);
        if self.state.session_switch(conversation).await?.is_some() {
            return Ok(false);
        }
        let mut switch = SessionSwitch::new(
            Uuid::new_v4().to_string(),
            conversation.clone(),
            session.clone(),
            client.into(),
            self.state.binding_epoch(conversation).await?,
            u64::try_from(notification_now()).unwrap_or_default() + 30,
        );
        switch.owner_id = self
            .interactions
            .owners
            .lock()
            .await
            .get(conversation)
            .cloned();
        if !self.state.save_session_switch(&mut switch).await? {
            return Ok(false);
        }
        switch.notice = Some(
            self.send_view(
                conversation,
                &OutboundView::text(
                    "New session",
                    "Waiting for the new session. Messages sent here will be queued.",
                ),
            )
            .await?,
        );
        self.state.save_session_switch(&mut switch).await?;
        Ok(true)
    }

    pub(super) async fn request_new_session(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
    ) -> Result<(), EngineError> {
        self.expire_session_switch(conversation).await?;
        let Some(client) = self.agent.session_client_id(session).await else {
            return self
                .send_view(
                    conversation,
                    &OutboundView::text(
                        "Command unavailable",
                        "The attached session has no verified client identity.",
                    ),
                )
                .await
                .map(|_| ());
        };
        let retry = self.state.session_switch(conversation).await?;
        if let Some(switch) = &retry
            && !(switch.paused.is_some()
                && switch.target.is_none()
                && switch.candidate.is_none()
                && switch.old_session == *session
                && switch.client_id == client
                && switch.epoch == self.state.binding_epoch(conversation).await?
                && switch.messages.iter().all(|m| !m.sending))
        {
            return self.switch_notice(switch, "A session switch is already pending or its queue needs review. Use /queue to inspect it; the terminal draft was preserved.").await;
        }
        if self
            .check_terminal_input(conversation, session, None)
            .await?
        {
            return Ok(());
        }
        if let Some(mut switch) = retry {
            switch.paused = None;
            switch.deadline = u64::try_from(notification_now()).unwrap_or_default() + 30;
            if !self.state.save_session_switch(&mut switch).await? {
                return Ok(());
            }
            self.switch_notice(
                &switch,
                "Retrying the new session. Queued messages were retained.",
            )
            .await?;
        } else if !self
            .begin_session_switch(conversation, session, &client)
            .await?
        {
            return Ok(());
        }
        // The adapter accepts the request; completion arrives as lifecycle events.
        if let Ok(Err(error)) = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.operations.command(session, SessionCommand::New),
        )
        .await
        {
            self.fail_session_switch(session.as_str(), &client, &error.to_string())
                .await?;
        }
        self.expire_session_switch(conversation).await?;
        Ok(())
    }

    pub(super) async fn enqueue_switch_prompt(
        &self,
        conversation: &ConversationRef,
        id: &str,
        text: &str,
    ) -> Result<bool, EngineError> {
        self.expire_session_switch(conversation).await?;
        let Some(mut switch) = self.state.session_switch(conversation).await? else {
            return Ok(false);
        };
        if self.state.binding_epoch(conversation).await? != switch.epoch {
            return Ok(false);
        }
        if switch.messages.len() >= 100 {
            self.send_view(
                conversation,
                &OutboundView::text(
                    "Queue full",
                    "The session switch queue contains 100 messages. Use `/queue` to review it.",
                ),
            )
            .await?;
            return Ok(true);
        }
        if switch.enqueue(id, text) {
            self.state.save_session_switch(&mut switch).await?;
        }
        self.drain_session_switch(conversation).await?;
        Ok(true)
    }

    pub(super) async fn cancel_session_switch(
        &self,
        conversation: &ConversationRef,
    ) -> Result<(), EngineError> {
        if let Some(mut switch) = self.state.session_switch(conversation).await? {
            if switch.messages.is_empty() {
                self.state.delete_session_switch(&switch).await?;
            } else {
                switch.paused = Some("Attachment changed. Queued messages were retained; use `/queue clear` to discard them.".into());
                self.state.save_session_switch(&mut switch).await?;
            }
        }
        Ok(())
    }

    pub(super) async fn replace_session(
        &self,
        old: &str,
        new: &str,
        client: &str,
    ) -> Result<(), EngineError> {
        for mut switch in self.state.list_session_switches().await? {
            if self.expire_session_switch(&switch.conversation).await? {
                continue;
            }
            if switch.old_session.as_str() != old
                || switch.client_id != client
                || switch.target.is_some()
                || switch.paused.is_some()
            {
                continue;
            }
            if self.state.binding_epoch(&switch.conversation).await? != switch.epoch
                || notification_now() >= i64::try_from(switch.deadline).unwrap_or(i64::MAX)
            {
                continue;
            }
            let target = SessionId::new(new);
            switch.candidate = Some(target.clone());
            if !self.state.save_session_switch(&mut switch).await? {
                continue;
            }
            let remaining = switch
                .deadline
                .saturating_sub(u64::try_from(notification_now()).unwrap_or_default());
            match tokio::time::timeout(
                std::time::Duration::from_secs(remaining),
                self.agent.attach(&target),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, session = %target, "new session not attachable yet; retaining handoff");
                    continue;
                }
                Err(_) => {
                    self.expire_session_switch(&switch.conversation).await?;
                    continue;
                }
            }
            if self.expire_session_switch(&switch.conversation).await? {
                continue;
            }
            if !self
                .state
                .complete_session_switch(&mut switch, &target)
                .await?
            {
                continue;
            }
            let outcome = self
                .sessions
                .attach_at_epoch(
                    switch.conversation.clone(),
                    target.clone(),
                    false,
                    switch.epoch,
                )
                .await;
            self.apply_binding_effects(&switch.conversation, &target, false, outcome)
                .await;
            self.switch_notice(&switch, "Attached to the new session.")
                .await?;
            self.drain_session_switch(&switch.conversation).await?;
        }
        Ok(())
    }

    pub(super) async fn fail_session_switch(
        &self,
        old: &str,
        client: &str,
        reason: &str,
    ) -> Result<(), EngineError> {
        for mut switch in self.state.list_session_switches().await? {
            if switch.old_session.as_str() != old
                || switch.client_id != client
                || switch.target.is_some()
            {
                continue;
            }
            switch.paused = Some(reason.into());
            self.state.save_session_switch(&mut switch).await?;
            self.switch_notice(
                &switch,
                &format!("{reason}\n\nQueued messages were retained. Use `/queue` to review them."),
            )
            .await?;
        }
        Ok(())
    }

    pub(super) async fn drain_session_switch(
        &self,
        conversation: &ConversationRef,
    ) -> Result<(), EngineError> {
        if self
            .interactions
            .terminal_inputs
            .lock()
            .await
            .contains_key(conversation)
        {
            return Ok(());
        }
        let Some(mut switch) = self.state.session_switch(conversation).await? else {
            return Ok(());
        };
        let Some(target) = &switch.target else {
            return Ok(());
        };
        if switch.paused.is_some()
            || self.state.binding_epoch(conversation).await? != switch.epoch
            || self.sessions.current(conversation).await.as_ref() != Some(target)
            || self.turns.active_turn(target).await.is_some()
        {
            return Ok(());
        }
        if !self.agent.session_access(target).await.can_write() {
            switch.paused =
                Some("The new session is not writable. Queued messages were retained.".into());
            self.state.save_session_switch(&mut switch).await?;
            return Ok(());
        }
        let Some(message) = switch.messages.first_mut() else {
            self.state.delete_session_switch(&switch).await?;
            return Ok(());
        };
        if message.sending {
            return Ok(());
        }
        message.sending = true;
        let text = message.text.clone();
        if !self.state.save_session_switch(&mut switch).await? {
            return Ok(());
        }
        match self.send_prompt(conversation, &text).await {
            Ok(()) => {
                if self
                    .interactions
                    .terminal_inputs
                    .lock()
                    .await
                    .contains_key(conversation)
                {
                    switch.messages[0].sending = false;
                } else {
                    switch.messages.remove(0);
                }
            }
            Err(error) => {
                switch.paused = Some(format!(
                    "Delivery may have succeeded: {error}. Review the session before clearing this message."
                ));
            }
        }
        self.state.save_session_switch(&mut switch).await?;
        Ok(())
    }
}

impl Engine {
    async fn switch_notice(&self, switch: &SessionSwitch, body: &str) -> Result<(), EngineError> {
        let view = OutboundView::text("New session", body);
        if let Some(message) = &switch.notice
            && self
                .channel(switch.conversation.channel)?
                .update(&switch.conversation, message, &view)
                .await
                .is_ok()
        {
            return Ok(());
        }
        self.send_view(&switch.conversation, &view)
            .await
            .map(|_| ())
    }

    pub async fn session_switch_conversations(&self) -> Result<Vec<ConversationRef>, EngineError> {
        Ok(self
            .state
            .list_session_switches()
            .await?
            .into_iter()
            .map(|s| s.conversation)
            .collect())
    }

    /// Delete the failed handoff rather than keeping a paused command barrier.
    async fn expire_session_switch(
        &self,
        conversation: &ConversationRef,
    ) -> Result<bool, EngineError> {
        let Some(switch) = self.state.session_switch(conversation).await? else {
            return Ok(false);
        };
        if switch.target.is_some()
            || notification_now() < i64::try_from(switch.deadline).unwrap_or(i64::MAX)
        {
            return Ok(false);
        }
        if !self.state.delete_session_switch(&switch).await? {
            return Ok(false);
        }
        let mut notice = "New session timed out after 30 seconds. The switch was cancelled; subsequent commands can be sent normally.".to_string();
        if !switch.messages.is_empty() {
            notice.push_str("\n\nThese queued messages were not sent. Resend them if needed:\n\n");
            notice.push_str(
                &switch
                    .messages
                    .iter()
                    .map(|m| m.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            );
        }
        self.switch_notice(&switch, &notice).await?;
        Ok(true)
    }

    pub async fn refresh_session_switch(
        &self,
        conversation: &ConversationRef,
    ) -> Result<(), EngineError> {
        self.expire_session_switch(conversation).await?;
        let Some(mut switch) = self.state.session_switch(conversation).await? else {
            return Ok(());
        };
        if switch.paused.is_some() {
            return Ok(());
        }
        if switch.epoch != self.state.binding_epoch(conversation).await? {
            return self.cancel_session_switch(conversation).await;
        }
        if switch.target.is_none()
            && let Some(candidate) = &switch.candidate
        {
            return self
                .replace_session(
                    switch.old_session.as_str(),
                    candidate.as_str(),
                    &switch.client_id,
                )
                .await;
        }
        if switch.messages.iter().any(|m| m.sending) {
            switch.paused = Some("A delivery may have succeeded before restart. Review the session and use `/queue clear`; it will not be sent again automatically.".into());
            self.state.save_session_switch(&mut switch).await?;
            self.switch_notice(&switch, switch.paused.as_deref().unwrap_or_default())
                .await?;
            return Ok(());
        }
        self.drain_session_switch(conversation).await
    }

    pub(super) async fn show_switch_queue(
        &self,
        conversation: &ConversationRef,
    ) -> Result<bool, EngineError> {
        let Some(switch) = self.state.session_switch(conversation).await? else {
            return Ok(false);
        };
        let status = switch
            .paused
            .as_deref()
            .unwrap_or(if switch.target.is_some() {
                "Sending to the new session"
            } else {
                "Waiting for the new session"
            });
        let messages = switch
            .messages
            .iter()
            .enumerate()
            .map(|(i, m)| {
                format!(
                    "{}. {}{}",
                    i + 1,
                    m.text,
                    if m.sending {
                        " (delivery uncertain)"
                    } else {
                        ""
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        self.send_view(
            conversation,
            &OutboundView::text(
                "Session switch queue",
                format!("{status}\n\n{messages}\n\nUse `/queue resume` or `/queue clear`."),
            ),
        )
        .await?;
        Ok(true)
    }

    pub(super) async fn control_switch_queue(
        &self,
        conversation: &ConversationRef,
        action: &str,
    ) -> Result<bool, EngineError> {
        let Some(mut switch) = self.state.session_switch(conversation).await? else {
            return Ok(false);
        };
        match action {
            "clear" => {
                switch.messages.clear();
                if switch.paused.is_some() || switch.target.is_some() {
                    self.state.delete_session_switch(&switch).await?;
                } else {
                    self.state.save_session_switch(&mut switch).await?;
                }
                self.send_view(
                    conversation,
                    &OutboundView::text("Session switch queue", "Queued messages cleared."),
                )
                .await?;
            }
            "resume" => {
                if switch.messages.iter().any(|m| m.sending)
                    || switch.epoch != self.state.binding_epoch(conversation).await?
                    || switch.target.is_none()
                {
                    self.show_switch_queue(conversation).await?;
                    return Ok(true);
                }
                switch.paused = None;
                self.state.save_session_switch(&mut switch).await?;
                self.drain_session_switch(conversation).await?;
            }
            _ => return Err(EngineError::InvalidInput("Unknown queue action".into())),
        }
        Ok(true)
    }
}
