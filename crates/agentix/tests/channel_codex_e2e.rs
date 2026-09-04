// These end-to-end fixtures use Codex's Unix-domain socket transport.
#![cfg(unix)]

#[path = "../../agentix-codex/tests/support/mod.rs"]
#[allow(dead_code)]
mod codex_support;
#[path = "../../agentix-feishu/tests/support/mod.rs"]
#[allow(dead_code)]
mod feishu_support;
#[path = "../../agentix-telegram/tests/support/mod.rs"]
#[allow(dead_code)]
mod telegram_support;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use agentix_codex::CodexClient;
use agentix_core::{AgentAdapter, ChannelAdapter, Engine, SqliteState};
use agentix_feishu::FeishuAdapter;
use agentix_telegram::{TelegramAdapter, TelegramPolicy};
use codex_support::{MockCodexAppServer, MockThread};
use feishu_support::MockFeishuApi;
use larksuite_oapi_sdk_rs::LarkClient;
use telegram_support::MockTelegramApi;
use teloxide::Bot;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn telegram_message_traverses_channel_engine_and_codex_then_updates_telegram() {
    let codex = MockCodexAppServer::start();
    codex
        .add_thread(MockThread::new(
            "thr_telegram_e2e",
            "Telegram integration",
            "/work/telegram",
        ))
        .await;
    let client = Arc::new(CodexClient::connect(codex.endpoint()).await.unwrap());
    let telegram = MockTelegramApi::start().await;
    telegram
        .push_updates(vec![
            telegram_message(100, 10, "/attach thr_telegram_e2e"),
            telegram_message(101, 11, "run the integration"),
        ])
        .await;
    let bot = Bot::new("test-token").set_api_url(telegram.api_url().parse().unwrap());
    let channel = Arc::new(TelegramAdapter::with_bot(bot, TelegramPolicy::new([42])));
    let shutdown = CancellationToken::new();
    let tasks = run_stack(client.clone(), channel, shutdown.clone()).await;

    let turn_id = wait_for_value(|| codex.latest_turn_id("thr_telegram_e2e")).await;
    codex
        .complete_turn("thr_telegram_e2e", &turn_id, "telegram integration answer")
        .await;
    let updated = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let requests = telegram.requests().await;
            if requests
                .iter()
                .any(|request| request.body.contains("telegram integration answer"))
            {
                break true;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or(false);
    assert!(
        updated,
        "Telegram requests: {:?}",
        telegram.requests().await
    );

    shutdown.cancel();
    join_stack(tasks).await;
}

#[tokio::test]
async fn feishu_message_traverses_channel_engine_and_codex_then_updates_feishu() {
    let codex = MockCodexAppServer::start();
    codex
        .add_thread(MockThread::new(
            "thr_feishu_e2e",
            "Feishu integration",
            "/work/feishu",
        ))
        .await;
    let client = Arc::new(CodexClient::connect(codex.endpoint()).await.unwrap());
    let feishu = MockFeishuApi::start().await;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .to_string();
    feishu
        .push_event(feishu_message(
            "om_attach",
            &timestamp,
            "/attach thr_feishu_e2e",
        ))
        .await;
    feishu
        .push_event(feishu_message(
            "om_prompt",
            &timestamp,
            "run the integration",
        ))
        .await;
    let lark = LarkClient::builder("mock-app", "mock-secret")
        .base_url(feishu.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let channel = Arc::new(FeishuAdapter::with_client(lark, ["ou_owner"]));
    let shutdown = CancellationToken::new();
    let tasks = run_stack(client.clone(), channel, shutdown.clone()).await;

    let turn_id = wait_for_value(|| codex.latest_turn_id("thr_feishu_e2e")).await;
    codex
        .complete_turn("thr_feishu_e2e", &turn_id, "feishu integration answer")
        .await;
    wait_until(|| async {
        feishu
            .requests()
            .await
            .iter()
            .any(|request| request.body.contains("feishu integration answer"))
    })
    .await;
    feishu.wait_for_acknowledgements(2).await;

    shutdown.cancel();
    join_stack(tasks).await;
}

struct StackTasks {
    channel: tokio::task::JoinHandle<Result<(), agentix_core::ChannelError>>,
    inbound: tokio::task::JoinHandle<()>,
    events: tokio::task::JoinHandle<()>,
}

async fn run_stack<C>(
    client: Arc<CodexClient>,
    channel: Arc<C>,
    shutdown: CancellationToken,
) -> StackTasks
where
    C: ChannelAdapter + 'static,
{
    let channel_for_engine: Arc<dyn ChannelAdapter> = channel.clone();
    let engine = Arc::new(Engine::new(
        client.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel_for_engine],
    ));
    let (inbound_tx, mut inbound_rx) = mpsc::channel(32);
    let channel_task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { channel.run(inbound_tx, shutdown).await }
    });
    let inbound_task = tokio::spawn({
        let engine = engine.clone();
        let shutdown = shutdown.clone();
        async move {
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    inbound = inbound_rx.recv() => match inbound {
                        Some(inbound) => engine.handle_inbound(inbound).await.unwrap(),
                        None => break,
                    }
                }
            }
        }
    });
    let mut agent_events = client.subscribe();
    let event_task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    event = agent_events.recv() => match event {
                        Ok(event) => engine.handle_agent_event(event).await.unwrap(),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    });
    StackTasks {
        channel: channel_task,
        inbound: inbound_task,
        events: event_task,
    }
}

async fn join_stack(tasks: StackTasks) {
    tokio::time::timeout(Duration::from_secs(2), tasks.channel)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tasks.inbound.await.unwrap();
    tasks.events.await.unwrap();
}

async fn wait_for_value<F, Fut>(mut read: F) -> String
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<String>>,
{
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(value) = read().await {
                return value;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

async fn wait_until<F, Fut>(mut predicate: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if predicate().await {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

fn telegram_message(update_id: u64, message_id: i32, text: &str) -> serde_json::Value {
    serde_json::json!({
        "update_id": update_id,
        "message": {
            "message_id": message_id,
            "date": 1,
            "chat": {"id": 42, "type": "private", "first_name": "Owner"},
            "from": {"id": 42, "is_bot": false, "first_name": "Owner"},
            "text": text
        }
    })
}

fn feishu_message(message_id: &str, timestamp: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "header": {
            "event_id": format!("ev_{message_id}"),
            "event_type": "im.message.receive_v1",
            "app_id": "mock-app",
            "tenant_key": "tenant",
            "create_time": timestamp
        },
        "event": {
            "sender": {
                "sender_id": {"open_id": "ou_owner", "user_id": "owner_user"},
                "sender_type": "user",
                "tenant_key": "tenant"
            },
            "message": {
                "message_id": message_id,
                "chat_id": "oc_mock_chat",
                "chat_type": "p2p",
                "message_type": "text",
                "content": serde_json::json!({"text": text}).to_string(),
                "create_time": timestamp
            }
        }
    })
}
