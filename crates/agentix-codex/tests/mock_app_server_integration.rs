// The mock uses the same Unix-domain socket transport as Codex app-server.
#![cfg(unix)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentix_codex::CodexClient;
use agentix_core::{
    AgentAdapter, AgentEvent, ChannelAdapter, ChannelError, ChannelKind, CommandMenu,
    ConversationRef, Engine, GoalCommand, InboundEnvelope, InteractionDecision, MessageRef,
    OutboundView, QueuedPromptPort, SessionCommand, SessionId, SqliteState, TurnStatus,
};
use async_trait::async_trait;
use serde_json::json;

use support::{MockCodexAppServer, MockThread, MockTurn};

#[tokio::test]
async fn session_selection_exposes_read_only_history_and_menu() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(
            MockThread::new("thr_external", "External", "/work").with_turn(
                MockTurn::in_progress_with_output("turn_external", "Question", "Latest output"),
            ),
        )
        .await;
    server.set_active_writer("thr_external").await;
    let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
    let channel = Arc::new(RecordingChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(client.clone(), state.clone(), vec![channel.clone()]);
    engine.handle_inbound(inbound("/sessions")).await.unwrap();
    let listed = channel.views().last().unwrap().clone();
    assert!(listed.body.contains("Active"));
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-e2e");
    engine
        .handle_inbound(InboundEnvelope::action_from_message(
            "select-external",
            conversation.clone(),
            "owner-e2e",
            listed.actions[0].token.clone(),
            MessageRef::new(conversation.clone(), "session-list"),
        ))
        .await
        .unwrap();
    let views = channel.views();
    assert!(views.iter().any(|v| v.body.contains("read-only")));
    assert!(views.last().unwrap().body.contains("Latest output"));
    assert!(
        views
            .last()
            .unwrap()
            .actions
            .iter()
            .all(|a| a.label != "Stop")
    );
    assert_eq!(
        state.list_bindings().await.unwrap(),
        vec![(conversation, SessionId::new("thr_external"))]
    );
    let menu = channel.menus.lock().unwrap().last().unwrap().clone();
    for name in ["current", "history", "detach"] {
        assert!(menu.commands.iter().any(|c| c.name == name));
    }
    for name in ["stop", "model", "compact", "plan", "review"] {
        assert!(menu.commands.iter().all(|c| c.name != name));
    }
    for text in ["New prompt", "/stop", "/model", "/queue"] {
        engine.handle_inbound(inbound(text)).await.unwrap();
        assert!(channel.views().last().unwrap().body.contains("read-only"));
    }
    assert!(
        !server
            .request_methods()
            .await
            .contains(&"turn/start".into())
    );
}

#[tokio::test]
async fn failed_session_selection_reports_error_and_offers_a_fresh_retry() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("thr_existing", "Existing", "/work"))
        .await;
    server
        .add_thread(MockThread::new("thr_selected", "Selected", "/work"))
        .await;
    let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
    let channel = Arc::new(RecordingChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(client, state.clone(), vec![channel.clone()]);
    engine
        .handle_inbound(inbound("/attach thr_existing"))
        .await
        .unwrap();
    engine.handle_inbound(inbound("/sessions")).await.unwrap();
    let token = channel
        .views()
        .last()
        .unwrap()
        .actions
        .iter()
        .find(|action| !action.disabled)
        .unwrap()
        .token
        .clone();
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-e2e");
    server
        .fail_next("thread/resume", -32600, "Permission denied")
        .await;
    engine
        .handle_inbound(InboundEnvelope::action_from_message(
            "failed-selection",
            conversation.clone(),
            "owner-e2e",
            token.clone(),
            MessageRef::new(conversation.clone(), "session-list"),
        ))
        .await
        .unwrap();
    let failed = channel.views().last().unwrap().clone();
    assert_eq!(failed.status, agentix_core::ViewStatus::Error);
    assert!(failed.body.contains("Permission denied"));
    assert_eq!(failed.actions.len(), 1);
    assert_ne!(failed.actions[0].token, token);
    assert_eq!(
        state.list_bindings().await.unwrap()[0].1,
        SessionId::new("thr_existing")
    );
    engine
        .handle_inbound(InboundEnvelope::action(
            "retry-selection",
            conversation,
            "owner-e2e",
            failed.actions[0].token.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(
        state.list_bindings().await.unwrap()[0].1,
        SessionId::new("thr_selected")
    );
}

#[tokio::test]
async fn failed_history_during_transfer_preserves_the_existing_subscription() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("thr_shared", "Shared", "/work"))
        .await;
    let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
    let channel = Arc::new(RecordingChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(client, state.clone(), vec![channel.clone()]);
    engine
        .handle_inbound(inbound("/attach thr_shared"))
        .await
        .unwrap();
    let original = state.list_bindings().await.unwrap();
    server
        .fail_next("thread/turns/list", -32600, "History unavailable")
        .await;
    let mut request = inbound("/attach thr_shared");
    request.event_id = "transfer-session".into();
    request.conversation = ConversationRef::new(ChannelKind::Telegram, "other-chat");
    engine.handle_inbound(request).await.unwrap();
    assert!(
        channel
            .views()
            .last()
            .unwrap()
            .body
            .contains("History unavailable")
    );
    assert_eq!(state.list_bindings().await.unwrap(), original);
    assert!(
        !server
            .request_methods()
            .await
            .contains(&"thread/unsubscribe".into())
    );
}

#[tokio::test]
async fn read_only_attachment_survives_reconnect_without_resuming() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("thr_external", "External", "/work"))
        .await;
    server.set_active_writer("thr_external").await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let session = SessionId::new("thr_external");
    client.attach(&session).await.unwrap();
    let mut events = client.subscribe();
    server.disconnect_clients();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !matches!(events.recv().await.unwrap(), AgentEvent::Connected { .. }) {}
    })
    .await
    .unwrap();
    assert!(client.is_read_only(&session).await);
    client.read_history(&session, None, 1).await.unwrap();
    assert_eq!(
        server
            .request_methods()
            .await
            .iter()
            .filter(|method| *method == "thread/resume")
            .count(),
        1
    );
}

#[tokio::test]
async fn external_writer_sessions_show_latest_turn_status_without_resuming() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(
            MockThread::new("thr_external", "External", "/work").with_turn(
                MockTurn::in_progress_with_output("turn_external", "Work", "Working"),
            ),
        )
        .await;
    server.set_active_writer("thr_external").await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    assert_eq!(
        client.list_sessions(None, 25).await.unwrap().sessions[0].status,
        agentix_core::SessionStatus::Active
    );
    server
        .complete_turn("thr_external", "turn_external", "Done")
        .await;
    assert_eq!(
        client.list_sessions(None, 25).await.unwrap().sessions[0].status,
        agentix_core::SessionStatus::Idle
    );
    assert!(
        !server
            .request_methods()
            .await
            .contains(&"thread/resume".into())
    );
}

#[tokio::test]
async fn external_writer_polling_delivers_content_without_live_notifications() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(
            MockThread::new("thr_observed", "Observed", "/work").with_turn(
                MockTurn::in_progress_with_output("turn_observed", "Question", "Working"),
            ),
        )
        .await;
    server.set_active_writer("thr_observed").await;
    let client = CodexClient::connect_with_background_turn_notifications(
        server.endpoint(),
        std::path::Path::new("codex"),
        std::path::Path::new("~"),
        false,
    )
    .await
    .unwrap();
    let mut events = client.subscribe();
    client
        .attach(&SessionId::new("thr_observed"))
        .await
        .unwrap();
    let history = client
        .read_history(&SessionId::new("thr_observed"), None, 1)
        .await
        .unwrap();
    let answer_id = history.turns[0]
        .items
        .iter()
        .find(|item| item.kind == "agentMessage")
        .unwrap()
        .id
        .clone();
    // Updating storage without a notification reproduces a different writer process.
    server
        .add_thread(
            MockThread::new("thr_observed", "Observed", "/work").with_turn(MockTurn::completed(
                "turn_observed",
                "Question",
                "Finished externally",
            )),
        )
        .await;
    let mut saw_content = false;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match events.recv().await.unwrap() {
                AgentEvent::ItemCompleted { item, .. } => {
                    if item.kind == "agentMessage" {
                        assert_eq!(item.id, answer_id);
                    }
                    saw_content |= item.text.as_deref() == Some("Finished externally");
                }
                AgentEvent::TurnCompleted { turn_id, .. } if turn_id == "turn_observed" => break,
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert!(saw_content);
    assert_eq!(
        server
            .request_methods()
            .await
            .iter()
            .filter(|m| *m == "thread/resume")
            .count(),
        1
    );
}

#[tokio::test]
async fn attaching_an_external_writer_preserves_read_access_and_rejects_writes_locally() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(
            MockThread::new("thr_external", "External", "/work").with_turn(
                MockTurn::in_progress_with_output("turn_external", "Work", "Working"),
            ),
        )
        .await;
    server.set_active_writer("thr_external").await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let session = SessionId::new("thr_external");
    client.attach(&session).await.unwrap();
    let history = client.read_history(&session, None, 1).await.unwrap();
    assert_eq!(history.turns[0].agent_text.as_deref(), Some("Working"));
    assert!(
        client
            .start_turn(&session, "Do more")
            .await
            .unwrap_err()
            .to_string()
            .contains("read-only")
    );
    assert!(
        client
            .interrupt(&session, "turn_external")
            .await
            .unwrap_err()
            .to_string()
            .contains("read-only")
    );
    let methods = server.request_methods().await;
    assert!(!methods.contains(&"turn/start".into()));
    assert!(!methods.contains(&"turn/interrupt".into()));
    client.unsubscribe(&session).await.unwrap();
    assert!(
        !server
            .request_methods()
            .await
            .contains(&"thread/unsubscribe".into())
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn mock_frames_include_official_required_fields_for_supported_flows() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new(
            "thr_contract",
            "Protocol contract",
            "/work/contract",
        ))
        .await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let session_id = SessionId::new("thr_contract");

    client.list_sessions(None, 25).await.unwrap();
    client.attach(&session_id).await.unwrap();
    let turn_id = client
        .start_turn(&session_id, "verify schema")
        .await
        .unwrap();
    server
        .complete_turn(&session_id.to_string(), &turn_id, "schema verified")
        .await;
    let controls = client.session_control().unwrap();
    controls
        .run_session_command(&session_id, SessionCommand::Model(None))
        .await
        .unwrap();
    controls
        .run_session_command(&session_id, SessionCommand::Review)
        .await
        .unwrap();
    controls
        .run_session_command(&session_id, SessionCommand::Goal(GoalCommand::Clear))
        .await
        .unwrap();
    let _approval = server
        .request_command_approval(
            "thr_contract",
            "turn_contract",
            "item_contract",
            "cargo test",
        )
        .await;
    client.unsubscribe(&session_id).await.unwrap();

    assert_fields(
        &server.last_result("initialize").await.unwrap(),
        &["codexHome", "platformFamily", "platformOs", "userAgent"],
    );
    let thread = &server.last_result("thread/read").await.unwrap()["thread"];
    assert_fields(
        thread,
        &[
            "cliVersion",
            "createdAt",
            "cwd",
            "ephemeral",
            "id",
            "modelProvider",
            "preview",
            "projectId",
            "sessionId",
            "source",
            "status",
            "turns",
            "updatedAt",
        ],
    );
    assert_fields(
        &server.last_result("thread/resume").await.unwrap(),
        &[
            "approvalPolicy",
            "approvalsReviewer",
            "cwd",
            "model",
            "modelProvider",
            "sandbox",
            "thread",
        ],
    );
    assert_fields(
        &server.last_result("thread/unsubscribe").await.unwrap(),
        &["status"],
    );
    assert_fields(
        &server.last_result("thread/goal/clear").await.unwrap(),
        &["cleared"],
    );
    assert_fields(
        &server.last_result("review/start").await.unwrap(),
        &["reviewThreadId", "turn"],
    );
    for model in server.last_result("model/list").await.unwrap()["data"]
        .as_array()
        .unwrap()
    {
        assert_fields(
            model,
            &[
                "defaultReasoningEffort",
                "description",
                "displayName",
                "hidden",
                "id",
                "isDefault",
                "model",
                "supportedReasoningEfforts",
            ],
        );
    }
    let notifications = server.notifications().await;
    let item_completed = notifications
        .iter()
        .find(|frame| frame["method"] == "item/completed")
        .unwrap();
    assert_fields(
        &item_completed["params"],
        &["completedAtMs", "item", "threadId", "turnId"],
    );
    let turn_started = notifications
        .iter()
        .find(|frame| frame["method"] == "turn/started")
        .unwrap();
    assert_fields(&turn_started["params"]["turn"], &["id", "items", "status"]);
    let approval = server.server_requests().await.pop().unwrap();
    assert_fields(
        &approval["params"],
        &["itemId", "startedAtMs", "threadId", "turnId"],
    );
}

#[tokio::test]
async fn codex_client_runs_a_complete_session_lifecycle_against_the_mock_server() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(
            MockThread::new("thr_agentix", "Agentix", "/work/agentix").with_turn(
                MockTurn::completed("turn_1", "inspect the tests", "Existing tests inspected."),
            ),
        )
        .await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let session_id = SessionId::new("thr_agentix");

    let page = client.list_sessions(None, 25).await.unwrap();
    assert_eq!(page.sessions.len(), 1);
    assert_eq!(page.sessions[0].id, session_id);

    client.attach(&session_id).await.unwrap();
    let history = client.read_history(&session_id, None, 1).await.unwrap();
    assert_eq!(history.turns.len(), 1);
    assert_eq!(
        history.turns[0].user_text.as_deref(),
        Some("inspect the tests")
    );
    assert_eq!(
        history.turns[0].agent_text.as_deref(),
        Some("Existing tests inspected.")
    );

    let mut events = client.subscribe();
    let turn_id = client
        .start_turn(&session_id, "add integration coverage")
        .await
        .unwrap();
    assert_eq!(turn_id, "turn_2");
    server
        .complete_turn("thr_agentix", &turn_id, "Integration coverage added.")
        .await;

    assert_eq!(
        recv_event(&mut events).await,
        AgentEvent::TurnStarted {
            session_id: "thr_agentix".into(),
            turn_id: "turn_2".into(),
        }
    );
    assert!(matches!(
        recv_event(&mut events).await,
        AgentEvent::ItemCompleted { item, .. }
            if item.kind == "userMessage" && item.text.as_deref() == Some("add integration coverage")
    ));
    assert!(matches!(
        recv_event(&mut events).await,
        AgentEvent::AgentMessageDelta { delta, .. } if delta == "Integration coverage added."
    ));
    assert!(matches!(
        recv_event(&mut events).await,
        AgentEvent::ItemCompleted { item, .. }
            if item.kind == "agentMessage"
                && item.text.as_deref() == Some("Integration coverage added.")
    ));
    assert_eq!(
        recv_event(&mut events).await,
        AgentEvent::TurnCompleted {
            session_id: "thr_agentix".into(),
            turn_id: "turn_2".into(),
            status: TurnStatus::Completed,
            error: None,
        }
    );

    let stored = server.thread("thr_agentix").await.unwrap();
    assert_eq!(stored.turns.len(), 2);
    assert_eq!(stored.turns[1].user_text, "add integration coverage");
    assert_eq!(stored.turns[1].agent_text, "Integration coverage added.");
}

#[tokio::test]
async fn mock_pagination_covers_sessions_history_models_and_queues() {
    let server = MockCodexAppServer::start();
    server.set_page_size(1).await;
    for index in 1..=3 {
        server
            .add_thread(
                MockThread::new(
                    format!("thr_page_{index}"),
                    format!("Page {index}"),
                    "/work/pages",
                )
                .with_turn(MockTurn::completed(
                    format!("turn_{index}_1"),
                    "first",
                    "first answer",
                ))
                .with_turn(MockTurn::completed(
                    format!("turn_{index}_2"),
                    "second",
                    "second answer",
                )),
            )
            .await;
    }
    let client = CodexClient::connect(server.endpoint()).await.unwrap();

    let first_sessions = client.list_sessions(None, 25).await.unwrap();
    assert_eq!(first_sessions.sessions.len(), 1);
    let second_sessions = client
        .list_sessions(first_sessions.next_cursor, 25)
        .await
        .unwrap();
    assert_eq!(second_sessions.sessions.len(), 1);
    assert_ne!(
        first_sessions.sessions[0].id,
        second_sessions.sessions[0].id
    );

    let session_id = SessionId::new("thr_page_1");
    client.attach(&session_id).await.unwrap();
    let recent = client.read_history(&session_id, None, 25).await.unwrap();
    assert_eq!(recent.turns[0].user_text.as_deref(), Some("second"));
    let older = client
        .read_history(&session_id, recent.older_cursor, 25)
        .await
        .unwrap();
    assert_eq!(older.turns[0].user_text.as_deref(), Some("first"));

    for index in 1..=3 {
        client
            .queue_prompt(
                &session_id,
                &format!("queued {index}"),
                &format!("message-{index}"),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        client.list_queued_prompts(&session_id).await.unwrap().len(),
        3
    );
    let models = client
        .session_control()
        .unwrap()
        .run_session_command(&session_id, SessionCommand::Model(None))
        .await
        .unwrap();
    assert_eq!(models.choices.len(), 2);
}

#[tokio::test]
async fn mock_rpc_failures_cover_history_fallback_and_rejected_requests() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(
            MockThread::new("thr_failure", "Failure paths", "/work/failures").with_turn(
                MockTurn::completed("turn_failure", "recover history", "fallback worked"),
            ),
        )
        .await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let session_id = SessionId::new("thr_failure");
    client.attach(&session_id).await.unwrap();

    server
        .fail_next("thread/turns/list", -32601, "method not found")
        .await;
    let history = client.read_history(&session_id, None, 1).await.unwrap();
    assert_eq!(
        history.turns[0].agent_text.as_deref(),
        Some("fallback worked")
    );
    assert_eq!(
        server
            .request_methods()
            .await
            .iter()
            .rev()
            .take(2)
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["thread/read", "thread/turns/list"]
    );

    server
        .fail_next("turn/start", -32600, "thread is unavailable")
        .await;
    let error = client
        .start_turn(&session_id, "must fail")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("-32600: thread is unavailable"));
}

#[tokio::test]
async fn codex_client_reconnects_and_resubscribes_to_mock_threads() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new(
            "thr_reconnect",
            "Reconnect",
            "/work/reconnect",
        ))
        .await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let session_id = SessionId::new("thr_reconnect");
    client.attach(&session_id).await.unwrap();
    let mut events = client.subscribe();

    server.disconnect_clients();
    let disconnected = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .unwrap()
        .unwrap();
    let disconnected_generation = match disconnected {
        AgentEvent::Disconnected { generation, .. } => generation,
        event => panic!("expected disconnect event, got {event:?}"),
    };
    let connected = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .unwrap()
        .unwrap();
    let connected_generation = match connected {
        AgentEvent::Connected { generation } => generation,
        event => panic!("expected reconnect event, got {event:?}"),
    };
    assert_ne!(connected_generation, disconnected_generation);
    assert_eq!(client.generation(), connected_generation);

    server.wait_for_request_count("initialize", 2).await;
    server.wait_for_request_count("thread/resume", 2).await;
}

#[tokio::test]
async fn codex_client_retries_attach_when_the_connection_closes_before_a_response() {
    for disconnect_method in ["thread/read", "thread/resume"] {
        let server = MockCodexAppServer::start();
        server
            .add_thread(MockThread::new(
                "thr_attach_reconnect",
                "Attach after reconnect",
                "/work/reconnect",
            ))
            .await;
        let client = CodexClient::connect(server.endpoint()).await.unwrap();
        let initial_generation = client.generation();
        server.disconnect_next_response(disconnect_method).await;

        client
            .attach(&SessionId::new("thr_attach_reconnect"))
            .await
            .unwrap();

        server.wait_for_request_count(disconnect_method, 2).await;
        assert_ne!(client.generation(), initial_generation);
    }
}

#[tokio::test]
async fn codex_client_retries_history_when_the_connection_closes_before_a_response() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new(
            "thr_history_reconnect",
            "History after reconnect",
            "/work/reconnect",
        ))
        .await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let session_id = SessionId::new("thr_history_reconnect");
    client.attach(&session_id).await.unwrap();
    server.disconnect_next_response("thread/turns/list").await;

    client.read_history(&session_id, None, 1).await.unwrap();

    server.wait_for_request_count("thread/turns/list", 2).await;
}

#[tokio::test]
async fn session_button_attach_returns_latest_history_across_repeated_reconnects() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(
            MockThread::new("thr_button", "Button attach", "/work/button").with_turn(
                MockTurn::in_progress_with_output(
                    "turn_button",
                    "show the latest history",
                    "Latest history returned.",
                ),
            ),
        )
        .await;
    let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
    let channel = Arc::new(RecordingChannel::default());
    let engine = Engine::new(
        client,
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );

    engine.handle_inbound(inbound("/sessions")).await.unwrap();
    let sessions = channel.views().last().unwrap().clone();
    let attach = sessions
        .actions
        .iter()
        .find(|action| action.label == "Attach" && !action.disabled)
        .unwrap();
    server.disconnect_responses("thread/turns/list", 2).await;

    engine
        .handle_inbound(InboundEnvelope::action(
            "attach-button",
            ConversationRef::new(ChannelKind::Telegram, "chat-e2e"),
            "owner-e2e",
            attach.token.clone(),
        ))
        .await
        .unwrap();

    let views = channel.views();
    let history = views.last().unwrap();
    assert!(history.body.contains("show the latest history"));
    assert!(history.body.contains("Latest history returned."));
    assert_eq!(history.actions[0].label, "Stop");
    server.wait_for_request_count("thread/turns/list", 3).await;
}

#[tokio::test]
async fn codex_client_steers_and_interrupts_an_active_mock_turn() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new(
            "thr_control",
            "Turn control",
            "/work/control",
        ))
        .await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let session_id = SessionId::new("thr_control");
    client.attach(&session_id).await.unwrap();
    let mut events = client.subscribe();

    let turn_id = client
        .start_turn(&session_id, "start the task")
        .await
        .unwrap();
    assert_eq!(
        recv_event(&mut events).await.session_id(),
        Some("thr_control")
    );
    assert_eq!(
        recv_event(&mut events).await.session_id(),
        Some("thr_control")
    );
    let steered = client
        .steer(&session_id, &turn_id, "include edge cases")
        .await
        .unwrap();
    assert_eq!(steered, turn_id);
    client.interrupt(&session_id, &turn_id).await.unwrap();

    assert_eq!(
        recv_event(&mut events).await,
        AgentEvent::TurnCompleted {
            session_id: "thr_control".into(),
            turn_id: turn_id.clone(),
            status: TurnStatus::Interrupted,
            error: None,
        }
    );
    let thread = server.thread("thr_control").await.unwrap();
    assert_eq!(
        thread.turns[0].user_text,
        "start the task\ninclude edge cases"
    );
    assert_eq!(thread.turns[0].status, "interrupted");
}

#[tokio::test]
async fn mock_emits_status_queue_tool_and_external_resolution_events() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new(
            "thr_events",
            "Event coverage",
            "/work/events",
        ))
        .await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let session_id = SessionId::new("thr_events");
    client.attach(&session_id).await.unwrap();
    let mut events = client.subscribe();

    server.set_session_status("thr_events", "active").await;
    assert!(matches!(
        recv_event(&mut events).await,
        AgentEvent::SessionStatusChanged { session_id, status }
            if session_id == "thr_events" && status == agentix_core::SessionStatus::Active
    ));

    client
        .queue_prompt(&session_id, "queued event", "event-message")
        .await
        .unwrap();
    assert_eq!(
        recv_event(&mut events).await,
        AgentEvent::QueueChanged {
            session_id: "thr_events".into()
        }
    );

    server
        .emit_tool_lifecycle(
            "thr_events",
            "turn_tool",
            "item_tool",
            "cargo test --workspace",
        )
        .await;
    assert!(matches!(
        recv_event(&mut events).await,
        AgentEvent::ItemStarted { item_id, kind, .. }
            if item_id == "item_tool" && kind == "commandExecution"
    ));
    assert!(matches!(
        recv_event(&mut events).await,
        AgentEvent::ItemCompleted { item, .. }
            if item.id == "item_tool" && item.kind == "commandExecution"
    ));

    server
        .resolve_interaction_externally("thr_events", "request-external")
        .await;
    assert_eq!(
        recv_event(&mut events).await,
        AgentEvent::InteractionResolved {
            session_id: "thr_events".into(),
            request_id: "request-external".into(),
        }
    );
}

#[tokio::test]
async fn codex_client_exercises_queue_commands_and_interactions_against_stateful_data() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new(
            "thr_commands",
            "Commands",
            "/work/commands",
        ))
        .await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let session_id = SessionId::new("thr_commands");
    client.attach(&session_id).await.unwrap();

    let queued = client
        .queue_prompt(&session_id, "follow up", "telegram-42")
        .await
        .unwrap();
    assert_eq!(queued.text, "follow up");
    assert_eq!(
        client.list_queued_prompts(&session_id).await.unwrap(),
        vec![queued]
    );

    let controls = client.session_control().unwrap();
    let models = controls
        .run_session_command(&session_id, SessionCommand::Model(None))
        .await
        .unwrap();
    assert_eq!(
        models
            .choices
            .iter()
            .map(|choice| choice.label.as_str())
            .collect::<Vec<_>>(),
        vec!["GPT-5.6", "GPT-5.6 Terra"]
    );
    controls
        .run_session_command(
            &session_id,
            SessionCommand::Model(Some("gpt-5.6-terra".into())),
        )
        .await
        .unwrap();
    controls
        .run_session_command(&session_id, SessionCommand::Reasoning(Some("high".into())))
        .await
        .unwrap();
    controls
        .run_session_command(
            &session_id,
            SessionCommand::Goal(GoalCommand::Set("finish integration tests".into())),
        )
        .await
        .unwrap();

    let status = controls
        .run_session_command(&session_id, SessionCommand::Status)
        .await
        .unwrap();
    assert!(status.body.contains("`gpt-5.6-terra`"));
    assert!(status.body.contains("`high`"));
    assert!(status.body.contains("finish integration tests"));

    let mut events = client.subscribe();
    let pending = server
        .request_command_approval(
            "thr_commands",
            "turn_approval",
            "item_approval",
            "cargo test",
        )
        .await;
    let request = match recv_event(&mut events).await {
        AgentEvent::InteractionRequested(request) => request,
        event => panic!("expected interaction request, got {event:?}"),
    };
    client
        .resolve_interaction(InteractionDecision {
            rpc_id: request.rpc_id,
            response: json!({"decision": "accept"}),
        })
        .await
        .unwrap();
    assert_eq!(pending.await.unwrap(), json!({"decision": "accept"}));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn every_attached_session_command_runs_against_the_mock_server() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new(
            "thr_command_suite",
            "Command suite",
            "/work/command-suite",
        ))
        .await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let session_id = SessionId::new("thr_command_suite");
    client.attach(&session_id).await.unwrap();
    let controls = client.session_control().unwrap();

    let compact = controls
        .run_session_command(&session_id, SessionCommand::Compact)
        .await
        .unwrap();
    assert!(compact.body.contains("compaction started"));

    let fork = controls
        .run_session_command(&session_id, SessionCommand::Fork)
        .await
        .unwrap();
    assert_eq!(
        fork.replacement_session.unwrap().id.as_str(),
        "thr_command_suite_fork"
    );

    let reasoning = controls
        .run_session_command(&session_id, SessionCommand::Reasoning(None))
        .await
        .unwrap();
    assert_eq!(
        reasoning
            .choices
            .iter()
            .map(|choice| choice.label.as_str())
            .collect::<Vec<_>>(),
        ["Medium", "High"]
    );

    let skills = controls
        .run_session_command(&session_id, SessionCommand::Skills)
        .await
        .unwrap();
    assert!(skills.body.contains("**testing** · `repo`"));

    let plan = controls
        .run_session_command(
            &session_id,
            SessionCommand::Plan {
                enabled: true,
                prompt: Some("design the implementation".into()),
            },
        )
        .await
        .unwrap();
    assert!(plan.body.contains("Plan mode enabled"));
    let plan = controls
        .run_session_command(
            &session_id,
            SessionCommand::Plan {
                enabled: false,
                prompt: None,
            },
        )
        .await
        .unwrap();
    assert!(plan.body.contains("Plan mode disabled"));

    let initial_goal = controls
        .run_session_command(&session_id, SessionCommand::Goal(GoalCommand::Show))
        .await
        .unwrap();
    assert!(initial_goal.body.contains("No goal is set"));
    for command in [
        GoalCommand::Set("ship the test suite".into()),
        GoalCommand::Pause,
        GoalCommand::Resume,
    ] {
        let goal = controls
            .run_session_command(&session_id, SessionCommand::Goal(command))
            .await
            .unwrap();
        assert!(goal.body.contains("ship the test suite"));
    }
    let cleared = controls
        .run_session_command(&session_id, SessionCommand::Goal(GoalCommand::Clear))
        .await
        .unwrap();
    assert!(cleared.body.contains("goal was cleared"));

    let review = controls
        .run_session_command(&session_id, SessionCommand::Review)
        .await
        .unwrap();
    assert_eq!(review.active_turn.as_deref(), Some("turn_review"));

    let status = controls
        .run_session_command(&session_id, SessionCommand::Status)
        .await
        .unwrap();
    assert!(status.body.contains("**Session:** Command suite"));
    assert!(status.body.contains("**Directory:** `/work/command-suite`"));
    assert!(status.body.contains("**Approval:** `on-request`"));
    assert!(status.body.contains("**Sandbox:** `workspace-write`"));

    let fast = controls
        .run_session_command(&session_id, SessionCommand::Fast(Some(true)))
        .await
        .unwrap();
    assert!(fast.body.contains("Fast mode enabled"));

    let renamed = controls
        .run_session_command(
            &session_id,
            SessionCommand::Rename(Some("Renamed command suite".into())),
        )
        .await
        .unwrap();
    assert!(renamed.body.contains("Renamed command suite"));

    let diff = controls
        .run_session_command(&session_id, SessionCommand::Diff)
        .await
        .unwrap();
    assert!(diff.body.contains("not a Git worktree"));

    let cleared = controls
        .run_session_command(
            &session_id,
            SessionCommand::Clear(Some("Fresh command suite".into())),
        )
        .await
        .unwrap();
    let replacement = cleared.replacement_session.unwrap();
    assert_eq!(replacement.name.as_deref(), Some("Fresh command suite"));
    let replacement_thread = server.thread(replacement.id.as_str()).await.unwrap();
    assert_eq!(replacement_thread.reasoning_effort, "medium");
    assert_eq!(replacement_thread.service_tier.as_deref(), Some("fast"));

    let mcp = controls
        .run_session_command(&session_id, SessionCommand::Mcp)
        .await
        .unwrap();
    assert!(mcp.body.contains("**filesystem** · `connected`"));
    assert!(mcp.body.contains("2 tools"));

    let methods = server.request_methods().await;
    for expected in [
        "thread/compact/start",
        "thread/fork",
        "model/list",
        "skills/list",
        "thread/settings/update",
        "thread/name/set",
        "thread/start",
        "thread/goal/get",
        "thread/goal/set",
        "thread/goal/clear",
        "review/start",
        "mcpServerStatus/list",
    ] {
        assert!(
            methods.iter().any(|method| method == expected),
            "{expected}"
        );
    }
}

#[tokio::test]
async fn plan_prompt_starts_a_turn_and_status_includes_live_token_usage() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new(
            "thr_plan_prompt",
            "Plan prompt",
            "/work/plan",
        ))
        .await;
    let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
    let channel = Arc::new(RecordingChannel::default());
    let engine = Engine::new(
        client.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );

    engine
        .handle_inbound(InboundEnvelope::text(
            "attach-plan",
            ConversationRef::new(ChannelKind::Telegram, "chat-e2e"),
            "owner-e2e",
            "/attach thr_plan_prompt",
        ))
        .await
        .unwrap();
    engine
        .handle_inbound(InboundEnvelope::text(
            "plan-prompt",
            ConversationRef::new(ChannelKind::Telegram, "chat-e2e"),
            "owner-e2e",
            "/plan design a safe rollout",
        ))
        .await
        .unwrap();
    assert_eq!(
        server
            .thread("thr_plan_prompt")
            .await
            .unwrap()
            .turns
            .last()
            .unwrap()
            .user_text,
        "design a safe rollout"
    );

    server
        .send_token_usage("thr_plan_prompt", 12_000, 800, 100_000)
        .await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let status = client
        .session_control()
        .unwrap()
        .run_session_command(&SessionId::new("thr_plan_prompt"), SessionCommand::Status)
        .await
        .unwrap();
    assert!(status.body.contains("800 / 100000 tokens (0.8%)"));
    assert!(status.body.contains("**Tokens:** 12000 total"));
}

#[tokio::test]
async fn diff_command_includes_staged_unstaged_and_untracked_files() {
    let repository = tempfile::tempdir().unwrap();
    let git = |arguments: &[&str]| {
        let output = std::process::Command::new("git")
            .args(arguments)
            .current_dir(repository.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init"]);
    std::fs::write(repository.path().join("tracked.txt"), "before\n").unwrap();
    git(&["add", "tracked.txt"]);
    std::fs::write(repository.path().join("tracked.txt"), "after\n").unwrap();
    std::fs::write(repository.path().join("staged.txt"), "staged\n").unwrap();
    git(&["add", "staged.txt"]);
    std::fs::write(repository.path().join("untracked.txt"), "untracked\n").unwrap();

    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new(
            "thr_diff",
            "Diff",
            repository.path().to_string_lossy(),
        ))
        .await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let diff = client
        .session_control()
        .unwrap()
        .run_session_command(&SessionId::new("thr_diff"), SessionCommand::Diff)
        .await
        .unwrap();

    assert!(diff.body.contains("tracked.txt"));
    assert!(diff.body.contains("staged.txt"));
    assert!(diff.body.contains("untracked.txt"));
    assert!(diff.body.starts_with("```diff\n"));
}

#[tokio::test]
async fn engine_resolves_a_codex_approval_and_clears_the_im_actions() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new(
            "thr_approval",
            "Approval flow",
            "/work/approval",
        ))
        .await;
    let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
    let channel = Arc::new(RecordingChannel::default());
    let engine = Engine::new(
        client.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    let mut events = client.subscribe();
    engine
        .handle_inbound(InboundEnvelope::text(
            "attach-approval",
            ConversationRef::new(ChannelKind::Telegram, "chat-e2e"),
            "owner-e2e",
            "/attach thr_approval",
        ))
        .await
        .unwrap();

    let pending = server
        .request_command_approval(
            "thr_approval",
            "turn_approval",
            "item_approval",
            "cargo test --workspace",
        )
        .await;
    engine
        .handle_agent_event(recv_event(&mut events).await)
        .await
        .unwrap();
    let approval = channel.views().last().unwrap().clone();
    let allow = approval
        .actions
        .iter()
        .find(|action| action.label == "Allow once")
        .unwrap();
    engine
        .handle_inbound(InboundEnvelope::action(
            "approve-command",
            ConversationRef::new(ChannelKind::Telegram, "chat-e2e"),
            "owner-e2e",
            allow.token.clone(),
        ))
        .await
        .unwrap();

    assert_eq!(pending.await.unwrap(), json!({"decision": "accept"}));
    let resolved = channel.views().last().unwrap().clone();
    assert!(resolved.actions.is_empty());
    assert!(resolved.body.contains("**Selected:** Allow once"));
    assert_eq!(resolved.status, agentix_core::ViewStatus::Success);
}

#[tokio::test]
async fn engine_handles_file_approval_and_plan_input_from_the_mock_server() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new(
            "thr_interactions",
            "Interactions",
            "/work/interactions",
        ))
        .await;
    let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
    let channel = Arc::new(RecordingChannel::default());
    let engine = Engine::new(
        client.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    let mut events = client.subscribe();
    engine
        .handle_inbound(InboundEnvelope::text(
            "attach-interactions",
            ConversationRef::new(ChannelKind::Telegram, "chat-e2e"),
            "owner-e2e",
            "/attach thr_interactions",
        ))
        .await
        .unwrap();

    let file_response = server
        .request_file_approval(
            "thr_interactions",
            "turn_file",
            "item_file",
            "write test fixtures",
        )
        .await;
    engine
        .handle_agent_event(recv_event(&mut events).await)
        .await
        .unwrap();
    let file_view = channel.views().last().unwrap().clone();
    let allow = file_view
        .actions
        .iter()
        .find(|action| action.label == "Allow once")
        .unwrap();
    engine
        .handle_inbound(InboundEnvelope::action(
            "approve-file",
            ConversationRef::new(ChannelKind::Telegram, "chat-e2e"),
            "owner-e2e",
            allow.token.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(file_response.await.unwrap(), json!({"decision": "accept"}));

    let input_response = server
        .request_user_input(
            "thr_interactions",
            "turn_plan",
            "item_plan",
            json!([{
                "id": "approach",
                "header": "Approach",
                "question": "How should this be implemented?",
                "options": [
                    {"label": "Thorough", "description": "Cover every supported flow."},
                    {"label": "Minimal", "description": "Cover only the happy path."}
                ]
            }]),
        )
        .await;
    engine
        .handle_agent_event(recv_event(&mut events).await)
        .await
        .unwrap();
    let input_view = channel.views().last().unwrap().clone();
    let thorough = input_view
        .actions
        .iter()
        .find(|action| action.label == "Thorough")
        .unwrap();
    engine
        .handle_inbound(InboundEnvelope::action(
            "select-plan-input",
            ConversationRef::new(ChannelKind::Telegram, "chat-e2e"),
            "owner-e2e",
            thorough.token.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(
        input_response.await.unwrap(),
        json!({"answers": {"approach": {"answers": ["Thorough"]}}})
    );
}

#[tokio::test]
async fn background_codex_turn_completion_notifies_im_with_attach_action() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(
            MockThread::new("thr_background", "Background work", "/work/background").with_turn(
                MockTurn::in_progress_with_output(
                    "turn_background",
                    "finish outside the attached session",
                    "",
                ),
            ),
        )
        .await;
    let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
    let channel = Arc::new(RecordingChannel::default());
    let engine = Engine::new(
        client.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    let mut events = client.subscribe();

    engine.handle_inbound(inbound("/help")).await.unwrap();
    let before = channel.views().len();
    server.wait_for_turn_reads("thr_background", 1).await;
    server
        .complete_turn(
            "thr_background",
            "turn_background",
            "Background work completed.",
        )
        .await;
    engine
        .handle_agent_event(recv_background_event(&mut events).await)
        .await
        .unwrap();

    let views = channel.views();
    assert_eq!(views.len(), before + 1);
    let notification = views.last().unwrap();
    // Completion delivery does not wait for optional title metadata.
    assert!(matches!(
        notification.title.as_str(),
        "Codex · thr_back · Background work" | "Codex · thr_back"
    ));
    assert!(
        notification
            .body
            .contains("> finish outside the attached session")
    );
    assert!(notification.body.contains("> Background work completed."));
    assert_eq!(notification.status, agentix_core::ViewStatus::Background);
    assert_eq!(notification.actions.len(), 1);
    assert_eq!(notification.actions[0].label, "Attach");

    engine
        .handle_inbound(InboundEnvelope::action(
            "attach-background",
            ConversationRef::new(ChannelKind::Telegram, "chat-e2e"),
            "owner-e2e",
            notification.actions[0].token.clone(),
        ))
        .await
        .unwrap();
    assert!(
        server
            .request_methods()
            .await
            .iter()
            .any(|method| method == "thread/resume")
    );
}

#[tokio::test]
async fn background_subagent_polled_completion_does_not_notify_im() {
    assert_subagent_completion_does_not_notify_im(true).await;
}

#[tokio::test]
async fn background_subagent_direct_completion_does_not_notify_im() {
    assert_subagent_completion_does_not_notify_im(false).await;
}

async fn assert_subagent_completion_does_not_notify_im(polled: bool) {
    for source in [
        json!({"subAgent": {"thread_spawn": {"parent_thread_id": "parent", "depth": 1}}}),
        json!({"subAgent": "review"}),
        json!({"subAgent": "compact"}),
        json!({"subAgent": "memory_consolidation"}),
        json!({"subAgent": {"other": "helper"}}),
    ]
    .into_iter()
    .take(if polled { 1 } else { 5 })
    {
        let server = MockCodexAppServer::start();
        let mut thread = MockThread::new("thr_subagent", "Subagent work", "/work").with_turn(
            MockTurn::in_progress_with_output("turn_subagent", "delegated work", ""),
        );
        thread.source = source.clone();
        server.add_thread(thread).await;
        let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
        let channel = Arc::new(RecordingChannel::default());
        let engine = Engine::new(
            client.clone(),
            SqliteState::in_memory().await.unwrap(),
            vec![channel.clone()],
        );
        let mut events = client.subscribe();
        engine.handle_inbound(inbound("/help")).await.unwrap();
        let before = channel.views().len();
        let event = if polled {
            server.wait_for_turn_reads("thr_subagent", 1).await;
            server
                .complete_turn("thr_subagent", "turn_subagent", "Delegated work completed.")
                .await;
            recv_background_event(&mut events).await
        } else {
            AgentEvent::TurnCompleted {
                session_id: "thr_subagent".into(),
                turn_id: "turn_subagent".into(),
                status: TurnStatus::Completed,
                error: None,
            }
        };
        engine.handle_agent_event(event).await.unwrap();
        assert_eq!(
            channel.views().len(),
            before,
            "subagent completion must not notify IM: source={source}, polled={polled}"
        );
        if !polled {
            for (turn_id, status) in [
                ("turn_failed", TurnStatus::Failed),
                ("turn_interrupted", TurnStatus::Interrupted),
            ] {
                engine
                    .handle_agent_event(AgentEvent::TurnCompleted {
                        session_id: "thr_subagent".into(),
                        turn_id: turn_id.into(),
                        status,
                        error: None,
                    })
                    .await
                    .unwrap();
                assert_eq!(
                    channel.views().len(),
                    before,
                    "source={source}, turn={turn_id}"
                );
            }
        }
    }
}

#[tokio::test]
async fn background_completion_source_lookup_failure_does_not_notify_im() {
    let server = MockCodexAppServer::start();
    let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
    let channel = Arc::new(RecordingChannel::default());
    let engine = Engine::new(
        client,
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    engine.handle_inbound(inbound("/help")).await.unwrap();
    let before = channel.views().len();
    let result = engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_missing".into(),
            turn_id: "turn_missing".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await;
    assert_eq!(channel.views().len(), before);
    assert!(result.is_err());
}

#[tokio::test]
async fn background_root_source_variants_still_notify_im() {
    for source in [json!("appServer"), json!({"custom": "desktop"})] {
        let server = MockCodexAppServer::start();
        let mut thread = MockThread::new("thr_root", "Root work", "/work")
            .with_turn(MockTurn::completed("turn_root", "root work", "done"));
        thread.source = source.clone();
        server.add_thread(thread).await;
        let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
        let channel = Arc::new(RecordingChannel::default());
        let engine = Engine::new(
            client,
            SqliteState::in_memory().await.unwrap(),
            vec![channel.clone()],
        );
        engine.handle_inbound(inbound("/help")).await.unwrap();
        let before = channel.views().len();
        engine
            .handle_agent_event(AgentEvent::TurnCompleted {
                session_id: "thr_root".into(),
                turn_id: "turn_root".into(),
                status: TurnStatus::Completed,
                error: None,
            })
            .await
            .unwrap();
        assert_eq!(channel.views().len(), before + 1, "source={source}");
    }
}

#[tokio::test]
async fn background_completion_with_an_active_writer_uses_only_reads() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(
            MockThread::new("thr_writer", "External writer", "/work").with_turn(
                MockTurn::in_progress_with_output("turn_writer", "external work", ""),
            ),
        )
        .await;
    server.set_active_writer("thr_writer").await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let mut events = client.subscribe();
    // Discovery also reads full history for external-writer status. Wait for the
    // separate metadata poll to establish its baseline before completing the turn.
    server.wait_for_background_turn_reads("thr_writer", 1).await;
    server
        .complete_turn("thr_writer", "turn_writer", "done")
        .await;
    assert_eq!(
        recv_background_event(&mut events).await,
        AgentEvent::TurnCompleted {
            session_id: "thr_writer".into(),
            turn_id: "turn_writer".into(),
            status: TurnStatus::Completed,
            error: None,
        }
    );
    // The fourth poll starts only after a post-completion poll has finished,
    // so duplicate-event assertions do not race the client's response handling.
    server.wait_for_background_turn_reads("thr_writer", 4).await;
    assert!(
        events.try_recv().is_err(),
        "completion must only be emitted once"
    );
    assert!(
        !server
            .request_methods()
            .await
            .iter()
            .any(|method| method == "thread/resume")
    );
}

#[tokio::test]
async fn background_polling_skips_history_and_reports_all_new_terminal_statuses() {
    let server = MockCodexAppServer::start();
    server.set_page_size(1).await;
    let old = MockTurn::completed("turn_old", "old work", "old answer");
    server
        .add_thread(MockThread::new("thr_history", "History", "/work").with_turn(old.clone()))
        .await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let mut events = client.subscribe();
    server.wait_for_turn_reads("thr_history", 2).await;
    assert!(
        events.try_recv().is_err(),
        "historical completions must stay silent"
    );
    let mut failed = MockTurn::completed("turn_failed", "failed work", "");
    failed.status = "failed".into();
    failed.error = Some("upstream failed".into());
    let mut interrupted = MockTurn::completed("turn_interrupted", "cancelled work", "");
    interrupted.status = "interrupted".into();
    server
        .add_thread(
            MockThread::new("thr_history", "History", "/work")
                .with_turn(old)
                .with_turn(failed)
                .with_turn(interrupted)
                .with_turn(MockTurn::completed("turn_done", "new work", "done")),
        )
        .await;
    for (turn_id, status) in [
        ("turn_failed", TurnStatus::Failed),
        ("turn_interrupted", TurnStatus::Interrupted),
        ("turn_done", TurnStatus::Completed),
    ] {
        assert_eq!(
            recv_background_event(&mut events).await,
            AgentEvent::TurnCompleted {
                session_id: "thr_history".into(),
                turn_id: turn_id.into(),
                error: (status == TurnStatus::Failed).then(|| "upstream failed".into()),
                status,
            }
        );
    }
}

#[tokio::test]
async fn background_polling_falls_back_to_reading_turns_without_resuming() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(
            MockThread::new("thr_fallback", "Fallback", "/work").with_turn(
                MockTurn::in_progress_with_output("turn_fallback", "work", ""),
            ),
        )
        .await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let mut events = client.subscribe();
    server.wait_for_turn_reads("thr_fallback", 1).await;
    server
        .fail_next("thread/turns/list", -32601, "method unavailable")
        .await;
    server
        .complete_turn("thr_fallback", "turn_fallback", "done")
        .await;
    assert!(matches!(
        recv_background_event(&mut events).await,
        AgentEvent::TurnCompleted {
            status: TurnStatus::Completed,
            ..
        }
    ));
    assert!(
        !server
            .request_methods()
            .await
            .iter()
            .any(|method| method == "thread/resume")
    );
}

#[tokio::test]
async fn background_polling_reports_a_turn_completed_before_first_discovery() {
    let server = MockCodexAppServer::start();
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let mut events = client.subscribe();
    server.wait_for_request_count("thread/loaded/list", 1).await;
    let mut turn = MockTurn::completed("turn_fast", "fast work", "done");
    turn.completed_at = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap();
    server
        .add_thread(MockThread::new("thr_fast", "Fast work", "/work").with_turn(turn))
        .await;
    assert!(
        matches!(recv_background_event(&mut events).await, AgentEvent::TurnCompleted {
        turn_id, status: TurnStatus::Completed, ..
    } if turn_id == "turn_fast")
    );
}

#[tokio::test]
async fn background_polling_does_not_replay_a_streamed_completion_after_detach() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(
            MockThread::new("thr_streamed", "Streamed", "/work").with_turn(
                MockTurn::in_progress_with_output("turn_streamed", "work", ""),
            ),
        )
        .await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let session = SessionId::new("thr_streamed");
    let mut events = client.subscribe();
    client.attach(&session).await.unwrap();
    server
        .complete_turn("thr_streamed", "turn_streamed", "done")
        .await;
    for _ in 0..3 {
        recv_event(&mut events).await;
    }
    server
        .add_thread(
            MockThread::new("thr_streamed", "Streamed", "/work")
                .with_turn(MockTurn::completed("turn_streamed", "work", "done"))
                .with_turn(MockTurn::in_progress_with_output(
                    "turn_second",
                    "more work",
                    "",
                )),
        )
        .await;
    server
        .complete_turn("thr_streamed", "turn_second", "also done")
        .await;
    for _ in 0..3 {
        recv_event(&mut events).await;
    }
    client.unsubscribe(&session).await.unwrap();
    server.wait_for_turn_reads("thr_streamed", 1).await;
    assert!(
        events.try_recv().is_err(),
        "streamed completions must not become background notices"
    );
    server
        .add_thread(
            MockThread::new("thr_streamed", "Streamed", "/work")
                .with_turn(MockTurn::completed("turn_streamed", "work", "done"))
                .with_turn(MockTurn::completed("turn_second", "more work", "also done"))
                .with_turn(MockTurn::completed(
                    "turn_third",
                    "background work",
                    "new result",
                )),
        )
        .await;
    assert!(matches!(recv_background_event(&mut events).await,
        AgentEvent::TurnCompleted { turn_id, .. } if turn_id == "turn_third"));
}

async fn recv_background_event(
    receiver: &mut tokio::sync::broadcast::Receiver<AgentEvent>,
) -> AgentEvent {
    tokio::time::timeout(Duration::from_secs(25), receiver.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn background_unmaterialized_thread_error_logs_at_debug() {
    assert_background_error_log(
        -32600,
        "thread thr_log is not materialized yet; thread/turns/list is unavailable before first user message",
        "DEBUG",
    )
    .await;
}

#[tokio::test]
async fn background_unexpected_invalid_request_logs_at_warn() {
    assert_background_error_log(-32600, "invalid turn list parameters", "WARN").await;
}

#[tokio::test]
async fn background_unmaterialized_message_with_unexpected_code_logs_at_warn() {
    assert_background_error_log(-32000, "thread thr_log is not materialized yet", "WARN").await;
}

async fn assert_background_error_log(code: i64, message: &str, level: &str) {
    let logs = RecordedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(logs.clone())
        .finish();
    // These tests use a current-thread runtime, including the spawned monitor.
    let _guard = tracing::subscriber::set_default(subscriber);
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("thr_log", "Log levels", "/work/logs"))
        .await;
    server.fail_next("thread/turns/list", code, message).await;
    let _client = CodexClient::connect(server.endpoint()).await.unwrap();
    let line = tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
            if let Some(line) = output
                .lines()
                .find(|line| line.contains("failed to read background Codex turns"))
            {
                break line.to_owned();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("background polling should log its error");
    assert!(
        line.trim_start().starts_with(level),
        "expected {level}, got {line}"
    );
    assert!(line.contains(message), "missing RPC error: {line}");
    assert!(line.contains("session=thr_log"), "missing session: {line}");
}

#[derive(Clone, Default)]
struct RecordedLogs(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for RecordedLogs {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RecordedLogs {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn background_monitor_discovers_later_pages_and_retries_failed_reads() {
    let server = MockCodexAppServer::start();
    server.set_page_size(1).await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let mut events = client.subscribe();
    // The service has no attached sessions, and these sessions appear after it starts.
    server.wait_for_request_count("thread/loaded/list", 1).await;
    for id in ["thr_first", "thr_later"] {
        server
            .add_thread(MockThread::new(id, id, "/work").with_turn(
                MockTurn::in_progress_with_output("turn_external", "external work", ""),
            ))
            .await;
    }
    server
        .fail_next("thread/turns/list", -32000, "temporary read failure")
        .await;
    server.wait_for_turn_reads("thr_later", 1).await;
    server.wait_for_turn_reads("thr_first", 1).await;
    server
        .complete_turn("thr_later", "turn_external", "done")
        .await;
    loop {
        if let AgentEvent::TurnCompleted {
            session_id, status, ..
        } = recv_background_event(&mut events).await
        {
            assert_eq!(session_id, "thr_later");
            assert_eq!(status, TurnStatus::Completed);
            break;
        }
    }
}

#[tokio::test]
async fn detached_codex_session_keeps_notifying_about_later_turns() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("thr_detached", "Detached work", "/work"))
        .await;
    let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
    let channel = Arc::new(RecordingChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(client.clone(), state.clone(), vec![channel.clone()]);
    let mut events = client.subscribe();
    engine
        .handle_inbound(inbound("/attach thr_detached"))
        .await
        .unwrap();
    engine.handle_inbound(inbound("/detach")).await.unwrap();
    let before = channel.views().len();
    server.wait_for_turn_reads("thr_detached", 2).await;
    server
        .add_thread(
            MockThread::new("thr_detached", "Detached work", "/work").with_turn(
                MockTurn::in_progress_with_output("turn_later", "more work", ""),
            ),
        )
        .await;
    server
        .complete_turn("thr_detached", "turn_later", "done")
        .await;
    engine
        .handle_agent_event(recv_background_event(&mut events).await)
        .await
        .unwrap();
    assert_eq!(channel.views().len(), before + 1);
    assert!(
        channel
            .views()
            .last()
            .unwrap()
            .subtitle
            .as_ref()
            .unwrap()
            .contains("Background")
    );
    assert!(state.list_bindings().await.unwrap().is_empty());
}

#[tokio::test]
async fn engine_and_codex_client_complete_an_im_turn_end_to_end() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("thr_e2e", "End-to-end", "/work/e2e"))
        .await;
    let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
    let channel = Arc::new(RecordingChannel::default());
    let engine = Engine::new(
        client.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    let mut events = client.subscribe();

    engine
        .handle_inbound(inbound("/attach thr_e2e"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("run integration tests"))
        .await
        .unwrap();
    let turn_id = server.latest_turn_id("thr_e2e").await.unwrap();
    server
        .complete_turn("thr_e2e", &turn_id, "All integration tests passed.")
        .await;

    for _ in 0..5 {
        engine
            .handle_agent_event(recv_event(&mut events).await)
            .await
            .unwrap();
    }

    let views = channel.views();
    let final_turn = views.last().unwrap();
    assert_eq!(final_turn.status, agentix_core::ViewStatus::Success);
    assert!(final_turn.body.contains("run integration tests"));
    assert!(final_turn.body.contains("All integration tests passed."));
    assert!(final_turn.actions.is_empty());

    let methods = server.request_methods().await;
    assert_eq!(methods.len(), 7);
    assert_eq!(
        &methods[..4],
        [
            "initialize",
            "thread/read",
            "thread/resume",
            "thread/loaded/list"
        ]
    );
    // Optional title metadata and required history are read concurrently.
    // Preserve the request budget without imposing an order between them.
    let mut attachment_reads = methods[4..6].to_vec();
    attachment_reads.sort();
    assert_eq!(attachment_reads, ["thread/read", "thread/turns/list"]);
    assert_eq!(methods[6], "turn/start");
}

async fn recv_event(receiver: &mut tokio::sync::broadcast::Receiver<AgentEvent>) -> AgentEvent {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("timed out waiting for Codex event")
            .expect("Codex event stream closed");
        if !matches!(event, AgentEvent::Connected { .. }) {
            return event;
        }
    }
}

fn assert_fields(value: &serde_json::Value, fields: &[&str]) {
    for field in fields {
        assert!(
            value.get(field).is_some(),
            "missing field {field} in {value}"
        );
    }
}

fn inbound(text: &str) -> InboundEnvelope {
    InboundEnvelope::text(
        format!("event-{text}"),
        ConversationRef::new(ChannelKind::Telegram, "chat-e2e"),
        "owner-e2e",
        text,
    )
}

#[derive(Clone, Default)]
struct RecordingChannel {
    streaming_interval: Option<Duration>,
    views: Arc<Mutex<Vec<OutboundView>>>,
    menus: Arc<Mutex<Vec<CommandMenu>>>,
}

impl RecordingChannel {
    fn views(&self) -> Vec<OutboundView> {
        self.views.lock().unwrap().clone()
    }
}

#[async_trait]
impl ChannelAdapter for RecordingChannel {
    fn streaming_update_interval(&self) -> Duration {
        self.streaming_interval.unwrap_or(Duration::from_secs(1))
    }

    fn kind(&self) -> ChannelKind {
        ChannelKind::Telegram
    }

    async fn send(
        &self,
        conversation: &ConversationRef,
        view: &OutboundView,
    ) -> Result<MessageRef, ChannelError> {
        let mut views = self.views.lock().unwrap();
        views.push(view.clone());
        Ok(MessageRef::new(
            conversation.clone(),
            format!("message-{}", views.len()),
        ))
    }

    async fn update(
        &self,
        _conversation: &ConversationRef,
        _message: &MessageRef,
        view: &OutboundView,
    ) -> Result<(), ChannelError> {
        self.views.lock().unwrap().push(view.clone());
        Ok(())
    }

    fn supports_command_menu_sync(&self) -> bool {
        true
    }

    async fn sync_command_menu(
        &self,
        conversation: &ConversationRef,
        menu: &CommandMenu,
    ) -> Result<(), ChannelError> {
        self.set_command_menu(conversation, menu).await
    }

    async fn set_command_menu(
        &self,
        _conversation: &ConversationRef,
        menu: &CommandMenu,
    ) -> Result<(), ChannelError> {
        self.menus.lock().unwrap().push(menu.clone());
        Ok(())
    }
}

#[tokio::test]
async fn background_notifications_can_be_disabled_and_enabled_on_a_live_connection() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("thr_reload", "Reload", "/work"))
        .await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let original_generation = client.generation();
    client.clone().set_background_turn_notifications(false);
    tokio::time::sleep(Duration::from_secs(11)).await;
    assert_eq!(server.request_methods().await, ["initialize"]);
    client.set_background_turn_notifications(true);
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            if server
                .request_methods()
                .await
                .contains(&"thread/loaded/list".into())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(client.generation(), original_generation);
    assert_eq!(
        server
            .request_methods()
            .await
            .iter()
            .filter(|method| *method == "initialize")
            .count(),
        1
    );
}

#[tokio::test]
async fn disabled_background_notifications_do_not_poll_sessions_or_turns() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("thr_disabled", "Disabled", "/work"))
        .await;
    let client = CodexClient::connect_with_background_turn_notifications(
        server.endpoint(),
        std::path::Path::new("codex"),
        std::path::Path::new("~"),
        false,
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_secs(11)).await;
    assert_eq!(server.request_methods().await, ["initialize"]);
    // Explicit user requests remain usable while automatic background reads are disabled.
    assert_eq!(
        client.list_sessions(None, 20).await.unwrap().sessions.len(),
        1
    );
    assert!(
        !server
            .request_methods()
            .await
            .contains(&"thread/turns/list".into())
    );
}

#[tokio::test]
async fn status_reports_remaining_quota_windows_and_survives_quota_errors() {
    use agentix_core::SessionControlPort;
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("thr_quota", "Quota", "/work/quota"))
        .await;
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    let session = SessionId::new("thr_quota");
    server.set_rate_limits(json!({"rateLimitsByLimitId": {
        "codex":{"limitName":"Codex", "primary":{"usedPercent":25,"windowDurationMins":300,"resetsAt":1_788_566_400},"secondary":{"usedPercent":80.5,"windowDurationMins":10080,"resetsAt":1_788_652_800}},
        "review":{"primary":{"usedPercent":100,"windowDurationMins":60},"credits":{"balance":"12.50","unlimited":false}}
    }})).await;
    let status = client
        .run_session_command(&session, SessionCommand::Status)
        .await
        .unwrap();
    for expected in [
        "75% remaining",
        "19.5% remaining",
        "0% remaining",
        "5h",
        "7d",
        "resets",
        "12.50",
    ] {
        assert!(
            status.body.contains(expected),
            "{expected}: {}",
            status.body
        );
    }
    server
        .set_rate_limits(
            json!({"rateLimits":{"primary":{"usedPercent":40,"windowDurationMins":15}}}),
        )
        .await;
    assert!(
        client
            .run_session_command(&session, SessionCommand::Status)
            .await
            .unwrap()
            .body
            .contains("60% remaining")
    );
    server
        .fail_next(
            "account/rateLimits/read",
            -32601,
            "Unsupported quota endpoint",
        )
        .await;
    let status = client
        .run_session_command(&session, SessionCommand::Status)
        .await
        .unwrap();
    assert!(status.body.contains("**Session:** Quota"));
    assert!(status.body.contains("Quota unavailable"));
    server
        .set_rate_limits(json!({"rateLimits":{"primary":{"windowDurationMins":300}}}))
        .await;
    let status = client
        .run_session_command(&session, SessionCommand::Status)
        .await
        .unwrap();
    assert!(status.body.contains("not reported"));
    assert!(!status.body.contains("100% remaining"));
}

#[tokio::test]
async fn cli_proxy_questions_can_be_answered_from_attached_or_background_im() {
    use agentix_codex::{CodexEndpoint, CodexProxy};
    use std::path::Path;
    for attached in [false, true] {
        let server = MockCodexAppServer::start();
        server
            .add_thread(MockThread::new("thr_question", "CLI question", "/work"))
            .await;
        let directory = tempfile::tempdir().unwrap();
        let proxy = CodexProxy::bind(
            &format!("unix://{}", directory.path().join("proxy.sock").display()),
            &format!("unix://{}", server.endpoint().socket_path().display()),
        )
        .await
        .unwrap();
        let cli = CodexClient::connect(CodexEndpoint::parse(proxy.endpoint()).unwrap())
            .await
            .unwrap();
        cli.set_background_turn_notifications(false);
        cli.attach(&SessionId::new("thr_question")).await.unwrap();
        server.set_active_writer("thr_question").await;
        let client = Arc::new(
            CodexClient::connect_with_registry(
                server.endpoint(),
                Path::new("codex"),
                Path::new("/tmp"),
                true,
                proxy.registry(),
            )
            .await
            .unwrap(),
        );
        let mut events = client.subscribe();
        let channel = Arc::new(RecordingChannel::default());
        let engine = Engine::new(
            client.clone(),
            SqliteState::in_memory().await.unwrap(),
            vec![channel.clone()],
        );
        engine
            .handle_inbound(inbound(if attached {
                "/attach thr_question"
            } else {
                "/help"
            }))
            .await
            .unwrap();
        let response = server.request_user_input("thr_question", "turn-question", "item-question", json!([{"id":"choice","header":"Choice","question":"Which approach?","options":[{"label":"Fast","description":"Small change"}]}])).await;
        let event = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let event = events.recv().await.unwrap();
                if matches!(event, AgentEvent::InteractionRequested(_)) {
                    break event;
                }
            }
        })
        .await
        .unwrap();
        engine.handle_agent_event(event).await.unwrap();
        if !attached {
            let notice = channel.views().last().unwrap().clone();
            assert_eq!(notice.actions.len(), 1);
            assert_eq!(notice.actions[0].label, "Attach");
            engine
                .handle_inbound(InboundEnvelope::action(
                    "attach-question",
                    ConversationRef::new(ChannelKind::Telegram, "chat-e2e"),
                    "owner-e2e",
                    notice.actions[0].token.clone(),
                ))
                .await
                .unwrap();
        }
        assert!(client.is_read_only(&SessionId::new("thr_question")).await);
        let question = channel.views().last().unwrap().clone();
        assert!(question.body.contains("Which approach?"));
        assert_eq!(question.actions[0].label, "Fast");
        engine
            .handle_inbound(InboundEnvelope::action(
                "answer-question",
                ConversationRef::new(ChannelKind::Telegram, "chat-e2e"),
                "owner-e2e",
                question.actions[0].token.clone(),
            ))
            .await
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), response)
                .await
                .unwrap()
                .unwrap(),
            json!({"answers":{"choice":{"answers":["Fast"]}}})
        );
    }
}

#[tokio::test]
async fn async_cli_questions_can_be_answered_from_attached_or_background_im() {
    use agentix_codex::{CodexEndpoint, CodexProxy};
    use std::path::Path;
    for attached in [false, true] {
        let server = MockCodexAppServer::start();
        server
            .add_thread(MockThread::new("thr_question", "CLI question", "/work"))
            .await;
        let directory = tempfile::tempdir().unwrap();
        let proxy = CodexProxy::bind(
            &format!("unix://{}", directory.path().join("proxy.sock").display()),
            &format!("unix://{}", server.endpoint().socket_path().display()),
        )
        .await
        .unwrap();
        let cli = CodexClient::connect(CodexEndpoint::parse(proxy.endpoint()).unwrap())
            .await
            .unwrap();
        cli.set_background_turn_notifications(false);
        cli.attach(&SessionId::new("thr_question")).await.unwrap();
        server.set_active_writer("thr_question").await;
        let client = Arc::new(
            CodexClient::connect_with_registry(
                server.endpoint(),
                Path::new("codex"),
                Path::new("/tmp"),
                true,
                proxy.registry(),
            )
            .await
            .unwrap(),
        );
        let mut events = client.subscribe();
        let channel = Arc::new(RecordingChannel::default());
        let engine = Engine::new(
            client.clone(),
            SqliteState::in_memory().await.unwrap(),
            vec![channel.clone()],
        );
        engine
            .handle_inbound(inbound(if attached {
                "/attach thr_question"
            } else {
                "/help"
            }))
            .await
            .unwrap();
        server.send_notification(json!({"method":"item/completed","params":{"threadId":"thr_question","turnId":"turn-question","item":{"id":"item-question","type":"agentMessage","text":"","questions":[{"title":"Which approach?","options":["Fast"]}]}}})).await;
        let event = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let event = events.recv().await.unwrap();
                if matches!(event, AgentEvent::InteractionRequested(_)) {
                    break event;
                }
            }
        })
        .await
        .unwrap();
        engine.handle_agent_event(event).await.unwrap();
        if !attached {
            let notice = channel.views().last().unwrap().clone();
            assert_eq!(notice.actions.len(), 1);
            assert_eq!(notice.actions[0].label, "Attach");
            engine
                .handle_inbound(InboundEnvelope::action(
                    "attach-question",
                    ConversationRef::new(ChannelKind::Telegram, "chat-e2e"),
                    "owner-e2e",
                    notice.actions[0].token.clone(),
                ))
                .await
                .unwrap();
        }
        assert!(client.is_read_only(&SessionId::new("thr_question")).await);
        let question = channel.views().last().unwrap().clone();
        assert!(question.body.contains("Which approach?"));
        assert_eq!(question.actions[0].label, "Fast");
        engine
            .handle_inbound(InboundEnvelope::action(
                "answer-question",
                ConversationRef::new(ChannelKind::Telegram, "chat-e2e"),
                "owner-e2e",
                question.actions[0].token.clone(),
            ))
            .await
            .unwrap();
        assert!(
            server
                .thread("thr_question")
                .await
                .unwrap()
                .turns
                .last()
                .unwrap()
                .user_text
                .contains("Fast")
        );
    }
}

#[tokio::test]
async fn restarted_engine_receives_discovered_background_completion_without_new_im_input() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(
            MockThread::new("thr_restart", "Existing session", "/work").with_turn(
                MockTurn::in_progress_with_output("turn_restart", "Finish existing work", ""),
            ),
        )
        .await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
    let channel = Arc::new(RecordingChannel::default());
    let engine = Engine::new(
        client.clone(),
        SqliteState::open(&path).await.unwrap(),
        vec![channel.clone()],
    );
    engine.restore_bindings_deferred().await.unwrap();
    engine.handle_inbound(inbound("/help")).await.unwrap();
    drop(engine);
    drop(client);
    let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
    let mut events = client.subscribe();
    let restarted = Engine::new(
        client,
        SqliteState::open(&path).await.unwrap(),
        vec![channel.clone()],
    );
    assert_eq!(restarted.restore_bindings().await.unwrap(), 0);
    let before = channel.views().len();
    server.wait_for_turn_reads("thr_restart", 1).await;
    server
        .complete_turn("thr_restart", "turn_restart", "Finished after restart")
        .await;
    restarted
        .handle_agent_event(recv_background_event(&mut events).await)
        .await
        .unwrap();
    let views = channel.views();
    assert_eq!(views.len(), before + 1);
    assert!(
        views
            .last()
            .unwrap()
            .body
            .contains("Finished after restart")
    );
    assert!(
        !server
            .request_methods()
            .await
            .contains(&"thread/resume".into())
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One scenario checks the same turn across all delivery paths.
async fn goal_input_is_restored_per_turn_for_history_and_read_only_attach() {
    let server = MockCodexAppServer::start();
    let file = goal_input_rollout();
    let path = file.path();
    let mut thread = MockThread::new("thr_goal_input", "Goal", "/work")
        .with_turn(MockTurn::completed("old_goal", "", "First result"))
        .with_turn(MockTurn::completed(
            "ordinary",
            "Normal question",
            "Normal answer",
        ))
        .with_turn(MockTurn::in_progress_with_output(
            "current_goal",
            "",
            "Reviewing",
        ));
    thread.rollout_path = Some(path.to_string_lossy().into_owned());
    server.add_thread(thread).await;
    server.set_active_writer("thr_goal_input").await;
    let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
    let history = client
        .read_history(&SessionId::new("thr_goal_input"), None, 10)
        .await
        .unwrap();
    assert_eq!(
        history.turns[0].user_text.as_deref(),
        Some("/goal First objective")
    );
    assert_eq!(
        history.turns[1].user_text.as_deref(),
        Some("Normal question")
    );
    assert_eq!(
        history.turns[2].user_text.as_deref(),
        Some("/goal Continue reviewing")
    );
    server
        .fail_next("thread/turns/list", -32601, "unsupported")
        .await;
    let fallback = client
        .read_history(&SessionId::new("thr_goal_input"), None, 10)
        .await
        .unwrap();
    assert_eq!(fallback.turns, history.turns);
    assert!(
        !server
            .request_methods()
            .await
            .contains(&"thread/resume".into())
    );
    let channel = Arc::new(RecordingChannel::default());
    let engine = Engine::new(
        client.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    engine
        .handle_inbound(inbound("/attach thr_goal_input"))
        .await
        .unwrap();
    let views = channel.views();
    assert!(views.iter().any(|view| {
        view.sections
            .iter()
            .any(|section| section.body.contains("/goal Continue reviewing"))
    }));
    assert!(
        views
            .iter()
            .all(|view| !format!("{view:?}").contains("Internal instructions"))
    );
    assert!(client.is_read_only(&SessionId::new("thr_goal_input")).await);
    engine.handle_inbound(inbound("/detach")).await.unwrap();
    server
        .complete_turn("thr_goal_input", "current_goal", "Review complete")
        .await;
    engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_goal_input".into(),
            turn_id: "current_goal".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await
        .unwrap();
    assert!(
        channel
            .views()
            .last()
            .unwrap()
            .sections
            .iter()
            .any(|section| section.body.contains("/goal Continue reviewing"))
    );
    std::fs::remove_file(path).unwrap();
    let unavailable = client
        .read_history(&SessionId::new("thr_goal_input"), None, 10)
        .await
        .unwrap();
    assert_eq!(
        unavailable.turns[2].agent_text.as_deref(),
        Some("Review complete")
    );
}

#[tokio::test]
async fn goal_input_is_recovered_after_first_live_output_without_user_message_event() {
    let server = MockCodexAppServer::start();
    let file = tempfile::NamedTempFile::new().unwrap();
    let entries = [
        json!({"type":"session_meta","payload":{"id":"thr_live_goal"}}),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"live_goal"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<codex_internal_context source=\"goal\">\nInternal\n<objective>\nFix all issues\n</objective>\nInternal\n</codex_internal_context>"}]}}),
    ];
    std::fs::write(
        file.path(),
        entries
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let mut thread = MockThread::new("thr_live_goal", "Live goal", "/work");
    thread.rollout_path = Some(file.path().to_string_lossy().into_owned());
    server.add_thread(thread).await;
    let client = Arc::new(CodexClient::connect(server.endpoint()).await.unwrap());
    let deferred = Arc::new(agentix_core::DeferredAgent::new(
        "Codex",
        "/work".into(),
        move || {
            let client = client.clone();
            async move { Ok(client as Arc<dyn AgentAdapter>) }
        },
    ));
    let mut ready = deferred.subscribe();
    tokio::time::timeout(Duration::from_secs(2), ready.recv())
        .await
        .unwrap()
        .unwrap();
    let client = Arc::new(
        agentix_core::AgentRegistry::new(vec![(agentix_core::AgentKind::Codex, deferred)]).unwrap(),
    );
    let channel = Arc::new(RecordingChannel::default());
    let engine = Engine::new(
        client,
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    engine
        .handle_inbound(inbound("/attach codex:thr_live_goal"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::TurnStarted {
            session_id: "codex:thr_live_goal".into(),
            turn_id: "live_goal".into(),
        })
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "codex:thr_live_goal".into(),
            turn_id: "live_goal".into(),
            item_id: "answer".into(),
            delta: "Working".into(),
        })
        .await
        .unwrap();
    assert!(
        channel
            .views()
            .last()
            .unwrap()
            .sections
            .iter()
            .any(|section| { section.body.contains("Working") })
    );
    // Drive the same completion work that the runtime dispatches. History I/O
    // must not delay the output above, but its result must update that turn.
    let recovered = tokio::time::timeout(Duration::from_secs(2), engine.next_input_recovery())
        .await
        .unwrap();
    engine.execute_work(recovered).await.unwrap();
    let views = channel.views();
    assert!(
        views
            .last()
            .unwrap()
            .sections
            .iter()
            .any(|section| section.body.contains("/goal Fix all issues"))
    );
}

fn goal_input_rollout() -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    let path = file.path();
    let mut entries = vec![json!({"type":"session_meta","payload":{"id":"thr_goal_input"}})];
    for (id, objective) in [
        ("old_goal", "First objective"),
        ("current_goal", "Continue reviewing"),
    ] {
        entries.push(json!({"type":"event_msg","payload":{"type":"task_started","turn_id":id}}));
        entries.push(json!({"type":"response_item","payload":{
            "type":"message","role":"user","content":[{"type":"input_text","text":format!(
                "<codex_internal_context source=\"goal\">\nInternal instructions\n<objective>\n{objective}\n</objective>\nMore internal instructions\n</codex_internal_context>"
            )}]
        }}));
        entries.push(json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":id}}));
    }
    entries
        .push(json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"ordinary"}}));
    std::fs::write(
        path,
        entries
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    file
}

#[tokio::test]
async fn registry_exit_is_distinct_from_delayed_native_new() {
    use agentix_codex::ClientRegistry;
    use std::path::Path;

    for replacement in [false, true] {
        let server = MockCodexAppServer::start();
        server
            .add_thread(MockThread::new("old", "Old", "/work"))
            .await;
        server
            .add_thread(MockThread::new("new", "New", "/work"))
            .await;
        let registry = ClientRegistry::default();
        let connection = registry.connect(None);
        registry.client_message(
            connection,
            &json!({"id":1,"method":"thread/resume","params":{}}),
        );
        registry.server_message(
            connection,
            &json!({"id":1,"result":{"thread":{"id":"old"}}}),
        );
        let client = Arc::new(
            CodexClient::connect_with_registry(
                server.endpoint(),
                Path::new("codex"),
                Path::new("/tmp"),
                true,
                registry.clone(),
            )
            .await
            .unwrap(),
        );
        client.set_background_turn_notifications(false);
        let mut events = client.subscribe();
        let state = SqliteState::in_memory().await.unwrap();
        let channel = Arc::new(RecordingChannel::default());
        let engine = Engine::new(client, state.clone(), vec![channel.clone()]);
        engine.handle_inbound(inbound("/attach old")).await.unwrap();
        registry.client_message(
            connection,
            &json!({"id":2,"method":"thread/unsubscribe","params":{"threadId":"old"}}),
        );
        registry.server_message(
            connection,
            &json!({"id":2,"result":{"status":"unsubscribed"}}),
        );
        // Give the monitor a separate wakeup before a replacement exists.
        assert!(
            tokio::time::timeout(Duration::from_millis(100), events.recv())
                .await
                .is_err()
        );
        if replacement {
            registry.client_message(
                connection,
                &json!({"id":3,"method":"thread/start","params":{}}),
            );
            registry.server_message(
                connection,
                &json!({"id":3,"result":{"thread":{"id":"new"}}}),
            );
        } else {
            registry.disconnect(connection);
        }
        let mut saw_replacement = false;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = events.recv().await.unwrap();
                if matches!(event, AgentEvent::SessionSwitchStarted { .. }) {
                    assert!(replacement);
                }
                if matches!(event, AgentEvent::SessionReplaced { .. }) {
                    saw_replacement = true;
                }
                let exited = matches!(&event, AgentEvent::SessionExited { session_id } if session_id == "old");
                engine.handle_agent_event(event).await.unwrap();
                if exited {
                    assert_eq!(saw_replacement, replacement);
                    break;
                }
            }
        }).await.unwrap();
        let bindings = state.list_bindings().await.unwrap();
        if replacement {
            assert_eq!(bindings[0].1, SessionId::new("new"));
        } else {
            assert!(state.list_session_switches().await.unwrap().is_empty());
            assert!(
                !channel
                    .views()
                    .iter()
                    .any(|view| view.body.contains("Waiting for the new session")
                        || view.body.contains("timed out"))
            );
        }
    }
}

#[tokio::test]
async fn registry_disconnect_reports_exit_while_process_is_still_alive() {
    use agentix_codex::ClientRegistry;
    use std::path::Path;

    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("old", "Old", "/work"))
        .await;
    let mut process = tokio::process::Command::new("sleep")
        .arg("30")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let registry = ClientRegistry::default();
    let connection = registry.connect(process.id());
    registry.client_message(
        connection,
        &json!({"id":1,"method":"thread/resume","params":{}}),
    );
    registry.server_message(
        connection,
        &json!({"id":1,"result":{"thread":{"id":"old"}}}),
    );
    let client = CodexClient::connect_with_registry(
        server.endpoint(),
        Path::new("codex"),
        Path::new("/tmp"),
        true,
        registry.clone(),
    )
    .await
    .unwrap();
    client.set_background_turn_notifications(false);
    let mut events = client.subscribe();
    client.attach(&SessionId::new("old")).await.unwrap();
    registry.client_message(
        connection,
        &json!({"id":2,"method":"thread/unsubscribe","params":{"threadId":"old"}}),
    );
    registry.server_message(
        connection,
        &json!({"id":2,"result":{"status":"unsubscribed"}}),
    );
    registry.disconnect(connection);
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("connection closure must not wait for process exit")
        .unwrap();
    assert!(process.try_wait().unwrap().is_none());
    process.kill().await.unwrap();
    process.wait().await.unwrap();
    assert!(matches!(event, AgentEvent::SessionExited { session_id } if session_id == "old"));
}

#[tokio::test]
async fn empty_attach_recovers_first_turn_after_subscription_becomes_available() {
    assert_empty_attach_recovers_first_turn(false, false).await;
}

#[tokio::test]
async fn empty_attach_recovers_first_turn_when_original_client_becomes_writer() {
    assert_empty_attach_recovers_first_turn(true, false).await;
}

#[tokio::test]
async fn empty_attach_recovers_running_turn_and_continues_live_updates() {
    assert_empty_attach_recovers_first_turn(false, true).await;
}

#[tokio::test]
async fn empty_attach_recovers_running_turn_and_continues_observed_updates() {
    assert_empty_attach_recovers_first_turn(true, true).await;
}

async fn assert_empty_attach_recovers_first_turn(active_writer: bool, running: bool) {
    use agentix_codex::ClientRegistry;
    use std::path::Path;
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("empty", "Empty", "/work"))
        .await;
    let registry = ClientRegistry::default();
    let connection = registry.connect(None);
    registry.client_message(
        connection,
        &json!({"id":1,"method":"thread/start","params":{}}),
    );
    registry.server_message(
        connection,
        &json!({"id":1,"result":{"thread":{"id":"empty"}}}),
    );
    let client = Arc::new(
        CodexClient::connect_with_registry(
            server.endpoint(),
            Path::new("codex"),
            Path::new("/tmp"),
            true,
            registry.clone(),
        )
        .await
        .unwrap(),
    );
    client.set_background_turn_notifications(false);
    let mut events = client.subscribe();
    let state = SqliteState::in_memory().await.unwrap();
    let channel = Arc::new(RecordingChannel {
        streaming_interval: Some(Duration::from_secs(1)),
        ..RecordingChannel::default()
    });
    let engine = Engine::new(client.clone(), state.clone(), vec![channel.clone()]);
    server
        .fail_next(
            "thread/resume",
            -32600,
            "no rollout found for thread id empty",
        )
        .await;
    engine
        .handle_inbound(inbound("/attach empty"))
        .await
        .unwrap();
    assert_eq!(state.list_bindings().await.unwrap().len(), 1);
    // Native events happen before Agentix can establish its subscription.
    let turn = if running {
        MockTurn::in_progress_with_output("first", "First native input", "First native answer")
    } else {
        MockTurn::completed("first", "First native input", "First native answer")
    };
    server
        .add_thread(MockThread::new("empty", "Empty", "/work").with_turn(turn))
        .await;
    if active_writer {
        server.set_active_writer("empty").await;
    }
    // The proxy sees turn notifications even before Agentix can subscribe.
    registry.server_frame(
        connection,
        &json!({"method":"turn/started","params":{"threadId":"empty","turn":{"id":"first"}}})
            .to_string(),
    );
    receive_recovered_turn(&engine, &mut events, running, 3).await;
    refresh_recovered_first_turn(&engine, &channel, running).await;
    let view = channel.views().last().unwrap().clone();
    assert!(view.body.contains("First native input"), "{view:?}");
    assert!(view.body.contains("First native answer"), "{view:?}");
    if running {
        assert!(
            !view
                .subtitle
                .as_deref()
                .unwrap_or_default()
                .contains("Completed")
        );
    }
    assert_eq!(
        client.is_read_only(&SessionId::new("empty")).await,
        active_writer
    );
    if running {
        server
            .complete_turn("empty", "first", "Final native answer")
            .await;
        registry.server_frame(connection, &json!({"method":"turn/completed","params":{"threadId":"empty","turn":{"id":"first","status":"completed"}}}).to_string());
        receive_recovered_turn(&engine, &mut events, false, 3).await;
        let view = channel.views().last().unwrap().clone();
        assert!(view.body.contains("First native input"), "{view:?}");
        assert!(view.body.contains("Final native answer"), "{view:?}");
        assert_eq!(view.body.matches("First native input").count(), 1);
    }
}

async fn refresh_recovered_first_turn(engine: &Engine, channel: &RecordingChannel, running: bool) {
    assert!(
        channel
            .views()
            .last()
            .unwrap()
            .body
            .contains("First native input"),
        "native input must be visible before waiting for a stream refresh"
    );
    if running {
        // Use the production Feishu pacing and drive the runtime's working refresh.
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        engine.refresh_working_turns().await;
    }
}

async fn receive_recovered_turn(
    engine: &Engine,
    events: &mut tokio::sync::broadcast::Receiver<AgentEvent>,
    running: bool,
    timeout_seconds: u64,
) {
    tokio::time::timeout(Duration::from_secs(timeout_seconds), async {
        loop {
            let event = events.recv().await.unwrap();
            let ready = if running {
                matches!(&event, AgentEvent::ItemCompleted { item, .. } if item.kind == "agentMessage")
            } else {
                matches!(&event, AgentEvent::TurnCompleted { turn_id, .. } if turn_id == "first")
            };
            engine.handle_agent_event(event).await.unwrap();
            if ready {
                break;
            }
        }
    })
    .await
    .expect("the recovered turn must deliver content and continue updating");
}

#[tokio::test]
async fn registry_exit_does_not_wait_for_background_history() {
    assert_lifecycle_does_not_wait_for_background_history(false).await;
}

#[tokio::test]
async fn registry_new_does_not_wait_for_background_history() {
    assert_lifecycle_does_not_wait_for_background_history(true).await;
}

async fn assert_lifecycle_does_not_wait_for_background_history(replacement: bool) {
    use agentix_codex::ClientRegistry;
    use std::path::Path;
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("old", "Old", "/work"))
        .await;
    let registry = ClientRegistry::default();
    let connection = registry.connect(None);
    registry.client_message(
        connection,
        &json!({"id":1,"method":"thread/resume","params":{}}),
    );
    registry.server_message(
        connection,
        &json!({"id":1,"result":{"thread":{"id":"old"}}}),
    );
    let client = CodexClient::connect_with_registry(
        server.endpoint(),
        Path::new("codex"),
        Path::new("/tmp"),
        true,
        registry.clone(),
    )
    .await
    .unwrap();
    client.attach(&SessionId::new("old")).await.unwrap();
    let mut events = client.subscribe();
    let (entered, release) = server.hold_next_request("thread/turns/list").await;
    // Hold a genuinely unattached background session; attached sessions now
    // share their foreground read and must not be queried by this monitor.
    server
        .add_thread(MockThread::new("background", "Background", "/work"))
        .await;
    let background_connection = registry.connect(None);
    registry.client_message(
        background_connection,
        &json!({"id":10,"method":"thread/resume"}),
    );
    registry.server_message(
        background_connection,
        &json!({"id":10,"result":{"thread":{"id":"background"}}}),
    );
    tokio::time::timeout(Duration::from_secs(3), entered)
        .await
        .unwrap()
        .unwrap();
    if replacement {
        registry.client_message(
            connection,
            &json!({"id":2,"method":"thread/unsubscribe","params":{"threadId":"old"}}),
        );
        registry.server_message(
            connection,
            &json!({"id":2,"result":{"status":"unsubscribed"}}),
        );
        registry.client_message(
            connection,
            &json!({"id":3,"method":"thread/start","params":{}}),
        );
        registry.server_message(
            connection,
            &json!({"id":3,"result":{"thread":{"id":"new"}}}),
        );
    } else {
        registry.disconnect(connection);
    }
    let delivered = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let event = events.recv().await.unwrap();
            if if replacement {
                matches!(event, AgentEvent::SessionReplaced { .. })
            } else {
                matches!(event, AgentEvent::SessionExited { .. })
            } {
                break;
            }
        }
    })
    .await;
    let _ = release.send(());
    assert!(
        delivered.is_ok(),
        "lifecycle event waited for unrelated background history"
    );
}

#[tokio::test]
async fn registered_attach_defers_a_stalled_metadata_read() {
    use agentix_codex::ClientRegistry;
    use std::path::Path;
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("new", "New", "/work"))
        .await;
    let registry = ClientRegistry::default();
    let connection = registry.connect(None);
    registry.client_message(
        connection,
        &json!({"id":1,"method":"thread/start","params":{}}),
    );
    registry.server_message(
        connection,
        &json!({"id":1,"result":{"thread":{"id":"new"}}}),
    );
    let client = CodexClient::connect_with_registry(
        server.endpoint(),
        Path::new("codex"),
        Path::new("/tmp"),
        false,
        registry,
    )
    .await
    .unwrap();
    let (_entered, release) = server.hold_next_request("thread/read").await;
    let attached = tokio::time::timeout(
        Duration::from_secs(2),
        client.attach(&SessionId::new("new")),
    )
    .await;
    let _ = release.send(());
    assert!(
        attached.is_ok(),
        "registered attach must defer slow metadata instead of blocking the IM"
    );
    attached.unwrap().unwrap();
}

#[tokio::test]
async fn observed_sessions_continue_while_another_history_read_is_stalled() {
    use agentix_codex::ClientRegistry;
    use std::path::Path;
    let server = MockCodexAppServer::start();
    let registry = ClientRegistry::default();
    let connection = registry.connect(None);
    for (id, session) in [(1, "one"), (2, "two")] {
        server
            .add_thread(MockThread::new(session, session, "/work"))
            .await;
        server.set_active_writer(session).await;
        registry.client_message(
            connection,
            &json!({"id":id,"method":"thread/start","params":{}}),
        );
        registry.server_message(
            connection,
            &json!({"id":id,"result":{"thread":{"id":session}}}),
        );
    }
    let client = CodexClient::connect_with_registry(
        server.endpoint(),
        Path::new("codex"),
        Path::new("/tmp"),
        false,
        registry.clone(),
    )
    .await
    .unwrap();
    client.attach(&SessionId::new("one")).await.unwrap();
    client.attach(&SessionId::new("two")).await.unwrap();
    let mut events = client.subscribe();
    let (entered, release) = server.hold_next_request("thread/turns/list").await;
    tokio::time::timeout(Duration::from_secs(2), entered)
        .await
        .unwrap()
        .unwrap();
    for session in ["one", "two"] {
        server
            .add_thread(MockThread::new(session, session, "/work").with_turn(
                MockTurn::in_progress_with_output("first", "input", "working"),
            ))
            .await;
    }
    for session in ["one", "two"] {
        registry.server_frame(connection, &json!({"method":"item/agentMessage/delta","params":{"threadId":session,"turnId":"first","itemId":"answer","delta":"working"}}).to_string());
    }
    let delivered = tokio::time::timeout(Duration::from_secs(1), events.recv()).await;
    let _ = release.send(());
    assert!(
        delivered.is_ok(),
        "one stalled session must not block another session"
    );
}
#[tokio::test]
async fn detached_pending_session_ignores_a_late_writer_response() {
    use agentix_codex::ClientRegistry;
    use std::path::Path;
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("new", "New", "/work"))
        .await;
    let registry = ClientRegistry::default();
    let connection = registry.connect(None);
    registry.client_message(
        connection,
        &json!({"id":1,"method":"thread/start","params":{}}),
    );
    registry.server_message(
        connection,
        &json!({"id":1,"result":{"thread":{"id":"new"}}}),
    );
    let client = CodexClient::connect_with_registry(
        server.endpoint(),
        Path::new("codex"),
        Path::new("/tmp"),
        false,
        registry,
    )
    .await
    .unwrap();
    server.set_active_writer("new").await;
    let (resume_entered, resume_release) = server.hold_next_request("thread/resume").await;
    let (_entered, release) = server.hold_next_request("thread/read").await;
    let attached = tokio::time::timeout(
        Duration::from_secs(2),
        client.attach(&SessionId::new("new")),
    )
    .await;
    let _ = release.send(());
    assert!(
        attached.is_ok(),
        "registered attach must defer slow metadata instead of blocking the IM"
    );
    attached.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(2), resume_entered)
        .await
        .unwrap()
        .unwrap();
    client.unsubscribe(&SessionId::new("new")).await.unwrap();
    let _ = resume_release.send(());
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !client.is_read_only(&SessionId::new("new")).await,
        "late recovery must not restore a detached observation"
    );
}
#[tokio::test]
async fn registered_observation_is_idle_until_a_notification() {
    use agentix_codex::ClientRegistry;
    use std::path::Path;
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("new", "New", "/work"))
        .await;
    server.set_active_writer("new").await;
    let registry = ClientRegistry::default();
    let connection = registry.connect(None);
    registry.client_message(
        connection,
        &json!({"id":1,"method":"thread/start","params":{}}),
    );
    registry.server_message(
        connection,
        &json!({"id":1,"result":{"thread":{"id":"new"}}}),
    );
    let client = CodexClient::connect_with_registry(
        server.endpoint(),
        Path::new("codex"),
        Path::new("/tmp"),
        true,
        registry.clone(),
    )
    .await
    .unwrap();
    client.attach(&SessionId::new("new")).await.unwrap();
    let mut events = client.subscribe();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let before = server.request_methods().await.len();
    tokio::time::sleep(Duration::from_secs(11)).await;
    assert_eq!(
        server.request_methods().await.len(),
        before,
        "idle registered sessions must not poll upstream"
    );
    server
        .add_thread(MockThread::new("new", "New", "/work").with_turn(
            MockTurn::in_progress_with_output("first", "input", "answer"),
        ))
        .await;
    registry.server_frame(connection, &json!({"method":"item/agentMessage/delta","params":{"threadId":"new","turnId":"first","itemId":"answer","delta":"answer"}}).to_string());
    tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("notification must wake observation")
        .unwrap();
}
