mod support;

use std::sync::{Arc, Mutex};

use agentix_core::{
    ActionButton, ActionStyle, ChannelAdapter, ChannelCommand, ChannelKind, CommandMenu,
    ConversationRef, MessageRef, OutboundView, ViewStatus, parse_input,
};
use agentix_telegram::{
    TelegramAdapter, TelegramOwnerClaimer, TelegramPolicy, attached_menu_commands,
    include_reply_context, menu_commands, render_keyboard, render_text,
};
use async_trait::async_trait;
use teloxide::Bot;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use support::MockTelegramApi;

#[derive(Default)]
struct RecordingOwnerClaimer {
    claims: Mutex<Vec<(String, u64)>>,
}

#[async_trait]
impl TelegramOwnerClaimer for RecordingOwnerClaimer {
    async fn claim(&self, code: &str, owner_user_id: u64) -> Result<bool, String> {
        self.claims
            .lock()
            .unwrap()
            .push((code.into(), owner_user_id));
        Ok(true)
    }
}

async fn wait_for_api_method(server: &MockTelegramApi, method: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if server
                .requests()
                .await
                .iter()
                .any(|request| request.target.to_ascii_lowercase().ends_with(method))
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn only_the_owner_can_use_private_chats() {
    let policy = TelegramPolicy::new([42]);

    assert_eq!(
        policy.accept_text(42, true, "hello", "agentix_bot"),
        Some("hello".into())
    );
    assert_eq!(policy.accept_text(7, true, "hello", "agentix_bot"), None);
}

#[test]
fn groups_require_and_strip_the_bot_mention() {
    let policy = TelegramPolicy::new([42]);

    assert_eq!(policy.accept_text(42, false, "hello", "agentix_bot"), None);
    assert_eq!(
        policy.accept_text(42, false, "@agentix_bot /sessions", "agentix_bot"),
        Some("/sessions".into())
    );
    assert_eq!(
        policy.accept_text(42, false, "/current@agentix_bot", "agentix_bot"),
        Some("/current".into())
    );
}

#[test]
fn telegram_reply_context_is_quoted_before_the_new_prompt() {
    assert_eq!(
        include_reply_context("Please revise it", Some("first line\n\nsecond line")),
        "**Quoted message**\n\n> first line\n>\n> second line\n\nPlease revise it"
    );
    assert_eq!(include_reply_context("plain prompt", None), "plain prompt");
}

#[test]
fn telegram_commands_do_not_include_reply_context() {
    assert_eq!(
        include_reply_context("/sessions", Some("an earlier message")),
        "/sessions"
    );
}

#[test]
fn telegram_default_menu_only_lists_commands_available_before_attach() {
    let commands = menu_commands();

    assert_eq!(
        commands
            .iter()
            .map(|command| command.command.as_str())
            .collect::<Vec<_>>(),
        ["sessions", "rmux", "cancel", "help"]
    );
    assert!(
        commands
            .iter()
            .all(|command| !command.description.trim().is_empty())
    );
    assert!(
        commands
            .iter()
            .all(|command| parse_input(&format!("/{}", command.command)).is_ok())
    );
}

#[test]
fn telegram_attached_menu_lists_session_commands() {
    let commands = attached_menu_commands();
    let names = commands
        .iter()
        .map(|command| command.command.as_str())
        .collect::<Vec<_>>();

    for name in [
        "compact",
        "fork",
        "fast",
        "clear",
        "exit",
        "diff",
        "rename",
        "model",
        "reasoning",
        "skills",
        "plan",
        "goal",
        "review",
        "status",
        "mcp",
    ] {
        assert!(names.contains(&name), "attached menu is missing /{name}");
    }
    assert!(!names.contains(&"new"));
    assert!(!names.contains(&"thinking"));
    assert!(
        commands
            .iter()
            .all(|command| parse_input(&format!("/{}", command.command)).is_ok())
    );
    for command in &commands {
        if ["sessions", "rmux", "cancel", "help"].contains(&command.command.as_str()) {
            assert!(!command.description.starts_with("✌️ "));
        } else {
            assert!(
                command.description.starts_with("✌️ "),
                "/{} is missing the attached-only marker",
                command.command
            );
            assert!(!command.description.contains("Attached"));
        }
    }
}

#[tokio::test]
async fn telegram_registers_commands_and_the_private_chat_menu_button() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = tokio::spawn(capture_boolean_requests(listener, 2));
    let bot = Bot::new("test-token").set_api_url(format!("http://{address}/").parse().unwrap());
    let adapter = TelegramAdapter::with_bot(bot, TelegramPolicy::new([42]));

    adapter.register_menu().await.unwrap();
    let requests = requests.await.unwrap();
    let commands: serde_json::Value = serde_json::from_str(&requests[0].1).unwrap();
    let menu_button: serde_json::Value = serde_json::from_str(&requests[1].1).unwrap();

    assert!(
        requests[0].0.ends_with("/SetMyCommands"),
        "unexpected target: {}",
        requests[0].0
    );
    assert_eq!(commands["commands"][0]["command"], "sessions");
    assert_eq!(commands["commands"][1]["command"], "rmux");
    assert_eq!(commands["commands"][3]["command"], "help");
    assert!(requests[1].0.ends_with("/SetChatMenuButton"));
    assert_eq!(menu_button["menu_button"]["type"], "commands");
}

#[tokio::test]
async fn telegram_replaces_the_chat_scoped_menu_when_attachment_changes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = tokio::spawn(capture_boolean_requests(listener, 2));
    let bot = Bot::new("test-token").set_api_url(format!("http://{address}/").parse().unwrap());
    let adapter = TelegramAdapter::with_bot(bot, TelegramPolicy::new([42]));
    let conversation = ConversationRef::new(ChannelKind::Telegram, "42");

    adapter
        .set_command_menu(
            &conversation,
            &CommandMenu::new(vec![
                ChannelCommand::new("sessions", "Browse running sessions"),
                ChannelCommand::new("goal", "Show or manage the goal").contextual(),
            ]),
        )
        .await
        .unwrap();
    adapter
        .set_command_menu(
            &conversation,
            &CommandMenu::new(vec![ChannelCommand::new(
                "sessions",
                "Browse running sessions",
            )]),
        )
        .await
        .unwrap();
    let requests = requests.await.unwrap();
    let attached: serde_json::Value = serde_json::from_str(&requests[0].1).unwrap();
    let detached: serde_json::Value = serde_json::from_str(&requests[1].1).unwrap();

    assert_eq!(attached["scope"]["type"], "chat");
    assert_eq!(attached["scope"]["chat_id"], 42);
    assert!(
        attached["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command["command"] == "goal")
    );
    assert_eq!(detached["scope"]["type"], "chat");
    assert_eq!(detached["commands"].as_array().unwrap().len(), 1);
}

#[test]
fn telegram_views_are_labeled_and_bounded() {
    let view = OutboundView {
        title: "Codex · 9f31c2ab".into(),
        subtitle: Some("Turn 0187 · running".into()),
        body: "你".repeat(2_000),
        status: ViewStatus::Running,
        actions: Vec::new(),
    };
    let rendered = render_text(&view);

    assert!(rendered.starts_with("Codex · 9f31c2ab\nTurn 0187 · running"));
    assert!(rendered.len() <= 4_096);
    assert!(rendered.is_char_boundary(rendered.len()));
}

#[test]
fn agent_markdown_is_rendered_as_telegram_markdown_v2() {
    let view = OutboundView {
        title: "Codex (development)".into(),
        subtitle: Some("Turn 0187 · Completed".into()),
        body: "## Result\n\nUse **bold text**, `cargo test`, and [the docs](https://example.com)."
            .into(),
        status: ViewStatus::Success,
        actions: Vec::new(),
    };

    let rendered = render_text(&view);

    assert!(rendered.starts_with("Codex \\(development\\)\nTurn 0187 · Completed"));
    assert!(rendered.contains("*Result*"));
    assert!(
        rendered.contains("Use *bold text*, `cargo test`, and [the docs](https://example.com)\\.")
    );
    assert!(!rendered.contains("**bold text**"));
}

#[test]
fn agent_reply_blockquotes_keep_markdown_formatting() {
    let view = OutboundView {
        title: "Codex".into(),
        subtitle: Some("Turn 0187 · Completed".into()),
        body: "**🤖 Codex**\n\n> Updated **README.md**:\n>\n> - Preserved [the link](https://example.com)."
            .into(),
        status: ViewStatus::Success,
        actions: Vec::new(),
    };

    let rendered = render_text(&view);

    assert!(rendered.contains(r"> Updated *README\.md*:"), "{rendered}");
    assert!(
        rendered.contains(r"> •   Preserved [the link](https://example.com)\."),
        "{rendered}"
    );
}

#[test]
fn nested_agent_blockquotes_are_escaped_inside_the_outer_quote() {
    let view = OutboundView::text(
        "Codex",
        "**🤖 Codex**\n\n> Summary\n>\n> > quoted text\n>\n> ```console\n> > shell prompt\n> ```",
    );

    let rendered = render_text(&view);

    assert!(rendered.contains("> \\> quoted text"), "{rendered}");
    assert!(!rendered.contains("\n> > quoted text"), "{rendered}");
    assert!(
        rendered.contains("> ```console\n> > shell prompt\n> ```"),
        "{rendered}"
    );
}

#[test]
fn session_blockquotes_keep_each_field_on_its_own_line() {
    let view = OutboundView::text(
        "Existing Codex sessions",
        "> **1 · Parser cleanup**\n> 🟡 **Status:** Idle\n> 📁 **Workspace:** `/work/parser`",
    );

    let rendered = render_text(&view);

    assert!(
        rendered.contains(
            "> *1 · Parser cleanup*\n> 🟡 *Status:* Idle\n> 📁 *Workspace:* `/work/parser`"
        ),
        "{rendered}"
    );
}

#[test]
fn greater_than_sign_inside_a_code_fence_is_not_a_blockquote() {
    let view = OutboundView::text("Codex", "```console\n> prompt\n```");

    let rendered = render_text(&view);

    assert!(rendered.contains("```console\n> prompt\n```"), "{rendered}");
}

#[tokio::test]
async fn telegram_sends_rendered_agent_replies_in_markdown_v2_mode() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let request = tokio::spawn(capture_request(listener));
    let bot = Bot::new("test-token").set_api_url(format!("http://{address}/").parse().unwrap());
    let adapter = TelegramAdapter::with_bot(bot, TelegramPolicy::new([42]));
    let view = OutboundView::text("Codex", "The answer is **bold**.");

    let result = adapter
        .send(&ConversationRef::new(ChannelKind::Telegram, "42"), &view)
        .await;
    let body = request.await.unwrap();
    let payload: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert!(result.is_ok());
    assert_eq!(payload["parse_mode"], "MarkdownV2");
    assert_eq!(payload["link_preview_options"]["is_disabled"], true);
    assert_eq!(payload["text"], "Codex\n\nThe answer is *bold*\\.");
}

#[tokio::test]
async fn telegram_stream_updates_keep_markdown_v2_mode() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let request = tokio::spawn(capture_request(listener));
    let bot = Bot::new("test-token").set_api_url(format!("http://{address}/").parse().unwrap());
    let adapter = TelegramAdapter::with_bot(bot, TelegramPolicy::new([42]));
    let conversation = ConversationRef::new(ChannelKind::Telegram, "42");
    let message = MessageRef::new(conversation.clone(), "1");
    let view = OutboundView::text("Codex", "Still **working**...");

    let result = adapter.update(&conversation, &message, &view).await;
    let body = request.await.unwrap();
    let payload: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert!(result.is_ok());
    assert_eq!(payload["parse_mode"], "MarkdownV2");
    assert_eq!(payload["link_preview_options"]["is_disabled"], true);
    assert_eq!(payload["text"], "Codex\n\nStill *working*\\.\\.\\.");
    assert_eq!(
        payload["reply_markup"]["inline_keyboard"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn telegram_disables_consumed_actions_by_removing_the_keyboard() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let request = tokio::spawn(capture_request(listener));
    let bot = Bot::new("test-token").set_api_url(format!("http://{address}/").parse().unwrap());
    let adapter = TelegramAdapter::with_bot(bot, TelegramPolicy::new([42]));
    let message = MessageRef::new(ConversationRef::new(ChannelKind::Telegram, "42"), "20");

    adapter.disable_actions(&message).await.unwrap();

    let body = request.await.unwrap();
    let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        payload["reply_markup"]["inline_keyboard"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn telegram_mock_long_polling_forwards_only_authorized_messages() {
    let server = MockTelegramApi::start().await;
    server
        .push_updates(vec![
            serde_json::json!({
                "update_id": 100,
                "message": {
                    "message_id": 10,
                    "date": 1,
                    "chat": {"id": 42, "type": "private", "first_name": "Owner"},
                    "from": {"id": 7, "is_bot": false, "first_name": "Stranger"},
                    "text": "ignored"
                }
            }),
            serde_json::json!({
                "update_id": 101,
                "message": {
                    "message_id": 11,
                    "date": 1,
                    "chat": {"id": 42, "type": "private", "first_name": "Owner"},
                    "from": {"id": 42, "is_bot": false, "first_name": "Owner"},
                    "text": "/sessions"
                }
            }),
        ])
        .await;
    let bot = Bot::new("test-token").set_api_url(server.api_url().parse().unwrap());
    let adapter = TelegramAdapter::with_bot(bot, TelegramPolicy::new([42]));
    let shutdown = CancellationToken::new();
    let (sender, mut receiver) = mpsc::channel(4);
    let task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { adapter.run(sender, shutdown).await }
    });

    let envelope = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .unwrap();
    assert!(
        envelope.is_some(),
        "adapter result: {:?}; requests: {:?}",
        task.await,
        server.requests().await
    );
    let envelope = envelope.unwrap();
    assert_eq!(envelope.event_id, "42:11");
    assert_eq!(envelope.conversation.conversation_id, "42");
    assert_eq!(envelope.owner_id, "42");
    assert_eq!(
        envelope.payload,
        agentix_core::InboundPayload::Text("/sessions".into())
    );

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let requests = server.requests().await;
    assert!(requests.iter().all(|request| request.method == "POST"));
    let methods = requests
        .iter()
        .map(|request| request.target.to_ascii_lowercase())
        .collect::<Vec<_>>();
    assert!(methods.iter().any(|target| target.ends_with("/getme")));
    assert!(
        methods
            .iter()
            .any(|target| target.ends_with("/setmycommands"))
    );
    assert!(methods.iter().any(|target| target.ends_with("/getupdates")));
    let polling = requests
        .iter()
        .find(|request| request.target.to_ascii_lowercase().ends_with("/getupdates"))
        .unwrap();
    assert!(polling.body.contains("timeout"));
}

#[tokio::test]
async fn private_claim_persists_the_telegram_owner_and_authorizes_follow_up_messages() {
    let server = MockTelegramApi::start().await;
    server
        .push_updates(vec![serde_json::json!({
            "update_id": 150,
            "message": {
                "message_id": 15,
                "date": 1,
                "chat": {"id": 7, "type": "private", "first_name": "New owner"},
                "from": {"id": 7, "is_bot": false, "first_name": "New owner"},
                "text": "/claim TEST-CODE"
            }
        })])
        .await;
    let bot = Bot::new("test-token").set_api_url(server.api_url().parse().unwrap());
    let claimer = Arc::new(RecordingOwnerClaimer::default());
    let adapter =
        TelegramAdapter::with_bot(bot, TelegramPolicy::new([])).with_owner_claimer(claimer.clone());
    let shutdown = CancellationToken::new();
    let (sender, mut receiver) = mpsc::channel(4);
    let task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { adapter.run(sender, shutdown).await }
    });

    wait_for_api_method(&server, "/sendmessage").await;
    server
        .push_updates(vec![
            serde_json::json!({
                "update_id": 151,
                "message": {
                    "message_id": 16,
                    "date": 1,
                    "chat": {"id": 8, "type": "private", "first_name": "Other"},
                    "from": {"id": 8, "is_bot": false, "first_name": "Other"},
                    "text": "/claim TEST-CODE"
                }
            }),
            serde_json::json!({
                "update_id": 152,
                "message": {
                    "message_id": 17,
                    "date": 1,
                    "chat": {"id": -100, "type": "group", "title": "Group"},
                    "from": {"id": 9, "is_bot": false, "first_name": "Group user"},
                    "text": "/claim TEST-CODE"
                }
            }),
            serde_json::json!({
                "update_id": 153,
                "message": {
                    "message_id": 18,
                    "date": 1,
                    "chat": {"id": 7, "type": "private", "first_name": "New owner"},
                    "from": {"id": 7, "is_bot": false, "first_name": "New owner"},
                    "text": "/sessions"
                }
            }),
        ])
        .await;

    let message = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(message.event_id, "7:18");
    assert_eq!(message.owner_id, "7");
    assert_eq!(
        message.payload,
        agentix_core::InboundPayload::Text("/sessions".into())
    );
    assert_eq!(
        claimer.claims.lock().unwrap().as_slice(),
        [("TEST-CODE".into(), 7)]
    );
    let confirmations = server
        .requests()
        .await
        .into_iter()
        .filter(|request| {
            request
                .target
                .to_ascii_lowercase()
                .ends_with("/sendmessage")
        })
        .collect::<Vec<_>>();
    assert_eq!(confirmations.len(), 1, "claim must be single-use");
    assert!(confirmations[0].body.contains("Owner claimed"));

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn telegram_mock_long_polling_answers_callbacks_and_forwards_actions() {
    let server = MockTelegramApi::start().await;
    server
        .push_updates(vec![serde_json::json!({
            "update_id": 200,
            "callback_query": {
                "id": "callback-1",
                "from": {"id": 42, "is_bot": false, "first_name": "Owner"},
                "chat_instance": "mock-chat-instance",
                "message": {
                    "message_id": 20,
                    "date": 1,
                    "chat": {"id": 42, "type": "private", "first_name": "Owner"},
                    "text": "Choose"
                },
                "data": "opaque-action-token"
            }
        })])
        .await;
    let bot = Bot::new("test-token").set_api_url(server.api_url().parse().unwrap());
    let adapter = TelegramAdapter::with_bot(bot, TelegramPolicy::new([42]));
    let shutdown = CancellationToken::new();
    let (sender, mut receiver) = mpsc::channel(4);
    let task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { adapter.run(sender, shutdown).await }
    });

    let envelope = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("adapter stopped before callback delivery"));
    assert_eq!(envelope.event_id, "callback-1");
    assert_eq!(
        envelope.payload,
        agentix_core::InboundPayload::Action {
            token: "opaque-action-token".into(),
            message: Some(MessageRef::new(
                ConversationRef::new(ChannelKind::Telegram, "42"),
                "20"
            ))
        }
    );

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let requests = server.requests().await;
    let answer = requests
        .iter()
        .find(|request| {
            request
                .target
                .to_ascii_lowercase()
                .ends_with("/answercallbackquery")
        })
        .unwrap();
    assert_eq!(answer.method, "POST");
    let answer_body: serde_json::Value = serde_json::from_str(&answer.body).unwrap();
    assert_eq!(answer_body["callback_query_id"], "callback-1");
}

#[tokio::test]
async fn telegram_retries_rate_limited_menu_registration() {
    let server = MockTelegramApi::start().await;
    server.rate_limit_next("setmycommands", 1).await;
    server.rate_limit_next("setchatmenubutton", 1).await;
    let bot = Bot::new("test-token").set_api_url(server.api_url().parse().unwrap());
    let adapter = TelegramAdapter::with_bot(bot, TelegramPolicy::new([42]));
    let started = std::time::Instant::now();

    tokio::time::timeout(std::time::Duration::from_secs(10), adapter.register_menu())
        .await
        .unwrap()
        .unwrap();

    assert!(started.elapsed() >= std::time::Duration::from_secs(2));
    for method in ["setmycommands", "setchatmenubutton"] {
        let requests = server.requests().await;
        let attempts = requests
            .iter()
            .filter(|r| r.target.to_ascii_lowercase().ends_with(method))
            .collect::<Vec<_>>();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].body, attempts[1].body);
    }
}

#[tokio::test]
async fn telegram_retries_rate_limited_turn_messages_and_controls() {
    let server = MockTelegramApi::start().await;
    let bot = Bot::new("test-token").set_api_url(server.api_url().parse().unwrap());
    let adapter = TelegramAdapter::with_bot(bot, TelegramPolicy::new([42]));
    let conversation = ConversationRef::new(ChannelKind::Telegram, "42");
    let view = OutboundView::text("Restored turn", "Saved response");
    let message = MessageRef::new(conversation.clone(), "77");

    for method in [
        "sendmessage",
        "editmessagetext",
        "editmessagereplymarkup",
        "setmycommands",
    ] {
        server.rate_limit_next(method, 1).await;
        let started = std::time::Instant::now();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            match method {
                "sendmessage" => adapter.send(&conversation, &view).await.map(|_| ()),
                "editmessagetext" => adapter.update(&conversation, &message, &view).await,
                "editmessagereplymarkup" => adapter.disable_actions(&message).await,
                _ => {
                    adapter
                        .set_command_menu(&conversation, &CommandMenu::default())
                        .await
                }
            }
        })
        .await
        .unwrap()
        .unwrap();
        assert!(started.elapsed() >= std::time::Duration::from_secs(1));
        let requests = server.requests().await;
        let attempts = requests
            .iter()
            .filter(|r| r.target.to_ascii_lowercase().ends_with(method))
            .collect::<Vec<_>>();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].body, attempts[1].body);
    }
}

#[tokio::test]
async fn telegram_mock_api_errors_are_channel_transport_errors() {
    let server = MockTelegramApi::start().await;
    server
        .fail_next("sendmessage", "mock Telegram send failure")
        .await;
    let bot = Bot::new("test-token").set_api_url(server.api_url().parse().unwrap());
    let adapter = TelegramAdapter::with_bot(bot, TelegramPolicy::new([42]));

    let error = adapter
        .send(
            &ConversationRef::new(ChannelKind::Telegram, "42"),
            &OutboundView::text("Failure", "Expected"),
        )
        .await
        .unwrap_err();

    assert!(
        matches!(error, agentix_core::ChannelError::Transport(message) if message.contains("mock Telegram send failure"))
    );
}

#[test]
fn callback_keyboards_only_expose_opaque_tokens() {
    let keyboard = render_keyboard(&[
        ActionButton {
            label: "Allow once".into(),
            token: "0123456789abcdef".into(),
            style: ActionStyle::Primary,
        },
        ActionButton {
            label: "Decline".into(),
            token: "fedcba9876543210".into(),
            style: ActionStyle::Danger,
        },
    ])
    .unwrap();
    let encoded = serde_json::to_value(keyboard).unwrap();

    assert_eq!(
        encoded["inline_keyboard"][0][0]["callback_data"],
        "0123456789abcdef"
    );
    assert_eq!(encoded["inline_keyboard"][0][0]["text"], "Allow once");
}

async fn capture_request(listener: TcpListener) -> String {
    let (mut stream, _) = listener.accept().await.unwrap();
    let (_, body) = read_http_request(&mut stream).await;
    write_json_response(
        &mut stream,
        r#"{"ok":true,"result":{"message_id":1,"date":0,"chat":{"id":42,"type":"private","first_name":"Test"},"text":"sent"}}"#,
    )
    .await;
    body
}

async fn capture_boolean_requests(listener: TcpListener, count: usize) -> Vec<(String, String)> {
    let mut requests = Vec::with_capacity(count);
    for _ in 0..count {
        let (mut stream, _) = listener.accept().await.unwrap();
        requests.push(read_http_request(&mut stream).await);
        write_json_response(&mut stream, r#"{"ok":true,"result":true}"#).await;
    }
    requests
}

async fn read_http_request(stream: &mut TcpStream) -> (String, String) {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let (header_end, content_length) = loop {
        let read = stream.read(&mut buffer).await.unwrap();
        assert_ne!(read, 0, "request ended before its headers were complete");
        request.extend_from_slice(&buffer[..read]);
        if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&request[..header_end]).unwrap();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(str::parse::<usize>)
                })
                .expect("request should have a Content-Length header")
                .unwrap();
            break (header_end + 4, content_length);
        }
    };
    while request.len() < header_end + content_length {
        let read = stream.read(&mut buffer).await.unwrap();
        assert_ne!(read, 0, "request ended before its body was complete");
        request.extend_from_slice(&buffer[..read]);
    }
    let headers = std::str::from_utf8(&request[..header_end]).unwrap();
    let target = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("request should have an HTTP target")
        .to_owned();
    let body =
        String::from_utf8(request[header_end..header_end + content_length].to_vec()).unwrap();
    (target, body)
}

async fn write_json_response(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
}

#[test]
fn background_turns_have_a_distinct_marker_and_preserve_quotes() {
    let mut view = OutboundView {
        title: "Codex · Background task".into(),
        subtitle: Some("Background turn 12345678 · Completed".into()),
        body: "**🤖 Codex**\n\n> Completed **the task**.".into(),
        status: serde_json::from_str("\"background\"").unwrap(),
        actions: Vec::new(),
    };
    let background = render_text(&view);
    assert!(background.starts_with("⚫ Background\n"));
    assert!(background.contains("> Completed *the task*\\."));
    view.status = ViewStatus::Success;
    assert!(!render_text(&view).starts_with("⚫ Background"));
}

#[tokio::test]
async fn telegram_cooldown_is_shared_between_cloned_adapters_and_api_methods() {
    let server = MockTelegramApi::start().await;
    server.rate_limit_next("sendmessage", 1).await;
    let bot = Bot::new("test-token").set_api_url(server.api_url().parse().unwrap());
    let adapter = TelegramAdapter::with_bot(bot, TelegramPolicy::new([42]));
    let sender = adapter.clone();
    let send = tokio::spawn(async move {
        sender
            .send(
                &ConversationRef::new(ChannelKind::Telegram, "42"),
                &OutboundView::text("First", "Message"),
            )
            .await
    });
    wait_for_api_method(&server, "sendmessage").await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let conversation = ConversationRef::new(ChannelKind::Telegram, "43");
    let menu = CommandMenu::default();
    let mut update = Box::pin(adapter.set_command_menu(&conversation, &menu));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(300), &mut update)
            .await
            .is_err(),
        "another API method must wait for the same bot cooldown"
    );
    assert_eq!(server.requests().await.len(), 1);
    tokio::time::timeout(std::time::Duration::from_secs(5), update)
        .await
        .unwrap()
        .unwrap();
    send.await.unwrap().unwrap();
}

#[tokio::test]
async fn telegram_spaces_sends_and_edits_in_the_same_chat() {
    let server = MockTelegramApi::start().await;
    let bot = Bot::new("test-token").set_api_url(server.api_url().parse().unwrap());
    let adapter = TelegramAdapter::with_bot(bot, TelegramPolicy::new([42]));
    for (chat, minimum_ms) in [("42", 1_000), ("-42", 3_000)] {
        let conversation = ConversationRef::new(ChannelKind::Telegram, chat);
        let view = OutboundView::text("Turn", "Working");
        let message = adapter.send(&conversation, &view).await.unwrap();
        let started = std::time::Instant::now();
        adapter
            .clone()
            .update(&conversation, &message, &view)
            .await
            .unwrap();
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(minimum_ms),
            "message edits must share the per-chat send budget"
        );
    }
}

#[tokio::test]
async fn telegram_rate_limited_head_is_retried_before_later_requests() {
    let server = MockTelegramApi::start().await;
    server.rate_limit_next("sendmessage", 1).await;
    server.rate_limit_next("sendmessage", 1).await;
    let bot = Bot::new("test-token").set_api_url(server.api_url().parse().unwrap());
    let adapter = TelegramAdapter::with_bot(bot, TelegramPolicy::new([42]));
    let sender = adapter.clone();
    let head = tokio::spawn(async move {
        sender
            .send(
                &ConversationRef::new(ChannelKind::Telegram, "42"),
                &OutboundView::text("First", "Retry this before later requests"),
            )
            .await
    });
    wait_for_api_method(&server, "sendmessage").await;
    tokio::time::timeout(std::time::Duration::from_secs(6), async {
        adapter
            .set_command_menu(
                &ConversationRef::new(ChannelKind::Telegram, "43"),
                &CommandMenu::default(),
            )
            .await
            .unwrap();
        head.await.unwrap().unwrap();
    })
    .await
    .unwrap();
    let methods = server
        .requests()
        .await
        .iter()
        .map(|request| {
            request
                .target
                .rsplit('/')
                .next()
                .unwrap()
                .to_ascii_lowercase()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        methods,
        ["sendmessage", "sendmessage", "sendmessage", "setmycommands"]
    );
}

#[tokio::test]
async fn telegram_inbound_action_does_not_wait_for_rate_limited_acknowledgement() {
    let server = MockTelegramApi::start().await;
    server.rate_limit_next("answercallbackquery", 4).await;
    server
        .push_updates(vec![serde_json::json!({
            "update_id": 200,
            "callback_query": {
                "id": "callback-1",
                "from": {"id": 42, "is_bot": false, "first_name": "Owner"},
                "chat_instance": "mock-chat-instance",
                "message": {
                    "message_id": 20,
                    "date": 1,
                    "chat": {"id": 42, "type": "private", "first_name": "Owner"},
                    "text": "Choose"
                },
                "data": "opaque-action-token"
            }
        })])
        .await;
    let bot = Bot::new("test-token").set_api_url(server.api_url().parse().unwrap());
    let adapter = TelegramAdapter::with_bot(bot, TelegramPolicy::new([42]));
    let shutdown = CancellationToken::new();
    let (sender, mut receiver) = mpsc::channel(4);
    let task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { adapter.run(sender, shutdown).await }
    });

    wait_for_api_method(&server, "answercallbackquery").await;
    let envelope = tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv())
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("adapter stopped before callback delivery"));
    assert_eq!(envelope.event_id, "callback-1");
    assert_eq!(
        envelope.payload,
        agentix_core::InboundPayload::Action {
            token: "opaque-action-token".into(),
            message: Some(MessageRef::new(
                ConversationRef::new(ChannelKind::Telegram, "42"),
                "20"
            ))
        }
    );

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(6), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let requests = server.requests().await;
    let answer = requests
        .iter()
        .find(|request| {
            request
                .target
                .to_ascii_lowercase()
                .ends_with("/answercallbackquery")
        })
        .unwrap();
    assert_eq!(answer.method, "POST");
    let answer_body: serde_json::Value = serde_json::from_str(&answer.body).unwrap();
    assert_eq!(answer_body["callback_query_id"], "callback-1");
}
