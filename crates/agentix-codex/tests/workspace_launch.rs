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
            foreground_pid: None,
        }]))
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
