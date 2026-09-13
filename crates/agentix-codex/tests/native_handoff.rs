// Exercise native session handoff through the production engine.
#![cfg(unix)]
use agentix_codex::{CodexClient, CodexEndpoint};
use agentix_core::{AgentRegistry, Engine, SqliteState};
use agentix_domain::*;
use async_trait::async_trait;
use std::{path::Path, process::Command, sync::Arc, time::Duration};

struct LocalChannel;
#[async_trait]
impl ChannelAdapter for LocalChannel {
    fn kind(&self) -> ChannelKind {
        ChannelKind::Telegram
    }
    async fn send(
        &self,
        chat: &ConversationRef,
        _view: &OutboundView,
    ) -> Result<MessageRef, ChannelError> {
        Ok(MessageRef::new(chat.clone(), "notice"))
    }
    async fn update(
        &self,
        _: &ConversationRef,
        _: &MessageRef,
        _: &OutboundView,
    ) -> Result<(), ChannelError> {
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

/// Opt-in real TUI, production proxy/registry/engine, and local channel sink.
/// No model prompt or live IM message is sent.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn real_codex_new_reattaches_the_namespaced_engine_binding() {
    let (Ok(binary), Ok(upstream)) = (
        std::env::var("AGENTIX_TEST_CODEX_BINARY"),
        std::env::var("AGENTIX_TEST_CODEX_ENDPOINT"),
    ) else {
        return;
    };
    for driver in ["tmux", "rmux"] {
        let root = tempfile::tempdir().unwrap();
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
        let registry =
            Arc::new(AgentRegistry::new(vec![(AgentKind::Codex, client.clone())]).unwrap());
        let mut events = registry.subscribe();
        let state = SqliteState::in_memory().await.unwrap();
        let engine = Engine::new(registry, state.clone(), vec![Arc::new(LocalChannel)]);
        let chat = ConversationRef::new(ChannelKind::Telegram, "isolated-test");
        let socket = root.path().join("mux.sock").to_str().unwrap().to_owned();
        let command = format!(
            "exec {} --remote {} --no-alt-screen -C {}",
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
        if mux(driver, &socket, &["capture-pane", "-p", "-t", &pane])
            .contains("Do you trust the contents of this directory?")
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
        .unwrap();
        engine
            .handle_inbound(InboundEnvelope::text(
                "attach",
                chat.clone(),
                "owner",
                format!("/attach codex:{old}"),
            ))
            .await
            .unwrap();
        let pid = mux(
            driver,
            &socket,
            &["display-message", "-p", "-t", &pane, "#{pane_pid}"],
        )
        .parse()
        .unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        agentix_multiplexer::send_codex_new(
            Path::new(driver),
            &["-S".into(), socket.clone()],
            &pane,
            pid,
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let event = events.recv().await.unwrap();
                eprintln!("{driver}: {event:?}");
                let target = match &event {
                    AgentEvent::SessionReplaced {
                        replacement_session_id,
                        ..
                    } => Some(SessionId::new(replacement_session_id)),
                    _ => None,
                };
                engine.handle_agent_event(event).await.unwrap();
                if let Some(target) = target {
                    assert_eq!(state.current_session(&chat).await.unwrap(), Some(target));
                    break;
                }
            }
        })
        .await
        .unwrap();
    }
}
