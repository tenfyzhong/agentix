use std::time::Duration;

use super::{
    AgentError, ConversationRef, Engine, EngineError, OutboundView, SessionId, ViewStatus,
    markdown_quote,
};

impl Engine {
    pub(super) async fn start_prompt_with_feedback(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
        prompt: &str,
    ) -> Result<Option<String>, EngineError> {
        let generation = self.agent.generation();
        let operations = self.operations.clone();
        let target = session.clone();
        let text = prompt.to_owned();
        let mut request: super::pending_prompts::SendFuture =
            Box::pin(async move { operations.send(&target, &text, None).await });
        if let Ok(result) = tokio::time::timeout(Duration::from_millis(100), &mut request).await {
            return result.map(Some).map_err(EngineError::from);
        }

        let label = self.session_label(session).await;
        let mut view = OutboundView::text(
            format!("{} · {label}", self.agent.display_name()),
            markdown_quote(prompt),
        );
        view.subtitle = Some("Sending…".into());
        view.status = ViewStatus::Waiting;
        let Some(request) = self
            .defer_prompt(conversation, session, prompt, request, &view, generation)
            .await?
        else {
            return Ok(None);
        };
        // Keep polling the original request while the channel posts feedback.
        // This is a display deadline, never a reason to cancel or resend input.
        let (result, message) = tokio::join!(request, self.send_view(conversation, &view));
        let message = match message {
            Ok(message) => Some(message),
            Err(error) => {
                tracing::warn!(%error, %session, "failed to show pending input");
                None
            }
        };
        match result {
            Ok(turn) => {
                if let Some(message) = message {
                    self.turns
                        .views
                        .lock()
                        .await
                        .insert((session.clone(), turn.clone()), message);
                }
                Ok(Some(turn))
            }
            Err(error) => {
                if let Some(message) = message {
                    let uncertain = matches!(error, AgentError::Uncertain(_));
                    view.subtitle = Some(
                        if uncertain {
                            "Delivery unconfirmed"
                        } else {
                            "Send failed"
                        }
                        .into(),
                    );
                    view.status = if uncertain {
                        ViewStatus::Warning
                    } else {
                        ViewStatus::Error
                    };
                    view.body.push_str(&format!("\n\n{error}"));
                    if let Err(update_error) = self
                        .channel(conversation.channel)?
                        .update(conversation, &message, &view)
                        .await
                    {
                        tracing::warn!(%update_error, %session, "failed to finalize pending input");
                    }
                }
                Err(error.into())
            }
        }
    }
}
