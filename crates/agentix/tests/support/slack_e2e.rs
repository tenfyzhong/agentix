use super::{
    codex_support::{MockCodexAppServer, MockThread},
    join_stack, run_stack, slack_support, wait_for_value,
};
use agentix_codex::CodexClient;
use agentix_slack::SlackAdapter;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_tungstenite::WebSocketStream;
use tokio_util::sync::CancellationToken;

async fn deliver(socket: &mut WebSocketStream<TcpStream>, event: Value) {
    let id = event["envelope_id"].clone();
    socket.send(event.to_string().into()).await.unwrap();
    let ack = timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&ack).unwrap()["envelope_id"],
        id
    );
}

fn message(id: &str, text: &str) -> Value {
    json!({"type":"events_api","envelope_id":id,"payload":{"team_id":"T1","event":{"type":"message","channel":"D1","user":"U1","ts":format!("{id}.000001"),"text":text}}})
}

async fn next_request(
    server: &mut slack_support::Server,
    predicate: impl Fn(&slack_support::Request) -> bool,
) -> slack_support::Request {
    timeout(Duration::from_secs(10), async {
        loop {
            let request = server.requests.recv().await.unwrap();
            if predicate(&request) {
                return request;
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn slack_socket_engine_codex_streaming_and_approval() {
    let codex = MockCodexAppServer::start();
    codex
        .add_thread(MockThread::new(
            "thr_slack_e2e",
            "Slack integration",
            "/work/slack",
        ))
        .await;
    let client = Arc::new(CodexClient::connect(codex.endpoint()).await.unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_url = format!("ws://{}", listener.local_addr().unwrap());
    let counter = AtomicUsize::new(100);
    let approval_message = Arc::new(std::sync::Mutex::new(None));
    let captured = approval_message.clone();
    let mut server = slack_support::Server::new(move |request| {
        let body = match request.path.as_str() {
            "/api/auth.test" => json!({"ok":true,"team_id":"T1","user_id":"BOT"}),
            "/api/apps.connections.open" => json!({"ok":true,"url":ws_url}),
            _ => {
                let ts = format!("{}.000001", counter.fetch_add(1, Ordering::SeqCst));
                if request.path == "/api/chat.postMessage"
                    && request.body.to_string().contains("cargo test")
                {
                    *captured.lock().unwrap() = Some(ts.clone());
                }
                json!({"ok":true,"ts":ts})
            }
        };
        (200, vec![], body)
    })
    .await;
    let channel = Arc::new(
        SlackAdapter::with_client(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            server.url.parse().unwrap(),
            "bot-token",
            "app-token",
            vec!["U1".into()],
        )
        .unwrap(),
    );
    let shutdown = CancellationToken::new();
    let tasks = run_stack(client, channel, shutdown.clone()).await;
    let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
    deliver(&mut socket, message("1", "/attach thr_slack_e2e")).await;
    next_request(&mut server, |request| {
        request.body.to_string().contains("Slack integration")
    })
    .await;
    deliver(&mut socket, message("2", "run the integration")).await;
    let turn = wait_for_value(|| codex.latest_turn_id("thr_slack_e2e")).await;
    let approval = codex
        .request_command_approval("thr_slack_e2e", &turn, "item_slack", "cargo test")
        .await;
    let request = next_request(&mut server, |request| {
        request.body.to_string().contains("cargo test")
            && request.body["blocks"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "actions"))
    })
    .await;
    let button = request.body["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|block| block["type"] == "actions")
        .unwrap()["elements"][0]
        .clone();
    deliver(&mut socket,json!({"type":"interactive","envelope_id":"approval","payload":{"type":"block_actions","team":{"id":"T1"},"user":{"id":"U1"},"channel":{"id":"D1"},"message":{"ts":approval_message.lock().unwrap().clone().unwrap()},"actions":[button]}})).await;
    let answer = timeout(Duration::from_secs(5), approval)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answer["decision"], "accept");
    codex
        .complete_turn("thr_slack_e2e", &turn, "slack integration answer")
        .await;
    let response = next_request(&mut server, |request| {
        request.path == "/api/chat.update"
            && request
                .body
                .to_string()
                .contains("slack integration answer")
    })
    .await;
    assert_eq!(response.body["channel"], "D1");
    assert_eq!(response.authorization, "Bearer bot-token");
    shutdown.cancel();
    join_stack(tasks).await;
}
