//! A logical Feishu message owns an ordered group of physical cards.
use super::{
    ChannelError, ConversationRef, FeishuAdapter, MessageRef, OutboundView, PatchMessageReqBody,
    RequestOption, SendInput,
    card_pages::{self, Page},
    delivery_error, ensure_feishu, retry_tenant_token,
};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(super) struct CardGroup {
    cards: Vec<SentCard>,
}

impl CardGroup {
    fn needs_tracking(&self) -> bool {
        self.cards.len() > 1
            || self.cards.iter().any(|card| {
                card.page
                    .as_ref()
                    .is_some_and(|page| page.view.actions.iter().any(|action| !action.disabled))
            })
    }
}

struct SentCard {
    id: String,
    page: Option<Page>,
}

impl FeishuAdapter {
    async fn post_card(
        &self,
        conversation: &ConversationRef,
        page: &Page,
    ) -> Result<String, ChannelError> {
        let input = SendInput {
            chat_id: Some(conversation.conversation_id.clone()),
            card: Some(page.json.clone()),
            uuid: Some(uuid::Uuid::new_v4().to_string()),
            ..SendInput::default()
        };
        let option = RequestOption::default();
        retry_tenant_token(self, || async {
            self.client.channel_messaging().send(&input, &option).await
        })
        .await
        .map(|result| result.message_id)
        .map_err(|error| delivery_error(&error))
    }

    async fn patch_card(&self, id: &str, page: &Page) -> Result<(), ChannelError> {
        let body = PatchMessageReqBody {
            content: Some(page.json.clone()),
        };
        let option = RequestOption::default();
        retry_tenant_token(self, || async {
            self.client.im().message.patch(id, &body, &option).await
        })
        .await
        .map_err(|error| delivery_error(&error))?;
        Ok(())
    }

    async fn delete_card(&self, id: &str) -> Result<(), ChannelError> {
        let option = RequestOption::default();
        retry_tenant_token(self, || async {
            self.client.im().message.delete(id, &option).await
        })
        .await
        .map_err(|error| delivery_error(&error))?;
        Ok(())
    }

    pub(super) async fn send_cards(
        &self,
        conversation: &ConversationRef,
        view: &OutboundView,
    ) -> Result<MessageRef, ChannelError> {
        ensure_feishu(conversation)?;
        let pages = card_pages::paginate(view)?;
        self.messages.outbound(Some(conversation), async {
            let mut cards: Vec<SentCard> = Vec::new();
            for page in pages {
                match Box::pin(self.post_card(conversation, &page)).await {
                    Ok(id) => cards.push(SentCard { id, page: Some(page) }),
                    Err(error) => {
                        // A failed initial send has no MessageRef to resume from.
                        // Retract confirmed pages before reporting a rejection.
                        for card in cards.iter().rev() {
                            if let Err(cleanup) = Box::pin(self.delete_card(&card.id)).await {
                                return Err(ChannelError::Transport(format!("partial Feishu card delivery: {error}; cleanup failed: {cleanup}")));
                            }
                        }
                        return Err(error);
                    }
                }
            }
            let message = MessageRef::new(conversation.clone(), cards[0].id.clone());
            let group = CardGroup { cards };
            if group.needs_tracking() {
                self.views.lock().await.insert(message.clone(), Arc::new(Mutex::new(group)));
            }
            Ok(message)
        }).await
    }

    pub(super) async fn update_cards(
        &self,
        conversation: &ConversationRef,
        message: &MessageRef,
        view: &OutboundView,
    ) -> Result<(), ChannelError> {
        ensure_feishu(conversation)?;
        if conversation != &message.conversation {
            return Err(ChannelError::InvalidPayload(
                "Feishu update conversation does not match the message".into(),
            ));
        }
        let pages = card_pages::paginate(view)?;
        self.messages
            .outbound(Some(conversation), async {
                let state = self
                    .views
                    .lock()
                    .await
                    .entry(message.clone())
                    .or_insert_with(|| {
                        Arc::new(Mutex::new(CardGroup {
                            cards: vec![SentCard {
                                id: message.message_id.clone(),
                                page: None,
                            }],
                        }))
                    })
                    .clone();
                let mut group = state.lock().await;
                let count = pages.len();
                for (index, page) in pages.into_iter().enumerate() {
                    if let Some(card) = group.cards.get_mut(index) {
                        if count > 1
                            && card.page.as_ref().is_some_and(|previous| {
                                if previous.json == page.json {
                                    return true;
                                }
                                // Elapsed-time ticks only refresh the live tail; final
                                // status, content and action changes still update all pages.
                                let mut previous_view = previous.view.clone();
                                if index + 1 < count
                                    && page.view.status == agentix_domain::ViewStatus::Running
                                {
                                    previous_view.subtitle.clone_from(&page.view.subtitle);
                                }
                                previous_view == page.view
                            })
                        {
                            continue;
                        }
                        Box::pin(self.patch_card(&card.id, &page)).await?;
                        card.page = Some(page);
                    } else {
                        let id = Box::pin(self.post_card(conversation, &page)).await?;
                        // Keep confirmed progress even if a later page is rejected.
                        group.cards.push(SentCard {
                            id,
                            page: Some(page),
                        });
                    }
                }
                while group.cards.len() > count {
                    Box::pin(self.delete_card(&group.cards.last().unwrap().id)).await?;
                    group.cards.pop();
                }
                if !group.needs_tracking() {
                    self.views.lock().await.remove(message);
                }
                Ok(())
            })
            .await
    }

    pub(super) async fn disable_card_actions(
        &self,
        message: &MessageRef,
    ) -> Result<(), ChannelError> {
        ensure_feishu(&message.conversation)?;
        self.messages
            .outbound(Some(&message.conversation), async {
                let Some(state) = self.views.lock().await.get(message).cloned() else {
                    return Ok(());
                };
                let mut group = state.lock().await;
                for card in &mut group.cards {
                    let Some(page) = &card.page else {
                        continue;
                    };
                    if page.view.actions.iter().all(|action| action.disabled) {
                        continue;
                    }
                    let mut view = page.view.clone();
                    for action in &mut view.actions {
                        action.disabled = true;
                    }
                    let page = card_pages::render(view)?;
                    // Use the action-state renderer to retain the original callback
                    // payload while disabling controls, as for single-card messages.
                    let page = Page {
                        json: super::card_sections::wire_json(
                            &super::render_card_with_disabled_actions(
                                &card.page.as_ref().unwrap().view,
                            )?,
                        )?,
                        ..page
                    };
                    Box::pin(self.patch_card(&card.id, &page)).await?;
                    card.page = Some(page);
                }
                if !group.needs_tracking() {
                    self.views.lock().await.remove(message);
                }
                Ok(())
            })
            .await
    }
}
