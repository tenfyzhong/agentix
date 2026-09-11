//! Slack Socket Mode transport and Block Kit rendering.
mod affixes;
pub use affixes::CommandAffixes;
mod events;
mod render;

pub use agentix_domain::OwnerClaimer as SlackOwnerClaimer;
pub use events::normalize_event;
pub use render::render_view;

mod api;
mod cache;
mod inbound;
mod manifest;
mod menu;
mod socket;
mod startup;
use agentix_domain::{
    ChannelAdapter, ChannelError, ChannelKind, CommandMenu, ConversationRef, InboundEnvelope,
    MessageCenter, MessageRef, OutboundView,
};
use async_trait::async_trait;
pub use manifest::merge_command_manifest;
use serde_json::{Value, json};
pub use startup::SlackCommandSync;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Clone)]
pub struct SlackAdapter {
    api: api::Api,
    command_sync: Option<SlackCommandSync>,
    command_affixes: CommandAffixes,
    owner_claimer: Option<Arc<dyn SlackOwnerClaimer>>,
    owners: Arc<Mutex<Vec<String>>>,
    messages: MessageCenter,
    views: Arc<Mutex<cache::ViewCache>>,
    menus: Arc<Mutex<HashMap<ConversationRef, (MessageRef, CommandMenu)>>>,
}
impl SlackAdapter {
    pub fn with_client(
        client: reqwest::Client,
        api_url: Url,
        bot_token: impl Into<String>,
        app_token: impl Into<String>,
        owners: Vec<String>,
    ) -> Result<Self, ChannelError> {
        let bot_token = bot_token.into();
        let app_token = app_token.into();
        if bot_token.trim().is_empty() || app_token.trim().is_empty() {
            return Err(ChannelError::InvalidPayload(
                "Slack bot_token and app_token are required".into(),
            ));
        }
        Ok(Self {
            owner_claimer: None,
            command_sync: None,
            command_affixes: CommandAffixes::default(),
            api: api::Api::new(client, api_url, bot_token, app_token),
            owners: Arc::new(Mutex::new(owners)),
            messages: MessageCenter::default(),
            views: Arc::default(),
            menus: Arc::default(),
        })
    }
    #[must_use]
    pub fn with_command_affixes(mut self, affixes: CommandAffixes) -> Self {
        self.command_affixes = affixes;
        self
    }
    #[must_use]
    pub fn with_owner_claimer(mut self, claimer: Arc<dyn SlackOwnerClaimer>) -> Self {
        self.owner_claimer = Some(claimer);
        self
    }
    #[must_use]
    pub fn with_command_sync(mut self, sync: SlackCommandSync) -> Self {
        self.command_sync = Some(sync);
        self
    }
    async fn cache(&self, message: MessageRef, body: Value, has_actions: bool) {
        let mut views = self.views.lock().await;
        if has_actions {
            views.insert(message, body);
        } else {
            views.remove(&message);
        }
    }
    async fn send_inner(
        &self,
        conversation: &ConversationRef,
        view: &OutboundView,
    ) -> Result<MessageRef, ChannelError> {
        let mut body = render_view(view)?;
        destination(conversation, &mut body)?;
        let response = self.api.call("chat.postMessage", &body).await?;
        let ts = response["ts"].as_str().ok_or_else(|| {
            ChannelError::InvalidPayload("Slack omitted message timestamp".into())
        })?;
        let message = MessageRef::new(conversation.clone(), ts);
        self.cache(message.clone(), body, !view.actions.is_empty())
            .await;
        Ok(message)
    }
    async fn update_inner(
        &self,
        message: &MessageRef,
        view: &OutboundView,
    ) -> Result<(), ChannelError> {
        let mut body = render_view(view)?;
        destination(&message.conversation, &mut body)?;
        body["ts"] = json!(message.message_id);
        self.api.call("chat.update", &body).await?;
        self.cache(message.clone(), body, !view.actions.is_empty())
            .await;
        Ok(())
    }
}

fn destination(conversation: &ConversationRef, body: &mut Value) -> Result<(), ChannelError> {
    let parts: Vec<_> = conversation.conversation_id.split(':').collect();
    if conversation.channel != ChannelKind::Slack
        || !(2..=3).contains(&parts.len())
        || parts.iter().any(|part| part.is_empty())
    {
        return Err(ChannelError::InvalidPayload(
            "invalid Slack conversation".into(),
        ));
    }
    body["channel"] = json!(parts[1]);
    if parts.len() == 3 {
        body["thread_ts"] = json!(parts[2]);
    }
    Ok(())
}

#[async_trait]
impl ChannelAdapter for SlackAdapter {
    async fn prepare_connection(&self) -> Result<(), ChannelError> {
        self.identity().await?;
        let response = self.api.call("apps.connections.open", &json!({})).await?;
        if response["url"].as_str().is_none() {
            return Err(ChannelError::InvalidPayload(
                "missing Slack socket URL".into(),
            ));
        }
        Ok(())
    }

    async fn replace_owners(&self, owners: &[String]) {
        *self.owners.lock().await = owners.to_vec();
    }

    async fn identity(&self) -> Result<Option<String>, ChannelError> {
        let auth = self.api.call("auth.test", &json!({})).await?;
        let field = |key| {
            auth[key]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ChannelError::InvalidPayload("Slack auth.test omitted bot identity".into())
                })
        };
        Ok(Some(format!("{}:{}", field("team_id")?, field("user_id")?)))
    }

    fn kind(&self) -> ChannelKind {
        ChannelKind::Slack
    }
    fn streaming_update_interval(&self) -> Duration {
        Duration::from_secs(2)
    }
    async fn run(
        &self,
        inbound: mpsc::Sender<InboundEnvelope>,
        shutdown: CancellationToken,
    ) -> Result<(), ChannelError> {
        self.run_inbound(inbound, shutdown).await
    }
    async fn send(
        &self,
        conversation: &ConversationRef,
        view: &OutboundView,
    ) -> Result<MessageRef, ChannelError> {
        self.messages
            .outbound(Some(conversation), self.send_inner(conversation, view))
            .await
    }

    async fn update(
        &self,
        conversation: &ConversationRef,
        message: &MessageRef,
        view: &OutboundView,
    ) -> Result<(), ChannelError> {
        if conversation != &message.conversation {
            return Err(ChannelError::InvalidPayload(
                "Slack message belongs to another conversation".into(),
            ));
        }
        self.messages
            .outbound(Some(conversation), self.update_inner(message, view))
            .await
    }
    async fn disable_actions(&self, message: &MessageRef) -> Result<(), ChannelError> {
        self.messages
            .outbound(Some(&message.conversation), async {
                let body = self.views.lock().await.get(message).cloned();
                if let Some(mut body) = body {
                    if let Some(blocks) = body["blocks"].as_array_mut() {
                        blocks.retain(|block| block["type"] != "actions");
                    }
                    body["ts"] = json!(message.message_id);
                    self.api.call("chat.update", &body).await?;
                    self.views.lock().await.remove(message);
                }
                Ok(())
            })
            .await
    }
    async fn set_command_menu(
        &self,
        conversation: &ConversationRef,
        menu: &CommandMenu,
    ) -> Result<(), ChannelError> {
        self.refresh_menu(conversation, menu).await
    }
}

#[cfg(test)]
mod reload_tests {
    use super::*;
    #[tokio::test]
    async fn owner_reload_updates_existing_connection_clones() {
        let adapter = SlackAdapter::with_client(
            reqwest::Client::new(),
            "https://slack.com/api/".parse().unwrap(),
            "bot",
            "app",
            vec!["1".into()],
        )
        .unwrap();
        let live = adapter.clone();
        adapter.replace_owners(&["2".into()]).await;
        assert_eq!(*live.owners.lock().await, vec!["2".to_owned()]);
    }
}
