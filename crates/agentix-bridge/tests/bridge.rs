//! Native connection lifecycle and original-process integration tests.
#![cfg(unix)]

#[path = "support/control.rs"]
mod control;
use control::TestControl;

use agentix_bridge::{BridgeAdapter, BridgeHub, BridgeKind};
use agentix_domain::{AgentAdapter, AgentEvent, SessionId};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, BufReader};

// Process startup can be delayed by concurrent compilation on CI runners.
const HOST_STARTUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

#[tokio::test]
async fn bridge_controls_original_host_and_detach_does_not_kill_it() {
    let directory = tempfile::tempdir().unwrap();
    let server = TestControl::bind(directory.path()).unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/agentix-bridge/tests/host.mjs");
    let mut host = tokio::process::Command::new("node")
        .arg(fixture)
        .arg(&server.endpoint)
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut line = String::new();
    tokio::time::timeout(
        HOST_STARTUP_TIMEOUT,
        BufReader::new(host.stdout.take().unwrap()).read_line(&mut line),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(line.trim(), "ready");
    let adapter = BridgeAdapter::new(BridgeKind::Pi, server.hub.clone(), Path::new("/tmp"));
    let mut events = adapter.subscribe();
    wait_for_session(&adapter).await;
    adapter.refresh().await.unwrap();
    assert_single_resume(&mut events);
    let page = adapter.list_sessions(None, 10).await.unwrap();
    assert_eq!(page.sessions.len(), 1);
    assert!(
        page.sessions[0].updated_at.is_some(),
        "native timestamp must survive the wire contract"
    );
    let id = SessionId::new("native-id");
    adapter.attach(&id).await.unwrap();
    assert!(adapter.supports_command(&id, "model").await);
    assert!(!adapter.supports_command(&id, "plan").await);
    let models = adapter
        .session_control()
        .unwrap()
        .run_session_command(&id, agentix_domain::SessionCommand::Model(None))
        .await
        .unwrap();
    assert_eq!(models.choices.len(), 1);
    assert_eq!(models.choices[0].label, "openai/test");
    let turn = adapter.start_turn(&id, "hello").await.unwrap();
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(3), events.recv())
            .await
            .unwrap()
            .unwrap();
        if let AgentEvent::TurnCompleted {
            session_id,
            turn_id,
            ..
        } = event
        {
            assert_eq!(session_id, "native-id");
            assert_eq!(turn_id, turn);
            break;
        }
    }
    assert_eq!(
        adapter
            .read_history(&id, None, 5)
            .await
            .unwrap()
            .turns
            .last()
            .unwrap()
            .agent_text
            .as_deref(),
        Some("bridge answer")
    );
    adapter.unsubscribe(&id).await.unwrap();
    assert!(host.try_wait().unwrap().is_none());
    drop(adapter);
    drop(server);
    let server = TestControl::bind(directory.path()).unwrap();
    let again = BridgeAdapter::new(BridgeKind::Pi, server.hub.clone(), Path::new("/tmp"));
    wait_for_session(&again).await;
    again.attach(&id).await.unwrap();
    assert_eq!(
        again
            .read_history(&id, None, 5)
            .await
            .unwrap()
            .turns
            .last()
            .unwrap()
            .id,
        turn
    );
    assert!(host.try_wait().unwrap().is_none());
}

#[tokio::test]
async fn native_workspace_port_requires_explicit_launch_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let server = TestControl::bind(dir.path()).unwrap();
    let adapter = BridgeAdapter::new(BridgeKind::Pi, server.hub.clone(), dir.path());
    assert!(adapter.workspace_runtime().is_none());
    let adapter = adapter.with_workspace(
        std::path::Path::new("pi"),
        vec!["-e".into(), "bridge.ts".into()],
        dir.path(),
    );
    assert_eq!(
        adapter.workspace_runtime().unwrap().default_directory(),
        dir.path().to_string_lossy()
    );
}

async fn wait_for_session(adapter: &BridgeAdapter) {
    tokio::time::timeout(HOST_STARTUP_TIMEOUT, async {
        while adapter
            .list_sessions(None, 10)
            .await
            .unwrap()
            .sessions
            .is_empty()
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn listener_is_exclusive_and_accepts_both_backends() {
    let directory = tempfile::tempdir().unwrap();
    let server = TestControl::bind(directory.path()).unwrap();
    assert!(TestControl::bind(directory.path()).is_err());
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/agentix-bridge/tests/host.mjs");
    let mut hosts = Vec::new();
    for (kind, flavor) in [("pi", BridgeKind::Pi), ("omp", BridgeKind::Omp)] {
        hosts.push(
            tokio::process::Command::new("node")
                .arg(&fixture)
                .arg(&server.endpoint)
                .arg(kind)
                .kill_on_drop(true)
                .spawn()
                .unwrap(),
        );
        let adapter = BridgeAdapter::new(flavor, server.hub.clone(), Path::new("/tmp"));
        wait_for_session(&adapter).await;
        adapter.attach(&SessionId::new("native-id")).await.unwrap();
    }
    assert!(
        hosts
            .iter_mut()
            .all(|host| host.try_wait().unwrap().is_none())
    );
}

#[cfg(unix)]
#[tokio::test]
async fn registration_rejects_bad_version_paths_and_duplicate_session() {
    use serde_json::json;
    use tokio::io::AsyncWriteExt;
    async fn register(
        dir: &Path,
        version: u32,
        file: &str,
        instance: &str,
    ) -> (tokio::net::UnixStream, String) {
        let mut socket = tokio::net::UnixStream::connect(dir.join("control.sock"))
            .await
            .unwrap();
        let frame = json!({"id":"register","method":"register","params":{"version":version,"agent":"pi","pid":42,"cwd":"/tmp","session_id":"same","session_file":file,"instance":instance,"snapshot":{"instance":instance,"session":{"id":"same","name":null,"preview":null,"cwd":"/tmp","status":"idle","updatedAt":1},"seq":0,"capabilities":[],"turns":[],"queue":{"items":[],"paused":false,"uncertain":null}}}});
        socket
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .unwrap();
        let mut response = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            BufReader::new(&mut socket).read_line(&mut response),
        )
        .await
        .unwrap()
        .unwrap();
        (socket, response)
    }
    let directory = tempfile::tempdir().unwrap();
    let server = TestControl::bind(directory.path()).unwrap();
    let _adapter = BridgeAdapter::new(
        BridgeKind::Pi,
        server.hub.clone(),
        Path::new("/tmp/allowed"),
    );
    let (_, response) = register(directory.path(), 1, "/tmp/allowed/a.jsonl", "a").await;
    assert!(response.is_empty());
    for path in [
        "/tmp/elsewhere/a.jsonl",
        "/tmp/allowed/../elsewhere/a.jsonl",
    ] {
        let (_, response) = register(directory.path(), 2, path, "a").await;
        assert!(
            response.is_empty(),
            "out-of-root registration accepted: {path}"
        );
    }
    let (_first, response) = register(directory.path(), 2, "/tmp/allowed/a.jsonl", "a").await;
    assert!(response.contains("true"));
    let (_, response) = register(directory.path(), 2, "/tmp/allowed/a.jsonl", "b").await;
    assert!(response.is_empty(), "duplicate session accepted");
    assert_eq!(
        BridgeHub::live_count(&server.endpoint, BridgeKind::Pi, Path::new("/tmp/allowed"))
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn shutdown_rejects_a_registration_already_in_flight() {
    let hub = std::sync::Arc::new(BridgeHub::new());
    let _adapter = BridgeAdapter::new(BridgeKind::Pi, hub.clone(), Path::new("/tmp"));
    hub.shutdown().await;
    let (stream, _remote) = tokio::io::duplex(4096);
    let frame = serde_json::json!({"id":"register","method":"register","params":{"version":2,"agent":"pi","pid":42,"cwd":"/tmp","session_id":"native","session_file":"/tmp/session.jsonl","instance":"one","snapshot":{"instance":"one","seq":0,"session":{"id":"native","name":null,"preview":null,"cwd":"/tmp","status":"idle","updatedAt":1},"capabilities":[],"turns":[],"queue":{"items":[],"paused":false,"uncertain":null}}}});
    assert!(hub.accept(frame, stream).await.is_err());
}

fn assert_single_resume(events: &mut tokio::sync::broadcast::Receiver<AgentEvent>) {
    let mut resumed = 0;
    while let Ok(event) = events.try_recv() {
        if matches!(event, AgentEvent::SessionResumed { .. }) {
            resumed += 1;
        }
    }
    assert_eq!(
        resumed, 1,
        "one connection must produce one lifecycle event"
    );
}

#[tokio::test]
async fn listing_polls_independent_hosts_concurrently_with_metadata_only() {
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;
    let hub = Arc::new(BridgeHub::new());
    let root = std::env::temp_dir();
    let adapter = BridgeAdapter::new(BridgeKind::Pi, hub.clone(), &root);
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut hosts = Vec::new();
    for index in 0..2 {
        let mut snapshot: Value = serde_json::from_str(include_str!(
            "../../../plugins/agentix-bridge/protocol/snapshot.json"
        ))
        .unwrap();
        let id = format!("concurrent-{index}");
        snapshot["session"]["id"] = json!(id);
        let (server, host) = tokio::io::duplex(65_536);
        let response = snapshot.clone();
        let barrier = barrier.clone();
        hosts.push(tokio::spawn(async move {
            let mut host = BufReader::new(host);
            let mut line = String::new();
            loop {
                if host.read_line(&mut line).await.unwrap() == 0 {
                    return;
                }
                let frame: Value = serde_json::from_str(&line).unwrap();
                line.clear();
                if frame["method"].is_string() {
                    barrier.wait().await;
                    assert_eq!(frame["method"], "info");
                    let mut metadata = response.clone();
                    metadata.as_object_mut().unwrap().remove("turns");
                    metadata.as_object_mut().unwrap().remove("queue");
                    let frame = json!({"id":frame["id"],"ok":true,"result":metadata});
                    host.get_mut()
                        .write_all(format!("{frame}\n").as_bytes())
                        .await
                        .unwrap();
                    return;
                }
            }
        }));
        hub.accept(json!({"id":"register","method":"register","params":{
            "version":2,"agent":"pi","instance":snapshot["instance"],"pid":1,
            "session_id":id,"session_file":root.join(format!("{id}.jsonl")),"cwd":root,"snapshot":snapshot,
        }}), server).await.unwrap();
    }
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        adapter.list_sessions(None, 20),
    )
    .await;
    if result.is_err() {
        for host in &hosts {
            host.abort();
        }
    }
    assert_eq!(
        result
            .expect("independent hosts must be polled concurrently")
            .unwrap()
            .sessions
            .len(),
        2
    );
    for host in hosts {
        host.await.unwrap();
    }
    hub.shutdown().await;
}

#[tokio::test]
async fn metadata_cannot_redirect_an_existing_connection_to_another_session() {
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;
    let hub = Arc::new(BridgeHub::new());
    let root = std::env::temp_dir();
    let adapter = BridgeAdapter::new(BridgeKind::Pi, hub.clone(), &root);
    let snapshot: Value = serde_json::from_str(include_str!(
        "../../../plugins/agentix-bridge/protocol/snapshot.json"
    ))
    .unwrap();
    let (server, host) = tokio::io::duplex(65_536);
    let mut response = snapshot.clone();
    response["session"]["id"] = json!("different-session");
    let remote = tokio::spawn(async move {
        let mut host = BufReader::new(host);
        let mut line = String::new();
        loop {
            if host.read_line(&mut line).await.unwrap() == 0 {
                return;
            }
            let frame: Value = serde_json::from_str(&line).unwrap();
            line.clear();
            if frame["method"].is_string() {
                let frame = json!({"id":frame["id"],"ok":true,"result":response});
                host.get_mut()
                    .write_all(format!("{frame}\n").as_bytes())
                    .await
                    .unwrap();
                return;
            }
        }
    });
    hub.accept(json!({"id":"register","method":"register","params":{
        "version":2,"agent":"pi","instance":snapshot["instance"],"pid":1,
        "session_id":snapshot["session"]["id"],"session_file":root.join("native.jsonl"),"cwd":root,"snapshot":snapshot,
    }}), server).await.unwrap();
    assert!(
        adapter.list_sessions(None, 20).await.is_err(),
        "foreign metadata must be rejected"
    );
    remote.await.unwrap();
    hub.shutdown().await;
}

#[tokio::test]
async fn claude_plugin_uses_existing_bridge_contract() {
    let directory = tempfile::tempdir().unwrap();
    let server = TestControl::bind(directory.path()).unwrap();
    let adapter = BridgeAdapter::new(BridgeKind::Claude, server.hub.clone(), directory.path());
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/agentix-bridge/tests/claude-host.mjs");
    let mut host = tokio::process::Command::new("node")
        .arg(fixture)
        .arg(&server.endpoint)
        .arg(directory.path())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut line = String::new();
    tokio::time::timeout(
        HOST_STARTUP_TIMEOUT,
        BufReader::new(host.stdout.take().unwrap()).read_line(&mut line),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(line.trim(), "ready");
    wait_for_session(&adapter).await;
    let id = SessionId::new("native-claude");
    adapter.attach(&id).await.unwrap();
    assert!(!adapter.supports_command(&id, "model").await);
    let mut events = adapter.subscribe();
    let turn_id = adapter.start_turn(&id, "hello").await.unwrap();
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        if let AgentEvent::TurnCompleted {
            turn_id: completed, ..
        } = event
        {
            assert_eq!(completed, turn_id);
            break;
        }
    }
    let history = adapter.read_history(&id, None, 20).await.unwrap();
    assert_eq!(
        history.turns.last().unwrap().agent_text.as_deref(),
        Some("Claude reply")
    );
    let queue = adapter.queued_prompts().unwrap();
    assert!(
        !queue
            .queue_status(&id)
            .await
            .unwrap()
            .unwrap()
            .contains("FIFO")
    );
    adapter.unsubscribe(&id).await.unwrap();
    assert!(host.try_wait().unwrap().is_none());
    tokio::process::Command::new("kill")
        .args(["-TERM", &host.id().unwrap().to_string()])
        .status()
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), host.wait())
        .await
        .unwrap()
        .unwrap();
}
