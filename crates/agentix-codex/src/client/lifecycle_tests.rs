use super::*;

#[path = "../../tests/support/mod.rs"]
#[allow(dead_code)]
mod support;
use support::{MockCodexAppServer, MockThread, MockTurn};

#[tokio::test]
async fn offline_proxy_attachment_waits_for_registration_and_recovers() {
    for read_only in [false, true] {
        let server = MockCodexAppServer::start();
        server
            .add_thread(MockThread::new("saved", "Saved", "/work"))
            .await;
        if read_only {
            server.set_active_writer("saved").await;
        }
        let registry = crate::ClientRegistry::default();
        let client = CodexClient::connect_with_registry(
            server.endpoint(),
            Path::new("must-not-launch"),
            Path::new("/tmp"),
            false,
            registry.clone(),
        )
        .await
        .unwrap();
        let session = SessionId::new("saved");
        let mut events = client.subscribe();
        let error = client.attach(&session).await.unwrap_err();
        assert!(matches!(error, AgentError::Unavailable(_)), "{error}");
        assert!(client.process_sessions.lock().await.contains(&session));
        assert!(
            client
                .exited_process_sessions
                .lock()
                .await
                .contains(&session)
        );
        assert!(
            !server
                .request_methods()
                .await
                .iter()
                .any(|m| m == "thread/resume")
        );

        let connection = registry.connect(None);
        registry.client_message(connection, &json!({"id": 1, "method": "thread/resume"}));
        registry.server_message(
            connection,
            &json!({"id": 1, "result": {"thread": {"id": "saved"}}}),
        );
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                if let AgentEvent::SessionResumed { session_id } = events.recv().await.unwrap() {
                    assert_eq!(session_id, "saved");
                    break;
                }
            }
        })
        .await
        .expect("saved attachment must recover after client registration");
        assert_eq!(client.is_read_only(&session).await, read_only);
        assert_eq!(
            client.subscriptions.lock().await.contains(&session),
            !read_only
        );
        assert!(
            !client
                .exited_process_sessions
                .lock()
                .await
                .contains(&session)
        );
        assert!(!client.resume_exited_session(&session).await.unwrap());
    }
}

#[tokio::test]
async fn managed_session_matching_excludes_subagents_before_assigning_terminal_slots() {
    use crate::process::{DaemonClient, RunningProcessSnapshot, resolve_running_sessions};

    for source in [
        json!({"subAgent": {"thread_spawn": {"parent_thread_id": "parent", "depth": 1}}}),
        json!({"subAgent": "review"}),
        json!({"subAgent": "compact"}),
        json!({"subAgent": "memory_consolidation"}),
        json!({"subAgent": {"other": "helper"}}),
    ] {
        let server = MockCodexAppServer::start();
        server
            .add_thread(MockThread::new("parent", "Parent", "/work"))
            .await;
        let mut child = MockThread::new("child", "Child", "/work").with_turn(
            MockTurn::in_progress_with_output("child_turn", "Delegated work", ""),
        );
        child.source = source.clone();
        server.add_thread(child).await;
        let mut client = CodexClient::connect(server.endpoint()).await.unwrap();

        // A custom socket retains the app-server's loaded-thread view.
        assert_eq!(
            client.list_sessions(None, 25).await.unwrap().sessions.len(),
            2
        );
        let endpoint = CodexEndpoint::parse_with_codex_home(
            "unix://",
            Some(server.endpoint().socket_path().parent().unwrap()),
        )
        .unwrap();
        client.process_discovery = CodexProcessDiscovery::for_endpoint(&endpoint);
        let (ids, _) = client.loaded_session_ids(None, None).await.unwrap();
        let loaded = client.read_sessions(&ids).await.unwrap();
        let selected = resolve_running_sessions(
            &loaded,
            &RunningProcessSnapshot {
                daemon_clients: vec![DaemonClient {
                    pid: 1,
                    cwd: "/work".into(),
                    terminal: None,
                }],
                ..RunningProcessSnapshot::default()
            },
        );
        assert_eq!(
            selected.ids,
            HashSet::from([SessionId::new("parent")]),
            "a higher-priority subagent must not displace its parent: {source}"
        );
        assert_eq!(loaded.len(), 1, "subagents must be removed before matching");
    }
}

async fn fixture() -> (MockCodexAppServer, CodexClient, SessionId) {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("thr_observed", "Observed", "/work"))
        .await;
    server.set_active_writer("thr_observed").await;
    // The monitor uses the local service's running-thread list; attach sees the
    // discovery-enabled configuration without inspecting the test machine.
    let mut client = CodexClient::connect(server.endpoint()).await.unwrap();
    let endpoint = CodexEndpoint::parse_with_codex_home(
        "unix://",
        Some(server.endpoint().socket_path().parent().unwrap()),
    )
    .unwrap();
    client.process_discovery = CodexProcessDiscovery::for_endpoint(&endpoint);
    (server, client, SessionId::new("thr_observed"))
}

#[tokio::test]
async fn observed_attachment_tracks_exit_and_reappearance_without_acquiring_writer() {
    let (server, client, session) = fixture().await;
    client.attach(&session).await.unwrap();
    assert!(
        client.process_sessions.lock().await.contains(&session),
        "observed sessions must be watched"
    );
    assert!(!client.subscriptions.lock().await.contains(&session));
    let mut events = client.subscribe();
    server.remove_thread(session.as_str()).await;
    let exited = tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            if let event @ AgentEvent::SessionExited { .. } = events.recv().await.unwrap() {
                break event;
            }
        }
    })
    .await
    .expect("external writer exit must be reported");
    assert_eq!(
        exited,
        AgentEvent::SessionExited {
            session_id: session.to_string()
        }
    );
    assert!(client.is_read_only(&session).await);
    server
        .add_thread(MockThread::new(session.as_str(), "Returned", "/work"))
        .await;
    let resumed = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let event @ AgentEvent::SessionResumed { .. } = events.recv().await.unwrap() {
                break event;
            }
        }
    })
    .await
    .expect("observed session reappearance must be reported");
    assert_eq!(
        resumed,
        AgentEvent::SessionResumed {
            session_id: session.to_string()
        }
    );
    assert!(client.is_read_only(&session).await);
    assert!(!client.subscriptions.lock().await.contains(&session));
    assert!(
        !client
            .exited_process_sessions
            .lock()
            .await
            .contains(&session)
    );
    let methods = server.request_methods().await;
    assert_eq!(methods.iter().filter(|m| *m == "thread/resume").count(), 1);
    assert!(!methods.iter().any(|m| m == "thread/unsubscribe"));
    client.unsubscribe(&session).await.unwrap();
}

#[tokio::test]
async fn observed_reappearance_does_not_attempt_a_resume_rpc() {
    let (server, client, session) = fixture().await;
    client.attach(&session).await.unwrap();
    client
        .exited_process_sessions
        .lock()
        .await
        .insert(session.clone());
    let mut events = client.subscribe();
    assert!(client.resume_exited_session(&session).await.unwrap());
    assert!(matches!(
        events.recv().await.unwrap(),
        AgentEvent::SessionResumed { .. }
    ));
    assert!(
        !client.resume_exited_session(&session).await.unwrap(),
        "no duplicate lifecycle event"
    );
    assert!(client.is_read_only(&session).await);
    assert!(!client.subscriptions.lock().await.contains(&session));
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
async fn detaching_an_observed_session_clears_all_lifecycle_tracking() {
    let (server, client, session) = fixture().await;
    client.attach(&session).await.unwrap();
    client.process_sessions.lock().await.insert(session.clone());
    client
        .exited_process_sessions
        .lock()
        .await
        .insert(session.clone());
    client.unsubscribe(&session).await.unwrap();
    assert!(!client.process_sessions.lock().await.contains(&session));
    assert!(
        !client
            .exited_process_sessions
            .lock()
            .await
            .contains(&session)
    );
    assert!(!client.is_read_only(&session).await);
    assert!(
        !server
            .request_methods()
            .await
            .iter()
            .any(|m| m == "thread/unsubscribe")
    );
}

#[tokio::test]
async fn final_client_drop_releases_background_tasks_and_upstream_state() {
    for proxied in [false, true] {
        let server = MockCodexAppServer::start();
        let directory = tempfile::tempdir().unwrap();
        let client = if proxied {
            CodexClient::connect_with_proxy(
                &format!("unix://{}", directory.path().join("proxy.sock").display()),
                server.endpoint(),
                Path::new("must-not-launch"),
                directory.path(),
                false,
            )
            .await
            .unwrap()
        } else {
            CodexClient::connect(server.endpoint()).await.unwrap()
        };
        let writer = Arc::downgrade(&client.writer);
        let pending = Arc::downgrade(&client.pending);
        let clone = client.clone();
        drop(client);
        clone
            .request("thread/loaded/list", json!({}))
            .await
            .unwrap();
        assert!(writer.upgrade().is_some());
        drop(clone);
        tokio::time::timeout(Duration::from_secs(2), async {
            while writer.upgrade().is_some() || pending.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("background reader/monitor retained the dropped client's upstream state");
        if proxied {
            assert!(!directory.path().join("proxy.sock").exists());
        }
    }
}

#[tokio::test]
async fn managed_proxy_tracks_same_directory_clients_and_releases_its_runtime() {
    for listen_ws in [false, true] {
        let server = MockCodexAppServer::start();
        for id in ["a", "b"] {
            server.add_thread(MockThread::new(id, id, "/same")).await;
        }
        let directory = tempfile::tempdir().unwrap();
        let listen = if listen_ws {
            "ws://127.0.0.1:0".to_owned()
        } else {
            format!("unix://{}", directory.path().join("proxy.sock").display())
        };
        let client = CodexClient::connect_with_proxy(
            &listen,
            server.endpoint(),
            Path::new("must-not-launch"),
            directory.path(),
            false,
        )
        .await
        .unwrap();
        let endpoint = client.runtime.as_ref().unwrap().proxy.endpoint().to_owned();
        let registry = client.registry.as_ref().unwrap().clone();
        assert!(
            client
                .list_sessions(None, 25)
                .await
                .unwrap()
                .sessions
                .is_empty()
        );
        let mut clients = Vec::new();
        for id in ["a", "b"] {
            let mut socket = crate::proxy::open_socket(&endpoint).await.unwrap();
            socket
                .send(Message::text(
                    json!({
                        "id": 1, "method": "thread/resume", "params": {"threadId": id, "excludeTurns": true}
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
            loop {
                let message = socket.next().await.unwrap().unwrap();
                let value: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
                if value["id"] == 1 {
                    assert_eq!(value["result"]["thread"]["id"], id, "{value}");
                    break;
                }
            }
            clients.push(socket);
        }
        assert_eq!(
            client.list_sessions(None, 25).await.unwrap().sessions.len(),
            2
        );
        drop(clients.pop());
        tokio::time::timeout(Duration::from_secs(2), async {
            while registry.snapshot().len() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let page = client.list_sessions(None, 25).await.unwrap();
        assert_eq!(page.sessions.len(), 1);
        assert_eq!(page.sessions[0].id.as_str(), "a");
        let loaded = client
            .request("thread/loaded/list", json!({}))
            .await
            .unwrap();
        assert!(loaded["data"].as_array().unwrap().contains(&json!("b")));
        assert!(
            AgentAdapter::attach(&client, &SessionId::new("b"))
                .await
                .is_err()
        );
        drop(client);
        tokio::time::timeout(Duration::from_secs(2), async {
            while !registry.snapshot().is_empty() {
                tokio::task::yield_now().await;
            }
            let remaining = clients[0].next().await;
            assert!(remaining.is_none() || remaining.unwrap().is_err());
            if !listen_ws {
                while directory.path().join("proxy.sock").exists() {
                    tokio::task::yield_now().await;
                }
            }
        })
        .await
        .unwrap();
        let direct = CodexClient::connect(server.endpoint()).await.unwrap();
        direct
            .request("thread/loaded/list", json!({}))
            .await
            .unwrap();
    }
}
