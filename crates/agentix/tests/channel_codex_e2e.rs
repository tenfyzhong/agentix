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

// Multi-message round trips must allow Telegram's 1.1-second per-chat pacing.
const ROUND_TRIP_TIMEOUT: Duration = Duration::from_secs(10);

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
    // Allow the chat budget to pace the start acknowledgement and final edit.
    let updated = tokio::time::timeout(ROUND_TRIP_TIMEOUT, async {
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
    engine: Arc<Engine>,
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
    run_stack_with_tasks(client, channel, shutdown, None).await
}

async fn run_stack_with_tasks<C>(
    client: Arc<CodexClient>,
    channel: Arc<C>,
    shutdown: CancellationToken,
    task_board: Option<Arc<agentix_task::Service>>,
) -> StackTasks
where
    C: ChannelAdapter + 'static,
{
    let channel_for_engine: Arc<dyn ChannelAdapter> = channel.clone();
    let mut engine = Engine::new(
        client.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel_for_engine],
    );
    if let Some(service) = task_board {
        engine = engine.with_task_board(service);
    }
    let engine = Arc::new(engine);
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
        let engine = engine.clone();
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
        engine,
        channel: channel_task,
        inbound: inbound_task,
        events: event_task,
    }
}

async fn task_board_fixture() -> (tempfile::TempDir, Arc<agentix_task::Service>, String) {
    use agentix_task::{
        Config, DocumentConfig, DocumentFormat, Service, StorageConfig, WriteOptions,
    };
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let service = Arc::new(
        Service::open(Config {
            schema_version: 1,
            storage: StorageConfig {
                path: dir.path().join("tasks.sqlite3"),
            },
            documents: DocumentConfig {
                format: DocumentFormat::Markdown,
                root: dir.path().to_owned(),
                directory: "docs".into(),
            },
        })
        .await
        .unwrap(),
    );
    let project = service
        .execute(
            json!({"command":"project.register","root":dir.path(),"name":"Channel tests"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let job = service
        .execute(
            json!({"command":"job.create","project":project["id"],"title":"IM integration"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let task = service
        .execute(
            json!({"command":"task.add","job":job["id"],"title":"Channel task"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let id = task["id"].as_str().unwrap().to_owned();
    let claim = service.execute(json!({"command":"task.claim","task":id,"executor":"agent:codex","session":"thr_tasks"}),WriteOptions::default()).await.unwrap().result;
    let options = WriteOptions {
        session_ref: Some("thr_tasks".into()),
        lease_token: claim["lease"]["token"].as_str().map(str::to_owned),
        ..WriteOptions::default()
    };
    service
        .execute(
            json!({"command":"plan.create","task":id,"body":"# Plan"}),
            options.clone(),
        )
        .await
        .unwrap();
    service
        .execute(json!({"command":"task.start","task":id}), options)
        .await
        .unwrap();
    (dir, service, id)
}

fn task_button_token(value: &serde_json::Value) -> Option<String> {
    if value["text"] == "Wait" {
        return value["callback_data"].as_str().map(str::to_owned);
    }
    if value["text"]["content"] == "Wait" {
        return value["behaviors"][0]["value"]["token"]
            .as_str()
            .map(str::to_owned);
    }
    match value {
        serde_json::Value::Object(values) => values.values().find_map(task_button_token),
        serde_json::Value::Array(values) => values.iter().find_map(task_button_token),
        serde_json::Value::String(value) => {
            serde_json::from_str(value)
                .ok()
                .and_then(|value: serde_json::Value| {
                    if value.is_object() {
                        task_button_token(&value)
                    } else {
                        None
                    }
                })
        }
        _ => None,
    }
}

#[tokio::test]
async fn telegram_task_button_reason_database_projection_and_notification_round_trip() {
    let (_dir, service, id) = task_board_fixture().await;
    let codex = MockCodexAppServer::start();
    codex
        .add_thread(MockThread::new(
            "thr_tasks",
            "Task integration",
            "/work/tasks",
        ))
        .await;
    let client = Arc::new(CodexClient::connect(codex.endpoint()).await.unwrap());
    let server = MockTelegramApi::start().await;
    server
        .push_updates(vec![
            telegram_message(300, 30, "/attach thr_tasks"),
            telegram_message(301, 31, &format!("/task {id}")),
        ])
        .await;
    let channel = Arc::new(TelegramAdapter::with_bot(
        Bot::new("test-token").set_api_url(server.api_url().parse().unwrap()),
        TelegramPolicy::new([42]),
    ));
    let shutdown = CancellationToken::new();
    let stack =
        run_stack_with_tasks(client, channel, shutdown.clone(), Some(service.clone())).await;
    let token = wait_for_value(|| async {
        server
            .requests()
            .await
            .iter()
            .filter_map(|r| serde_json::from_str(&r.body).ok())
            .find_map(|v| task_button_token(&v))
    })
    .await;
    server.push_updates(vec![serde_json::json!({"update_id":302,"callback_query":{"id":"task-callback","from":{"id":42,"is_bot":false,"first_name":"Owner"},"chat_instance":"mock-chat","message":{"message_id":77,"date":1,"chat":{"id":42,"type":"private","first_name":"Owner"},"text":"Task"},"data":token}}),telegram_message(303,33,"Task channel reason")]).await;
    wait_until(|| async {
        service.store().snapshot().await.unwrap().tasks[0].status
            == agentix_task::TaskStatus::WaitingUser
    })
    .await;
    stack.engine.refresh_task_board().await.unwrap();
    let requests = server.requests().await;
    assert!(
        requests
            .iter()
            .any(|r| r.body.contains("Task update") && r.body.contains("Task channel reason"))
    );
    assert!(requests.iter().any(|r| r.body.contains("task-callback")));
    assert_task_projection(&service, "Task channel reason").await;
    shutdown.cancel();
    join_stack(stack).await;
}

#[tokio::test]
async fn feishu_task_card_reason_database_projection_and_notification_round_trip() {
    let (_dir, service, id) = task_board_fixture().await;
    let codex = MockCodexAppServer::start();
    codex
        .add_thread(MockThread::new(
            "thr_tasks",
            "Task integration",
            "/work/tasks",
        ))
        .await;
    let client = Arc::new(CodexClient::connect(codex.endpoint()).await.unwrap());
    let server = MockFeishuApi::start().await;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .to_string();
    server
        .push_event(feishu_message(
            "task_attach",
            &timestamp,
            "/attach thr_tasks",
        ))
        .await;
    server
        .push_event(feishu_message(
            "task_show",
            &timestamp,
            &format!("/task {id}"),
        ))
        .await;
    let lark = LarkClient::builder("mock-app", "mock-secret")
        .base_url(server.base_url())
        .max_retries(1)
        .build()
        .unwrap();
    let channel = Arc::new(FeishuAdapter::with_client(lark, ["ou_owner"]));
    let shutdown = CancellationToken::new();
    let stack =
        run_stack_with_tasks(client, channel, shutdown.clone(), Some(service.clone())).await;
    let token = wait_for_value(|| async {
        server
            .requests()
            .await
            .iter()
            .filter_map(|r| serde_json::from_str(&r.body).ok())
            .find_map(|v| task_button_token(&v))
    })
    .await;
    server.push_event(serde_json::json!({"schema":"2.0","header":{"event_id":"task_action","event_type":"card.action.trigger","app_id":"mock-app","tenant_key":"tenant","create_time":timestamp},"event":{"operator":{"open_id":"ou_owner","user_id":"owner_user","tenant_key":"tenant"},"context":{"open_message_id":"om_mock_message","open_chat_id":"oc_mock_chat"},"action":{"tag":"button","name":"wait","value":{"token":token}},"token":"verification-token","host":"feishu","delivery_type":"push"}})).await;
    server
        .push_event(feishu_message(
            "task_reason",
            &timestamp,
            "Task channel reason",
        ))
        .await;
    wait_until(|| async {
        service.store().snapshot().await.unwrap().tasks[0].status
            == agentix_task::TaskStatus::WaitingUser
    })
    .await;
    stack.engine.refresh_task_board().await.unwrap();
    assert!(
        server
            .requests()
            .await
            .iter()
            .any(|r| r.body.contains("Task update") && r.body.contains("Task channel reason"))
    );
    server.wait_for_acknowledgements(4).await;
    assert_task_projection(&service, "Task channel reason").await;
    shutdown.cancel();
    join_stack(stack).await;
}

async fn assert_task_projection(service: &agentix_task::Service, reason: &str) {
    let state = service.store().snapshot().await.unwrap();
    assert!(state.leases.is_empty());
    assert_eq!(state.tasks[0].reason.as_deref(), Some(reason));
    service.sync().await.unwrap();
    let body = std::fs::read_to_string(
        service
            .config()
            .output_dir()
            .join(&state.jobs[0].document_path),
    )
    .unwrap();
    assert_eq!(state.tasks[0].status, agentix_task::TaskStatus::WaitingUser);
    assert!(body.contains(&format!("- [?] {}", state.tasks[0].name)) && body.contains(reason));
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
    tokio::time::timeout(ROUND_TRIP_TIMEOUT, async {
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
    tokio::time::timeout(ROUND_TRIP_TIMEOUT, async {
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
