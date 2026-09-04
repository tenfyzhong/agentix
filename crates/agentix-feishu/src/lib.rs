//! Feishu long-connection and Card JSON 2.0 channel adapter.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use agentix_core::{
    ActionStyle, ChannelAdapter, ChannelError, ChannelKind, CommandMenu, ConversationRef,
    InboundEnvelope, MessageRef, OutboundView, ViewStatus, include_reply_context,
};
use async_trait::async_trait;
use larksuite_oapi_sdk_rs::card::v2::{
    Behavior, Body, Button, ButtonType, Card, CardDocument, Config, Element, Header, Markdown,
    TemplateColor, Text,
};
use larksuite_oapi_sdk_rs::channel::{
    Channel, ChannelPolicy, DmMode, NormalizedMessage, SendInput,
};
use larksuite_oapi_sdk_rs::{EventDispatcher, LarkClient, LarkError, RequestOption};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

const CARD_BODY_LIMIT: usize = 25_000;
const ATTACHED_COMMAND_MARKER: &str = "✌️ ";
const INVALID_TENANT_ACCESS_TOKEN: i64 = 99_991_663;
const TENANT_ACCESS_TOKEN_CACHE_KEY_PREFIX: &str = "tenant_access_token:app_secret:";

#[derive(Debug, Clone)]
pub struct FeishuPolicy {
    owner_open_ids: Arc<RwLock<HashSet<String>>>,
}

impl FeishuPolicy {
    #[must_use]
    pub fn new<I, S>(owner_open_ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            owner_open_ids: Arc::new(RwLock::new(
                owner_open_ids.into_iter().map(Into::into).collect(),
            )),
        }
    }

    #[must_use]
    pub fn accept(&self, sender_open_id: &str, private_chat: bool, mentioned_bot: bool) -> bool {
        self.contains_owner(sender_open_id) && (private_chat || mentioned_bot)
    }

    fn contains_owner(&self, sender_open_id: &str) -> bool {
        self.owner_open_ids
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(sender_open_id)
    }

    fn add_owner(&self, sender_open_id: String) {
        self.owner_open_ids
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(sender_open_id);
    }

    fn channel_policy(&self, owner_claim_enabled: bool) -> ChannelPolicy {
        let policy = ChannelPolicy::default()
            .allow_message_type("text")
            .require_mention(true);
        if owner_claim_enabled {
            return policy.dm_mode(DmMode::Open);
        }
        self.owner_open_ids
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .fold(policy.dm_mode(DmMode::Allowlist), |policy, owner| {
                policy
                    .allow_sender(owner.clone())
                    .allow_dm_sender(owner.clone())
            })
    }
}

#[async_trait]
pub trait FeishuOwnerClaimer: Send + Sync {
    async fn claim(&self, code: &str, owner_open_id: &str) -> Result<bool, String>;
}

#[derive(Clone)]
struct OwnerClaim {
    claimer: Arc<dyn FeishuOwnerClaimer>,
    completed: Arc<Mutex<bool>>,
}

impl OwnerClaim {
    async fn claim(&self, code: &str, owner_open_id: &str) -> Option<Result<bool, String>> {
        let mut completed = self.completed.lock().await;
        if *completed {
            return None;
        }
        let result = self.claimer.claim(code, owner_open_id).await;
        if matches!(result, Ok(true)) {
            *completed = true;
        }
        Some(result)
    }
}

#[derive(Clone)]
pub struct FeishuAdapter {
    client: LarkClient,
    policy: FeishuPolicy,
    owner_claim: Option<OwnerClaim>,
    views: Arc<Mutex<HashMap<String, OutboundView>>>,
    command_menu_messages: Arc<Mutex<HashMap<ConversationRef, MessageRef>>>,
}

impl FeishuAdapter {
    pub fn new<I, S>(
        app_id: impl Into<String>,
        app_secret: impl Into<String>,
        owners: I,
    ) -> Result<Self, ChannelError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let client = LarkClient::builder(app_id.into(), app_secret.into())
            .max_retries(1)
            .build()
            .map_err(|error| ChannelError::InvalidPayload(error.to_string()))?;
        Ok(Self::with_client(client, owners))
    }

    #[must_use]
    pub fn with_client<I, S>(client: LarkClient, owners: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            client,
            policy: FeishuPolicy::new(owners),
            owner_claim: None,
            views: Arc::new(Mutex::new(HashMap::new())),
            command_menu_messages: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[must_use]
    pub fn with_owner_claimer(mut self, claimer: Arc<dyn FeishuOwnerClaimer>) -> Self {
        self.owner_claim = Some(OwnerClaim {
            claimer,
            completed: Arc::new(Mutex::new(false)),
        });
        self
    }
}

#[async_trait]
impl ChannelAdapter for FeishuAdapter {
    fn kind(&self) -> ChannelKind {
        ChannelKind::Feishu
    }

    async fn run(
        &self,
        inbound: mpsc::Sender<InboundEnvelope>,
        shutdown: CancellationToken,
    ) -> Result<(), ChannelError> {
        let message_inbound = inbound.clone();
        let message_policy = self.policy.clone();
        let message_claim = self.owner_claim.clone();
        let message_client = self.client.clone();
        let action_inbound = inbound;
        let action_policy = self.policy.clone();
        let channel = Channel::builder(&self.client, EventDispatcher::new("", ""))
            .policy(self.policy.channel_policy(self.owner_claim.is_some()))
            .on_message(move |message| {
                let inbound = message_inbound.clone();
                let policy = message_policy.clone();
                let claim = message_claim.clone();
                let client = message_client.clone();
                async move {
                    handle_message(message, inbound, policy, claim, client).await;
                    Ok(())
                }
            })
            .on_card_action(move |action| {
                let inbound = action_inbound.clone();
                let policy = action_policy.clone();
                async move {
                    let Some(owner_id) = action.operator_open_id else {
                        return Ok(());
                    };
                    let Some(chat_id) = action.open_chat_id else {
                        return Ok(());
                    };
                    let token = action
                        .action_value
                        .get("token")
                        .and_then(|value| value.as_str())
                        .map(str::to_owned);
                    let command = action
                        .action_value
                        .get("command")
                        .and_then(|value| value.as_str())
                        .filter(|command| command.starts_with('/'))
                        .map(str::to_owned);
                    if token.is_none() && command.is_none() {
                        return Ok(());
                    }
                    if !policy.contains_owner(&owner_id) {
                        return Ok(());
                    }
                    let event_value = token.as_deref().or(command.as_deref()).unwrap();
                    let event_id = format!(
                        "card:{}:{}",
                        action.open_message_id.as_deref().unwrap_or("unknown"),
                        event_value
                    );
                    let conversation = ConversationRef::new(ChannelKind::Feishu, chat_id);
                    let envelope = if let Some(command) = command {
                        InboundEnvelope::text(event_id, conversation, owner_id, command)
                    } else if let Some(message_id) = action.open_message_id {
                        InboundEnvelope::action_from_message(
                            event_id,
                            conversation.clone(),
                            owner_id,
                            token.unwrap(),
                            MessageRef::new(conversation, message_id),
                        )
                    } else {
                        InboundEnvelope::action(event_id, conversation, owner_id, token.unwrap())
                    };
                    if inbound.send(envelope).await.is_err() {
                        tracing::warn!("Feishu inbound queue is closed");
                    }
                    Ok(())
                }
            })
            .auto_reconnect(true)
            .build();

        tokio::select! {
            result = channel.start() => result.map_err(|error| ChannelError::Transport(error.to_string())),
            () = shutdown.cancelled() => Ok(()),
        }
    }

    async fn send(
        &self,
        conversation: &ConversationRef,
        view: &OutboundView,
    ) -> Result<MessageRef, ChannelError> {
        ensure_feishu(conversation)?;
        let card = render_card(view)?;
        let card_json = serde_json::to_string(card.card())
            .map_err(|error| ChannelError::InvalidPayload(error.to_string()))?;
        let input = SendInput {
            chat_id: Some(conversation.conversation_id.clone()),
            card: Some(card_json),
            ..SendInput::default()
        };
        let option = RequestOption::default();
        let result = with_tenant_token_refresh(&self.client, || async {
            self.client.channel_messaging().send(&input, &option).await
        })
        .await
        .map_err(|error| ChannelError::Transport(error.to_string()))?;
        if !view.actions.is_empty() {
            self.views
                .lock()
                .await
                .insert(result.message_id.clone(), view.clone());
        }
        Ok(MessageRef::new(conversation.clone(), result.message_id))
    }

    async fn update(
        &self,
        conversation: &ConversationRef,
        message: &MessageRef,
        view: &OutboundView,
    ) -> Result<(), ChannelError> {
        ensure_feishu(conversation)?;
        let card = render_card(view)?;
        let option = RequestOption::default();
        with_tenant_token_refresh(&self.client, || async {
            self.client
                .channel_messaging()
                .edit_card(&message.message_id, &card, &option)
                .await
        })
        .await
        .map_err(|error| ChannelError::Transport(error.to_string()))?;
        let mut views = self.views.lock().await;
        if view.actions.is_empty() {
            views.remove(&message.message_id);
        } else {
            views.insert(message.message_id.clone(), view.clone());
        }
        Ok(())
    }

    async fn disable_actions(&self, message: &MessageRef) -> Result<(), ChannelError> {
        ensure_feishu(&message.conversation)?;
        let Some(view) = self.views.lock().await.remove(&message.message_id) else {
            return Ok(());
        };
        let card = render_card_with_disabled_actions(&view)?;
        let option = RequestOption::default();
        with_tenant_token_refresh(&self.client, || async {
            self.client
                .channel_messaging()
                .edit_card(&message.message_id, &card, &option)
                .await
        })
        .await
        .map_err(|error| ChannelError::Transport(error.to_string()))?;
        Ok(())
    }

    async fn set_command_menu(
        &self,
        conversation: &ConversationRef,
        menu: &CommandMenu,
    ) -> Result<(), ChannelError> {
        ensure_feishu(conversation)?;
        let card = render_command_menu(menu)?;
        let mut messages = self.command_menu_messages.lock().await;
        if let Some(message) = messages.get(conversation) {
            let option = RequestOption::default();
            with_tenant_token_refresh(&self.client, || async {
                self.client
                    .channel_messaging()
                    .edit_card(&message.message_id, &card, &option)
                    .await
            })
            .await
            .map_err(|error| ChannelError::Transport(error.to_string()))?;
            return Ok(());
        }
        let card_json = serde_json::to_string(card.card())
            .map_err(|error| ChannelError::InvalidPayload(error.to_string()))?;
        let input = SendInput {
            chat_id: Some(conversation.conversation_id.clone()),
            card: Some(card_json),
            ..SendInput::default()
        };
        let option = RequestOption::default();
        let result = with_tenant_token_refresh(&self.client, || async {
            self.client.channel_messaging().send(&input, &option).await
        })
        .await
        .map_err(|error| ChannelError::Transport(error.to_string()))?;
        messages.insert(
            conversation.clone(),
            MessageRef::new(conversation.clone(), result.message_id),
        );
        Ok(())
    }
}

async fn handle_message(
    message: NormalizedMessage,
    inbound: mpsc::Sender<InboundEnvelope>,
    policy: FeishuPolicy,
    claim: Option<OwnerClaim>,
    client: LarkClient,
) {
    let owner_id = message
        .sender
        .user_id
        .as_ref()
        .and_then(|user| user.open_id())
        .unwrap_or_default()
        .to_owned();
    let private_chat = message.chat_type == "p2p";
    let text = strip_bot_mentions(
        &message.text.unwrap_or_default(),
        message
            .mentions
            .iter()
            .filter(|mention| mention.is_bot)
            .map(|mention| mention.key.as_str()),
    );
    if let Some(code) = parse_claim_command(&text) {
        handle_owner_claim(
            claim.as_ref(),
            &policy,
            &client,
            &message.chat_id,
            &owner_id,
            code,
            private_chat,
        )
        .await;
        return;
    }
    if !policy.accept(&owner_id, private_chat, message.mentioned_bot) {
        return;
    }
    let text = if message.parent_id.is_empty() || text.trim_start().starts_with('/') {
        text
    } else {
        match fetch_message_text(&client, &message.parent_id).await {
            Ok(quoted) => include_reply_context(&text, quoted.as_deref()),
            Err(error) => {
                tracing::debug!(
                    %error,
                    parent_message_id = %message.parent_id,
                    "failed to fetch replied-to Feishu message"
                );
                text
            }
        }
    };
    if !text.is_empty()
        && inbound
            .send(InboundEnvelope::text(
                message.message_id,
                ConversationRef::new(ChannelKind::Feishu, message.chat_id),
                owner_id,
                text,
            ))
            .await
            .is_err()
    {
        tracing::warn!("Feishu inbound queue is closed");
    }
}

async fn fetch_message_text(
    client: &LarkClient,
    message_id: &str,
) -> Result<Option<String>, String> {
    let option = RequestOption::default();
    let response = with_tenant_token_refresh(client, || async {
        client
            .im()
            .message
            .get(message_id, Some("open_id"), &option)
            .await
    })
    .await
    .map_err(|error| error.to_string())?;
    if !response.success() {
        return Err(response.code_error.to_string());
    }
    let message = response
        .data
        .and_then(|data| data.items)
        .and_then(|items| items.into_iter().next());
    Ok(message.and_then(|message| {
        let message_type = message.msg_type.unwrap_or_default();
        let content = message.body.and_then(|body| body.content)?;
        extract_message_text(&message_type, &content)
    }))
}

fn extract_message_text(message_type: &str, content: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
    if message_type == "text" {
        return value
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
    }
    let mut parts = Vec::new();
    collect_visible_text(&value, &mut parts);
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn collect_visible_text(value: &serde_json::Value, parts: &mut Vec<String>) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                collect_visible_text(value, parts);
            }
        }
        serde_json::Value::Object(object) => {
            if object
                .get("tag")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|tag| matches!(tag, "button" | "select_static" | "overflow"))
            {
                return;
            }
            for key in ["title", "text", "content"] {
                if let Some(text) = object.get(key).and_then(serde_json::Value::as_str)
                    && !text.trim().is_empty()
                {
                    parts.push(text.trim().to_owned());
                }
            }
            for (key, value) in object {
                if !matches!(key.as_str(), "title" | "text" | "content") {
                    collect_visible_text(value, parts);
                }
            }
        }
        _ => {}
    }
}

async fn handle_owner_claim(
    claim: Option<&OwnerClaim>,
    policy: &FeishuPolicy,
    client: &LarkClient,
    chat_id: &str,
    owner_open_id: &str,
    code: &str,
    private_chat: bool,
) {
    if !private_chat {
        return;
    }
    let Some(claim) = claim else {
        return;
    };
    let Some(result) = claim.claim(code, owner_open_id).await else {
        return;
    };
    match result {
        Ok(true) => {
            policy.add_owner(owner_open_id.to_owned());
            send_claim_response(
                client,
                chat_id,
                "Owner claimed. This Feishu account can now use Agentix.",
            )
            .await;
        }
        Ok(false) => {
            send_claim_response(
                client,
                chat_id,
                "The claim code is invalid or expired. Generate a new code in the local Agentix terminal.",
            )
            .await;
        }
        Err(error) => {
            tracing::error!(%error, "failed to persist claimed Feishu owner");
            send_claim_response(
                client,
                chat_id,
                "Owner claim failed. Check the Agentix server logs and try again.",
            )
            .await;
        }
    }
}

fn parse_claim_command(input: &str) -> Option<&str> {
    let mut words = input.split_whitespace();
    if words.next() != Some("/claim") {
        return None;
    }
    let code = words.next()?;
    words.next().is_none().then_some(code)
}

async fn send_claim_response(client: &LarkClient, chat_id: &str, text: &str) {
    let input = SendInput {
        chat_id: Some(chat_id.to_owned()),
        text: Some(text.to_owned()),
        ..SendInput::default()
    };
    let option = RequestOption::default();
    if let Err(error) = with_tenant_token_refresh(client, || async {
        client.channel_messaging().send(&input, &option).await
    })
    .await
    {
        tracing::warn!(%error, "failed to send Feishu owner claim response");
    }
}

async fn with_tenant_token_refresh<T, F, Fut>(
    client: &LarkClient,
    mut operation: F,
) -> Result<T, LarkError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, LarkError>>,
{
    match operation().await {
        Err(error) if is_invalid_tenant_access_token(&error) => {
            tracing::debug!(
                app_id = client.config().app_id(),
                "refreshing an invalid Feishu tenant access token"
            );
            invalidate_tenant_access_token(client).await?;
            operation().await
        }
        result => result,
    }
}

fn is_invalid_tenant_access_token(error: &LarkError) -> bool {
    matches!(error, LarkError::Api(error) if error.code == INVALID_TENANT_ACCESS_TOKEN)
}

async fn invalidate_tenant_access_token(client: &LarkClient) -> Result<(), LarkError> {
    let key = format!(
        "{TENANT_ACCESS_TOKEN_CACHE_KEY_PREFIX}{}-",
        client.config().app_id()
    );
    client
        .config()
        .token_cache()
        .set(&key, "", Duration::ZERO)
        .await
}

pub fn render_card(view: &OutboundView) -> Result<CardDocument, ChannelError> {
    render_card_with_action_state(view, false)
}

pub fn render_command_menu(menu: &CommandMenu) -> Result<CardDocument, ChannelError> {
    let attached = menu.commands.iter().any(|command| command.contextual);
    let header = Header::new(Text::plain("Agentix commands")).template(TemplateColor::Default);
    let mut body = Body::new().element(Element::Markdown(Markdown::new(if attached {
        "✌️ Attached session commands are available."
    } else {
        "Select a command."
    })));
    for command in &menu.commands {
        let marker = if command.contextual {
            ATTACHED_COMMAND_MARKER
        } else {
            ""
        };
        let button = Button::new(Text::plain(format!("{marker}/{}", command.name)))
            .button_type(ButtonType::Default)
            .behavior(Behavior::callback(
                serde_json::json!({"command": format!("/{}", command.name)}),
            ));
        body = body.element(Element::Button(button));
    }
    CardDocument::new(
        Card::new()
            .config(Config::new().update_multi())
            .header(header)
            .body(body),
    )
    .map_err(|error| ChannelError::InvalidPayload(error.to_string()))
}

fn render_card_with_disabled_actions(view: &OutboundView) -> Result<CardDocument, ChannelError> {
    render_card_with_action_state(view, true)
}

fn render_card_with_action_state(
    view: &OutboundView,
    actions_disabled: bool,
) -> Result<CardDocument, ChannelError> {
    let mut header = Header::new(Text::plain(&view.title)).template(template_color(view.status));
    if let Some(subtitle) = &view.subtitle {
        header = header.subtitle(Text::plain(subtitle));
    }
    let mut body = Body::new().element(Element::Markdown(Markdown::new(truncate_utf8(
        &view.body,
        CARD_BODY_LIMIT,
    ))));
    for action in &view.actions {
        let button_type = match action.style {
            ActionStyle::Primary => ButtonType::Primary,
            ActionStyle::Default => ButtonType::Default,
            ActionStyle::Danger => ButtonType::Danger,
        };
        let mut button = Button::new(Text::plain(&action.label))
            .button_type(button_type)
            .behavior(Behavior::callback(
                serde_json::json!({"token": action.token}),
            ));
        button.disabled = actions_disabled.then_some(true);
        body = body.element(Element::Button(button));
    }
    CardDocument::new(
        Card::new()
            .config(Config::new().update_multi())
            .header(header)
            .body(body),
    )
    .map_err(|error| ChannelError::InvalidPayload(error.to_string()))
}

#[must_use]
pub fn strip_bot_mentions<I, S>(text: &str, mention_keys: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut text = text.to_owned();
    for key in mention_keys {
        text = text.replace(key.as_ref(), "");
    }
    text.trim().to_owned()
}

fn ensure_feishu(conversation: &ConversationRef) -> Result<(), ChannelError> {
    if conversation.channel == ChannelKind::Feishu {
        Ok(())
    } else {
        Err(ChannelError::InvalidPayload(
            "Feishu adapter received a non-Feishu conversation".into(),
        ))
    }
}

const fn template_color(status: ViewStatus) -> TemplateColor {
    match status {
        ViewStatus::Info => TemplateColor::Default,
        ViewStatus::Running => TemplateColor::Blue,
        ViewStatus::Waiting | ViewStatus::Warning => TemplateColor::Orange,
        ViewStatus::Success => TemplateColor::Green,
        ViewStatus::Error => TemplateColor::Red,
        ViewStatus::Muted => TemplateColor::Grey,
    }
}

fn truncate_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let suffix = "…";
    let end = text.floor_char_boundary(max_bytes.saturating_sub(suffix.len()));
    format!("{}{}", &text[..end], suffix)
}
