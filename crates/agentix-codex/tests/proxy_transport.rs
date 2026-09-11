//! Proxy transport compatibility and forwarding tests.
#![cfg(unix)]
use agentix_codex::CodexProxy;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

async fn rpc<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    value: Value,
) -> Value {
    ws.send(Message::text(value.to_string())).await.unwrap();
    serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap()
}

#[tokio::test]
async fn unix_and_ws_clients_have_independent_upstreams_and_pid_bindings() {
    for tcp in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let upstream = d.path().join("o.sock");
        let listener = UnixListener::bind(&upstream).unwrap();
        let server = tokio::spawn(async move {
            let mut jobs = tokio::task::JoinSet::new();
            for session in ["a", "b"] {
                let (stream, _) = listener.accept().await.unwrap();
                jobs.spawn(async move {
                    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                    while let Some(Ok(Message::Text(text))) = ws.next().await {
                        let v: Value = serde_json::from_str(&text).unwrap();
                        ws.send(Message::text(
                            json!({"id":v["id"],"result":{"thread":{"id":session}}}).to_string(),
                        ))
                        .await
                        .unwrap();
                    }
                });
            }
            while jobs.join_next().await.is_some() {}
        });
        let listen = if tcp {
            "ws://127.0.0.1:0".to_owned()
        } else {
            format!("unix://{}", d.path().join("p.sock").display())
        };
        let proxy = CodexProxy::bind(&listen, &format!("unix://{}", upstream.display()))
            .await
            .unwrap();
        let registry = proxy.registry();
        // Boxed stream gives both transports the same test path.
        let mut clients = Vec::new();
        for _ in 0..2 {
            let stream: Box<dyn TestStream> = if tcp {
                Box::new(
                    tokio::net::TcpStream::connect(proxy.endpoint().strip_prefix("ws://").unwrap())
                        .await
                        .unwrap(),
                )
            } else {
                Box::new(
                    UnixStream::connect(proxy.endpoint().strip_prefix("unix://").unwrap())
                        .await
                        .unwrap(),
                )
            };
            let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost/", stream)
                .await
                .unwrap();
            rpc(
                &mut ws,
                json!({"id":1,"method":"thread/start","params":{"cwd":"/same"}}),
            )
            .await;
            clients.push(ws);
        }
        assert_eq!(registry.snapshot().len(), 2);
        assert!(
            registry
                .snapshot()
                .iter()
                .all(|c| c.pid == if tcp { None } else { Some(std::process::id()) })
        );
        clients.pop().unwrap().close(None).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while registry.snapshot().len() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(registry.snapshot()[0].sessions, vec!["a"]);
        proxy.shutdown().await;
        assert!(registry.snapshot().is_empty());
        server.await.unwrap();
    }
}
trait TestStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> TestStream for T {}

#[tokio::test]
async fn refuses_existing_socket_or_regular_file_without_unlinking() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("occupied");
    std::fs::write(&path, "preserve").unwrap();
    assert!(
        CodexProxy::bind(&format!("unix://{}", path.display()), "ws://127.0.0.1:1")
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "preserve");
}

#[tokio::test]
async fn internal_client_uses_upstream_and_only_lists_registered_sessions() {
    let d = tempfile::tempdir().unwrap();
    let upstream = d.path().join("upstream.sock");
    let listener = UnixListener::bind(&upstream).unwrap();
    let server = tokio::spawn(async move {
        let (s, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
        while let Some(Ok(Message::Text(text))) = ws.next().await {
            let v: Value = serde_json::from_str(&text).unwrap();
            let result = match v["method"].as_str() {
                Some("initialize") => json!({}),
                Some("initialized") => continue,
                Some("thread/read" | "thread/resume") => {
                    json!({"thread":{"id":v["params"]["threadId"],"cwd":"/same","path":null,"createdAt":1,"updatedAt":2,"status":{"type":"idle"}}})
                }
                method => panic!("Unexpected discovery RPC: {method:?}"),
            };
            ws.send(Message::text(
                json!({"id":v["id"],"result":result}).to_string(),
            ))
            .await
            .unwrap();
        }
    });
    let registry = agentix_codex::ClientRegistry::default();
    let client = agentix_codex::CodexClient::connect_with_registry(
        agentix_codex::CodexEndpoint::from_socket_path(&upstream).unwrap(),
        std::path::Path::new("codex"),
        std::path::Path::new("/tmp"),
        false,
        registry.clone(),
    )
    .await
    .unwrap();
    let a = registry.connect(Some(101));
    let b = registry.connect(Some(102));
    for (c, id) in [(a, "a"), (b, "b")] {
        registry.client_message(c, &json!({"id":1,"method":"thread/start"}));
        registry.server_message(c, &json!({"id":1,"result":{"thread":{"id":id}}}));
    }
    assert_eq!(
        client.list_sessions(None, 25).await.unwrap().sessions.len(),
        2
    );
    agentix_domain::AgentAdapter::attach(&client, &agentix_domain::SessionId::new("a"))
        .await
        .unwrap();
    registry.disconnect(b);
    let error = agentix_domain::AgentAdapter::attach(&client, &agentix_domain::SessionId::new("b"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("no connected Codex client"));
    let page = client.list_sessions(None, 25).await.unwrap();
    assert_eq!(page.sessions.len(), 1);
    assert_eq!(page.sessions[0].id.as_str(), "a");
    server.abort();
}

#[tokio::test]
async fn upstream_start_uses_explicit_listen_address_and_reports_failure() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let script = d.path().join("codex");
    let args = d.path().join("args");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 7\n",
            args.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let endpoint =
        agentix_codex::CodexEndpoint::from_socket_path(&d.path().join("upstream.sock")).unwrap();
    assert!(
        agentix_codex::UpstreamServer::ensure(&endpoint, &script)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(args).unwrap(),
        format!("app-server\n--listen\n{}\n", endpoint.address())
    );
}

#[tokio::test]
async fn runtime_refuses_proxy_upstream_alias_before_starting_server() {
    let d = tempfile::tempdir().unwrap();
    let endpoint =
        agentix_codex::CodexEndpoint::from_socket_path(&d.path().join("same.sock")).unwrap();
    assert!(
        agentix_codex::CodexClient::connect_with_proxy(
            &endpoint.address(),
            endpoint,
            std::path::Path::new("must-not-execute"),
            std::path::Path::new("/tmp"),
            false,
        )
        .await
        .is_err()
    );
    assert!(!d.path().join("same.sock").exists());
}

#[tokio::test]
async fn stdio_frontend_forwards_json_lines_and_cleans_registration_on_eof() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("upstream.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let server = tokio::spawn(async move {
        let (s, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
        let v: Value =
            serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        ws.send(Message::text(
            json!({"id":v["id"],"result":{"thread":{"id":"stdio-session"}}}).to_string(),
        ))
        .await
        .unwrap();
        let _ = ws.next().await;
    });
    let registry = agentix_codex::ClientRegistry::default();
    let (input, mut write_input) = tokio::io::duplex(4096);
    let (read_output, output) = tokio::io::duplex(4096);
    let endpoint = format!("unix://{}", path.display());
    let r = registry.clone();
    let proxy = tokio::spawn(async move {
        agentix_codex::proxy_stdio(input, output, &endpoint, r, None)
            .await
            .unwrap();
    });
    write_input
        .write_all(b"{\"id\":1,\"method\":\"thread/start\"}\n")
        .await
        .unwrap();
    let mut lines = BufReader::new(read_output).lines();
    let v: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(v["result"]["thread"]["id"], "stdio-session");
    assert_eq!(registry.snapshot()[0].sessions, vec!["stdio-session"]);
    drop(write_input);
    proxy.await.unwrap();
    assert!(registry.snapshot().is_empty());
    server.await.unwrap();
}

#[tokio::test]
async fn unix_proxy_forwards_to_websocket_upstream() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (s, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
        let request: Value =
            serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        ws.send(Message::text(
            json!({"id":request["id"],"result":{"thread":{"id":"ws-upstream"}}}).to_string(),
        ))
        .await
        .unwrap();
        let _ = ws.next().await;
    });
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("proxy.sock");
    let proxy = CodexProxy::bind(&format!("unix://{}", path.display()), &upstream)
        .await
        .unwrap();
    let stream = UnixStream::connect(path).await.unwrap();
    let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost/", stream)
        .await
        .unwrap();
    let result = rpc(&mut ws, json!({"id":"request","method":"thread/start"})).await;
    assert_eq!(result["id"], "request");
    assert_eq!(proxy.registry().snapshot()[0].sessions, vec!["ws-upstream"]);
    proxy.shutdown().await;
    server.await.unwrap();
}

#[tokio::test]
#[ignore = "subprocess fixture for upstream lifetime test"]
async fn upstream_process_fixture() {
    let path = std::env::var("AGENTIX_TEST_UPSTREAM_SOCKET").unwrap();
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::write(format!("{path}.pid"), std::process::id().to_string()).unwrap();
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = ws.next().await;
    }
}

#[tokio::test]
async fn started_upstream_survives_owner_drop_and_has_separate_process_group() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("upstream.sock");
    let command = d.path().join("codex");
    let exe = std::env::current_exe().unwrap();
    std::fs::write(&command, format!("#!/bin/sh\nexport AGENTIX_TEST_UPSTREAM_SOCKET='{}'\nexec '{}' --ignored --exact upstream_process_fixture\n", path.display(), exe.display())).unwrap();
    std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).unwrap();
    let endpoint = agentix_codex::CodexEndpoint::from_socket_path(&path).unwrap();
    let owner = agentix_codex::UpstreamServer::ensure(&endpoint, &command)
        .await
        .unwrap();
    let pid: i32 = std::fs::read_to_string(format!("{}.pid", path.display()))
        .unwrap()
        .parse()
        .unwrap();
    let _cleanup = Cleanup(pid);
    drop(owner);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let stream = UnixStream::connect(&path)
        .await
        .expect("upstream must survive owner drop");
    let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost/", stream)
        .await
        .unwrap();
    assert_ne!(
        nix::unistd::getpgid(Some(nix::unistd::Pid::from_raw(pid))).unwrap(),
        nix::unistd::getpgrp()
    );
    ws.close(None).await.unwrap();
    let reused =
        agentix_codex::UpstreamServer::ensure(&endpoint, std::path::Path::new("must-not-launch"))
            .await
            .unwrap();
    drop(reused);
    assert!(path.exists());
}

struct Cleanup(i32);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &self.0.to_string()])
            .output();
    }
}

#[tokio::test]
async fn frames_are_preserved_and_upstream_loss_clears_registration() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("upstream.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let reply = r#"{ "id":1, "result": {"thread":{"id":"exact"}} }"#;
    let notification =
        "{\n \"method\":\"item/agentMessage/delta\",\"params\":{\"delta\":\"中文 🌍\"}}";
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        assert_eq!(
            ws.next().await.unwrap().unwrap().to_text().unwrap(),
            r#"{ "id":1,"method":"thread/start" }"#
        );
        ws.send(Message::text(reply)).await.unwrap();
        ws.send(Message::text(notification)).await.unwrap();
        ws.send(Message::text(
            r#"{"id":1,"method":"item/commandExecution/requestApproval","params":{}}"#,
        ))
        .await
        .unwrap();
        assert_eq!(
            ws.next().await.unwrap().unwrap().to_text().unwrap(),
            r#"{"id":1,"result":{"decision":"accept"}}"#
        );
        let _ = stopped.await;
        // Abrupt loss, without a WebSocket close handshake.
    });
    let proxy_path = d.path().join("proxy.sock");
    let proxy = CodexProxy::bind(
        &format!("unix://{}", proxy_path.display()),
        &format!("unix://{}", path.display()),
    )
    .await
    .unwrap();
    let registry = proxy.registry();
    let (mut ws, _) = tokio_tungstenite::client_async(
        "ws://localhost/",
        UnixStream::connect(&proxy_path).await.unwrap(),
    )
    .await
    .unwrap();
    ws.send(Message::text(r#"{ "id":1,"method":"thread/start" }"#))
        .await
        .unwrap();
    assert_eq!(ws.next().await.unwrap().unwrap().to_text().unwrap(), reply);
    assert_eq!(
        ws.next().await.unwrap().unwrap().to_text().unwrap(),
        notification
    );
    assert!(
        ws.next()
            .await
            .unwrap()
            .unwrap()
            .to_text()
            .unwrap()
            .contains("requestApproval")
    );
    ws.send(Message::text(r#"{"id":1,"result":{"decision":"accept"}}"#))
        .await
        .unwrap();
    assert_eq!(registry.snapshot()[0].sessions, vec!["exact"]);
    stop.send(()).unwrap();
    server.await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !registry.snapshot().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    proxy.shutdown().await;
    assert!(!proxy_path.exists());
}

#[tokio::test]
async fn proxy_shutdown_preserves_a_replacement_socket() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("proxy.sock");
    let proxy = CodexProxy::bind(&format!("unix://{}", path.display()), "ws://127.0.0.1:1")
        .await
        .unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::rename(&path, d.path().join("old.sock")).unwrap();
    let replacement = UnixListener::bind(&path).unwrap();
    let inode = std::fs::metadata(&path).unwrap().ino();
    proxy.shutdown().await;
    assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
    drop(replacement);
}

#[tokio::test]
async fn control_frames_are_transparent_without_local_or_duplicate_pongs() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for tcp in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("upstream.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let listen = if tcp {
            "ws://127.0.0.1:0".into()
        } else {
            format!("unix://{}", d.path().join("proxy.sock").display())
        };
        let proxy = CodexProxy::bind(&listen, &format!("unix://{}", path.display()))
            .await
            .unwrap();
        let connect = async {
            let stream: Box<dyn TestIo> = if tcp {
                let addr = proxy.endpoint().strip_prefix("ws://").unwrap();
                Box::new(tokio::net::TcpStream::connect(addr).await.unwrap())
            } else {
                Box::new(
                    UnixStream::connect(d.path().join("proxy.sock"))
                        .await
                        .unwrap(),
                )
            };
            tokio_tungstenite::client_async("ws://localhost/", stream)
                .await
                .unwrap()
                .0
                .into_inner()
        };
        let accept = async {
            let (stream, _) = listener.accept().await.unwrap();
            tokio_tungstenite::accept_async(stream)
                .await
                .unwrap()
                .into_inner()
        };
        let (mut cli, mut server) = tokio::join!(connect, accept);
        // Raw frames avoid automatic Pong generation in either test peer.
        let ping = [0x89, 0x83, 1, 2, 3, 4, 8, 10, 4];
        cli.write_all(&ping).await.unwrap();
        let mut received = [0; 9];
        tokio::time::timeout(Duration::from_secs(1), server.read_exact(&mut received))
            .await
            .expect("upstream must receive CLI Ping")
            .unwrap();
        assert_eq!(received, ping);
        let mut byte = [0];
        assert!(
            tokio::time::timeout(Duration::from_millis(50), cli.read(&mut byte))
                .await
                .is_err()
        );
        let reply = [0x8a, 3, 9, 8, 7];
        server.write_all(&reply).await.unwrap();
        let mut response = [0; 5];
        cli.read_exact(&mut response).await.unwrap();
        assert_eq!(response, reply);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), cli.read(&mut byte))
                .await
                .is_err()
        );
        server.write_all(&[0x89, 3, 9, 8, 7]).await.unwrap();
        cli.read_exact(&mut response).await.unwrap();
        assert_eq!(response, [0x89, 3, 9, 8, 7]);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), server.read(&mut byte))
                .await
                .is_err()
        );
        let mut reply = ping;
        reply[0] = 0x8a;
        cli.write_all(&reply).await.unwrap();
        server.read_exact(&mut received).await.unwrap();
        assert_eq!(received, reply);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), server.read(&mut byte))
                .await
                .is_err()
        );
        drop(server);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), cli.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        assert!(proxy.registry().snapshot().is_empty());
        proxy.shutdown().await;
    }
}
trait TestIo: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin> TestIo for T {}

async fn raw_pair() -> (tempfile::TempDir, CodexProxy, UnixStream, UnixStream) {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("server.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let proxy = CodexProxy::bind(
        &format!("unix://{}", d.path().join("proxy.sock").display()),
        &format!("unix://{}", path.display()),
    )
    .await
    .unwrap();
    let cli = async {
        let stream = UnixStream::connect(d.path().join("proxy.sock"))
            .await
            .unwrap();
        tokio_tungstenite::client_async("ws://localhost/", stream)
            .await
            .unwrap()
            .0
            .into_inner()
    };
    let server = async {
        let (stream, _) = listener.accept().await.unwrap();
        tokio_tungstenite::accept_async(stream)
            .await
            .unwrap()
            .into_inner()
    };
    let (cli, server) = tokio::join!(cli, server);
    (d, proxy, cli, server)
}

#[tokio::test]
async fn observer_errors_do_not_interrupt_either_direction() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    tokio::time::timeout(Duration::from_secs(5), async {
        let (_d, proxy, mut cli, mut server) = raw_pair().await;
        // Invalid UTF-8 text is for the actual peer to accept/reject, not the observer.
        for bytes in [
            &[0x81, 0x81, 0, 0, 0, 0, 0xff][..],
            &[0x89, 0x80, 1, 2, 3, 4],
        ] {
            cli.write_all(bytes).await.unwrap();
            let mut forwarded = vec![0; bytes.len()];
            server
                .read_exact(&mut forwarded)
                .await
                .expect("observer must not disconnect the wire");
            assert_eq!(forwarded, bytes);
        }
        for bytes in [&[0x81, 1, 0xff][..], &[0x8a, 0]] {
            server.write_all(bytes).await.unwrap();
            let mut forwarded = vec![0; bytes.len()];
            cli.read_exact(&mut forwarded).await.unwrap();
            assert_eq!(forwarded, bytes);
        }
        drop(cli);
        assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
        assert!(proxy.registry().snapshot().is_empty());
        proxy.shutdown().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn close_waits_for_both_peers_then_closes_transport() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    tokio::time::timeout(Duration::from_secs(5), async {
        for server_first in [false, true] {
            let (_d, proxy, mut cli, mut server) = raw_pair().await;
            let client_close = [0x88, 0x82, 1, 2, 3, 4, 2, 0xea]; // 1000, masked
            let server_close = [0x88, 2, 3, 0xe8];
            let (first, second, sent, reply): (&mut UnixStream, &mut UnixStream, &[u8], &[u8]) =
                if server_first {
                    (&mut server, &mut cli, &server_close, &client_close)
                } else {
                    (&mut cli, &mut server, &client_close, &server_close)
                };
            first.write_all(sent).await.unwrap();
            let mut forwarded = vec![0; sent.len()];
            second.read_exact(&mut forwarded).await.unwrap();
            assert_eq!(forwarded, sent);
            assert!(
                tokio::time::timeout(Duration::from_millis(50), first.read(&mut [0]))
                    .await
                    .is_err(),
                "must not close before peer reply"
            );
            assert_eq!(proxy.registry().snapshot().len(), 1);
            // Split the reply so completion requires the whole Close payload.
            second.write_all(&reply[..reply.len() - 1]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
            second.write_all(&reply[reply.len() - 1..]).await.unwrap();
            let mut forwarded = vec![0; reply.len()];
            first.read_exact(&mut forwarded).await.unwrap();
            assert_eq!(forwarded, reply);
            assert_eq!(first.read(&mut [0]).await.unwrap(), 0);
            assert_eq!(second.read(&mut [0]).await.unwrap(), 0);
            assert!(proxy.registry().snapshot().is_empty());
            proxy.shutdown().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn upstream_rejection_is_a_failed_downstream_handshake() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("reject.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let proxy = CodexProxy::bind("ws://127.0.0.1:0", &format!("unix://{}", path.display()))
        .await
        .unwrap();
    let reject = async {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            headers.push(stream.read_u8().await.unwrap());
        }
        stream
            .write_all(
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nRetry-After: 3\r\n\r\n",
            )
            .await
            .unwrap();
    };
    let (result, ()) = tokio::join!(tokio_tungstenite::connect_async(proxy.endpoint()), reject);
    let error = result.expect_err("must not confirm upgrade before upstream succeeds");
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected upstream HTTP failure: {error}");
    };
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["retry-after"], "3");
    proxy.shutdown().await;
}

#[tokio::test]
async fn eof_after_first_close_cancels_wait_for_peer_reply() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (_d, proxy, mut cli, mut server) = raw_pair().await;
    let close = [0x88, 0x80, 1, 2, 3, 4];
    cli.write_all(&close).await.unwrap();
    let mut received = [0; 6];
    server.read_exact(&mut received).await.unwrap();
    assert_eq!(received, close);
    cli.shutdown().await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), server.read(&mut [0]))
            .await
            .expect("EOF after Close must terminate the relay")
            .unwrap(),
        0
    );
    assert!(proxy.registry().snapshot().is_empty());
    proxy.shutdown().await;
}

#[tokio::test]
async fn upgrade_waits_for_upstream_and_preserves_subprotocol() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("delayed.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let proxy = CodexProxy::bind("ws://127.0.0.1:0", &format!("unix://{}", path.display()))
        .await
        .unwrap();
    let mut cli = tokio::net::TcpStream::connect(proxy.endpoint().trim_start_matches("ws://"))
        .await
        .unwrap();
    cli.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: codex-test\r\n\r\n").await.unwrap();
    let (mut server, _) = listener.accept().await.unwrap();
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        request.push(server.read_u8().await.unwrap());
    }
    let request = String::from_utf8(request).unwrap().to_lowercase();
    assert!(request.contains("sec-websocket-protocol: codex-test"));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), cli.read(&mut [0]))
            .await
            .is_err(),
        "must not send 101 while upstream is pending"
    );
    let response = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\nSec-WebSocket-Protocol: codex-test\r\n\r\n";
    server.write_all(response).await.unwrap();
    let mut received = vec![0; response.len()];
    cli.read_exact(&mut received).await.unwrap();
    assert_eq!(received, response);
    proxy.shutdown().await;
}

#[tokio::test]
async fn unreachable_upstream_returns_bad_gateway() {
    let d = tempfile::tempdir().unwrap();
    let proxy = CodexProxy::bind(
        "ws://127.0.0.1:0",
        &format!("unix://{}/missing.sock", d.path().display()),
    )
    .await
    .unwrap();
    let error = tokio_tungstenite::connect_async(proxy.endpoint())
        .await
        .unwrap_err();
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected HTTP error: {error}");
    };
    assert_eq!(response.status(), 502);
    assert!(proxy.registry().snapshot().is_empty());
    proxy.shutdown().await;
}

#[tokio::test]
async fn upstream_http_error_body_is_forwarded_across_reads() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("body.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let proxy = CodexProxy::bind("ws://127.0.0.1:0", &format!("unix://{}", path.display()))
        .await
        .unwrap();
    let mut cli = tokio::net::TcpStream::connect(proxy.endpoint().trim_start_matches("ws://"))
        .await
        .unwrap();
    cli.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").await.unwrap();
    let (mut server, _) = listener.accept().await.unwrap();
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        headers.push(server.read_u8().await.unwrap());
    }
    let response = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 6\r\n\r\n";
    server
        .write_all(&[response.as_slice(), b"de"].concat())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    server.write_all(b"nied").await.unwrap();
    server.shutdown().await.unwrap();
    let mut received = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), cli.read_to_end(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received, [response.as_slice(), b"denied"].concat());
    assert!(proxy.registry().snapshot().is_empty());
    proxy.shutdown().await;
}

#[tokio::test]
async fn upstream_handshake_timeout_returns_gateway_timeout() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("timeout.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let proxy = CodexProxy::bind("ws://127.0.0.1:0", &format!("unix://{}", path.display()))
        .await
        .unwrap();
    let connect = tokio_tungstenite::connect_async(proxy.endpoint());
    let silent = async {
        let peer = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(11)).await;
        drop(peer);
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(connect, silent)
    })
    .await
    .unwrap();
    let error = result.expect_err("upstream never confirmed an upgrade");
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected HTTP timeout: {error}");
    };
    assert_eq!(response.status(), 504);
    proxy.shutdown().await;
}

#[tokio::test]
async fn ipv6_proxy_and_upstream_connect() {
    let upstream = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let proxy = CodexProxy::bind(
        "ws://[::1]:0",
        &format!("ws://{}", upstream.local_addr().unwrap()),
    )
    .await
    .expect("IPv6 listener must bind");
    let accept = async {
        tokio_tungstenite::accept_async(upstream.accept().await.unwrap().0)
            .await
            .unwrap()
    };
    let (client, _) = tokio::join!(tokio_tungstenite::connect_async(proxy.endpoint()), accept);
    assert!(client.is_ok(), "IPv6 upstream must connect");
    proxy.shutdown().await;
}

#[tokio::test]
async fn stdio_backpressure_does_not_block_input_eof() {
    use tokio::io::AsyncWriteExt;
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("stdio-pressure.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let (mut input, reader) = tokio::io::duplex(64);
    let (_output, writer) = tokio::io::duplex(64);
    let registry = agentix_codex::ClientRegistry::default();
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
    server
        .send(Message::text(
            json!({"method":"notification","data":"x".repeat(1024)}).to_string(),
        ))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    input
        .write_all(b"{\"id\":7,\"method\":\"test\"}\n")
        .await
        .unwrap();
    let request = tokio::time::timeout(Duration::from_secs(1), server.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        request.into_text().unwrap(),
        "{\"id\":7,\"method\":\"test\"}"
    );
    server
        .send(Message::Ping(vec![1, 2, 3].into()))
        .await
        .unwrap();
    let pong = tokio::time::timeout(Duration::from_secs(1), server.next())
        .await
        .expect("stdout backpressure must not block Pong")
        .unwrap()
        .unwrap();
    assert_eq!(pong, Message::Pong(vec![1, 2, 3].into()));
    input.shutdown().await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("stdout backpressure must not block stdin EOF")
        .unwrap()
        .unwrap();
    assert!(registry.snapshot().is_empty());
}

#[tokio::test]
async fn http_rejection_finishes_at_body_boundary_without_upstream_eof() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for response in [
        &b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n"[..],
        &b"HTTP/1.1 403 Forbidden\r\nContent-Length: 6\r\n\r\ndenied"[..],
        &b"HTTP/1.1 403 Forbidden\r\nTransfer-Encoding: chunked\r\n\r\n3;test=yes\r\nabc\r\n0\r\nX-End: yes\r\n\r\n"[..],
    ] {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("keepalive.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let proxy = CodexProxy::bind("ws://127.0.0.1:0", &format!("unix://{}", path.display())).await.unwrap();
        let mut cli = tokio::net::TcpStream::connect(proxy.endpoint().trim_start_matches("ws://")).await.unwrap();
        cli.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").await.unwrap();
        let (mut server, _) = listener.accept().await.unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") { headers.push(server.read_u8().await.unwrap()); }
        server.write_all(response).await.unwrap();
        let mut received = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), cli.read_to_end(&mut received)).await.expect("complete body must close without waiting for upstream EOF").unwrap();
        assert_eq!(received, response);
        assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
        proxy.shutdown().await;
    }
}
