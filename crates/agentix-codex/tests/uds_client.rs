#[cfg(unix)]
mod unix {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use agentix_codex::{CodexClient, CodexEndpoint};
    use agentix_core::{
        AgentAdapter, AgentEvent, GoalCommand, QueuedPromptPort, SessionCommand,
        SessionControlPort, SessionId,
    };
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use tempfile::tempdir;
    use tokio::net::UnixListener;
    use tokio_tungstenite::tungstenite::Message;

    #[tokio::test]
    async fn managed_endpoint_starts_daemon_and_waits_for_socket() {
        let directory = tempfile::Builder::new()
            .prefix("agentix-")
            .tempdir_in("/tmp")
            .unwrap();
        let codex_home = directory.path().join("codex-home");
        let socket = codex_home
            .join("app-server-control")
            .join("app-server-control.sock");
        let invocation = directory.path().join("invocation");
        let executable = directory.path().join("fake-codex");
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf '%s' \"$*\" > \"{}\"\n",
                invocation.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();

        let server_invocation = invocation.clone();
        let server_socket = socket.clone();
        let server = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !server_invocation.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            std::fs::create_dir_all(server_socket.parent().unwrap()).unwrap();
            let listener = UnixListener::bind(&server_socket).unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            initialize(&mut websocket).await;
        });

        let endpoint = CodexEndpoint::parse_with_codex_home("unix://", Some(&codex_home)).unwrap();
        let client = CodexClient::connect_with_command(endpoint, &executable)
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(invocation).unwrap(),
            "app-server daemon start"
        );
        drop(client);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn custom_endpoint_does_not_start_a_local_daemon() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("custom.sock");
        let invocation = directory.path().join("invocation");
        let executable = directory.path().join("fake-codex");
        std::fs::write(
            &executable,
            format!("#!/bin/sh\nprintf started > \"{}\"\n", invocation.display()),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();

        let endpoint = CodexEndpoint::from_socket_path(&socket).unwrap();
        let result = CodexClient::connect_with_command(endpoint, &executable).await;

        assert!(result.is_err());
        assert!(!invocation.exists());
    }

    #[tokio::test]
    async fn lists_only_sessions_loaded_in_the_running_codex() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("codex.sock");
        let listener = UnixListener::bind(&socket).unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();

            let initialize = next_json(&mut websocket).await;
            assert_eq!(initialize["method"], "initialize");
            assert_eq!(initialize["params"]["clientInfo"]["name"], "agentix");
            assert_eq!(
                initialize["params"]["capabilities"]["experimentalApi"],
                true
            );
            websocket
                .send(Message::Text(
                    json!({"id": initialize["id"], "result": {}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();

            let initialized = next_json(&mut websocket).await;
            assert_eq!(initialized["method"], "initialized");
            assert!(initialized.get("id").is_none());

            let list = next_json(&mut websocket).await;
            assert_eq!(list["method"], "thread/loaded/list");
            assert_eq!(list["params"]["cursor"], "next-page");
            assert_eq!(list["params"]["limit"], 25);
            websocket
                .send(Message::Text(
                    json!({
                        "id": list["id"],
                        "result": {
                            "data": ["thr_running"],
                            "nextCursor": "final-page"
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();

            let read = next_json(&mut websocket).await;
            assert_eq!(read["method"], "thread/read");
            assert_eq!(read["params"]["threadId"], "thr_running");
            assert_eq!(read["params"]["includeTurns"], false);
            websocket
                .send(Message::Text(
                    json!({
                        "id": read["id"],
                        "result": {
                            "thread": {
                                "id": "thr_running",
                                "name": "Agentix",
                                "preview": "Build the gateway",
                                "cwd": "/work/agentix",
                                "updatedAt": 123,
                                "status": {"type": "idle"}
                            }
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
        });

        let endpoint = CodexEndpoint::from_socket_path(&socket).unwrap();
        let client = CodexClient::connect(endpoint).await.unwrap();
        let page = client
            .list_sessions(Some("next-page".into()), 25)
            .await
            .unwrap();

        assert_eq!(page.sessions.len(), 1);
        assert_eq!(page.sessions[0].id.as_str(), "thr_running");
        assert_eq!(page.sessions[0].name.as_deref(), Some("Agentix"));
        assert_eq!(page.sessions[0].cwd.as_deref(), Some("/work/agentix"));
        assert_eq!(page.next_cursor.as_deref(), Some("final-page"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn filters_loaded_threads_without_rollouts() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("codex.sock");
        let listener = UnixListener::bind(&socket).unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            initialize(&mut websocket).await;

            let list = next_json(&mut websocket).await;
            send_result(
                &mut websocket,
                &list["id"],
                json!({"data": ["thr_running", "thr_ephemeral"]}),
            )
            .await;

            for _ in 0..2 {
                let read = next_json(&mut websocket).await;
                let id = read["params"]["threadId"].as_str().unwrap();
                let thread = match id {
                    "thr_running" => json!({
                        "id": id,
                        "name": "Agentix",
                        "ephemeral": false,
                        "path": "/tmp/rollout.jsonl"
                    }),
                    "thr_ephemeral" => json!({
                        "id": id,
                        "ephemeral": true,
                        "path": null
                    }),
                    _ => panic!("unexpected thread {id}"),
                };
                send_result(&mut websocket, &read["id"], json!({"thread": thread})).await;
            }
        });

        let endpoint = CodexEndpoint::from_socket_path(&socket).unwrap();
        let client = CodexClient::connect(endpoint).await.unwrap();
        let page = client.list_sessions(None, 25).await.unwrap();

        assert_eq!(page.sessions.len(), 1);
        assert_eq!(page.sessions[0].id.as_str(), "thr_running");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_attaching_a_thread_without_a_rollout() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("codex.sock");
        let listener = UnixListener::bind(&socket).unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            initialize(&mut websocket).await;

            let request = next_json(&mut websocket).await;
            let method = request["method"].as_str().unwrap().to_owned();
            let result = if method == "thread/read" {
                json!({
                    "thread": {
                        "id": "thr_ephemeral",
                        "ephemeral": true,
                        "path": null
                    }
                })
            } else {
                json!({})
            };
            websocket
                .send(Message::Text(
                    json!({"id": request["id"], "result": result})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            method
        });

        let endpoint = CodexEndpoint::from_socket_path(&socket).unwrap();
        let client = CodexClient::connect(endpoint).await.unwrap();
        let error = client
            .attach(&SessionId::new("thr_ephemeral"))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("no rollout"));
        assert_eq!(server.await.unwrap(), "thread/read");
    }

    #[tokio::test]
    async fn attaches_a_loaded_unmaterialized_thread_and_resumes_after_first_turn() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("codex.sock");
        let listener = UnixListener::bind(&socket).unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            initialize(&mut websocket).await;

            let read = next_json(&mut websocket).await;
            assert_eq!(read["method"], "thread/read");
            send_result(
                &mut websocket,
                &read["id"],
                json!({
                    "thread": {
                        "id": "thr_empty",
                        "ephemeral": false,
                        "path": "/tmp/rollout-thr_empty.jsonl",
                        "turns": []
                    }
                }),
            )
            .await;

            let resume = next_json(&mut websocket).await;
            assert_eq!(resume["method"], "thread/resume");
            assert_eq!(resume["params"]["excludeTurns"], true);
            send_error(
                &mut websocket,
                &resume["id"],
                -32600,
                "no rollout found for thread id thr_empty",
            )
            .await;

            let loaded = next_json(&mut websocket).await;
            assert_eq!(loaded["method"], "thread/loaded/list");
            send_result(
                &mut websocket,
                &loaded["id"],
                json!({"data": ["thr_empty"], "nextCursor": null}),
            )
            .await;

            let listed_read = next_json(&mut websocket).await;
            assert_eq!(listed_read["method"], "thread/read");
            send_result(
                &mut websocket,
                &listed_read["id"],
                json!({
                    "thread": {
                        "id": "thr_empty",
                        "ephemeral": false,
                        "path": "/tmp/rollout-thr_empty.jsonl",
                        "status": {"type": "idle"},
                        "turns": []
                    }
                }),
            )
            .await;

            let history = next_json(&mut websocket).await;
            assert_eq!(history["method"], "thread/turns/list");
            send_error(
                &mut websocket,
                &history["id"],
                -32600,
                "thread thr_empty is not materialized yet; thread/turns/list is unavailable before first user message",
            )
            .await;

            let start = next_json(&mut websocket).await;
            assert_eq!(start["method"], "turn/start");
            send_result(
                &mut websocket,
                &start["id"],
                json!({"turn": {"id": "turn_first"}}),
            )
            .await;

            let resumed = next_json(&mut websocket).await;
            assert_eq!(resumed["method"], "thread/resume");
            assert_eq!(resumed["params"]["excludeTurns"], true);
            send_result(&mut websocket, &resumed["id"], json!({})).await;
        });

        let endpoint = CodexEndpoint::from_socket_path(&socket).unwrap();
        let client = CodexClient::connect(endpoint).await.unwrap();
        let session = SessionId::new("thr_empty");

        client.attach(&session).await.unwrap();
        let history = client.read_history(&session, None, 5).await.unwrap();
        assert!(history.turns.is_empty());
        assert_eq!(
            client.start_turn(&session, "hello").await.unwrap(),
            "turn_first"
        );

        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn queues_and_lists_follow_up_prompts() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("codex.sock");
        let listener = UnixListener::bind(&socket).unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            initialize(&mut websocket).await;

            let add = next_json(&mut websocket).await;
            assert_eq!(add["method"], "thread/queue/add");
            assert_eq!(add["params"]["threadId"], "thr_queue");
            assert_eq!(add["params"]["input"][0]["text"], "run all tests");
            assert_eq!(add["params"]["clientUserMessageId"], "im-message-1");
            send_result(
                &mut websocket,
                &add["id"],
                json!({
                    "queuedSubmission": {
                        "id": "queued_1",
                        "clientUserMessageId": "im-message-1",
                        "input": [{"type": "text", "text": "run all tests"}]
                    }
                }),
            )
            .await;

            let list = next_json(&mut websocket).await;
            assert_eq!(list["method"], "thread/queue/list");
            assert_eq!(list["params"]["threadId"], "thr_queue");
            assert_eq!(list["params"]["limit"], 100);
            assert!(list["params"]["cursor"].is_null());
            send_result(
                &mut websocket,
                &list["id"],
                json!({
                    "data": [
                        {
                            "id": "queued_1",
                            "clientUserMessageId": "im-message-1",
                            "input": [{"type": "text", "text": "run all tests"}]
                        },
                        {
                            "id": "queued_2",
                            "clientUserMessageId": "other-client-message",
                            "input": [{"type": "image", "url": "https://example.com/image.png"}]
                        }
                    ],
                    "nextCursor": null
                }),
            )
            .await;
        });

        let endpoint = CodexEndpoint::from_socket_path(&socket).unwrap();
        let client = CodexClient::connect(endpoint).await.unwrap();
        let queued = client
            .queue_prompt(
                &SessionId::new("thr_queue"),
                "run all tests",
                "im-message-1",
            )
            .await
            .unwrap();
        assert_eq!(queued.id, "queued_1");
        assert_eq!(queued.text, "run all tests");

        let queue = client
            .list_queued_prompts(&SessionId::new("thr_queue"))
            .await
            .unwrap();
        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0].text, "run all tests");
        assert_eq!(queue[1].text, "[non-text input]");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn reconnects_and_resumes_attached_threads_after_socket_loss() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("codex.sock");
        let listener = UnixListener::bind(&socket).unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut first = tokio_tungstenite::accept_async(stream).await.unwrap();
            initialize(&mut first).await;
            let read = next_json(&mut first).await;
            assert_eq!(read["method"], "thread/read");
            assert_eq!(read["params"]["threadId"], "thr_reconnect");
            first
                .send(Message::Text(
                    json!({
                        "id": read["id"],
                        "result": {
                            "thread": {
                                "id": "thr_reconnect",
                                "ephemeral": false,
                                "path": "/tmp/rollout.jsonl"
                            }
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            let resume = next_json(&mut first).await;
            assert_eq!(resume["method"], "thread/resume");
            assert_eq!(resume["params"]["threadId"], "thr_reconnect");
            assert_eq!(resume["params"]["excludeTurns"], true);
            first
                .send(Message::Text(
                    json!({"id": resume["id"], "result": {}}).to_string().into(),
                ))
                .await
                .unwrap();
            first.close(None).await.unwrap();

            let (stream, _) = listener.accept().await.unwrap();
            let mut second = tokio_tungstenite::accept_async(stream).await.unwrap();
            initialize(&mut second).await;
            let restored = next_json(&mut second).await;
            assert_eq!(restored["method"], "thread/resume");
            assert_eq!(restored["params"]["threadId"], "thr_reconnect");
            assert_eq!(restored["params"]["excludeTurns"], true);
        });

        let endpoint = CodexEndpoint::from_socket_path(&socket).unwrap();
        let client = CodexClient::connect(endpoint).await.unwrap();
        let initial_generation = client.generation();
        let mut events = client.subscribe();
        client
            .attach(&SessionId::new("thr_reconnect"))
            .await
            .unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if matches!(
                    events.recv().await.unwrap(),
                    AgentEvent::Connected { generation }
                        if generation != initial_generation && generation == client.generation()
                ) {
                    break;
                }
            }
        })
        .await
        .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn maps_attached_session_commands_to_codex_app_server_requests() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("codex.sock");
        let listener = UnixListener::bind(&socket).unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            initialize(&mut websocket).await;
            let mut requests = Vec::new();
            for _ in 0..20 {
                let request = next_json(&mut websocket).await;
                let result = session_command_response(request["method"].as_str().unwrap());
                send_result(&mut websocket, &request["id"], result).await;
                requests.push(request);
            }
            requests
        });

        let endpoint = CodexEndpoint::from_socket_path(&socket).unwrap();
        let client = CodexClient::connect(endpoint).await.unwrap();
        let session = SessionId::new("thr_commands");
        let commands = [
            SessionCommand::Compact,
            SessionCommand::Plan {
                enabled: true,
                prompt: None,
            },
            SessionCommand::Goal(GoalCommand::Set("ship it".into())),
            SessionCommand::Review,
            SessionCommand::Skills,
            SessionCommand::Status,
            SessionCommand::Mcp,
            SessionCommand::Fork,
            SessionCommand::Model(Some("gpt-5.6".into())),
            SessionCommand::Reasoning(Some("xhigh".into())),
            SessionCommand::Model(None),
            SessionCommand::Reasoning(None),
        ];
        let mut results = Vec::new();
        for command in commands {
            results.push(client.run_session_command(&session, command).await.unwrap());
        }

        assert_eq!(results[3].active_turn.as_deref(), Some("turn_review"));
        assert_eq!(
            results[7].replacement_session.as_ref().unwrap().id.as_str(),
            "thr_fork"
        );
        assert_eq!(results[10].choices[0].label, "GPT-5.6");
        assert_eq!(
            results[10].choices[0].command,
            SessionCommand::Model(Some("gpt-5.6".into()))
        );
        assert_eq!(
            results[11]
                .choices
                .iter()
                .map(|choice| choice.label.as_str())
                .collect::<Vec<_>>(),
            ["High", "Xhigh"]
        );
        let requests = server.await.unwrap();
        assert_session_command_requests(&requests);
    }

    fn assert_session_command_requests(requests: &[Value]) {
        assert!(requests.iter().any(|request| {
            request["method"] == "thread/compact/start"
                && request["params"]["threadId"] == "thr_commands"
        }));
        assert!(requests.iter().any(|request| {
            request["method"] == "thread/settings/update"
                && request["params"]["collaborationMode"]["mode"] == "plan"
        }));
        assert!(requests.iter().any(|request| {
            request["method"] == "review/start"
                && request["params"]["target"]["type"] == "uncommittedChanges"
        }));
        assert!(requests.iter().any(|request| {
            request["method"] == "skills/list" && request["params"]["cwds"][0] == "/work/agentix"
        }));
        assert!(requests.iter().any(|request| {
            request["method"] == "mcpServerStatus/list"
                && request["params"]["threadId"] == "thr_commands"
        }));
        assert!(requests.iter().any(|request| {
            request["method"] == "thread/fork" && request["params"]["excludeTurns"] == true
        }));
        assert!(requests.iter().any(|request| {
            request["method"] == "thread/settings/update" && request["params"]["effort"] == "xhigh"
        }));
    }

    fn session_command_response(method: &str) -> Value {
        match method {
            "thread/read" => json!({
                "thread": {
                    "id": "thr_commands",
                    "name": "Agentix",
                    "cwd": "/work/agentix",
                    "model": "gpt-5.6",
                    "modelProvider": "openai",
                    "reasoningEffort": "high",
                    "status": {"type": "idle"},
                    "updatedAt": 1
                }
            }),
            "thread/resume" => json!({
                "approvalPolicy": "on-request",
                "approvalsReviewer": "user",
                "cwd": "/work/agentix",
                "model": "gpt-5.6",
                "modelProvider": "openai",
                "reasoningEffort": "high",
                "runtimeWorkspaceRoots": ["/work/agentix"],
                "sandbox": {"type": "workspaceWrite"},
                "serviceTier": null,
                "thread": {
                    "id": "thr_commands",
                    "name": "Agentix",
                    "cwd": "/work/agentix",
                    "model": "gpt-5.6",
                    "modelProvider": "openai",
                    "reasoningEffort": "high",
                    "status": {"type": "idle"},
                    "updatedAt": 1
                }
            }),
            "model/list" => json!({
                "data": [{
                    "id": "gpt-5.6",
                    "displayName": "GPT-5.6",
                    "supportedReasoningEfforts": [
                        {"reasoningEffort": "high", "description": "High"},
                        {"reasoningEffort": "xhigh", "description": "Extra high"}
                    ]
                }]
            }),
            "thread/goal/set" => json!({
                "goal": {
                    "objective": "ship it",
                    "status": "active",
                    "tokensUsed": 0,
                    "timeUsedSeconds": 0
                }
            }),
            "thread/goal/get" => json!({"goal": null}),
            "review/start" => json!({
                "reviewThreadId": "thr_commands",
                "turn": {"id": "turn_review"}
            }),
            "skills/list" => json!({
                "data": [{
                    "cwd": "/work/agentix",
                    "skills": [{
                        "name": "openai-docs",
                        "description": "Read official documentation",
                        "enabled": true,
                        "path": "/skills/openai-docs/SKILL.md",
                        "scope": "system"
                    }],
                    "errors": []
                }]
            }),
            "mcpServerStatus/list" => json!({
                "data": [{
                    "name": "github",
                    "authStatus": "oAuth",
                    "runtimeStatus": "connected",
                    "tools": {"search": {}},
                    "resources": [],
                    "resourceTemplates": []
                }]
            }),
            "thread/fork" => replacement_thread("thr_fork"),
            _ => json!({}),
        }
    }

    fn replacement_thread(id: &str) -> Value {
        json!({
            "thread": {
                "id": id,
                "cwd": "/work/agentix",
                "model": "gpt-5.6",
                "status": {"type": "idle"},
                "updatedAt": 2
            }
        })
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
        let initialized = next_json(websocket).await;
        assert_eq!(initialized["method"], "initialized");
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

    async fn send_error<S>(
        websocket: &mut tokio_tungstenite::WebSocketStream<S>,
        id: &Value,
        code: i64,
        message: &str,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        websocket
            .send(Message::Text(
                json!({"id": id, "error": {"code": code, "message": message}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
    }
}
