#[path = "../../agentix-telegram/tests/support/mod.rs"]
#[allow(dead_code)]
mod telegram_support;

use std::time::Duration;

use agentix::Config;
use agentix_core::{ChannelAdapter, ChannelKind, ConversationRef, OutboundView};
use agentix_telegram::{TelegramAdapter, TelegramPolicy};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn telegram_polling_sends_edits_and_callbacks_share_the_global_proxy() {
    let proxy = telegram_support::MockTelegramApi::start().await;
    proxy
        .push_updates(vec![json!({
            "update_id": 1,
            "callback_query": {
                "id": "callback-proxy",
                "from": {"id": 42, "is_bot": false, "first_name": "Owner"},
                "chat_instance": "mock-chat",
                "message": {
                    "message_id": 20, "date": 1,
                    "chat": {"id": 42, "type": "private", "first_name": "Owner"},
                    "text": "Choose an action"
                },
                "data": "opaque-token"
            }
        })])
        .await;
    let config = Config::from_toml(&format!(
        r#"[network]
proxy = "{}"
[channel]
kind = "telegram"
[channel.telegram]
token = "mock-token"
owner_user_ids = [42]
[agent]
kind = "codex"
[storage]
path = "/tmp/unused.sqlite3"
"#,
        proxy.api_url()
    ))
    .unwrap();
    let bot = super::build_telegram_bot(config.channel.telegram.as_ref().unwrap(), &config.network)
        .unwrap()
        .set_api_url("http://telegram.invalid/".parse().unwrap());
    let adapter = TelegramAdapter::with_bot(bot, TelegramPolicy::new([42]));
    let shutdown = CancellationToken::new();
    let (sender, mut receiver) = mpsc::channel(4);
    let task = tokio::spawn({
        let adapter = adapter.clone();
        let shutdown = shutdown.clone();
        async move { adapter.run(sender, shutdown).await }
    });
    let envelope = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(envelope.event_id, "callback-proxy");
    let conversation = ConversationRef::new(ChannelKind::Telegram, "42");
    let view = OutboundView::text("Proxy", "All requests use the configured proxy");
    let message = adapter.send(&conversation, &view).await.unwrap();
    adapter
        .update(&conversation, &message, &view)
        .await
        .unwrap();
    adapter.disable_actions(&message).await.unwrap();
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let requests = proxy.requests().await;
    for method in [
        "getme",
        "getupdates",
        "setmycommands",
        "setchatmenubutton",
        "answercallbackquery",
        "sendmessage",
        "editmessagetext",
        "editmessagereplymarkup",
    ] {
        assert!(
            requests
                .iter()
                .any(|request| request.target.to_ascii_lowercase().ends_with(method)),
            "missing {method}"
        );
    }
    assert!(
        requests
            .iter()
            .all(|request| request.target.starts_with("http://telegram.invalid/"))
    );
}
