//! One editable command reference per active conversation.
use crate::SlackAdapter;
use agentix_domain::{ChannelError, CommandMenu, ConversationRef, OutboundView};

impl SlackAdapter {
    pub(crate) async fn refresh_menu(
        &self,
        conversation: &ConversationRef,
        menu: &CommandMenu,
    ) -> Result<(), ChannelError> {
        self.messages
            .outbound(Some(conversation), async {
                let previous = self.menus.lock().await.get(conversation).cloned();
                if previous
                    .as_ref()
                    .is_some_and(|(_, current)| current == menu)
                {
                    return Ok(());
                }
                let body = menu
                    .commands
                    .iter()
                    .map(|command| format!("/agentix /{} — {}", command.name, command.description))
                    .collect::<Vec<_>>()
                    .join("\n");
                let view = OutboundView::text("Commands", body);
                let message = if let Some((message, _)) = previous {
                    match self.update_inner(&message, &view).await {
                        Ok(()) => message,
                        Err(ChannelError::Rejected(error))
                            if error.ends_with(": message_not_found") =>
                        {
                            self.send_inner(conversation, &view).await?
                        }
                        Err(error) => return Err(error),
                    }
                } else {
                    self.send_inner(conversation, &view).await?
                };
                let mut menus = self.menus.lock().await;
                if menus.len() >= 256
                    && let Some(key) = menus.keys().next().cloned()
                {
                    menus.remove(&key);
                }
                menus.insert(conversation.clone(), (message, menu.clone()));
                Ok(())
            })
            .await
    }
}
