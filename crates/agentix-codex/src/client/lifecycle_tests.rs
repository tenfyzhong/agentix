use super::*;

#[path = "../../tests/support/mod.rs"]
#[allow(dead_code)]
mod support;
use support::{MockCodexAppServer, MockThread};

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
