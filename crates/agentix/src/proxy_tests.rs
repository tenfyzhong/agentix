#[path = "../../agentix-telegram/tests/support/mod.rs"]
#[allow(dead_code)]
mod telegram_support;

#[path = "../../../tests/support/environment_proxy.rs"]
mod environment_proxy;

use std::time::Duration;

use agentix::Config;
use agentix_core::{ChannelAdapter, ChannelKind, ConversationRef, OutboundView};
use agentix_telegram::{TelegramAdapter, TelegramPolicy};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn telegram_polling_sends_edits_and_callbacks_share_the_environment_proxy() {
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
    let mut command = environment_proxy::fixture_command("proxy_tests::telegram_proxy_fixture");
    command.env("http_proxy", proxy.api_url());
    environment_proxy::assert_success(&mut command).await;
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

fn proxy_config() -> Config {
    Config::from_toml(
        r#"[network]
proxy = "http://127.0.0.1:1"
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
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "runs in an isolated proxy environment"]
async fn telegram_proxy_fixture() {
    let config = proxy_config();
    let bot = super::build_telegram_bot(config.channel.telegram.as_ref().unwrap())
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
    let view = OutboundView::text("Proxy", "All requests use the environment proxy");
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
}

#[tokio::test]
async fn proxy_variables_select_the_request_scheme_and_honor_no_proxy() {
    for (variable, https) in [
        ("http_proxy", false),
        ("HTTP_PROXY", false),
        ("https_proxy", true),
        ("HTTPS_PROXY", true),
    ] {
        environment_proxy::check_routing(
            "proxy_tests::http_proxy_fixture",
            variable,
            https,
            None,
            1,
        )
        .await;
    }
    for bypass_variable in ["NO_PROXY", "no_proxy"] {
        environment_proxy::check_routing(
            "proxy_tests::http_proxy_fixture",
            "HTTP_PROXY",
            false,
            Some(bypass_variable),
            1,
        )
        .await;
    }
}

#[tokio::test]
#[ignore = "runs in an isolated proxy environment"]
async fn http_proxy_fixture() {
    let config = proxy_config();
    let bot = super::build_telegram_bot(config.channel.telegram.as_ref().unwrap()).unwrap();
    let base = std::env::var("PROXY_TEST_URL").unwrap();
    let result = bot
        .client()
        .get(format!("{base}/resource"))
        .timeout(Duration::from_secs(2))
        .send()
        .await;
    if base.starts_with("https:") {
        assert!(result.is_err());
    } else {
        assert_eq!(result.unwrap().text().await.unwrap(), "proxied");
    }
}
