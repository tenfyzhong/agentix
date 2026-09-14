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
        multiplexer: agentix_domain::MultiplexerKind::default(),
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

#[test]
fn native_new_handoff_survives_coalesced_changes_and_does_not_follow_forks() {
    let registry = ClientRegistry::default();
    let client = registry.connect(None);
    for (id, method, session) in [(1, "thread/resume", "old"), (2, "thread/fork", "fork")] {
        registry.client_message(
            client,
            &json!({"id":id,"method":method,"params":{"threadId":"old"}}),
        );
        registry.server_message(client, &json!({"id":id,"result":{"thread":{"id":session}}}));
    }
    assert!(registry.lifecycle_since(0).is_empty());
    registry.client_message(
        client,
        &json!({"id":3,"method":"thread/unsubscribe","params":{"threadId":"old"}}),
    );
    registry.server_message(client, &json!({"id":3,"result":{"status":"unsubscribed"}}));
    registry.client_message(client, &json!({"id":4,"method":"thread/start","params":{}}));
    registry.server_message(client, &json!({"id":4,"result":{"thread":{"id":"new"}}}));
    let events = registry.lifecycle_since(0);
    assert_eq!(events.len(), 2);
    assert!(
        matches!(&events[0].1, agentix_domain::AgentEvent::SessionSwitchStarted { session_id, .. } if session_id == "old")
    );
    assert!(
        matches!(&events[1].1, agentix_domain::AgentEvent::SessionReplaced { session_id, replacement_session_id, .. } if session_id == "old" && replacement_session_id == "new")
    );
    assert!(registry.lifecycle_since(events[1].0).is_empty());
}

#[test]
fn native_new_follows_start_before_unsubscribe_but_not_closed_connections() {
    for reconnect in [true, false] {
        let r = ClientRegistry::default();
        let mut c = r.connect(Some(std::process::id()));
        r.client_message(
            c,
            &json!({"id":1,"method":"thread/resume","params":{"threadId":"old"}}),
        );
        r.server_message(c, &json!({"id":1,"result":{"thread":{"id":"old"}}}));
        let identity = r.snapshot()[0].client_id.clone();
        if !reconnect {
            r.client_message(c, &json!({"id":2,"method":"thread/start","params":{}}));
            r.server_message(c, &json!({"id":2,"result":{"thread":{"id":"new"}}}));
        }
        r.client_message(
            c,
            &json!({"id":3,"method":"thread/unsubscribe","params":{"threadId":"old"}}),
        );
        r.server_message(c, &json!({"id":3,"result":{"status":"unsubscribed"}}));
        if reconnect {
            r.disconnect(c);
            assert!(!r.awaiting_replacement("old"));
            assert!(r.lifecycle_since(0).is_empty());
            c = r.connect(Some(std::process::id()));
            r.client_message(c, &json!({"id":2,"method":"thread/start","params":{}}));
            r.server_message(c, &json!({"id":2,"result":{"thread":{"id":"new"}}}));
        }
        assert!(!r.awaiting_replacement("old"));
        let events = r.lifecycle_since(0);
        if reconnect {
            assert!(events.is_empty());
        } else {
            assert!(events.iter().any(|(_, event)| matches!(event, agentix_domain::AgentEvent::SessionReplaced { session_id, replacement_session_id, client_id } if session_id == "old" && replacement_session_id == "new" && client_id == &identity)));
        }
    }
}

#[test]
fn native_new_ignores_ephemeral_starts_in_either_handoff_order() {
    for unsubscribe_first in [true, false] {
        for (request_flag, response_flag) in [(true, false), (false, true), (true, true)] {
            let r = ClientRegistry::default();
            let c = r.connect(None);
            let start = |id, thread, request_flag, response_flag| {
                r.client_message(
                    c,
                    &json!({"id":id,"method":"thread/start","params":{"ephemeral":request_flag}}),
                );
                r.server_message(
                    c,
                    &json!({"id":id,"result":{"thread":{"id":thread,"ephemeral":response_flag}}}),
                );
            };
            let unsubscribe = || {
                r.client_message(
                    c,
                    &json!({"id":4,"method":"thread/unsubscribe","params":{"threadId":"old"}}),
                );
                r.server_message(c, &json!({"id":4,"result":{"status":"unsubscribed"}}));
            };
            start(1, "old", false, false);
            if unsubscribe_first {
                unsubscribe();
            } else {
                start(2, "new", false, false);
            }
            start(3, "helper", request_flag, response_flag);
            if unsubscribe_first {
                start(2, "new", false, false);
            } else {
                unsubscribe();
            }
            let events = r.lifecycle_since(0);
            assert_eq!(events.len(), 2);
            assert!(events.iter().any(|(_, event)| matches!(event,
                agentix_domain::AgentEvent::SessionReplaced { session_id, replacement_session_id, .. }
                if session_id == "old" && replacement_session_id == "new")));
            assert!(!r.snapshot()[0].sessions.contains(&"helper".to_owned()));
        }
    }
}

#[test]
fn cli_questions_are_observed_once_and_cli_answers_resolve_them() {
    use agentix_domain::AgentEvent;
    let registry = ClientRegistry::default();
    let cli = registry.connect(None);
    let request = json!({"id":91,"method":"item/tool/requestUserInput","params":{"threadId":"thread-a","turnId":"turn-a","itemId":"item-a","questions":[{"id":"q","header":"Choice","question":"Choose?","options":[]}]}}).to_string();
    registry.server_frame(cli, &request);
    registry.server_frame(cli, &request);
    let events = registry.lifecycle_since(0);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0].1, AgentEvent::InteractionRequested(r) if r.rpc_id == json!(91)));
    registry.client_frame(
        cli,
        &json!({"id":91,"result":{"answers":{"q":{"answers":["Yes"]}}}}).to_string(),
    );
    let events = registry.lifecycle_since(0);
    assert_eq!(events.len(), 2);
    assert!(
        matches!(&events[1].1, AgentEvent::InteractionResolved{session_id,request_id} if session_id == "thread-a" && request_id == "91")
    );
}

#[test]
fn resolved_notifications_preserve_numeric_string_question_ids() {
    let registry = ClientRegistry::default();
    let cli = registry.connect(None);
    registry.server_frame(cli, &json!({"id":"91","method":"item/tool/requestUserInput","params":{"threadId":"t","turnId":"turn","itemId":"item","questions":[]}}).to_string());
    registry.server_frame(
        cli,
        &json!({"method":"serverRequest/resolved","params":{"threadId":"t","requestId":"91"}})
            .to_string(),
    );
    assert_eq!(registry.lifecycle_since(0).len(), 2);
}

#[test]
fn async_cli_questions_are_observed_once_without_consuming_message_output() {
    let registry = ClientRegistry::default();
    let cli = registry.connect(None);
    let frame = json!({"method":"item/completed","params":{"threadId":"t","turnId":"turn","item":{"type":"agentMessage","id":"message","text":"Context","questions":[{"title":"Which approach?","options":["Fast","Careful"]}]}}});
    registry.server_frame(cli, &frame.to_string());
    registry.server_frame(cli, &frame.to_string());
    let events = registry.lifecycle_since(0);
    assert_eq!(events.len(), 1);
    let agentix_domain::AgentEvent::InteractionRequested(request) = &events[0].1 else {
        panic!("missing question")
    };
    assert_eq!(
        request.payload["questions"][0]["question"],
        "Which approach?"
    );
    assert_eq!(
        request.payload["questions"][0]["options"][0]["label"],
        "Fast"
    );
    assert!(matches!(
        agentix_codex::decode_server_frame(&frame).unwrap(),
        agentix_codex::ServerMessage::Event(agentix_domain::AgentEvent::ItemCompleted { .. })
    ));
}

#[test]
fn ordinary_unsubscribe_and_exit_do_not_start_a_session_switch() {
    for method in ["thread/start", "thread/resume"] {
        for status in ["unsubscribed", "notSubscribed", "notLoaded"] {
            let r = ClientRegistry::default();
            let c = r.connect(None);
            r.client_message(c, &json!({"id":1,"method":method,"params":{}}));
            r.server_message(c, &json!({"id":1,"result":{"thread":{"id":"old"}}}));
            r.client_message(
                c,
                &json!({"id":2,"method":"thread/unsubscribe","params":{"threadId":"old"}}),
            );
            r.server_message(c, &json!({"id":2,"result":{"status":status}}));
            assert!(r.lifecycle_since(0).is_empty(), "{method}: {status}");
            assert!(r.snapshot()[0].sessions.is_empty());
            assert!(r.awaiting_replacement("old"));
            r.disconnect(c);
            assert!(!r.awaiting_replacement("old"));
            assert!(r.lifecycle_since(0).is_empty());
        }
    }
}

#[cfg(unix)]
#[test]
fn disconnect_drops_replacement_candidate_even_while_process_is_alive() {
    let mut process = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let r = ClientRegistry::default();
    let c = r.connect(Some(process.id()));
    r.client_message(c, &json!({"id":1,"method":"thread/resume","params":{}}));
    r.server_message(c, &json!({"id":1,"result":{"thread":{"id":"old"}}}));
    r.client_message(
        c,
        &json!({"id":2,"method":"thread/unsubscribe","params":{"threadId":"old"}}),
    );
    r.server_message(c, &json!({"id":2,"result":{"status":"unsubscribed"}}));
    r.disconnect(c);
    let retained_while_alive = r.awaiting_replacement("old");
    process.kill().unwrap();
    process.wait().unwrap();
    assert!(!retained_while_alive);
    assert!(!r.awaiting_replacement("old"));
    assert!(r.lifecycle_since(0).is_empty());
}
