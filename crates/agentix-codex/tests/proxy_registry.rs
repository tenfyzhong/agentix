//! Proxy connection and session registry tests.
#![cfg(unix)]
use agentix_codex::ClientRegistry;
use serde_json::json;

#[test]
fn responses_bind_exact_connection_and_disconnect_keeps_other_client() {
    let registry = ClientRegistry::default();
    let a = registry.connect(Some(101));
    let b = registry.connect(Some(102));
    for (connection, session) in [(a, "a"), (b, "b")] {
        registry.client_message(
            connection,
            &json!({"id":1,"method":"thread/start","params":{"cwd":"/same"}}),
        );
        assert!(
            registry
                .snapshot()
                .iter()
                .find(|c| c.connection_id == connection)
                .unwrap()
                .sessions
                .is_empty()
        );
        registry.server_message(
            connection,
            &json!({"id":1,"result":{"thread":{"id":session,"cwd":"/same"}}}),
        );
    }
    registry.disconnect(b);
    let clients = registry.snapshot();
    assert_eq!(clients.len(), 1);
    assert_eq!(clients[0].pid, Some(101));
    assert_eq!(clients[0].sessions, vec!["a"]);
}

#[test]
fn failed_resume_notifications_and_server_requests_do_not_create_bindings() {
    let r = ClientRegistry::default();
    let c = r.connect(None);
    r.client_message(
        c,
        &json!({"id":1,"method":"thread/resume","params":{"threadId":"bad"}}),
    );
    r.server_message(
        c,
        &json!({"method":"thread/started","params":{"thread":{"id":"unrelated"}}}),
    );
    r.server_message(
        c,
        &json!({"id":1,"method":"item/tool/requestUserInput","params":{}}),
    );
    r.server_message(c, &json!({"id":1,"error":{"code":-1,"message":"failed"}}));
    assert!(r.snapshot()[0].sessions.is_empty());
}

#[test]
fn unsubscribe_and_shared_thread_preserve_other_subscriber() {
    let r = ClientRegistry::default();
    let a = r.connect(Some(1));
    let b = r.connect(Some(2));
    for c in [a, b] {
        r.client_message(
            c,
            &json!({"id":"r","method":"thread/resume","params":{"threadId":"shared"}}),
        );
        r.server_message(c, &json!({"id":"r","result":{"thread":{"id":"shared"}}}));
    }
    r.client_message(
        a,
        &json!({"id":2,"method":"thread/unsubscribe","params":{"threadId":"shared"}}),
    );
    r.server_message(a, &json!({"id":2,"error":{"code":-1}}));
    assert_eq!(r.snapshot()[0].sessions, vec!["shared"]);
    r.client_message(
        a,
        &json!({"id":3,"method":"thread/unsubscribe","params":{"threadId":"shared"}}),
    );
    r.server_message(a, &json!({"id":3,"result":{"status":"unsubscribed"}}));
    assert!(r.snapshot()[0].sessions.is_empty());
    assert_eq!(r.snapshot()[1].sessions, vec!["shared"]);
    r.disconnect(a);
    assert_eq!(r.snapshot()[0].sessions, vec!["shared"]);
}

#[test]
fn terminal_lookup_uses_registered_pid_and_drops_departed_clients() {
    use agentix_domain::TerminalLocation;
    let registry = ClientRegistry::default();
    let a = registry.connect(Some(101));
    registry.client_message(a, &json!({"id":1,"method":"thread/start"}));
    registry.server_message(a, &json!({"id":1,"result":{"thread":{"id":"a"}}}));
    let terminal = TerminalLocation {
        session: "s".into(),
        window_index: "1".into(),
        window_name: "w".into(),
        pane_index: "0".into(),
        pane_id: "p".into(),
    };
    let panes = std::collections::HashMap::from([(101, terminal.clone())]);
    assert_eq!(registry.session_terminals(&panes).get("a"), Some(&terminal));
    registry.disconnect(a);
    assert!(registry.session_terminals(&panes).is_empty());
}

#[test]
fn frame_observation_preserves_bindings_and_ignores_streamed_broadcasts() {
    let registry = ClientRegistry::default();
    let id = registry.connect(Some(123));
    registry.client_frame(id, r#"{"id":"r","method":"thread/start"}"#);
    registry.server_frame(
        id,
        r#"{"id":"r","method":"item/delta","params":{"delta":"large output"}}"#,
    );
    assert!(registry.snapshot()[0].sessions.is_empty());
    registry.server_frame(id, r#"{"id":"r","result":{"thread":{"id":"session"}}}"#);
    assert_eq!(registry.snapshot()[0].sessions, vec!["session"]);
    registry.client_frame(
        id,
        r#"{"id":2,"method":"thread\/unsubscribe","params":{"threadId":"session"}}"#,
    );
    registry.server_frame(id, r#"{"id":2,"result":{"status":"unsubscribed"}}"#);
    assert!(registry.snapshot()[0].sessions.is_empty());
    registry.client_frame(id, "malformed");
    registry.server_frame(id, "malformed");
}

#[test]
#[ignore = "manual performance comparison; no timing threshold"]
fn benchmark_stream_notification_observation() {
    let registry = ClientRegistry::default();
    let id = registry.connect(None);
    let frame = json!({"method":"item/agentMessage/delta","params":{"threadId":"s","delta":"x".repeat(16384)}}).to_string();
    let start = std::time::Instant::now();
    for _ in 0..20_000 {
        registry.server_message(id, &serde_json::from_str(&frame).unwrap());
    }
    let baseline = start.elapsed();
    let start = std::time::Instant::now();
    for _ in 0..20_000 {
        registry.server_frame(id, &frame);
    }
    let observed = start.elapsed();
    eprintln!("20k 16KiB notifications: full Value {baseline:?}; header-only {observed:?}");
    assert!(
        observed * 3 < baseline,
        "untracked notifications still scan the complete payload: {observed:?} versus {baseline:?}"
    );
    assert!(registry.snapshot()[0].sessions.is_empty());
}

#[test]
fn unknown_pid_connections_share_and_release_session_ownership_independently() {
    let registry = ClientRegistry::default();
    let a = registry.connect(None);
    let b = registry.connect(None);
    for connection in [a, b] {
        registry.client_message(
            connection,
            &json!({"id":1,"method":"thread/resume","params":{"threadId":"shared"}}),
        );
        registry.server_message(
            connection,
            &json!({"id":1,"result":{"thread":{"id":"shared"}}}),
        );
    }
    registry.disconnect(b);
    assert_eq!(registry.snapshot()[0].sessions, ["shared"]);
    assert_eq!(registry.snapshot()[0].pid, None);
    registry.client_message(
        a,
        &json!({"id":2,"method":"thread/unsubscribe","params":{"threadId":"shared"}}),
    );
    registry.server_message(a, &json!({"id":2,"result":{"status":"unsubscribed"}}));
    assert!(registry.snapshot()[0].sessions.is_empty());
    registry.disconnect(a);
    assert!(registry.snapshot().is_empty());
}

#[test]
fn notification_fast_path_preserves_pending_response_across_field_orders() {
    let registry = ClientRegistry::default();
    let connection = registry.connect(None);
    registry.client_frame(
        connection,
        r#"{"id":7,"method":"thread/start","params":{}}"#,
    );
    for frame in [
        r#" { "m\u0065thod" : "thread/started", "id":7, "result":{"thread":{"id":"foreign"}}}"#,
        r#"{"id":7,"result":{"thread":{"id":"foreign"}},"method":"thread/started"}"#,
        r#"{"method":"thread/started","params":invalid}"#,
        r#"{"id":7,"result":{"thread":{"id":"invalid"}}} trailing"#,
    ] {
        registry.server_frame(connection, frame);
        assert!(registry.snapshot()[0].sessions.is_empty());
    }
    registry.server_frame(connection, r#"{"result":{"thread":{"id":"owned"}},"id":7}"#);
    assert_eq!(registry.snapshot()[0].sessions, vec!["owned"]);
}
