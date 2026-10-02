// Exercise native session handoff through the production engine.
#![cfg(unix)]
use agentix_codex::{CodexClient, CodexEndpoint};
use agentix_core::{AgentRegistry, Engine, SqliteState};
use agentix_domain::*;
use agentix_multiplexer::{
    MultiplexerDriver, MultiplexerError, MultiplexerOutcome, PaneState, PreparedMutation,
};
use async_trait::async_trait;
use std::{
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct LocalChannel(Mutex<Vec<OutboundView>>);
#[async_trait]
impl ChannelAdapter for LocalChannel {
    fn kind(&self) -> ChannelKind {
        ChannelKind::Telegram
    }
    async fn send(
        &self,
        chat: &ConversationRef,
        view: &OutboundView,
    ) -> Result<MessageRef, ChannelError> {
        self.0.lock().unwrap().push(view.clone());
        Ok(MessageRef::new(chat.clone(), "notice"))
    }
    async fn update(
        &self,
        _: &ConversationRef,
        _: &MessageRef,
        view: &OutboundView,
    ) -> Result<(), ChannelError> {
        self.0.lock().unwrap().push(view.clone());
        Ok(())
    }
    async fn set_command_menu(
        &self,
        _: &ConversationRef,
        _: &CommandMenu,
    ) -> Result<(), ChannelError> {
        Ok(())
    }
}
struct Pane(String, String);
impl Drop for Pane {
    fn drop(&mut self) {
        let _ = Command::new(&self.0)
            .args(["-S", &self.1, "kill-server"])
            .output();
    }
}
fn mux(driver: &str, socket: &str, args: &[&str]) -> String {
    let output = Command::new(driver)
        .args(["-S", socket])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

// Select an isolated socket, while exercising the production terminal policies.
#[derive(Debug)]
struct SocketDriver {
    driver: String,
    socket: String,
    pane: String,
    pid: u32,
    cwd: String,
}
#[async_trait]
impl MultiplexerDriver for SocketDriver {
    fn kind(&self) -> MultiplexerKind {
        if self.driver == "tmux" {
            MultiplexerKind::Tmux
        } else {
            MultiplexerKind::Rmux
        }
    }
    async fn inventory(&self, _: bool) -> Result<Option<Vec<PaneState>>, MultiplexerError> {
        Ok(Some(vec![PaneState {
            multiplexer: self.kind(),
            session_id: "test".into(),
            session_name: "test".into(),
            window_id: "window".into(),
            window_index: 0,
            window_name: "codex".into(),
            pane_id: self.pane.clone(),
            pane_index: 0,
            active: true,
            current_command: "codex".into(),
            cwd: self.cwd.clone(),
            foreground_pid: Some(self.pid),
        }]))
    }
    async fn execute(
        &self,
        _: &PreparedMutation,
        _: Option<&[String]>,
    ) -> Result<MultiplexerOutcome, MultiplexerError> {
        unreachable!("handoff uses the existing terminal")
    }
    async fn codex_terminal_input(
        &self,
        pid: u32,
        clear: Option<&str>,
    ) -> Result<Option<String>, MultiplexerError> {
        agentix_multiplexer::codex_terminal_input(
            Path::new(&self.driver),
            &["-S".into(), self.socket.clone()],
            &self.pane,
            pid,
            clear,
        )
        .await
    }
    async fn new_codex_session(
        &self,
        pid: u32,
    ) -> Result<agentix_multiplexer::CodexNewSessionOutcome, MultiplexerError> {
        agentix_multiplexer::send_codex_new(
            Path::new(&self.driver),
            &["-S".into(), self.socket.clone()],
            &self.pane,
            pid,
        )
        .await
    }
    async fn inspect_interaction(
        &self,
        agent: AgentKind,
        pid: u32,
    ) -> Result<Option<TerminalInteraction>, MultiplexerError> {
        agentix_multiplexer::inspect_terminal_interaction(
            Path::new(&self.driver),
            &["-S".into(), self.socket.clone()],
            &self.pane,
            pid,
            agent,
        )
        .await
    }
    async fn respond_interaction(
        &self,
        agent: AgentKind,
        pid: u32,
        expected: &TerminalInteraction,
        response: TerminalInteractionResponse,
    ) -> Result<(), MultiplexerError> {
        agentix_multiplexer::respond_terminal_interaction(
            Path::new(&self.driver),
            &["-S".into(), self.socket.clone()],
            &self.pane,
            pid,
            agent,
            expected,
            response,
        )
        .await
    }
}

/// Opt-in real TUI, production proxy/registry/engine, and local channel sink.
/// No model prompt or live IM message is sent.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn real_codex_new_reattaches_the_namespaced_engine_binding() {
    let Ok(binary) = std::env::var("AGENTIX_TEST_CODEX_BINARY") else {
        return;
    };
    for driver in ["tmux", "rmux"] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("codex-home");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join("config.toml"), format!(
            "model = \"gpt-6-astra\"\nmodel_provider = \"fixture\"\n[features]\nworktrees = true\n[model_providers.fixture]\nname = \"Offline fixture\"\nbase_url = \"http://127.0.0.1:1/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = false\n[projects.{:?}]\ntrust_level = \"trusted\"\n", root.path().canonicalize().unwrap().to_str().unwrap()
        )).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-s",
                "--allow-empty",
                "-qm",
                "Fixture",
            ],
        ] {
            assert!(
                Command::new("git")
                    .args(["-c", "core.hooksPath=/dev/null"])
                    .args(args)
                    .current_dir(root.path())
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let upstream = format!("unix://{}", root.path().join("upstream.sock").display());
        let _upstream = tokio::process::Command::new(&binary)
            .args(["app-server", "--listen", &upstream])
            .env("CODEX_HOME", &home)
            .current_dir(root.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !root.path().join("upstream.sock").exists() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        let endpoint = format!("unix://{}", root.path().join("proxy.sock").display());
        let client = Arc::new(
            CodexClient::connect_with_proxy(
                &endpoint,
                CodexEndpoint::parse(&upstream).unwrap(),
                Path::new(&binary),
                root.path(),
                false,
            )
            .await
            .unwrap(),
        );
        let chat = ConversationRef::new(ChannelKind::Telegram, "isolated-test");
        let socket = root.path().join("mux.sock").to_str().unwrap().to_owned();
        let command = format!(
            "exec env CODEX_HOME={} {} --remote {} --enable worktrees --no-alt-screen -C {}",
            quote(home.to_str().unwrap()),
            quote(&binary),
            quote(&endpoint),
            quote(root.path().to_str().unwrap())
        );
        let pane = mux(
            driver,
            &socket,
            &[
                "new-session",
                "-d",
                "-x",
                "120",
                "-y",
                "35",
                "-P",
                "-F",
                "#{pane_id}",
                &command,
            ],
        );
        let _cleanup = Pane(driver.into(), socket.clone());
        tokio::time::sleep(Duration::from_secs(2)).await;
        let startup = mux(driver, &socket, &["capture-pane", "-p", "-t", &pane]);
        if startup.contains("Do you trust the contents of this directory?")
            || startup.contains("Trust this folder?")
        {
            mux(driver, &socket, &["send-keys", "-t", &pane, "Enter"]);
        }
        let old = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(id) = client
                    .client_bindings()
                    .iter()
                    .flat_map(|c| c.sessions.iter())
                    .next()
                {
                    break id.clone();
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error}: {}",
                mux(driver, &socket, &["capture-pane", "-p", "-t", &pane])
            )
        });
        let pid = client
            .client_bindings()
            .into_iter()
            .find(|binding| binding.sessions.contains(&old))
            .unwrap()
            .pid
            .unwrap();
        let driver_port = Arc::new(SocketDriver {
            driver: driver.into(),
            socket: socket.clone(),
            pane: pane.clone(),
            pid,
            cwd: root.path().to_str().unwrap().into(),
        });
        let client = Arc::new(
            client
                .as_ref()
                .clone()
                .with_multiplexer(driver_port.clone()),
        );
        let registry = Arc::new(
            AgentRegistry::new(vec![(AgentKind::Codex, client.clone())])
                .unwrap()
                .with_terminal_interactions(Arc::new(
                    agentix_multiplexer::TerminalInteractions::new(driver_port),
                )),
        );
        let mut events = registry.subscribe();
        let state = SqliteState::in_memory().await.unwrap();
        let channel = Arc::new(LocalChannel::default());
        let engine = Engine::new(registry.clone(), state.clone(), vec![channel.clone()]);
        engine
            .handle_inbound(InboundEnvelope::text(
                "attach",
                chat.clone(),
                "owner",
                format!("/attach codex:{old}"),
            ))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        for option in [0, 1] {
            let previous = state.current_session(&chat).await.unwrap().unwrap();
            let requested_at = std::time::Instant::now();
            engine
                .handle_inbound(InboundEnvelope::text(
                    format!("new-{option}"),
                    chat.clone(),
                    "owner",
                    "/new",
                ))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    let event = events.recv().await.unwrap();
                    if let AgentEvent::SessionSwitchFailed { reason, .. } = &event {
                        panic!(
                            "native /new failed: {reason}; {}",
                            mux(driver, &socket, &["capture-pane", "-p", "-t", &pane])
                        );
                    }
                    let target = match &event {
                        AgentEvent::SessionReplaced {
                            replacement_session_id,
                            ..
                        } => Some(SessionId::new(replacement_session_id)),
                        _ => None,
                    };
                    if matches!(event, AgentEvent::InteractionRequested(_)) {
                        engine.handle_agent_event(event).await.unwrap();
                        let token = channel.0.lock().unwrap().last().unwrap().actions[option]
                            .token
                            .clone();
                        let selected_at = std::time::Instant::now();
                        tokio::time::timeout(
                            Duration::from_secs(2),
                            engine.handle_inbound(InboundEnvelope::action(
                                format!("select-{option}"),
                                chat.clone(),
                                "owner",
                                token,
                            )),
                        )
                        .await
                        .expect("terminal selection must not repeatedly start login shells")
                        .unwrap();
                        eprintln!(
                            "{driver}: selection submitted in {:?}",
                            selected_at.elapsed()
                        );
                        continue;
                    }
                    engine.handle_agent_event(event).await.unwrap();
                    if let Some(target) = target {
                        assert_ne!(previous, target);
                        assert_eq!(state.current_session(&chat).await.unwrap(), Some(target));
                        assert_eq!(
                            registry
                                .list_sessions(None, 100)
                                .await
                                .unwrap()
                                .sessions
                                .len(),
                            1
                        );
                        eprintln!("{driver}: attached in {:?}", requested_at.elapsed());
                        break;
                    }
                }
            })
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "{error}: views={:?}; screen={}",
                    channel.0.lock().unwrap(),
                    mux(driver, &socket, &["capture-pane", "-p", "-t", &pane])
                )
            });
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}
