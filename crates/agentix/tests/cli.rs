#[cfg(unix)]
mod unix {
    use std::process::Stdio;

    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use tempfile::tempdir;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;
    use tokio::process::Command;
    use tokio_tungstenite::tungstenite::Message;

    #[tokio::test]
    async fn client_session_operations_use_backend_neutral_control_requests() {
        for session in ["codex:native", "pi:native", "omp:native", "claude:native"] {
            for (args, expected) in [
                (
                    vec!["send", session, "hello"],
                    json!({"operation":"send","session":session,"text":"hello","expected_turn":null}),
                ),
                (
                    vec!["send", session, "hello", "--turn", "turn-1"],
                    json!({"operation":"send","session":session,"text":"hello","expected_turn":"turn-1"}),
                ),
                (
                    vec![
                        "history",
                        session,
                        "--cursor",
                        "opaque:cursor",
                        "--limit",
                        "2",
                    ],
                    json!({"operation":"history","session":session,"cursor":"opaque:cursor","limit":2}),
                ),
                (
                    vec!["stop", session, "turn-1"],
                    json!({"operation":"stop","session":session,"turn":"turn-1"}),
                ),
                (
                    vec!["history", session],
                    json!({"operation":"history","session":session,"cursor":null,"limit":20}),
                ),
                (
                    vec!["command", session, r#"{"command":"model","params":null}"#],
                    json!({"operation":"command","session":session,"command":{"command":"model","params":null}}),
                ),
            ] {
                let directory = tempdir().unwrap();
                let socket = directory.path().join("control.sock");
                let config = write_control_config(directory.path(), &socket);
                let listener = UnixListener::bind(&socket).unwrap();
                let server = tokio::spawn(async move {
                    let (request, mut stream) = next_control_request(&listener).await;
                    assert_eq!(request, json!({"method":"session","params":expected}));
                    stream
                        .write_all(b"{\"ok\":true,\"result\":{}}\n")
                        .await
                        .unwrap();
                });
                let output = Command::new(env!("CARGO_BIN_EXE_agentix"))
                    .arg("--config")
                    .arg(&config)
                    .arg("client")
                    .args(args)
                    .output()
                    .await
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                server.await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn client_reports_control_errors_without_printing_success_json() {
        for error in [
            "Operation is not supported by this session",
            "session bridge is offline",
            "Delivery uncertain: inspect history",
        ] {
            let directory = tempdir().unwrap();
            let socket = directory.path().join("control.sock");
            let config = write_control_config(directory.path(), &socket);
            let listener = UnixListener::bind(&socket).unwrap();
            let server = tokio::spawn(async move {
                let (request, mut stream) = next_control_request(&listener).await;
                assert_eq!(request["params"]["session"], "claude:native");
                stream
                    .write_all(format!("{}\n", json!({"ok":false,"error":error})).as_bytes())
                    .await
                    .unwrap();
            });
            let output = Command::new(env!("CARGO_BIN_EXE_agentix"))
                .arg("--config")
                .arg(config)
                .args(["client", "send", "claude:native", "hello"])
                .output()
                .await
                .unwrap();
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            assert!(String::from_utf8_lossy(&output.stderr).contains(error));
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn client_claim_prints_the_code_returned_by_agentix_control() {
        let directory = tempdir().unwrap();
        let control_socket = directory.path().join("control.sock");
        let config = directory.path().join("agentix.toml");
        std::fs::write(
            &config,
            format!(
                r#"[server]
endpoint = "unix://{}"

[agent]
kind = "codex"

[storage]
path = "/tmp/unused-agentix.sqlite3"

[channel]
kind = "telegram"

[channel.telegram]
token = "mock-token"
owner_user_ids = []
"#,
                control_socket.display()
            ),
        )
        .unwrap();
        let original_config = std::fs::read_to_string(&config).unwrap();
        let listener = UnixListener::bind(&control_socket).unwrap();
        let server = tokio::spawn(async move {
            let (request, mut stream) = next_control_request(&listener).await;
            assert_eq!(
                request,
                json!({"method": "claim", "params": {"ttlMinutes": 5}})
            );
            stream
                .write_all(
                    b"{\"ok\":true,\"result\":{\"command\":\"/claim ABCD1234EFGH\",\"expiresAt\":1234}}\n",
                )
                .await
                .unwrap();
        });
        let output = Command::new(env!("CARGO_BIN_EXE_agentix"))
            .arg("--config")
            .arg(&config)
            .args(["client", "claim", "--ttl-minutes", "5"])
            .stdin(Stdio::null())
            .output()
            .await
            .unwrap();

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "claim code must not be logged");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("/claim ABCD1234EFGH"));
        assert!(stdout.contains("Valid for 5 minute(s)"));
        assert_eq!(std::fs::read_to_string(config).unwrap(), original_config);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn client_call_sends_a_raw_request_through_agentix_control() {
        let directory = tempdir().unwrap();
        let control_socket = directory.path().join("control.sock");
        let config = write_control_config(directory.path(), &control_socket);
        let listener = UnixListener::bind(&control_socket).unwrap();

        let server = tokio::spawn(async move {
            let (request, mut stream) = next_control_request(&listener).await;
            assert_eq!(
                request,
                json!({
                    "method": "call",
                    "params": {"method": "debug/echo", "params": {"message": "hello"}}
                })
            );
            stream
                .write_all(b"{\"ok\":true,\"result\":{\"echo\":\"hello\",\"ok\":true}}\n")
                .await
                .unwrap();
        });

        let output = Command::new(env!("CARGO_BIN_EXE_agentix"))
            .arg("--config")
            .arg(config)
            .args([
                "client",
                "call",
                "debug/echo",
                "--params",
                r#"{"message":"hello"}"#,
            ])
            .stdin(Stdio::null())
            .output()
            .await
            .unwrap();

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({"echo": "hello", "ok": true})
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn client_sessions_lists_threads_from_agentix_control() {
        let directory = tempdir().unwrap();
        let control_socket = directory.path().join("control.sock");
        let config = write_control_config(directory.path(), &control_socket);
        let listener = UnixListener::bind(&control_socket).unwrap();
        let server = tokio::spawn(async move {
            let (request, mut stream) = next_control_request(&listener).await;
            assert_eq!(
                request,
                json!({"method": "sessions", "params": {"cursor": null, "limit": 10}})
            );
            stream
                .write_all(
                    serde_json::json!({
                        "ok": true,
                        "result": {
                            "sessions": [{
                        "id": "thr_mock",
                        "name": "Mock session",
                        "cwd": "/work/mock",
                        "updatedAt": 42,
                                "status": "idle"
                            }],
                            "nextCursor": null
                        }
                    })
                    .to_string()
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(b"\n").await.unwrap();
        });

        let output = Command::new(env!("CARGO_BIN_EXE_agentix"))
            .arg("--config")
            .arg(config)
            .args(["client", "sessions", "--limit", "10"])
            .stdin(Stdio::null())
            .output()
            .await
            .unwrap();

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let page: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(page["sessions"][0]["id"], "thr_mock");
        assert_eq!(page["sessions"][0]["name"], "Mock session");
        assert_eq!(page["sessions"][0]["cwd"], "/work/mock");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn doctor_reports_live_bridges_instead_of_saved_transcripts() {
        for kind in ["pi", "omp", "claude"] {
            let directory = tempdir().unwrap();
            let config = directory.path().join("config.toml");
            std::fs::write(&config, format!("[channel]\nkind='telegram'\n[channel.telegram]\ntoken='test'\n[agent.{kind}]\ncommand='{}'\nsession_dir='{}'\n[storage]\npath='{}'\n[server]\nendpoint='unix://{}'\n[logging.file]\nenabled=false\n", env!("CARGO_BIN_EXE_agentix"), directory.path().display(), directory.path().join("state").display(), directory.path().join("control.sock").display())).unwrap();
            let output = Command::new(env!("CARGO_BIN_EXE_agentix"))
                .arg("--config")
                .arg(config)
                .arg("doctor")
                .output()
                .await
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("0 live bridges"), "{stdout}");
            assert!(stdout.contains("agentix-bridge"));
        }
    }

    #[tokio::test]
    async fn doctor_checks_configuration_and_the_mock_codex_handshake() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("codex.sock");
        let config = write_config(directory.path(), &socket);
        let log_path = directory.path().join("logs/agentix.log");
        let mut config_text = std::fs::read_to_string(&config).unwrap();
        config_text.push_str(&format!(
            "\n[logging]\nlevel = \"debug\"\n\n[logging.file]\nenabled = true\npath = \"{}\"\nrotation = \"never\"\nmax_files = 2\n",
            log_path.display()
        ));
        std::fs::write(&config, config_text).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            initialize(&mut websocket).await;
            let list = next_json(&mut websocket).await;
            assert_eq!(list["method"], "thread/loaded/list");
            assert_eq!(list["params"]["limit"], 1);
            send_result(
                &mut websocket,
                &list["id"],
                json!({"data": [], "nextCursor": null}),
            )
            .await;
        });

        let output = Command::new(env!("CARGO_BIN_EXE_agentix"))
            .arg("--config")
            .arg(config)
            .arg("doctor")
            .env_remove("AGENTIX_TELEGRAM_TOKEN")
            .env_remove("AGENTIX_FEISHU_APP_ID")
            .env_remove("AGENTIX_FEISHU_APP_SECRET")
            .stdin(Stdio::null())
            .output()
            .await
            .unwrap();

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("ok: configuration and selected-channel owner policy"));
        assert!(stdout.contains("ok: selected channel credentials are configured"));
        assert!(!stdout.contains("mock-token"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("mock-token"));
        assert!(
            !std::fs::read_to_string(&log_path)
                .unwrap()
                .contains("mock-token")
        );
        assert!(stdout.contains("ok: Codex WebSocket-over-UDS handshake"));
        assert!(log_path.exists(), "configured file logger was not created");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn doctor_writes_local_ansi_free_file_logs_and_prunes_old_rotations() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("codex.sock");
        let config = write_config(directory.path(), &socket);
        let log_directory = directory.path().join("logs");
        std::fs::create_dir_all(&log_directory).unwrap();
        for day in ["2026-01-01", "2026-01-02", "2026-01-03", "2026-01-04"] {
            std::fs::write(log_directory.join(format!("agentix.log.{day}")), day).unwrap();
        }
        let log_path = log_directory.join("agentix.log");
        let mut config_text = std::fs::read_to_string(&config).unwrap();
        config_text.push_str(&format!(
            "\n[logging]\nlevel = \"info\"\n\n[logging.file]\nenabled = true\npath = \"{}\"\nrotation = \"daily\"\nmax_files = 2\n",
            log_path.display()
        ));
        std::fs::write(&config, config_text).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            initialize(&mut websocket).await;
            let list = next_json(&mut websocket).await;
            send_result(
                &mut websocket,
                &list["id"],
                json!({"data": [], "nextCursor": null}),
            )
            .await;
        });

        let output = Command::new(env!("CARGO_BIN_EXE_agentix"))
            .arg("--config")
            .arg(&config)
            .arg("doctor")
            .env_remove("AGENTIX_TELEGRAM_TOKEN")
            .stdin(Stdio::null())
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        server.await.unwrap();

        let files = std::fs::read_dir(&log_directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(files.len(), 2, "retained logs: {files:?}");
        let contents = files
            .iter()
            .map(std::fs::read_to_string)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(contents.contains("Agentix diagnostics completed"));
        assert!(!contents.contains("\u{1b}["));
        let timestamp = contents
            .lines()
            .find(|line| line.contains("Agentix diagnostics completed"))
            .and_then(|line| line.split_whitespace().next())
            .unwrap();
        let timestamp =
            time::OffsetDateTime::parse(timestamp, &time::format_description::well_known::Rfc3339)
                .unwrap();
        assert_eq!(
            timestamp.offset(),
            time::UtcOffset::current_local_offset().unwrap()
        );
    }

    async fn next_control_request(listener: &UnixListener) -> (Value, tokio::net::UnixStream) {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        (serde_json::from_str(&line).unwrap(), reader.into_inner())
    }

    fn write_control_config(
        directory: &std::path::Path,
        control_socket: &std::path::Path,
    ) -> std::path::PathBuf {
        let config = directory.join("agentix.toml");
        std::fs::write(
            &config,
            format!(
                r#"[server]
endpoint = "unix://{}"

[channel]
kind = "telegram"

[agent]
kind = "codex"
endpoint = "unix:///must-not-connect-to-codex.sock"

[storage]
path = "{}"

[channel.telegram]
token = "mock-token"
owner_user_ids = [42]
"#,
                control_socket.display(),
                directory.join("state.sqlite3").display()
            ),
        )
        .unwrap();
        config
    }

    #[tokio::test]
    async fn serve_exits_and_logs_when_proxy_endpoint_is_occupied() {
        use std::os::unix::fs::MetadataExt;
        for kind in ["active", "stale", "file", "ws"] {
            let d = tempdir().unwrap();
            let path = d.path().join("proxy.sock");
            let mut unix_listener = None;
            let mut tcp_listener = None;
            let endpoint = if kind == "ws" {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let endpoint = format!("ws://{}", listener.local_addr().unwrap());
                tcp_listener = Some(listener);
                endpoint
            } else {
                if kind == "file" {
                    std::fs::write(&path, "preserve").unwrap();
                } else {
                    let listener = UnixListener::bind(&path).unwrap();
                    if kind == "active" {
                        unix_listener = Some(listener);
                    }
                }
                format!("unix://{}", path.display())
            };
            let inode = std::fs::symlink_metadata(&path).ok().map(|m| m.ino());
            let config = write_config(d.path(), &d.path().join("upstream.sock"));
            let mut source = std::fs::read_to_string(&config).unwrap();
            source = source.replace(
                "kind = \"codex\"",
                &format!(
                    "kind = \"codex\"\nproxy_endpoint = {endpoint:?}\ncommand = \"must-not-launch\""
                ),
            );
            let log = d.path().join("agentix.log");
            source.push_str(&format!(
                "\n[server]\nendpoint='unix://{}'\n[logging.file]\nenabled=true\npath='{}'\nrotation='never'\n",
                d.path().join("control.sock").display(),
                log.display()
            ));
            std::fs::write(&config, source).unwrap();
            let output = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                Command::new(env!("CARGO_BIN_EXE_agentix"))
                    .arg("--config")
                    .arg(&config)
                    .arg("serve")
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .expect("occupied proxy must fail startup promptly")
            .unwrap();
            assert!(!output.status.success());
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("proxy_endpoint"), "{kind}: {stderr}");
            let logs = std::fs::read_to_string(log).unwrap();
            assert!(
                logs.contains("ERROR") && logs.contains("proxy_endpoint"),
                "{logs}"
            );
            assert!(!logs.contains("continuing with retries"));
            assert!(!d.path().join("upstream.sock").exists());
            assert_eq!(
                std::fs::symlink_metadata(&path).ok().map(|m| m.ino()),
                inode
            );
            if kind == "file" {
                assert_eq!(std::fs::read_to_string(&path).unwrap(), "preserve");
            }
            drop((unix_listener, tcp_listener));
        }
    }

    #[tokio::test]
    async fn serve_rejects_public_ws_without_auth_before_launching_upstream() {
        let d = tempdir().unwrap();
        let config = write_config(d.path(), &d.path().join("upstream.sock"));
        let source = std::fs::read_to_string(&config).unwrap().replace(
            "kind = \"codex\"",
            "kind = \"codex\"\nproxy_endpoint = \"ws://0.0.0.0:0\"\ncommand = \"must-not-launch\"",
        );
        std::fs::write(&config, source).unwrap();
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Command::new(env!("CARGO_BIN_EXE_agentix"))
                .arg("--config")
                .arg(&config)
                .arg("serve")
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("invalid authentication must fail promptly")
        .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("ERROR") && stderr.contains("authentication"),
            "{stderr}"
        );
        assert!(!stderr.contains("continuing with retries"));
        assert!(!d.path().join("upstream.sock").exists());
    }

    fn write_config(directory: &std::path::Path, socket: &std::path::Path) -> std::path::PathBuf {
        let config = directory.join("agentix.toml");
        std::fs::write(
            &config,
            format!(
                r#"
[channel]
kind = "telegram"

[agent]
kind = "codex"
endpoint = "unix://{}"

[storage]
path = "{}"

[channel.telegram]
token = "mock-token"
owner_user_ids = [42]
"#,
                socket.display(),
                directory.join("state.sqlite3").display()
            ),
        )
        .unwrap();
        config
    }

    async fn initialize<S>(websocket: &mut tokio_tungstenite::WebSocketStream<S>)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let initialize = next_json(websocket).await;
        websocket
            .send(Message::Text(
                json!({"id": initialize["id"], "result": {}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        assert_eq!(next_json(websocket).await["method"], "initialized");
    }

    async fn next_json<S>(websocket: &mut tokio_tungstenite::WebSocketStream<S>) -> Value
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let message = websocket.next().await.unwrap().unwrap();
        serde_json::from_str(message.to_text().unwrap()).unwrap()
    }

    async fn send_result<S>(
        websocket: &mut tokio_tungstenite::WebSocketStream<S>,
        id: &Value,
        result: Value,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        websocket
            .send(Message::Text(
                json!({"id": id, "result": result}).to_string().into(),
            ))
            .await
            .unwrap();
    }
}

#[test]
fn serve_exposes_codex_proxy_websocket_auth_flags() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agentix"))
        .args(["serve", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for flag in [
        "--ws-auth",
        "--ws-token-file",
        "--ws-token-sha256",
        "--ws-shared-secret-file",
        "--ws-issuer",
        "--ws-audience",
        "--ws-max-clock-skew-seconds",
    ] {
        assert!(help.contains(flag), "missing {flag}");
    }
}
