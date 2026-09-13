use super::{
    ActionButton, ActionStyle, ConversationRef, Engine, EngineError, OutboundView, SessionId,
    UiAction, Uuid, markdown_quote,
};

impl Engine {
    async fn inspect_terminal_input(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
        new_session: bool,
        clear: Option<&str>,
    ) -> Result<Option<String>, EngineError> {
        match self.agent.terminal_input(session, new_session, clear).await {
            Ok(draft) => Ok(draft),
            Err(error) => {
                self.send_view(
                    conversation,
                    &OutboundView::text("Request not sent", error.to_string()),
                )
                .await?;
                Err(error.into())
            }
        }
    }

    pub(super) async fn cancel_terminal_queue(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
    ) -> Result<(), EngineError> {
        if let Some(mut switch) = self.state.session_switch(conversation).await?
            && switch.target.as_ref() == Some(session)
            && switch.paused.is_none()
            && switch.messages.first().is_some_and(|m| !m.sending)
        {
            switch.messages.remove(0);
            self.state.save_session_switch(&mut switch).await?;
        }
        Ok(())
    }

    pub(super) async fn check_terminal_input(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
        prompt: Option<String>,
    ) -> Result<bool, EngineError> {
        if self
            .interactions
            .terminal_inputs
            .lock()
            .await
            .contains_key(conversation)
        {
            return Err(EngineError::InvalidInput("A terminal draft is awaiting confirmation. Confirm it or use /cancel before sending another request.".into()));
        }
        let Some(draft) = self
            .inspect_terminal_input(conversation, session, prompt.is_none(), None)
            .await?
        else {
            return Ok(false);
        };
        self.show_terminal_input(conversation, session, draft, prompt)
            .await?;
        Ok(true)
    }

    async fn show_terminal_input(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
        draft: String,
        prompt: Option<String>,
    ) -> Result<(), EngineError> {
        let client_id = self
            .agent
            .session_client_id(session)
            .await
            .ok_or(EngineError::InvalidAction)?;
        let owner = self
            .interactions
            .owners
            .lock()
            .await
            .get(conversation)
            .cloned();
        let owner = match owner {
            Some(owner) => owner,
            None => self
                .state
                .session_switch(conversation)
                .await?
                .and_then(|switch| switch.owner_id)
                .ok_or(EngineError::InvalidAction)?,
        };
        let group = Uuid::new_v4().to_string();
        let mut view = OutboundView::text(
            "Terminal draft",
            format!(
                "The input box already contains:\n\n{}\n\nClear this draft and send the pending request, or cancel sending?",
                markdown_quote(&draft)
            ),
        );
        for (label, confirm) in [("Clear and send", true), ("Cancel sending", false)] {
            let token = self
                .issue_action(
                    conversation,
                    &owner,
                    &group,
                    UiAction::TerminalInput {
                        session_id: session.clone(),
                        client_id: client_id.clone(),
                        draft: draft.clone(),
                        prompt: prompt.clone(),
                        confirm,
                    },
                )
                .await;
            view.actions.push(ActionButton {
                label: label.into(),
                token,
                style: ActionStyle::Default,
            });
        }
        self.send_view(conversation, &view).await?;
        self.interactions
            .terminal_inputs
            .lock()
            .await
            .insert(conversation.clone(), session.clone());
        Ok(())
    }

    pub(super) async fn resolve_terminal_input(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
        client: &str,
        draft: &str,
        prompt: Option<String>,
        confirm: bool,
    ) -> Result<(), EngineError> {
        if self
            .interactions
            .terminal_inputs
            .lock()
            .await
            .get(conversation)
            != Some(session)
            || self.sessions.current(conversation).await.as_ref() != Some(session)
            || self.agent.session_client_id(session).await.as_deref() != Some(client)
        {
            return Err(EngineError::InvalidAction);
        }
        self.interactions
            .terminal_inputs
            .lock()
            .await
            .remove(conversation);
        if !confirm {
            self.cancel_terminal_queue(conversation, session).await?;
            self.send_view(
                conversation,
                &OutboundView::text("Sending cancelled", "The terminal draft was preserved."),
            )
            .await?;
            return Ok(());
        }
        if let Some(changed) = self
            .inspect_terminal_input(conversation, session, prompt.is_none(), Some(draft))
            .await?
        {
            return self
                .show_terminal_input(conversation, session, changed, prompt)
                .await;
        }
        match prompt {
            Some(text) => {
                if self
                    .state
                    .session_switch(conversation)
                    .await?
                    .is_some_and(|s| {
                        s.target.as_ref() == Some(session)
                            && s.paused.is_none()
                            && s.messages
                                .first()
                                .is_some_and(|m| !m.sending && m.text == text)
                    })
                {
                    Box::pin(self.drain_session_switch(conversation)).await
                } else {
                    Box::pin(self.send_prompt(conversation, &text)).await
                }
            }
            None => Box::pin(self.request_new_session(conversation, session)).await,
        }
    }
}
