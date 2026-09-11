mod support;
use agentix_domain::{
    ActionButton, ActionStyle, ChannelAdapter, ChannelKind, ConversationRef, OutboundView,
};
use agentix_slack::SlackAdapter;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use support::Server;
use tokio::{net::TcpListener, sync::mpsc, time::timeout};
use tokio_util::sync::CancellationToken;

fn adapter(server: &Server) -> SlackAdapter {
    SlackAdapter::with_client(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        server.url.parse().unwrap(),
        "xoxb-test",
        "xapp-test",
        vec!["U1".into()],
    )
    .unwrap()
}

#[tokio::test]
async fn send_update_and_disable_use_bot_token_and_preserve_thread() {
    let mut server = Server::new(|_| (200, vec![], json!({"ok":true,"ts":"3.000001"}))).await;
    let adapter = adapter(&server);
    let conversation = ConversationRef::new(ChannelKind::Slack, "T1:C1:1.000001");
    let mut view = OutboundView::text("Title", "Body");
    view.actions.push(ActionButton {
        label: "Go".into(),
        token: "token".into(),
        style: ActionStyle::Primary,
    });
    let message = adapter.send(&conversation, &view).await.unwrap();
    let request = server.requests.recv().await.unwrap();
    assert_eq!(request.path, "/api/chat.postMessage");
    assert_eq!(request.authorization, "Bearer xoxb-test");
    assert_eq!(request.body["channel"], "C1");
    assert_eq!(request.body["thread_ts"], "1.000001");
    view.body = "Updated".into();
    adapter
        .update(&conversation, &message, &view)
        .await
        .unwrap();
    let request = server.requests.recv().await.unwrap();
    assert_eq!(request.path, "/api/chat.update");
    assert_eq!(request.body["ts"], "3.000001");
    adapter.disable_actions(&message).await.unwrap();
    let request = server.requests.recv().await.unwrap();
    assert_eq!(request.path, "/api/chat.update");
    assert!(
        !request.body["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["type"] == "actions")
    );
    assert!(request.body["text"].as_str().unwrap().contains("Updated"));
    let wrong = ConversationRef::new(ChannelKind::Slack, "T1:C2");
    assert!(adapter.update(&wrong, &message, &view).await.is_err());
}

#[tokio::test]
async fn retries_rate_limits_but_reports_api_rejections_without_tokens() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let server = Server::new(move |_| {
        if count.fetch_add(1, Ordering::SeqCst) == 0 {
            (
                429,
                vec![("Retry-After".into(), "0".into())],
                json!({"ok":false,"error":"ratelimited"}),
            )
        } else {
            (200, vec![], json!({"ok":true,"ts":"1.1"}))
        }
    })
    .await;
    let adapter = adapter(&server);
    adapter
        .send(
            &ConversationRef::new(ChannelKind::Slack, "T1:D1"),
            &OutboundView::text("a", "b"),
        )
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let server = Server::new(|_| (200, vec![], json!({"ok":false,"error":"invalid_auth"}))).await;
    let error = crate::adapter(&server)
        .send(
            &ConversationRef::new(ChannelKind::Slack, "T1:D1"),
            &OutboundView::text("a", "b"),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("invalid_auth"));
    assert!(!error.contains("xoxb"));
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Exercise ACKs, native commands, and reconnect in one socket lifecycle.
async fn socket_mode_authenticates_acks_delivers_and_reconnects() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_url = format!("ws://{}", listener.local_addr().unwrap());
    let mut server = Server::new(move |request| {
        (
            200,
            vec![],
            match request.path.as_str() {
                "/api/auth.test" => json!({"ok":true,"user_id":"BOT","team_id":"T1"}),
                "/api/apps.connections.open" => json!({"ok":true,"url":ws_url}),
                _ => panic!("unexpected request"),
            },
        )
    })
    .await;
    let adapter = adapter(&server);
    let (tx, mut rx) = mpsc::channel(8);
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    let run = tokio::spawn(async move { adapter.run(tx, stop).await });
    let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
    socket.send(json!({"type":"events_api","envelope_id":"env","payload":{"team_id":"T1","event":{"type":"message","channel":"D1","channel_type":"im","user":"U1","text":"/sessions","ts":"1.000001"}}}).to_string().into()).await.unwrap();
    let ack = timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&ack).unwrap()["envelope_id"],
        "env"
    );
    assert_eq!(
        timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap()
            .owner_id,
        "U1"
    );
    assert_eq!(
        server.requests.recv().await.unwrap().authorization,
        "Bearer xoxb-test"
    );
    assert_eq!(
        server.requests.recv().await.unwrap().authorization,
        "Bearer xapp-test"
    );
    socket
        .send(
            json!({"type":"slash_commands","envelope_id":"native-sessions",
        "payload":{"team_id":"T1","user_id":"U1","channel_id":"D1",
            "command":"/sessions","text":""}})
            .to_string()
            .into(),
        )
        .await
        .unwrap();
    let ack = timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&ack).unwrap()["envelope_id"],
        "native-sessions"
    );
    let input = timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        input.payload,
        agentix_domain::InboundPayload::Text("/sessions".into())
    );
    socket
        .send(
            json!({"type":"disconnect","reason":"refresh_requested"})
                .to_string()
                .into(),
        )
        .await
        .unwrap();
    let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
    // Fill the engine-facing queue without draining it. ACKs must stay responsive.
    for index in 0..24 {
        socket.send(json!({"type":"events_api","envelope_id":format!("backpressure-{index}"),"payload":{"team_id":"T1","event":{"type":"message","channel":"D1","user":"U1","text":"hello","ts":format!("{index}.000002")}}}).to_string().into()).await.unwrap();
        timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    shutdown.cancel();
    timeout(Duration::from_secs(2), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

struct Claimer(AtomicUsize);
#[async_trait::async_trait]
impl agentix_slack::SlackOwnerClaimer for Claimer {
    async fn claim(&self, code: &str, owner: &str) -> Result<bool, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(code == "VALID" && owner == "U1")
    }
}

#[tokio::test]
async fn owner_bootstrap_accepts_only_private_claim_and_enables_owner_once() {
    check_owner_bootstrap(false).await;
}

#[tokio::test]
async fn native_claim_bootstrap_accepts_code_and_enables_owner_once() {
    check_owner_bootstrap(true).await;
}

async fn check_owner_bootstrap(native: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_url = format!("ws://{}", listener.local_addr().unwrap());
    let server = Server::new(move |request| {
        (
            200,
            vec![],
            match request.path.as_str() {
                "/api/auth.test" => json!({"ok":true,"user_id":"BOT","team_id":"T1"}),
                "/api/apps.connections.open" => json!({"ok":true,"url":ws_url}),
                _ => json!({"ok":true,"ts":"5.1"}),
            },
        )
    })
    .await;
    let claimer = Arc::new(Claimer(AtomicUsize::new(0)));
    let adapter = SlackAdapter::with_client(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        server.url.parse().unwrap(),
        "bot",
        "app",
        vec![],
    )
    .unwrap()
    .with_owner_claimer(claimer.clone());
    let (tx, mut rx) = mpsc::channel(8);
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    let run = tokio::spawn(async move { adapter.run(tx, stop).await });
    let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
    for (index, channel, text) in [
        (0, "C1", "<@BOT> /claim VALID"),
        (1, "D1", "/claim WRONG"),
        (2, "D1", "/claim VALID"),
        (3, "D1", "/claim VALID"),
        (4, "D1", "/sessions"),
    ] {
        let event = if index == 2 || index == 3 {
            json!({"type":"slash_commands","envelope_id":index.to_string(),"payload":{"team_id":"T1","user_id":"U1","channel_id":channel,"command":if native {"/claim"} else {"/agentix"},"text":if native {text.strip_prefix("/claim ").unwrap()} else {text}}})
        } else {
            json!({"type":"events_api","envelope_id":index.to_string(),"payload":{"team_id":"T1","event":{"type":"message","channel":channel,"user":"U1","text":text,"ts":format!("{index}.000001")}}})
        };
        socket.send(event.to_string().into()).await.unwrap();
        timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    let input = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        input.payload,
        agentix_domain::InboundPayload::Text("/sessions".into())
    );
    assert_eq!(claimer.0.load(Ordering::SeqCst), 2);
    shutdown.cancel();
    run.await.unwrap().unwrap();
}

#[tokio::test]
async fn command_menu_reuses_one_message_and_skips_unchanged_updates() {
    use agentix_domain::{ChannelCommand, CommandMenu};
    let mut server = Server::new(|_| (200, vec![], json!({"ok":true,"ts":"1.000001"}))).await;
    let adapter = adapter(&server);
    let conversation = ConversationRef::new(ChannelKind::Slack, "T1:D1");
    let menu = CommandMenu::new(vec![ChannelCommand::new("sessions", "List sessions")]);
    adapter
        .set_command_menu(&conversation, &menu)
        .await
        .unwrap();
    assert_eq!(
        server.requests.recv().await.unwrap().path,
        "/api/chat.postMessage"
    );
    adapter
        .set_command_menu(&conversation, &menu)
        .await
        .unwrap();
    assert!(server.requests.try_recv().is_err());
    let menu = CommandMenu::new(vec![ChannelCommand::new("cancel", "Stop")]);
    adapter
        .set_command_menu(&conversation, &menu)
        .await
        .unwrap();
    let request = server.requests.recv().await.unwrap();
    assert_eq!(request.path, "/api/chat.update");
    assert_eq!(request.body["ts"], "1.000001");
    assert!(
        request.body["text"]
            .as_str()
            .unwrap()
            .contains("/agentix /cancel")
    );
}

#[tokio::test]
async fn posts_share_channel_budget_across_threads_without_blocking_other_channels() {
    let server = Server::new(|_| (200, vec![], json!({"ok":true,"ts":"1.000001"}))).await;
    let adapter = adapter(&server);
    let first = ConversationRef::new(ChannelKind::Slack, "T1:C1:1.000001");
    let sibling = ConversationRef::new(ChannelKind::Slack, "T1:C1:2.000001");
    let other = ConversationRef::new(ChannelKind::Slack, "T1:C2");
    let view = OutboundView::text("Title", "Body");
    adapter.send(&first, &view).await.unwrap();
    assert!(
        timeout(Duration::from_millis(150), adapter.send(&sibling, &view))
            .await
            .is_err()
    );
    timeout(Duration::from_millis(500), adapter.send(&other, &view))
        .await
        .unwrap()
        .unwrap();
    // Cancellation of the waiting sibling must not erase the first send's budget.
    assert!(
        timeout(Duration::from_millis(150), adapter.send(&first, &view))
            .await
            .is_err()
    );
    timeout(Duration::from_secs(2), adapter.send(&sibling, &view))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn cancelled_retry_preserves_method_cooldown_for_other_conversations() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let server = Server::new(move |_| {
        if count.fetch_add(1, Ordering::SeqCst) == 0 {
            (
                429,
                vec![("Retry-After".into(), "1".into())],
                json!({"ok":false}),
            )
        } else {
            (200, vec![], json!({"ok":true,"ts":"1.000001"}))
        }
    })
    .await;
    let adapter = adapter(&server);
    let view = OutboundView::text("Title", "Body");
    let first = ConversationRef::new(ChannelKind::Slack, "T1:C1");
    assert!(
        timeout(Duration::from_millis(100), adapter.send(&first, &view))
            .await
            .is_err()
    );
    assert!(
        timeout(
            Duration::from_millis(100),
            adapter.send(&ConversationRef::new(ChannelKind::Slack, "T1:C2"), &view)
        )
        .await
        .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cooldown_started_during_post_pacing_is_rechecked_before_sending() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let server = Server::new(move |request| {
        count.fetch_add(1, Ordering::SeqCst);
        if request.body["channel"] == "C2" {
            (
                429,
                vec![("Retry-After".into(), "2".into())],
                json!({"ok":false}),
            )
        } else {
            (200, vec![], json!({"ok":true,"ts":"1.1"}))
        }
    })
    .await;
    let adapter = adapter(&server);
    let view = OutboundView::text("Title", "Body");
    let first = ConversationRef::new(ChannelKind::Slack, "T1:C1");
    adapter.send(&first, &view).await.unwrap();
    let pending = tokio::spawn({
        let adapter = adapter.clone();
        let view = view.clone();
        async move { adapter.send(&first, &view).await }
    });
    tokio::task::yield_now().await;
    assert!(
        timeout(
            Duration::from_millis(100),
            adapter.send(&ConversationRef::new(ChannelKind::Slack, "T1:C2"), &view)
        )
        .await
        .is_err()
    );
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(
        !pending.is_finished(),
        "a newer method cooldown must override an older channel wait"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    pending.abort();
    let _ = pending.await;
}

#[tokio::test]
#[allow(clippy::result_large_err)] // The tungstenite handshake callback fixes the error type.
async fn socket_upgrade_uses_the_supplied_http_proxy() {
    let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    let server = Server::new(|request| {
        (
            200,
            vec![],
            match request.path.as_str() {
                "/api/auth.test" => json!({"ok":true,"team_id":"T1","user_id":"BOT"}),
                "/api/apps.connections.open" => json!({"ok":true,"url":"ws://127.0.0.1:1/socket"}),
                _ => panic!("unexpected request"),
            },
        )
    })
    .await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .proxy(reqwest::Proxy::custom(move |url| {
            (url.port() == Some(1)).then(|| proxy_url.clone())
        }))
        .build()
        .unwrap();
    let adapter = SlackAdapter::with_client(
        client,
        server.url.parse().unwrap(),
        "bot",
        "app",
        vec!["U1".into()],
    )
    .unwrap();
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    let (tx, mut rx) = mpsc::channel(8);
    let run = tokio::spawn(async move { adapter.run(tx, stop).await });
    let (stream, _) = timeout(Duration::from_secs(5), proxy.accept())
        .await
        .unwrap()
        .unwrap();
    let mut socket = tokio_tungstenite::accept_hdr_async(
        stream,
        |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
            assert_eq!(request.uri().to_string(), "http://127.0.0.1:1/socket");
            assert!(request.headers().get("authorization").is_none());
            Ok(response)
        },
    )
    .await
    .unwrap();
    socket.send(json!({"type":"events_api","envelope_id":"proxy","payload":{"team_id":"T1","event":{"type":"message","channel":"D1","user":"U1","text":"hello","ts":"1.1"}}}).to_string().into()).await.unwrap();
    timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap()
            .owner_id,
        "U1"
    );
    shutdown.cancel();
    run.await.unwrap().unwrap();
}

#[tokio::test]
async fn identity_uses_authenticated_workspace_and_bot_user() {
    let server = Server::new(|_| {
        (
            200,
            vec![],
            json!({"ok":true,"team_id":"T1","user_id":"BOT1"}),
        )
    })
    .await;
    assert_eq!(
        adapter(&server).identity().await.unwrap().as_deref(),
        Some("T1:BOT1")
    );
    let invalid = Server::new(|_| (200, vec![], json!({"ok":true}))).await;
    assert!(adapter(&invalid).identity().await.is_err());
}

#[tokio::test]
async fn reload_preflight_checks_the_app_token() {
    let mut server = support::Server::new(|request| {
        if request.path.ends_with("auth.test") {
            (
                200,
                vec![],
                serde_json::json!({"ok":true,"team_id":"T","user_id":"U"}),
            )
        } else {
            (
                200,
                vec![],
                serde_json::json!({"ok":false,"error":"invalid_auth"}),
            )
        }
    })
    .await;
    let adapter = agentix_slack::SlackAdapter::with_client(
        reqwest::Client::new(),
        server.url.parse().unwrap(),
        "bot",
        "bad-app",
        vec![],
    )
    .unwrap();
    assert!(adapter.prepare_connection().await.is_err());
    assert!(
        server
            .requests
            .recv()
            .await
            .unwrap()
            .path
            .ends_with("auth.test")
    );
    let socket = server.requests.recv().await.unwrap();
    assert!(socket.path.ends_with("apps.connections.open"));
    assert_eq!(socket.authorization, "Bearer bad-app");
}
