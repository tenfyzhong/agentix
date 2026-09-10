use super::{fixture, support};
use agentix_domain::{ChannelAdapter, InboundPayload};
use agentix_slack::{SlackAdapter, SlackOwnerClaimer};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{net::TcpListener, sync::mpsc, time::timeout};
use tokio_util::sync::CancellationToken;

struct Claimer(AtomicUsize);
#[async_trait::async_trait]
impl SlackOwnerClaimer for Claimer {
    async fn claim(&self, code: &str, owner: &str) -> Result<bool, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        if code == "ERROR" {
            return Err("save failed".into());
        }
        Ok(code == "VALID" && owner == "U1")
    }
}

fn names(dir: &Path) -> Vec<String> {
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(dir.join("remote.json")).unwrap()).unwrap();
    manifest["features"]["slash_commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["command"].as_str().unwrap().to_owned())
        .collect()
}

async fn reply(server: &mut support::Server) -> String {
    timeout(Duration::from_secs(5), async {
        loop {
            let request = server.requests.recv().await.unwrap();
            if request.path.ends_with("chat.postMessage") {
                return request.body.to_string();
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn claim_only_menu_transitions_after_successful_owner_persistence() {
    lifecycle(false).await;
}

#[tokio::test]
async fn menu_sync_failure_keeps_claimed_owner_and_recovers_on_restart() {
    lifecycle(true).await;
}

#[allow(clippy::too_many_lines)] // Verify the complete startup/claim/restart lifecycle.
async fn lifecycle(fail_sync: bool) {
    let (dir, sync) = fixture();
    // A reset must remove a previously installed normal menu.
    sync.sync("T123").await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_url = format!("ws://{}", listener.local_addr().unwrap());
    let mut server = support::Server::new(move |request| {
        (
            200,
            vec![],
            match request.path.as_str() {
                "/api/auth.test" => json!({"ok":true,"team_id":"T123","user_id":"BOT"}),
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
    .with_owner_claimer(claimer.clone())
    .with_command_sync(sync.clone());
    let (tx, mut rx) = mpsc::channel(8);
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    let run = tokio::spawn(async move { adapter.run(tx, stop).await });
    let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
    assert_eq!(names(dir.path()), ["/claim"]);
    for (team, channel, user, code) in [
        ("T123", "C1", "U1", "VALID"),
        ("T999", "D1", "U1", "VALID"),
        ("T123", "D1", "BOT", "VALID"),
        ("T123", "D1", "U1", "VALID extra"),
    ] {
        socket.send(json!({"type":"slash_commands","envelope_id":format!("{team}-{channel}-{user}-{code}"),
            "payload":{"team_id":team,"user_id":user,"channel_id":channel,"command":"/claim","text":code}}).to_string().into()).await.unwrap();
        timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    for code in ["WRONG", "ERROR", "VALID"] {
        if code == "VALID" && fail_sync {
            std::fs::write(dir.path().join("install-fail"), "").unwrap();
        }
        socket
            .send(
                json!({"type":"slash_commands","envelope_id":code,"payload":{
            "team_id":"T123","user_id":"U1","channel_id":"D1","command":"/claim","text":code}})
                .to_string()
                .into(),
            )
            .await
            .unwrap();
        timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let message = reply(&mut server).await;
        if code == "VALID" {
            assert!(message.contains("Owner linked"));
            if fail_sync {
                assert!(message.contains("restart"));
            }
        } else {
            assert_eq!(names(dir.path()), ["/claim"]);
            assert!(!message.contains("Owner linked"));
        }
    }
    assert_eq!(claimer.0.load(Ordering::SeqCst), 3);
    let expected = if fail_sync {
        vec!["/claim"]
    } else {
        vec!["/agentix", "/sessions"]
    };
    assert_eq!(names(dir.path()), expected);
    // A duplicate claim cannot change the owner or rerun synchronization.
    let calls = std::fs::read(dir.path().join("calls")).unwrap();
    socket
        .send(
            json!({"type":"slash_commands","envelope_id":"duplicate","payload":{
        "team_id":"T123","user_id":"U2","channel_id":"D1","command":"/claim","text":"VALID"}})
            .to_string()
            .into(),
        )
        .await
        .unwrap();
    timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    socket
        .send(
            json!({"type":"events_api","envelope_id":"normal","payload":{"team_id":"T123",
        "event":{"type":"message","channel":"D1","user":"U1","text":"/sessions","ts":"8.1"}}})
            .to_string()
            .into(),
        )
        .await
        .unwrap();
    let input = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(input.payload, InboundPayload::Text("/sessions".into()));
    assert_eq!(claimer.0.load(Ordering::SeqCst), 3);
    assert_eq!(std::fs::read(dir.path().join("calls")).unwrap(), calls);
    shutdown.cancel();
    run.await.unwrap().unwrap();
    if fail_sync {
        std::fs::remove_file(dir.path().join("install-fail")).unwrap();
        let adapter = SlackAdapter::with_client(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            server.url.parse().unwrap(),
            "bot",
            "app",
            vec!["U1".into()],
        )
        .unwrap()
        .with_command_sync(sync);
        let (tx, _rx) = mpsc::channel(1);
        let stop = CancellationToken::new();
        let shutdown = stop.clone();
        let run = tokio::spawn(async move { adapter.run(tx, stop).await });
        let (_stream, _) = timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(names(dir.path()), ["/agentix", "/sessions"]);
        shutdown.cancel();
        run.await.unwrap().unwrap();
    }
    super::assert_projects_removed(&dir);
}
