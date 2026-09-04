use std::fs;

use agentix_core::{AgentAdapter, AgentEvent, InteractionKind, SessionStatus};
use agentix_pi::{PiFlavor, PiRpcAdapter, discover_sessions, map_event, process_args};

#[test]
fn discovers_pi_jsonl_sessions_from_headers() {
    let directory = tempfile::tempdir().unwrap();
    let nested = directory.path().join("--work-repo--");
    fs::create_dir(&nested).unwrap();
    fs::write(
        nested.join("2026-01-02_abc.jsonl"),
        concat!(
            "{\"type\":\"session\",\"version\":3,\"id\":\"pi-abc\",",
            "\"timestamp\":\"2026-01-02T03:04:05Z\",\"cwd\":\"/work/repo\"}\n",
            "{\"type\":\"session_info\",\"name\":\"Fix tests\"}\n"
        ),
    )
    .unwrap();

    let sessions = discover_sessions(directory.path()).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].summary.id.as_str(), "pi-abc");
    assert_eq!(sessions[0].summary.cwd.as_deref(), Some("/work/repo"));
    assert_eq!(sessions[0].summary.status, SessionStatus::Idle);
    assert_eq!(sessions[0].summary.name.as_deref(), Some("Fix tests"));
}

#[test]
fn discovers_omp_sessions_with_a_title_preamble() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("omp.jsonl"),
        concat!(
            "{\"type\":\"title\",\"v\":1,\"title\":\"Review parser\"}\n",
            "{\"type\":\"session\",\"version\":3,\"id\":\"omp-abc\",",
            "\"timestamp\":\"2026-01-02T03:04:05Z\",\"cwd\":\"/work/omp\"}\n"
        ),
    )
    .unwrap();

    let sessions = discover_sessions(directory.path()).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].summary.id.as_str(), "omp-abc");
    assert_eq!(sessions[0].summary.name.as_deref(), Some("Review parser"));
}

#[test]
fn pi_and_omp_use_the_same_rpc_mode_with_different_binaries() {
    assert_eq!(
        process_args(PiFlavor::Pi, "/tmp/s.jsonl")[0..3],
        ["--mode", "rpc", "--session"]
    );
    assert_eq!(
        process_args(PiFlavor::OhMyPi, "/tmp/s.jsonl"),
        ["--mode", "rpc", "--resume", "/tmp/s.jsonl"]
    );
    assert_eq!(PiFlavor::Pi.default_command(), "pi");
    assert_eq!(PiFlavor::OhMyPi.default_command(), "omp");
    let directory = tempfile::tempdir().unwrap();
    let pi = PiRpcAdapter::new(PiFlavor::Pi, "pi", directory.path());
    let omp = PiRpcAdapter::new(PiFlavor::OhMyPi, "omp", directory.path());
    assert_eq!(pi.display_name(), "Pi");
    assert_eq!(omp.display_name(), "Oh My Pi");
}

#[test]
fn rpc_events_keep_the_exact_session_and_turn_context() {
    let delta = map_event(
        "session-a",
        "turn-7",
        &serde_json::json!({
            "type": "message_update",
            "assistantMessageEvent": {"type": "text_delta", "delta": "hello"}
        }),
    )
    .unwrap();
    assert!(matches!(
        delta,
        AgentEvent::AgentMessageDelta { session_id, turn_id, delta, .. }
            if session_id == "session-a" && turn_id == "turn-7" && delta == "hello"
    ));

    let interaction = map_event(
        "session-a",
        "turn-7",
        &serde_json::json!({
            "type": "extension_ui_request",
            "id": "ui-1",
            "method": "confirm",
            "title": "Continue?",
            "message": "Run the command"
        }),
    )
    .unwrap();
    assert!(matches!(
        interaction,
        AgentEvent::InteractionRequested(request)
            if request.session_id == "session-a"
                && request.turn_id == "turn-7"
                && request.kind == InteractionKind::CommandApproval
    ));

    let completed = map_event(
        "omp-session",
        "turn-9",
        &serde_json::json!({"type": "agent_end", "isTerminal": true, "messages": []}),
    )
    .unwrap();
    assert!(matches!(
        completed,
        AgentEvent::TurnCompleted { session_id, turn_id, .. }
            if session_id == "omp-session" && turn_id == "turn-9"
    ));
}
