//! Shared rendering and consumption of one terminal browser list.
use super::{
    ActionButton, ConversationRef, Engine, EngineError, MessageRef, OutboundView, UiAction,
};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(super) type Picker = Arc<Mutex<PickerState>>;

#[derive(Debug, Default)]
pub(super) struct PickerState {
    consumed: bool,
    messages: Vec<(MessageRef, OutboundView)>,
}

impl Engine {
    pub(super) async fn issue_picker_action(
        &self,
        picker: &Picker,
        conversation: &ConversationRef,
        owner: &str,
        group: &str,
        action: UiAction,
    ) -> String {
        self.issue_action(
            conversation,
            owner,
            group,
            UiAction::WorkspacePicker {
                action: Box::new(action),
                picker: picker.clone(),
            },
        )
        .await
    }

    pub(super) async fn send_workspace_picker(
        &self,
        conversation: &ConversationRef,
        picker: &Picker,
        items: Vec<(String, String, Vec<ActionButton>)>,
        mut footer: OutboundView,
    ) -> Result<(), EngineError> {
        if conversation.channel == crate::ChannelKind::Telegram {
            for (title, body, actions) in items {
                let view = OutboundView {
                    title,
                    body,
                    actions,
                    subtitle: Some(footer.title.clone()),
                    sections: Vec::new(),
                    status: footer.status,
                };
                self.send_picker_view(conversation, picker, &view).await?;
            }
        } else {
            let management = std::mem::take(&mut footer.actions);
            for (title, body, actions) in items {
                footer.sections.push(agentix_domain::ViewSection {
                    title,
                    body,
                    action_tokens: actions.iter().map(|action| action.token.clone()).collect(),
                    ..agentix_domain::ViewSection::default()
                });
                footer.actions.extend(actions);
            }
            footer.actions.extend(management);
            if !footer.sections.is_empty() {
                footer.body = footer
                    .sections
                    .iter()
                    .map(|section| format!("**{}**\n{}", section.title, section.body))
                    .collect::<Vec<_>>()
                    .join("\n\n");
            }
        }
        self.send_picker_view(conversation, picker, &footer).await
    }

    async fn send_picker_view(
        &self,
        conversation: &ConversationRef,
        picker: &Picker,
        view: &OutboundView,
    ) -> Result<(), EngineError> {
        let message = self.send_view(conversation, view).await?;
        if view.actions.is_empty() {
            return Ok(());
        }
        let mut clean = view.clone();
        clean.actions.clear();
        for section in &mut clean.sections {
            section.action_tokens.clear();
        }
        let mut state = picker.lock().await;
        if state.consumed {
            // A callback may arrive while the remaining cards are still being sent.
            drop(state);
            self.clear_picker_message(conversation, &message, &clean)
                .await;
        } else {
            state.messages.push((message, clean));
        }
        Ok(())
    }

    pub(super) async fn clear_workspace_picker(
        &self,
        conversation: &ConversationRef,
        picker: &Picker,
    ) {
        let messages = {
            let mut state = picker.lock().await;
            state.consumed = true;
            std::mem::take(&mut state.messages)
        };
        for (message, view) in messages {
            self.clear_picker_message(conversation, &message, &view)
                .await;
        }
    }

    async fn clear_picker_message(
        &self,
        conversation: &ConversationRef,
        message: &MessageRef,
        view: &OutboundView,
    ) {
        let result = match self.channel(conversation.channel) {
            Ok(channel) => channel
                .update(conversation, message, view)
                .await
                .map_err(EngineError::from),
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            tracing::warn!(%error, message = %message.message_id, "failed to remove consumed picker controls");
        }
    }
}
