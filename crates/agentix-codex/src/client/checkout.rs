use super::{AgentAdapter, AgentEvent, ClientError, CodexClient, Ordering, SessionId, Value, json};
use agentix_domain::{InteractionKind, InteractionRequest};
use agentix_multiplexer::CodexCheckoutChoice;

pub(super) struct PendingCheckout {
    session: SessionId,
    client_id: String,
    pid: u32,
}

impl CodexClient {
    pub(super) async fn request_checkout_choice(
        &self,
        session: &SessionId,
        client_id: &str,
        pid: u32,
    ) {
        let token = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        self.checkout_choices
            .lock()
            .await
            .retain(|_, pending| pending.session != *session);
        self.checkout_choices.lock().await.insert(
            token.clone(),
            PendingCheckout {
                session: session.clone(),
                client_id: client_id.into(),
                pid,
            },
        );
        let request = InteractionRequest {
            rpc_id: json!({"agentixCheckout":token,"clientId":client_id}),
            method: "agentix/codex/checkoutChoice".into(),
            session_id: session.to_string(),
            turn_id: "new-session".into(),
            item_id: None,
            kind: InteractionKind::CommandApproval,
            title: "Where should the new conversation run?".into(),
            detail: "Choose where to start the new conversation:\n\n- **Current checkout**: Keep using the current working directory.\n- **New worktree**: Create an isolated managed checkout.\n\nUse `/cancel` to cancel the switch.".into(),
            available_decisions: vec!["Current checkout".into(), "New worktree".into()],
            payload: json!({"clientId":client_id}),
            auto_resolution_ms: None,
        };
        let _ = self.events.send(AgentEvent::InteractionRequested(request));
    }

    pub(super) async fn respond_checkout_choice(
        &self,
        id: &Value,
        response: &Value,
    ) -> Result<(), ClientError> {
        let invalid = |message: &str| ClientError::Rpc {
            code: -32602,
            message: message.into(),
        };
        let token = id["agentixCheckout"]
            .as_str()
            .ok_or_else(|| invalid("Invalid checkout choice"))?;
        let choice = match response["decision"].as_str() {
            Some("Current checkout") => CodexCheckoutChoice::CurrentCheckout,
            Some("New worktree") => CodexCheckoutChoice::NewWorktree,
            Some("cancel") => CodexCheckoutChoice::Cancel,
            _ => return Err(invalid("Select one of the displayed checkout choices")),
        };
        let pending = self
            .checkout_choices
            .lock()
            .await
            .remove(token)
            .ok_or_else(|| invalid("This checkout choice expired or was already answered"))?;
        let result = if self.session_client_id(&pending.session).await.as_deref()
            != Some(&pending.client_id)
            || id["clientId"].as_str() != Some(&pending.client_id)
        {
            Err(invalid("The original Codex client changed"))
        } else {
            self.set_native_session_switch(
                &pending.session,
                &pending.client_id,
                choice != CodexCheckoutChoice::Cancel,
            )
            .await
            .map_err(|error| invalid(&error.to_string()))?;
            self.workspace
                .select_codex_checkout(pending.pid, choice)
                .await
                .map_err(|error| invalid(&error.to_string()))
        };
        let _ = self.events.send(AgentEvent::InteractionResolved {
            session_id: pending.session.to_string(),
            request_id: id.to_string(),
        });
        if let Err(error) = &result {
            let _ = self
                .set_native_session_switch(&pending.session, &pending.client_id, false)
                .await;
            let _ = self.events.send(AgentEvent::SessionSwitchFailed {
                session_id: pending.session.to_string(),
                client_id: pending.client_id,
                reason: error.to_string(),
            });
        }
        result
    }
}
