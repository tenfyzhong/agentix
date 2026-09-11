//! Proxy connection lifecycle regression tests.
#![cfg(unix)]
use agentix_codex::{ClientRegistry, CodexProxy};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UnixListener};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn websocket_alias_to_same_listener_is_rejected() {
    let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    // A different path cannot make this a different TCP listener.
    let result = CodexProxy::bind(
        &format!("ws://127.0.0.1:{port}"),
        &format!("ws://127.0.0.1:{port}/upstream"),
    )
    .await;
    let accepted = result.is_ok();
    if let Ok(proxy) = result {
        proxy.shutdown().await;
    }
    assert!(
        !accepted,
        "same TCP listener accepted as upstream under another URL path"
    );
}

#[tokio::test]
async fn stdio_upstream_close_clears_sessions_before_output_drains() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("up.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let (mut input, reader) = tokio::io::duplex(1024);
    let (mut output, writer) = tokio::io::duplex(64);
    let registry = ClientRegistry::default();
    let r = registry.clone();
    let task = tokio::spawn(async move {
        agentix_codex::proxy_stdio(
            reader,
            writer,
            &format!("unix://{}", path.display()),
            r,
            None,
        )
        .await
    });
    let mut server = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
        .await
        .unwrap();
    input
        .write_all(b"{\"id\":1,\"method\":\"thread/start\",\"params\":{}}\n")
        .await
        .unwrap();
    assert!(server.next().await.unwrap().unwrap().is_text());
    server
        .send(Message::text(
            json!({
                "id":1,"result":{"thread":{"id":"closed-session"},"padding":"x".repeat(1024)}
            })
            .to_string(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !registry.snapshot().iter().any(|c| !c.sessions.is_empty()) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    server.close(None).await.unwrap();
    let ack = tokio::time::timeout(Duration::from_secs(1), server.next()).await;
    assert!(matches!(ack, Ok(Some(Ok(Message::Close(_))))), "{ack:?}");
    let mut raw = server.into_inner();
    let eof = tokio::time::timeout(Duration::from_secs(1), raw.read(&mut [0])).await;
    let cleared = tokio::time::timeout(Duration::from_millis(200), async {
        while registry.snapshot().iter().any(|c| !c.sessions.is_empty()) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok();
    if !cleared {
        task.abort();
        let _ = task.await;
        panic!("completed upstream Close still leaves closed-session active");
    }
    assert!(
        matches!(eof, Ok(Ok(0))),
        "upstream socket retained: {eof:?}"
    );
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(1), output.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    task.await.unwrap().unwrap();
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["result"]["thread"]["id"], "closed-session");
    assert!(
        cleared,
        "completed upstream Close still leaves closed-session active"
    );
}

#[test]
#[ignore = "Subprocess fixture: called by stdio_shutdown_does_not_wait_for_input"]
fn stdio_runtime_fixture() {
    let Some(endpoint) = std::env::var_os("AGENTIX_STDIO_FIXTURE_UPSTREAM") else {
        return;
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let proxy = CodexProxy::bind("stdio://", &endpoint.to_string_lossy())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while proxy.registry().snapshot().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        proxy.shutdown().await;
    });
    drop(runtime);
}

#[tokio::test]
async fn stdio_shutdown_does_not_wait_for_input() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("up.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "stdio_runtime_fixture"])
        .env(
            "AGENTIX_STDIO_FIXTURE_UPSTREAM",
            format!("unix://{}", path.display()),
        )
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let _input = child.stdin.take().unwrap(); // Keep stdin open without sending bytes.
    let _server = tokio::time::timeout(Duration::from_secs(3), async {
        tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
            .await
            .unwrap()
    })
    .await
    .unwrap();
    let exited = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
    if exited.is_err() {
        child.kill().await.unwrap();
    }
    assert!(
        matches!(exited, Ok(Ok(status)) if status.success()),
        "stdio shutdown waited for an input byte: {exited:?}"
    );
}

#[tokio::test]
async fn stdio_preserves_single_line_json_and_compacts_multiline_json() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("up.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let (mut input, reader) = tokio::io::duplex(1024);
    let (output, writer) = tokio::io::duplex(1024);
    let task = tokio::spawn(async move {
        agentix_codex::proxy_stdio(
            reader,
            writer,
            &format!("unix://{}", path.display()),
            ClientRegistry::default(),
            None,
        )
        .await
    });
    let mut server = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
        .await
        .unwrap();
    let single =
        "{ \"method\" : \"item/agentMessage/delta\", \"params\" : {\"delta\":\"hello\\nworld\"} }";
    server.send(Message::text(single)).await.unwrap();
    server
        .send(Message::text("{\n\"id\":2,\n\"result\":{}\n}"))
        .await
        .unwrap();
    let mut lines = BufReader::new(output).lines();
    let first = lines.next_line().await.unwrap().unwrap();
    let second = lines.next_line().await.unwrap().unwrap();
    input.shutdown().await.unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(
        first, single,
        "single-line JSON should need no reserialization"
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&second).unwrap(),
        json!({"id":2,"result":{}})
    );
}
