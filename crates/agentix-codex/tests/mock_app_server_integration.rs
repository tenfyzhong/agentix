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
        .find(|action| action.label.contains("Button attach"))
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
    assert_eq!(notification.title, "Codex · Background work · thr_back");
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
    server.wait_for_turn_reads("thr_writer", 1).await;
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
    server.wait_for_turn_reads("thr_writer", 3).await;
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
    server.wait_for_turn_reads("thr_streamed", 1).await;
    server
        .complete_turn("thr_streamed", "turn_streamed", "done")
        .await;
    for _ in 0..3 {
        recv_event(&mut events).await;
    }
    client.unsubscribe(&session).await.unwrap();
    server.wait_for_turn_reads("thr_streamed", 3).await;
    assert!(
        events.try_recv().is_err(),
        "streamed completion must not become a background notice"
    );
}

async fn recv_background_event(
    receiver: &mut tokio::sync::broadcast::Receiver<AgentEvent>,
) -> AgentEvent {
    tokio::time::timeout(Duration::from_secs(8), receiver.recv())
        .await
        .unwrap()
        .unwrap()
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
    assert_eq!(
        methods,
        [
            "initialize",
            "thread/read",
            "thread/resume",
            "thread/loaded/list",
            "thread/read",
            "thread/turns/list",
            "turn/start",
        ]
    );
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
    views: Arc<Mutex<Vec<OutboundView>>>,
}

impl RecordingChannel {
    fn views(&self) -> Vec<OutboundView> {
        self.views.lock().unwrap().clone()
    }
}

#[async_trait]
impl ChannelAdapter for RecordingChannel {
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

    async fn set_command_menu(
        &self,
        _conversation: &ConversationRef,
        _menu: &CommandMenu,
    ) -> Result<(), ChannelError> {
        Ok(())
    }
}
