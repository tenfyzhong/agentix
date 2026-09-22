//! Verify remote Codex workspace arguments at the multiplexer launch boundary.
#![cfg(unix)]

#[path = "support/mod.rs"]
#[allow(dead_code)]
mod support;

use std::sync::Arc;

use agentix_codex::CodexClient;
use agentix_domain::{
    MultiplexerKind, MultiplexerMutation, MultiplexerTarget, PaneSplitDirection,
    WorkspaceRuntimePort,
};
use agentix_multiplexer::{
    MultiplexerDriver, MultiplexerError, MultiplexerOutcome, PaneState, PreparedMutation,
};
use async_trait::async_trait;
use support::MockCodexAppServer;

#[derive(Debug)]
struct WorkspaceDriver {
    cwd: String,
    kind: MultiplexerKind,
    switched: Option<Arc<tokio::sync::Notify>>,
}

#[async_trait]
impl MultiplexerDriver for WorkspaceDriver {
    fn kind(&self) -> MultiplexerKind {
        self.kind
    }

    async fn inventory(&self, _: bool) -> Result<Option<Vec<PaneState>>, MultiplexerError> {
        Ok(Some(vec![PaneState {
            multiplexer: self.kind,
            session_id: "session".into(),
            session_name: "session".into(),
            window_id: "window".into(),
            window_index: 0,
            window_name: "shell".into(),
            pane_id: "%1".into(),
            pane_index: 0,
            active: true,
            current_command: "fish".into(),
            cwd: self.cwd.clone(),
            foreground_pid: Some(std::process::id()),
        }]))
    }

    async fn new_codex_session(&self, pid: u32) -> Result<(), MultiplexerError> {
        assert_eq!(pid, std::process::id());
        self.switched.as_ref().unwrap().notify_one();
        Ok(())
    }

    async fn execute(
        &self,
        prepared: &PreparedMutation,
        argv: Option<&[String]>,
    ) -> Result<MultiplexerOutcome, MultiplexerError> {
        assert_eq!(prepared.cwd, std::fs::canonicalize(&self.cwd).unwrap());
        if !prepared.mutation.launch_agent {
            assert!(argv.is_none(), "empty panes must remain ordinary shells");
            return Err(MultiplexerError::Backend("launch verified".into()));
        }
        let argv = argv.expect("launch Codex");
        let directory = argv.windows(2).find(|pair| pair[0] == "--cd");
        assert_eq!(
            directory.map(|pair| pair[1].as_str()),
            Some(prepared.cwd.to_str().unwrap()),
            "remote Codex must receive the selected absolute cwd, not inherit the app-server cwd; argv={argv:?}"
        );
        assert!(argv.windows(2).any(|pair| pair[0] == "--remote"));
        // Stop at the process-launch boundary; no developer pane is touched.
        Err(MultiplexerError::Backend("launch verified".into()))
    }
}

#[tokio::test]
async fn remote_codex_launch_explicitly_preserves_selected_workspace() {
    let server = MockCodexAppServer::start();
    let directory = tempfile::Builder::new()
        .prefix("workspace with spaces \u{4e2d}\u{6587} '")
        .tempdir()
        .unwrap();
    for kind in [MultiplexerKind::Rmux, MultiplexerKind::Tmux] {
        let client = CodexClient::connect(server.endpoint())
            .await
            .unwrap()
            .with_multiplexer(Arc::new(WorkspaceDriver {
                cwd: directory.path().to_str().unwrap().into(),
                kind,
                switched: None,
            }));
        let cwd = directory.path().to_str().unwrap().to_owned();
        for target in [
            MultiplexerTarget::ExistingPane {
                pane_id: "%1".into(),
            },
            MultiplexerTarget::NewSession {
                name: "new".into(),
                cwd: cwd.clone(),
            },
            MultiplexerTarget::NewWindow {
                session_id: "session".into(),
                name: "new".into(),
                cwd: cwd.clone(),
            },
            MultiplexerTarget::SplitPane {
                pane_id: "%1".into(),
                direction: PaneSplitDirection::Horizontal,
                cwd,
            },
        ] {
            for launch_agent in [true, false] {
                if !launch_agent && matches!(target, MultiplexerTarget::ExistingPane { .. }) {
                    continue;
                }
                let result = client
                    .mutate(MultiplexerMutation {
                        target: target.clone(),
                        launch_agent,
                    })
                    .await;
                assert!(result.unwrap_err().to_string().contains("launch verified"));
            }
        }
    }
}

#[tokio::test]
async fn native_new_reads_only_turn_metadata_without_input_enrichment() {
    check_native_new(None, None, None).await;
}

#[tokio::test]
async fn native_new_uses_paged_turns_when_embedded_turns_are_unsupported() {
    check_native_new(
        None,
        Some((-32601, "list_turns is not supported yet")),
        None,
    )
    .await;
}

#[tokio::test]
async fn native_new_falls_back_when_paged_turns_are_unsupported() {
    check_native_new(Some((-32601, "method not found")), None, None).await;
}

#[tokio::test]
async fn native_new_preserves_turn_read_failures_without_switching() {
    check_native_new(
        Some((-32000, "history unavailable")),
        None,
        Some("history unavailable"),
    )
    .await;
    check_native_new_states(
        &[Some(true)],
        Some((-32601, "method not found")),
        Some((-32601, "list_turns is not supported yet")),
        Some("list_turns is not supported yet"),
    )
    .await;
}

async fn check_native_new(
    paged_error: Option<(i64, &str)>,
    embedded_error: Option<(i64, &str)>,
    expected_error: Option<&str>,
) {
    check_native_new_states(
        &[Some(false), Some(true)],
        paged_error,
        embedded_error,
        expected_error,
    )
    .await;
}

#[tokio::test]
async fn native_new_handles_empty_sessions_and_unmaterialized_history() {
    check_native_new_states(&[None], None, None, None).await;
    check_native_new_states(
        &[None],
        Some((-32600, "thread old is not materialized yet; thread/turns/list is unavailable before first user message")),
        None,
        None,
    ).await;
}

#[tokio::test]
async fn native_new_uses_idle_metadata_when_empty_session_history_is_unsupported() {
    check_native_new_states(
        &[None],
        Some((-32601, "list_turns is not supported yet")),
        Some((-32601, "list_turns is not supported yet")),
        None,
    )
    .await;
}

#[tokio::test]
async fn native_new_does_not_switch_empty_sessions_after_real_read_failures() {
    check_native_new_states(
        &[None],
        Some((-32000, "history unavailable")),
        None,
        Some("history unavailable"),
    )
    .await;
    check_native_new_states(
        &[None],
        Some((-32600, "thread old is not materialized yet")),
        Some((-32000, "metadata unavailable")),
        Some("metadata unavailable"),
    )
    .await;
}

#[tokio::test]
async fn native_new_does_not_treat_active_unmaterialized_history_as_empty() {
    check_native_new_states(
        &[Some(true)],
        Some((-32600, "thread old is not materialized yet")),
        None,
        Some("is not materialized yet"),
    )
    .await;
}

async fn check_native_new_states(
    states: &[Option<bool>],
    paged_error: Option<(i64, &str)>,
    embedded_error: Option<(i64, &str)>,
    expected_error: Option<&str>,
) {
    use agentix_codex::ClientRegistry;
    use agentix_domain::{AgentAdapter, AgentEvent, SessionCommand, SessionId};
    use serde_json::json;
    use std::{path::Path, time::Duration};
    use support::{MockThread, MockTurn};

    for state in states {
        let active = *state == Some(true);
        let server = MockCodexAppServer::start();
        let turn = if active {
            MockTurn::in_progress_with_output("turn_new", "", "")
        } else {
            MockTurn::completed("turn_new", "", "done")
        };
        let thread = MockThread::new("old", "Old", "/work");
        server
            .add_thread(if state.is_some() {
                thread
                    .with_turn(MockTurn::completed("turn_old", "prior", "done"))
                    .with_turn(turn)
            } else {
                thread
            })
            .await;
        let registry = ClientRegistry::default();
        let connection = registry.connect(Some(std::process::id()));
        registry.client_message(
            connection,
            &json!({"id":1,"method":"thread/resume","params":{}}),
        );
        registry.server_message(
            connection,
            &json!({"id":1,"result":{"thread":{"id":"old"}}}),
        );
        let switched = Arc::new(tokio::sync::Notify::new());
        let client = CodexClient::connect_with_registry(
            server.endpoint(),
            Path::new("codex"),
            Path::new("/tmp"),
            true,
            registry,
        )
        .await
        .unwrap()
        .with_multiplexer(Arc::new(WorkspaceDriver {
            cwd: "/tmp".into(),
            kind: MultiplexerKind::Tmux,
            switched: Some(switched.clone()),
        }));
        client.set_background_turn_notifications(false);
        let mut events = client.subscribe();
        if let Some((code, message)) = paged_error {
            server.fail_next("thread/turns/list", code, message).await;
        }
        if let Some((code, message)) = embedded_error {
            server.fail_next("thread/read", code, message).await;
        }
        client
            .session_control()
            .unwrap()
            .run_session_command(&SessionId::new("old"), SessionCommand::New)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                tokio::select! {
                    () = switched.notified() => {
                        assert!(expected_error.is_none(), "must not switch after failed turn inspection");
                        break;
                    },
                    event = events.recv() => {
                        if let Ok(AgentEvent::SessionSwitchFailed { reason, .. }) = event {
                            let expected = expected_error.unwrap_or_else(|| panic!("native new failed: {reason}"));
                            assert!(reason.contains(expected), "{reason}");
                            assert!(!server.request_methods().await.iter().any(|m| m == "turn/interrupt"));
                            assert!(tokio::time::timeout(Duration::from_millis(20), switched.notified()).await.is_err());
                            break;
                        }
                    }
                }
            }
        }).await.expect("native switch reached multiplexer");
        assert_native_new_requests(&server, active, paged_error, embedded_error, expected_error)
            .await;
    }
}

async fn assert_native_new_requests(
    server: &MockCodexAppServer,
    active: bool,
    paged_error: Option<(i64, &str)>,
    embedded_error: Option<(i64, &str)>,
    expected_error: Option<&str>,
) {
    let methods = server.request_methods().await;
    assert_eq!(
        methods.iter().filter(|m| *m == "turn/interrupt").count(),
        usize::from(active && expected_error.is_none())
    );
    assert!(methods.iter().any(|m| m == "thread/turns/list"));
    for params in server.request_params("thread/turns/list").await {
        assert_eq!(params["limit"], 1);
        assert_eq!(params["itemsView"], "notLoaded");
    }
    if paged_error.is_some_and(|(code, _)| code == -32601)
        && embedded_error.is_none()
        && expected_error.is_none()
    {
        let reads = server.request_params("thread/read").await;
        assert_eq!(reads.len(), 1, "legacy fallback must not enrich turn input");
        assert_eq!(reads[0]["includeTurns"], true);
    }
    if paged_error.is_none() {
        assert_eq!(
            methods.iter().filter(|m| *m == "thread/read").count(),
            0,
            "turn-state checks must not fetch metadata to restore missing input"
        );
        assert_eq!(
            methods.iter().filter(|m| *m == "thread/turns/list").count(),
            if active { 2 } else { 1 },
            "only initial inspection and post-interrupt confirmation are needed"
        );
    }
}
