use agentix_codex::{CodexEndpoint, ServerMessage, decode_server_frame};
use agentix_core::{AgentEvent, InteractionKind, SessionStatus};
use serde_json::json;

#[test]
fn default_unix_endpoint_uses_codex_home() {
    let endpoint = CodexEndpoint::parse_with_codex_home(
        "unix://",
        Some(std::path::Path::new("/tmp/codex-home")),
    )
    .unwrap();

    assert_eq!(
        endpoint.socket_path(),
        std::path::Path::new("/tmp/codex-home/app-server-control/app-server-control.sock")
    );
}

#[test]
fn delta_events_keep_all_routing_identifiers() {
    let message = decode_server_frame(&json!({
        "method": "item/agentMessage/delta",
        "params": {
            "threadId": "thr_a",
            "turnId": "turn_b",
            "itemId": "item_c",
            "delta": "hello"
        }
    }))
    .unwrap();

    assert_eq!(
        message,
        ServerMessage::Event(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "turn_b".into(),
            item_id: "item_c".into(),
            delta: "hello".into(),
        })
    );
}

#[test]
fn closed_threads_do_not_imply_that_the_codex_process_exited() {
    assert_eq!(
        decode_server_frame(&json!({
            "method": "thread/closed",
            "params": {"threadId": "thr_a"}
        }))
        .unwrap(),
        ServerMessage::Ignored
    );

    assert_eq!(
        decode_server_frame(&json!({
            "method": "thread/status/changed",
            "params": {
                "threadId": "thr_a",
                "status": {"type": "notLoaded"}
            }
        }))
        .unwrap(),
        ServerMessage::Event(AgentEvent::SessionStatusChanged {
            session_id: "thr_a".into(),
            status: SessionStatus::NotLoaded,
        })
    );
}

#[test]
fn queue_changes_keep_the_thread_identity() {
    assert_eq!(
        decode_server_frame(&json!({
            "method": "thread/queue/changed",
            "params": {"threadId": "thr_a"}
        }))
        .unwrap(),
        ServerMessage::Event(AgentEvent::QueueChanged {
            session_id: "thr_a".into(),
        })
    );
}

#[test]
fn approval_requests_keep_rpc_and_session_context() {
    let message = decode_server_frame(&json!({
        "id": 91,
        "method": "item/commandExecution/requestApproval",
        "params": {
            "threadId": "thr_a",
            "turnId": "turn_b",
            "itemId": "item_c",
            "command": ["cargo", "test"],
            "cwd": "/work",
            "availableDecisions": ["accept", "decline"]
        }
    }))
    .unwrap();

    let ServerMessage::Interaction(request) = message else {
        panic!("expected an interaction request");
    };
    assert_eq!(request.rpc_id, json!(91));
    assert_eq!(request.session_id, "thr_a");
    assert_eq!(request.turn_id, "turn_b");
    assert_eq!(request.item_id.as_deref(), Some("item_c"));
    assert_eq!(request.kind, InteractionKind::CommandApproval);
    assert_eq!(request.available_decisions, vec!["accept", "decline"]);
}

#[test]
fn completed_reasoning_and_tool_items_preserve_visible_details() {
    for (item, expected) in [
        (
            json!({"id":"r","type":"reasoning","summary":["Visible summary"],"content":[]}),
            "Visible summary",
        ),
        (
            json!({"id":"c","type":"commandExecution","command":"cargo test","aggregatedOutput":"passed","status":"completed"}),
            "cargo test",
        ),
        (
            json!({"id":"m","type":"mcpToolCall","server":"test","tool":"read","arguments":{"path":"a"},"result":{"content":[]}}),
            "read",
        ),
    ] {
        let ServerMessage::Event(AgentEvent::ItemCompleted { item, .. }) = decode_server_frame(
            &json!({"method":"item/completed","params":{"threadId":"s","turnId":"t","item":item}}),
        )
        .unwrap() else {
            panic!("completed item")
        };
        assert!(item.text.unwrap_or_default().contains(expected));
    }
}

#[test]
fn websocket_endpoints_and_upstream_default_are_supported() {
    let e = CodexEndpoint::parse("ws://127.0.0.1:4500").unwrap();
    assert_eq!(e.address(), "ws://127.0.0.1:4500/");
    assert_eq!(
        CodexEndpoint::default_upstream()
            .unwrap()
            .socket_path()
            .file_name()
            .unwrap(),
        "app-server-control-upstream.sock"
    );
}

#[test]
fn stdio_endpoint_is_distinct_from_unix_default() {
    assert_eq!(
        CodexEndpoint::parse("stdio://").unwrap().address(),
        "stdio://"
    );
    assert!(CodexEndpoint::parse("stdio://extra").is_err());
}

#[test]
fn commentary_phase_is_distinct_from_the_final_answer() {
    for method in ["item/started", "item/completed"] {
        for (phase, expected) in [
            ("commentary", "commentary"),
            ("final_answer", "agentMessage"),
        ] {
            let message = decode_server_frame(&json!({
                "method": method,
                "params": {"threadId": "s", "turnId": "t", "item": {
                    "id": "a", "type": "agentMessage", "phase": phase,
                    "text": "I will check the card implementation."
                }}
            }))
            .unwrap();
            match message {
                ServerMessage::Event(AgentEvent::ItemStarted { kind, .. }) => {
                    assert_eq!(kind, expected);
                }
                ServerMessage::Event(AgentEvent::ItemCompleted { item, .. }) => {
                    assert_eq!(item.kind, expected);
                    assert_eq!(
                        item.text.as_deref(),
                        Some("I will check the card implementation.")
                    );
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
    }
}

#[test]
fn absent_or_unknown_message_phase_preserves_normal_output() {
    for phase in [serde_json::Value::Null, json!("future_phase")] {
        for method in ["item/started", "item/completed"] {
            let frame = json!({"method":method, "params":{
                "threadId":"s", "turnId":"t", "item":{
                    "id":"a", "type":"agentMessage", "phase":phase, "text":"Answer"
                }
            }});
            match decode_server_frame(&frame).unwrap() {
                ServerMessage::Event(AgentEvent::ItemStarted { kind, .. }) => {
                    assert_eq!(kind, "agentMessage");
                }
                ServerMessage::Event(AgentEvent::ItemCompleted { item, .. }) => {
                    assert_eq!(item.kind, "agentMessage");
                    assert_eq!(item.text.as_deref(), Some("Answer"));
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
    }
}
