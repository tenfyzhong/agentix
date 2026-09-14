//! Pending questions are actionable only in the currently attached conversation.
use super::{
    ConversationRef, DeliveryClass, Engine, EngineError, InteractionKey, InteractionRequest,
    MessageRef, OutboundView, SessionId, ViewStatus, interaction_key,
};

pub(super) struct QueuedQuestion {
    pub(super) request: InteractionRequest,
    notices: Vec<(MessageRef, OutboundView)>,
}

impl Engine {
    pub(super) async fn receive_question(
        &self,
        request: &InteractionRequest,
    ) -> Result<(), EngineError> {
        let key = interaction_key(request);
        {
            let mut questions = self.interactions.questions.lock().await;
            if questions.iter().any(|q| interaction_key(&q.request) == key) {
                return Ok(());
            }
            questions.push(QueuedQuestion {
                request: request.clone(),
                notices: Vec::new(),
            });
        }
        let session = SessionId::new(&request.session_id);
        if let Some(conversation) = self.sessions.bound_conversation(&session).await {
            return self.show_queued_questions(&conversation, &session).await;
        }
        if !self.background_turn_notifications {
            return Ok(());
        }
        let recipients = self.interactions.owners.lock().await.clone();
        let label = self.session_label(&session).await;
        for (conversation, owner) in recipients {
            let mut view = OutboundView::text(
                format!("{} · {label}", self.agent.display_name()),
                "This session is waiting for your answer. Attach to view and answer the question.",
            );
            view.status = ViewStatus::Waiting;
            view.actions
                .push(self.attach_action(&conversation, &owner, &session).await);
            let message = self.send_view(&conversation, &view).await?;
            if let Some(question) = self
                .interactions
                .questions
                .lock()
                .await
                .iter_mut()
                .find(|q| interaction_key(&q.request) == key)
            {
                question.notices.push((message, view));
            }
        }
        Ok(())
    }

    pub(super) async fn show_queued_questions(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
    ) -> Result<(), EngineError> {
        let requests = self
            .interactions
            .questions
            .lock()
            .await
            .iter()
            .filter(|q| q.request.session_id == session.as_str())
            .map(|q| q.request.clone())
            .collect::<Vec<_>>();
        for request in requests {
            let key = interaction_key(&request);
            let existing = self.interactions.pending.lock().await.get(&key).cloned();
            if let Some(existing) = existing {
                if existing.message.conversation == *conversation
                    && !existing.view.actions.is_empty()
                {
                    // Reissue controls after attach because its binding epoch changed.
                    self.revoke_action_group(&existing.action_group).await;
                    let owner = self
                        .interactions
                        .owners
                        .lock()
                        .await
                        .get(conversation)
                        .cloned()
                        .ok_or(EngineError::InvalidAction)?;
                    self.show_input_question(conversation, &owner, &key).await?;
                    continue;
                }
                self.resolve_external_interaction(&existing.message.conversation, &key)
                    .await?;
            }
            self.render_interaction(conversation, &request, DeliveryClass::Live)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn finish_queued_question(
        &self,
        key: &InteractionKey,
    ) -> Result<(), EngineError> {
        let question = {
            let mut questions = self.interactions.questions.lock().await;
            questions
                .iter()
                .position(|q| interaction_key(&q.request) == *key)
                .map(|index| questions.remove(index))
        };
        if let Some(question) = question {
            for (message, mut view) in question.notices {
                view.actions.clear();
                view.status = ViewStatus::Muted;
                view.body = "This question is no longer waiting for an answer.".into();
                self.channel(message.conversation.channel)?
                    .update(&message.conversation, &message, &view)
                    .await?;
            }
            let pending = self.interactions.pending.lock().await.get(key).cloned();
            if let Some(pending) = pending {
                self.resolve_external_interaction(&pending.message.conversation, key)
                    .await?;
            }
        }
        Ok(())
    }
}
