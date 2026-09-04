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
