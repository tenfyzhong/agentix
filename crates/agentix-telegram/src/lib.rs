//! Telegram long-polling channel adapter.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

pub use agentix_core::include_reply_context;
use agentix_core::{
    ActionButton, ChannelAdapter, ChannelError, ChannelKind, CommandMenu, ConversationRef,
    InboundEnvelope, MessageRef, OutboundView, ViewStatus,
};
use async_trait::async_trait;
use telegram_markdown_v2::{UnsupportedTagsStrategy, convert_with_strategy};
use teloxide::prelude::*;
use teloxide::sugar::request::RequestLinkPreviewExt;
use teloxide::types::{
    BotCommand, BotCommandScope, InlineKeyboardButton, InlineKeyboardMarkup, MenuButton, MessageId,
    ParseMode,
};

const BASE_MENU_COMMANDS: [(&str, &str); 4] = [
    ("sessions", "Browse running sessions"),
    ("rmux", "Manage rmux workspaces"),
    ("cancel", "Cancel pending input"),
    ("help", "Show available commands"),
];
const ATTACHED_COMMAND_MARKER: &str = "✌️ ";
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

const TELEGRAM_TEXT_LIMIT: usize = 4_096;

#[derive(Debug, Clone)]
pub struct TelegramPolicy {
    owner_user_ids: Arc<RwLock<HashSet<u64>>>,
}

impl TelegramPolicy {
    #[must_use]
    pub fn new(owner_user_ids: impl IntoIterator<Item = u64>) -> Self {
        Self {
            owner_user_ids: Arc::new(RwLock::new(owner_user_ids.into_iter().collect())),
        }
    }

    #[must_use]
    pub fn is_owner(&self, user_id: u64) -> bool {
        self.owner_user_ids
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&user_id)
    }

    fn add_owner(&self, user_id: u64) {
        self.owner_user_ids
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(user_id);
    }

    #[must_use]
    pub fn accept_text(
        &self,
        user_id: u64,
        private_chat: bool,
        text: &str,
        bot_username: &str,
    ) -> Option<String> {
        if !self.is_owner(user_id) {
            return None;
        }
        if private_chat {
            return Some(text.trim().to_owned());
        }
        let mention = format!("@{bot_username}");
        if !text
            .to_ascii_lowercase()
            .contains(&mention.to_ascii_lowercase())
        {
            return None;
        }
        Some(
            strip_ascii_case_insensitive(text, &mention)
                .trim()
                .to_owned(),
        )
    }
}

#[async_trait]
pub trait TelegramOwnerClaimer: Send + Sync {
    async fn claim(&self, code: &str, owner_user_id: u64) -> Result<bool, String>;
}

#[derive(Clone)]
struct OwnerClaim {
    claimer: Arc<dyn TelegramOwnerClaimer>,
    completed: Arc<Mutex<bool>>,
}

impl OwnerClaim {
    async fn claim(&self, code: &str, owner_user_id: u64) -> Option<Result<bool, String>> {
        let mut completed = self.completed.lock().await;
        if *completed {
            return None;
        }
        let result = self.claimer.claim(code, owner_user_id).await;
        if matches!(result, Ok(true)) {
            *completed = true;
        }
        Some(result)
    }
}

fn replied_text(message: &Message) -> Option<&str> {
    message
        .quote()
        .map(|quote| quote.text.as_str())
        .filter(|text| !text.trim().is_empty())
        .or_else(|| {
            message
                .reply_to_message()
                .and_then(|reply| reply.text().or_else(|| reply.caption()))
        })
}

#[derive(Clone)]
pub struct TelegramAdapter {
    bot: Bot,
    policy: TelegramPolicy,
    owner_claim: Option<OwnerClaim>,
}

impl TelegramAdapter {
    #[must_use]
    pub fn new(token: impl Into<String>, owner_user_ids: impl IntoIterator<Item = u64>) -> Self {
        Self {
            bot: Bot::new(token.into()),
            policy: TelegramPolicy::new(owner_user_ids),
            owner_claim: None,
        }
    }

    #[must_use]
    pub fn with_bot(bot: Bot, policy: TelegramPolicy) -> Self {
        Self {
            bot,
            policy,
            owner_claim: None,
        }
    }

    #[must_use]
    pub fn with_owner_claimer(mut self, claimer: Arc<dyn TelegramOwnerClaimer>) -> Self {
        self.owner_claim = Some(OwnerClaim {
            claimer,
            completed: Arc::new(Mutex::new(false)),
        });
        self
    }

    /// Registers the supported commands and selects the commands menu button.
    pub async fn register_menu(&self) -> Result<(), ChannelError> {
        self.bot
            .set_my_commands(menu_commands())
            .await
            .map_err(|error| ChannelError::Transport(error.to_string()))?;
        self.bot
            .set_chat_menu_button()
            .menu_button(MenuButton::Commands)
            .await
            .map_err(|error| ChannelError::Transport(error.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for TelegramAdapter {
    fn kind(&self) -> ChannelKind {
        ChannelKind::Telegram
    }

    async fn run(
        &self,
        inbound: mpsc::Sender<InboundEnvelope>,
        shutdown: CancellationToken,
    ) -> Result<(), ChannelError> {
        let me = self
            .bot
            .get_me()
            .await
            .map_err(|error| ChannelError::Transport(error.to_string()))?;
        self.register_menu().await?;
        let username = me.username().to_owned();

        let message_policy = self.policy.clone();
        let message_claim = self.owner_claim.clone();
        let message_bot = self.bot.clone();
        let message_inbound = inbound.clone();
        let message_username = username.clone();
        let message_handler = Update::filter_message().endpoint(move |message: Message| {
            let policy = message_policy.clone();
            let claim = message_claim.clone();
            let bot = message_bot.clone();
            let inbound = message_inbound.clone();
            let username = message_username.clone();
            async move {
                let Some(user) = message.from.as_ref() else {
                    return Ok::<(), teloxide::RequestError>(());
                };
                let Some(text) = message.text() else {
                    return Ok(());
                };
                if let Some(code) = parse_claim_command(text) {
                    handle_owner_claim(
                        claim.as_ref(),
                        &policy,
                        &bot,
                        message.chat.id,
                        user.id.0,
                        code,
                        message.chat.is_private(),
                    )
                    .await;
                    return Ok(());
                }
                let Some(text) =
                    policy.accept_text(user.id.0, message.chat.is_private(), text, &username)
                else {
                    return Ok(());
                };
                let text = include_reply_context(&text, replied_text(&message));
                let envelope = InboundEnvelope::text(
                    format!("{}:{}", message.chat.id.0, message.id.0),
                    ConversationRef::new(ChannelKind::Telegram, message.chat.id.0.to_string()),
                    user.id.0.to_string(),
                    text,
                );
                if inbound.send(envelope).await.is_err() {
                    tracing::warn!("Telegram inbound queue is closed");
                }
                Ok(())
            }
        });

        let callback_policy = self.policy.clone();
        let callback_inbound = inbound;
        let callback_handler =
            Update::filter_callback_query().endpoint(move |bot: Bot, query: CallbackQuery| {
                let policy = callback_policy.clone();
                let inbound = callback_inbound.clone();
                async move {
                    bot.answer_callback_query(query.id.clone()).await?;
                    if !policy.is_owner(query.from.id.0) {
                        return Ok::<(), teloxide::RequestError>(());
                    }
                    let (Some(token), Some(message)) =
                        (query.data.as_deref(), query.regular_message())
                    else {
                        return Ok(());
                    };
                    let conversation =
                        ConversationRef::new(ChannelKind::Telegram, message.chat.id.0.to_string());
                    let envelope = InboundEnvelope::action_from_message(
                        query.id.0.clone(),
                        conversation.clone(),
                        query.from.id.0.to_string(),
                        token,
                        MessageRef::new(conversation, message.id.0.to_string()),
                    );
                    if inbound.send(envelope).await.is_err() {
                        tracing::warn!("Telegram inbound queue is closed");
                    }
                    Ok(())
                }
            });

        let handler = dptree::entry()
            .branch(message_handler)
            .branch(callback_handler);
        let mut dispatcher = Dispatcher::builder(self.bot.clone(), handler).build();
        let shutdown_token = dispatcher.shutdown_token();
        tokio::spawn(async move {
            shutdown.cancelled().await;
            let _ = shutdown_token.shutdown();
        });
        dispatcher.dispatch().await;
        Ok(())
    }

    async fn send(
        &self,
        conversation: &ConversationRef,
        view: &OutboundView,
    ) -> Result<MessageRef, ChannelError> {
        let chat_id = parse_chat_id(conversation)?;
        let request = self
            .bot
            .send_message(chat_id, render_text(view))
            .parse_mode(ParseMode::MarkdownV2)
            .disable_link_preview(true);
        let message = if let Some(keyboard) = render_keyboard(&view.actions) {
            request
                .reply_markup(keyboard)
                .await
                .map_err(|error| ChannelError::Transport(error.to_string()))?
        } else {
            request
                .await
                .map_err(|error| ChannelError::Transport(error.to_string()))?
        };
        Ok(MessageRef::new(
            conversation.clone(),
            message.id.0.to_string(),
        ))
    }

    async fn update(
        &self,
        conversation: &ConversationRef,
        message: &MessageRef,
        view: &OutboundView,
    ) -> Result<(), ChannelError> {
        let chat_id = parse_chat_id(conversation)?;
        let message_id = message
            .message_id
            .parse::<i32>()
            .map(MessageId)
            .map_err(|error| ChannelError::InvalidPayload(error.to_string()))?;
        let request = self
            .bot
            .edit_message_text(chat_id, message_id, render_text(view))
            .parse_mode(ParseMode::MarkdownV2)
            .disable_link_preview(true);
        if let Some(keyboard) = render_keyboard(&view.actions) {
            request
                .reply_markup(keyboard)
                .await
                .map_err(|error| ChannelError::Transport(error.to_string()))?;
        } else {
            request
                .reply_markup(InlineKeyboardMarkup::new(
                    Vec::<Vec<InlineKeyboardButton>>::new(),
                ))
                .await
                .map_err(|error| ChannelError::Transport(error.to_string()))?;
        }
        Ok(())
    }

    async fn disable_actions(&self, message: &MessageRef) -> Result<(), ChannelError> {
        let chat_id = parse_chat_id(&message.conversation)?;
        let message_id = message
            .message_id
            .parse::<i32>()
            .map(MessageId)
            .map_err(|error| ChannelError::InvalidPayload(error.to_string()))?;
        self.bot
            .edit_message_reply_markup(chat_id, message_id)
            .reply_markup(InlineKeyboardMarkup::new(
                Vec::<Vec<InlineKeyboardButton>>::new(),
            ))
            .await
            .map_err(|error| ChannelError::Transport(error.to_string()))?;
        Ok(())
    }

    async fn set_command_menu(
        &self,
        conversation: &ConversationRef,
        menu: &CommandMenu,
    ) -> Result<(), ChannelError> {
        let chat_id = parse_chat_id(conversation)?;
        let commands = menu
            .commands
            .iter()
            .map(|command| {
                let description = if command.contextual {
                    format!("{ATTACHED_COMMAND_MARKER}{}", command.description)
                } else {
                    command.description.clone()
                };
                BotCommand::new(&command.name, description)
            })
            .collect::<Vec<_>>();
        self.bot
            .set_my_commands(commands)
            .scope(BotCommandScope::Chat {
                chat_id: chat_id.into(),
            })
            .await
            .map_err(|error| ChannelError::Transport(error.to_string()))?;
        Ok(())
    }
}

async fn handle_owner_claim(
    claim: Option<&OwnerClaim>,
    policy: &TelegramPolicy,
    bot: &Bot,
    chat_id: ChatId,
    owner_user_id: u64,
    code: &str,
    private_chat: bool,
) {
    if !private_chat {
        return;
    }
    let Some(claim) = claim else {
        return;
    };
    let Some(result) = claim.claim(code, owner_user_id).await else {
        return;
    };
    match result {
        Ok(true) => {
            policy.add_owner(owner_user_id);
            send_claim_response(
                bot,
                chat_id,
                "Owner claimed. This Telegram account can now use Agentix.",
            )
            .await;
        }
        Ok(false) => {
            send_claim_response(
                bot,
                chat_id,
                "The claim code is invalid or expired. Generate a new code in the local Agentix terminal.",
            )
            .await;
        }
        Err(error) => {
            tracing::error!(%error, "failed to persist claimed Telegram owner");
            send_claim_response(
                bot,
                chat_id,
                "Owner claim failed. Check the Agentix server logs and try again.",
            )
            .await;
        }
    }
}

fn parse_claim_command(input: &str) -> Option<&str> {
    let mut words = input.split_whitespace();
    let raw_command = words.next()?;
    let command = raw_command
        .split_once('@')
        .map_or(raw_command, |(name, _)| name);
    if command != "/claim" {
        return None;
    }
    let code = words.next()?;
    words.next().is_none().then_some(code)
}

async fn send_claim_response(bot: &Bot, chat_id: ChatId, text: &str) {
    if let Err(error) = bot.send_message(chat_id, text).await {
        tracing::warn!(%error, "failed to send Telegram owner claim response");
    }
}

#[must_use]
pub fn menu_commands() -> Vec<BotCommand> {
    BASE_MENU_COMMANDS
        .into_iter()
        .map(|(command, description)| BotCommand::new(command, description))
        .collect()
}

#[must_use]
pub fn attached_menu_commands() -> Vec<BotCommand> {
    [
        ("sessions", "Browse running sessions"),
        ("rmux", "Manage rmux workspaces"),
        ("current", "Show the attached session"),
        ("history", "Show recent conversation history"),
        ("queue", "Show queued follow-up messages"),
        ("stop", "Stop the active turn"),
        ("detach", "Detach the current session"),
        ("compact", "Compact the session context"),
        ("fork", "Fork and attach a copy"),
        ("fast", "Toggle Fast mode"),
        ("clear", "Start a fresh session"),
        ("exit", "Detach the session"),
        ("diff", "Show Git changes"),
        ("rename", "Rename the session"),
        ("model", "Show or change the model"),
        ("reasoning", "Show or change reasoning effort"),
        ("skills", "List available skills"),
        ("plan", "Enter plan mode"),
        ("goal", "Show or manage the goal"),
        ("review", "Review uncommitted changes"),
        ("status", "Show detailed session status"),
        ("mcp", "Show MCP server status"),
        ("cancel", "Cancel pending input"),
        ("help", "Show available commands"),
    ]
    .into_iter()
    .map(|(command, description)| {
        let is_base_command = BASE_MENU_COMMANDS
            .iter()
            .any(|(base_command, _)| base_command == &command);
        let description = if is_base_command {
            description.to_owned()
        } else {
            format!("{ATTACHED_COMMAND_MARKER}{description}")
        };
        BotCommand::new(command, description)
    })
    .collect()
}

#[must_use]
pub fn render_text(view: &OutboundView) -> String {
    let mut header = if view.status == ViewStatus::Background {
        format!("🟣 Background\n{}", view.title)
    } else {
        view.title.clone()
    };
    if let Some(subtitle) = &view.subtitle {
        header.push('\n');
        header.push_str(subtitle);
    }
    let header = render_plain_text_with_limit(&header, TELEGRAM_TEXT_LIMIT);
    if view.body.is_empty() || header.len() + 2 >= TELEGRAM_TEXT_LIMIT {
        return header;
    }

    let body_limit = TELEGRAM_TEXT_LIMIT - header.len() - 2;
    format!(
        "{header}\n\n{}",
        render_markdown_with_limit(&view.body, body_limit)
    )
}

#[must_use]
pub fn render_keyboard(actions: &[ActionButton]) -> Option<InlineKeyboardMarkup> {
    if actions.is_empty() {
        return None;
    }
    let rows = actions
        .chunks(2)
        .map(|actions| {
            actions
                .iter()
                .map(|action| {
                    InlineKeyboardButton::callback(action.label.clone(), action.token.clone())
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    Some(InlineKeyboardMarkup::new(rows))
}

fn parse_chat_id(conversation: &ConversationRef) -> Result<ChatId, ChannelError> {
    if conversation.channel != ChannelKind::Telegram {
        return Err(ChannelError::InvalidPayload(
            "Telegram adapter received a non-Telegram conversation".into(),
        ));
    }
    conversation
        .conversation_id
        .parse::<i64>()
        .map(ChatId)
        .map_err(|error| ChannelError::InvalidPayload(error.to_string()))
}

fn render_plain_text_with_limit(text: &str, max_bytes: usize) -> String {
    render_with_limit(text, max_bytes, |value| {
        teloxide::utils::markdown::escape(value)
    })
}

fn render_markdown_with_limit(text: &str, max_bytes: usize) -> String {
    render_with_limit(text, max_bytes, |value| {
        convert_markdown_with_blockquotes(value).map_or_else(
            |error| {
                tracing::warn!(%error, "failed to convert agent Markdown for Telegram");
                teloxide::utils::markdown::escape(value)
            },
            |rendered| rendered.trim_end_matches('\n').to_owned(),
        )
    })
}

fn convert_markdown_with_blockquotes(text: &str) -> telegram_markdown_v2::Result<String> {
    let mut chunks = Vec::new();
    let mut quoted = None;
    let mut lines = Vec::new();
    let mut fence: Option<MarkdownFence> = None;

    for line in text.lines() {
        let quote_content = line
            .strip_prefix('>')
            .map(|content| content.strip_prefix(' ').unwrap_or(content));
        let line_quoted = fence.map_or_else(|| quote_content.is_some(), |fence| fence.quoted);
        let content = if line_quoted {
            quote_content.unwrap_or(line)
        } else {
            line
        };
        if quoted.is_some_and(|current| current != line_quoted) {
            render_markdown_chunk(&mut chunks, quoted.unwrap_or(false), &lines)?;
            lines.clear();
        }
        quoted = Some(line_quoted);
        lines.push(content.to_owned());

        if let Some(marker) = markdown_fence_marker(content) {
            match fence {
                None => {
                    fence = Some(MarkdownFence {
                        marker: marker.marker,
                        length: marker.length,
                        quoted: line_quoted,
                    });
                }
                Some(open)
                    if marker.closing
                        && marker.marker == open.marker
                        && marker.length >= open.length =>
                {
                    fence = None;
                }
                Some(_) => {}
            }
        }
    }
    render_markdown_chunk(&mut chunks, quoted.unwrap_or(false), &lines)?;
    Ok(chunks.join("\n\n"))
}

fn render_markdown_chunk(
    chunks: &mut Vec<String>,
    quoted: bool,
    lines: &[String],
) -> telegram_markdown_v2::Result<()> {
    let source = lines.join("\n");
    let source = source.trim_matches('\n');
    if source.trim().is_empty() {
        return Ok(());
    }
    let rendered = convert_with_strategy(source, UnsupportedTagsStrategy::Escape)?;
    let rendered = rendered.trim_end_matches('\n');
    if quoted {
        let rendered = escape_nested_blockquotes(rendered);
        chunks.push(
            rendered
                .lines()
                .map(|line| {
                    if line.is_empty() {
                        ">".to_owned()
                    } else {
                        format!("> {line}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
    } else {
        chunks.push(rendered.to_owned());
    }
    Ok(())
}

fn escape_nested_blockquotes(text: &str) -> String {
    let mut inside_preformatted = false;
    text.split('\n')
        .map(|line| {
            let fence_boundary = line.starts_with("```");
            let rendered = if inside_preformatted {
                line.to_owned()
            } else {
                escape_blockquote_prefix(line)
            };
            if fence_boundary {
                inside_preformatted = !inside_preformatted;
            }
            rendered
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn escape_blockquote_prefix(line: &str) -> String {
    let mut rest = line;
    let mut escaped = String::new();
    while let Some(after_marker) = rest.strip_prefix('>') {
        escaped.push_str("\\>");
        rest = after_marker;
        if let Some(after_space) = rest.strip_prefix(' ') {
            escaped.push(' ');
            rest = after_space;
        }
    }
    if escaped.is_empty() {
        line.to_owned()
    } else {
        escaped.push_str(rest);
        escaped
    }
}

#[derive(Clone, Copy)]
struct MarkdownFence {
    marker: u8,
    length: usize,
    quoted: bool,
}

struct MarkdownFenceMarker {
    marker: u8,
    length: usize,
    closing: bool,
}

fn markdown_fence_marker(line: &str) -> Option<MarkdownFenceMarker> {
    let indentation = line.bytes().take_while(|byte| *byte == b' ').count();
    if indentation > 3 {
        return None;
    }
    let line = &line[indentation..];
    let marker = *line.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let length = line.bytes().take_while(|byte| *byte == marker).count();
    if length < 3 {
        return None;
    }
    Some(MarkdownFenceMarker {
        marker,
        length,
        closing: line[length..].trim().is_empty(),
    })
}

fn render_with_limit<F>(text: &str, max_bytes: usize, render: F) -> String
where
    F: Fn(&str) -> String,
{
    let rendered = render(text);
    if rendered.len() <= max_bytes {
        return rendered;
    }

    let suffix = "…";
    if max_bytes < suffix.len() {
        return String::new();
    }
    let mut end = text.floor_char_boundary(text.len().min(max_bytes - suffix.len()));
    loop {
        let candidate = format!("{}{suffix}", &text[..end]);
        let rendered = render(&candidate);
        if rendered.len() <= max_bytes {
            return rendered;
        }
        let overflow = rendered.len() - max_bytes;
        let next_end = end.saturating_sub(overflow.max(1));
        end = text.floor_char_boundary(next_end);
    }
}

fn strip_ascii_case_insensitive(text: &str, needle: &str) -> String {
    let mut result = text.to_owned();
    loop {
        let lowercase = result.to_ascii_lowercase();
        let needle = needle.to_ascii_lowercase();
        let Some(position) = lowercase.find(&needle) else {
            return result;
        };
        result.replace_range(position..position + needle.len(), "");
    }
}
