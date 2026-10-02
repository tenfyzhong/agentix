use super::*;
use agentix_codex::ClientRegistry;
use agentix_domain::MultiplexerKind;
use agentix_multiplexer::{
    CodexCheckoutChoice, CodexNewSessionOutcome, MultiplexerDriver, MultiplexerError,
    MultiplexerOutcome, PaneState, PreparedMutation,
};

#[derive(Debug, Default)]
struct CheckoutDriver(
    Mutex<Vec<CodexCheckoutChoice>>,
    std::sync::atomic::AtomicBool,
);

#[async_trait]
impl MultiplexerDriver for CheckoutDriver {
    fn kind(&self) -> MultiplexerKind {
        MultiplexerKind::Tmux
    }
    async fn inventory(&self, _: bool) -> Result<Option<Vec<PaneState>>, MultiplexerError> {
        Ok(Some(vec![PaneState {
            multiplexer: self.kind(),
            session_id: "s".into(),
            session_name: "s".into(),
            window_id: "w".into(),
            window_index: 0,
            window_name: "w".into(),
            pane_id: "%1".into(),
            pane_index: 0,
            active: true,
            current_command: "codex".into(),
            cwd: "/tmp".into(),
            foreground_pid: Some(std::process::id()),
        }]))
    }
    async fn execute(
        &self,
        _: &PreparedMutation,
        _: Option<&[String]>,
    ) -> Result<MultiplexerOutcome, MultiplexerError> {
        unreachable!("no workspace is launched directly")
    }
    async fn codex_terminal_input(
        &self,
        _: u32,
        _: Option<&str>,
    ) -> Result<Option<String>, MultiplexerError> {
        Ok(None)
    }
    async fn new_codex_session(&self, _: u32) -> Result<CodexNewSessionOutcome, MultiplexerError> {
        self.1.store(true, std::sync::atomic::Ordering::Release);
        Ok(CodexNewSessionOutcome::CheckoutChoice)
    }
    async fn inspect_interaction(
        &self,
        _: agentix_domain::AgentKind,
        _: u32,
    ) -> Result<Option<agentix_domain::TerminalInteraction>, MultiplexerError> {
        Ok(self.1.load(std::sync::atomic::Ordering::Acquire).then(|| {
            agentix_domain::TerminalInteraction {
                kind: agentix_domain::TerminalInteractionKind::Choice,
                title: "Where should the new conversation run?".into(),
                detail: "Native checkout choices".into(),
                choices: vec!["Current checkout".into(), "New worktree".into()],
                selected: Some(0),
                fingerprint: "checkout".into(),
                pane_id: "%1".into(),
            }
        }))
    }
    async fn respond_interaction(
        &self,
        _: agentix_domain::AgentKind,
        _: u32,
        _: &agentix_domain::TerminalInteraction,
        response: agentix_domain::TerminalInteractionResponse,
    ) -> Result<(), MultiplexerError> {
        let choice = match response {
            agentix_domain::TerminalInteractionResponse::Choice(0) => {
                CodexCheckoutChoice::CurrentCheckout
            }
            agentix_domain::TerminalInteractionResponse::Choice(1) => {
                CodexCheckoutChoice::NewWorktree
            }
            agentix_domain::TerminalInteractionResponse::Cancel => CodexCheckoutChoice::Cancel,
            agentix_domain::TerminalInteractionResponse::Choice(_) => panic!("unexpected choice"),
        };
        self.0.lock().unwrap().push(choice);
        self.1.store(false, std::sync::atomic::Ordering::Release);
        Ok(())
    }
    async fn select_codex_checkout(
        &self,
        _: u32,
        choice: CodexCheckoutChoice,
    ) -> Result<(), MultiplexerError> {
        self.0.lock().unwrap().push(choice);
        Ok(())
    }
}

#[tokio::test]
async fn codex_new_checkout_is_forwarded_to_im_and_requires_an_explicit_choice() {
    for (index, choice) in [
        (0, CodexCheckoutChoice::CurrentCheckout),
        (1, CodexCheckoutChoice::NewWorktree),
        (2, CodexCheckoutChoice::Cancel),
        (3, CodexCheckoutChoice::CurrentCheckout),
    ] {
        for (namespaced, generic) in [(false, false), (true, false), (true, true)] {
            check_choice(index, choice, namespaced, generic).await;
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn check_choice(index: usize, choice: CodexCheckoutChoice, namespaced: bool, generic: bool) {
    let server = MockCodexAppServer::start();
    server
        .add_thread(MockThread::new("checkout", "Checkout", "/tmp"))
        .await;
    server
        .add_thread(MockThread::new("replacement", "Replacement", "/tmp"))
        .await;
    let registry = ClientRegistry::default();
    let connection = registry.connect(Some(std::process::id()));
    registry.client_message(
        connection,
        &json!({"id":1,"method":"thread/resume","params":{}}),
    );
    registry.server_message(
        connection,
        &json!({"id":1,"result":{"thread":{"id":"checkout"}}}),
    );
    let driver = Arc::new(CheckoutDriver::default());
    let client = Arc::new(
        CodexClient::connect_with_registry(
            server.endpoint(),
            std::path::Path::new("codex"),
            std::path::Path::new("/tmp"),
            false,
            registry.clone(),
        )
        .await
        .unwrap()
        .with_multiplexer(driver.clone()),
    );
    let agent: Arc<dyn AgentAdapter> = if namespaced {
        let registry = agentix_core::AgentRegistry::new(vec![(
            agentix_domain::AgentKind::Codex,
            client.clone(),
        )])
        .unwrap();
        Arc::new(if generic {
            registry.with_terminal_interactions(Arc::new(
                agentix_multiplexer::TerminalInteractions::new(driver.clone()),
            ))
        } else {
            registry
        })
    } else {
        client.clone()
    };
    let mut events = agent.subscribe();
    let channel = Arc::new(RecordingChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state.clone(), vec![channel.clone()]);
    engine
        .handle_inbound(inbound(if namespaced {
            "/attach codex:checkout"
        } else {
            "/attach checkout"
        }))
        .await
        .unwrap();
    engine.handle_inbound(inbound("/new")).await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.unwrap();
            if matches!(event, AgentEvent::InteractionRequested(_)) {
                break event;
            }
        }
    })
    .await
    .expect("checkout choices must reach IM");
    assert!(
        driver.0.lock().unwrap().is_empty(),
        "never choose a default checkout"
    );
    engine.handle_agent_event(event).await.unwrap();
    let view = channel.views().last().unwrap().clone();
    assert!(
        view.title
            .contains("Where should the new conversation run?")
    );
    assert_eq!(
        view.actions
            .iter()
            .map(|a| a.label.as_str())
            .collect::<Vec<_>>(),
        if generic {
            vec!["Current checkout", "New worktree", "Cancel"]
        } else {
            vec!["Current checkout", "New worktree"]
        }
    );
    let chat = ConversationRef::new(ChannelKind::Telegram, "chat-e2e");
    let mut switch = state.session_switch(&chat).await.unwrap().unwrap();
    switch.deadline = 0;
    state.save_session_switch(&mut switch).await.unwrap();
    engine.refresh_session_switch(&chat).await.unwrap();
    assert!(
        state.session_switch(&chat).await.unwrap().is_some(),
        "human choice must not time out as a session launch"
    );
    if index == 3 {
        engine.handle_inbound(inbound("/detach")).await.unwrap();
        assert!(state.session_switch(&chat).await.unwrap().is_none());
    } else if index == 2 {
        engine.handle_inbound(inbound("/cancel")).await.unwrap();
        assert!(state.session_switch(&chat).await.unwrap().is_none());
    } else {
        engine
            .handle_inbound(InboundEnvelope::action(
                "checkout-choice",
                chat.clone(),
                "owner-e2e",
                view.actions[index].token.clone(),
            ))
            .await
            .unwrap();
    }
    assert_eq!(
        *driver.0.lock().unwrap(),
        if index == 3 { vec![] } else { vec![choice] }
    );
    assert!(
        engine
            .handle_inbound(InboundEnvelope::action(
                "duplicate-choice",
                chat.clone(),
                "owner-e2e",
                view.actions[index.min(1)].token.clone()
            ))
            .await
            .is_err()
    );
    assert_eq!(
        *driver.0.lock().unwrap(),
        if index == 3 { vec![] } else { vec![choice] },
        "repeated clicks must not type twice"
    );
    assert!(
        !server
            .request_methods()
            .await
            .contains(&"turn/start".into()),
        "terminal choices must not become model prompts"
    );
    if index < 2 {
        engine
            .handle_inbound(inbound("queued during checkout switch"))
            .await
            .unwrap();
        // Newer CLIs keep blank threads subscribed rather than unsubscribing them.
        registry.client_message(
            connection,
            &json!({"id":2,"method":"thread/start","params":{}}),
        );
        registry.server_message(
            connection,
            &json!({"id":2,"result":{"thread":{"id":"replacement"}}}),
        );
        let event = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let event = events.recv().await.unwrap();
                if matches!(event, AgentEvent::SessionReplaced { .. }) {
                    break event;
                }
            }
        })
        .await
        .expect("IM must follow a requested new thread without waiting for unsubscribe");
        engine.handle_agent_event(event).await.unwrap();
        let expected = SessionId::new(if namespaced {
            "codex:replacement"
        } else {
            "replacement"
        });
        assert_eq!(
            state.current_session(&chat).await.unwrap(),
            Some(expected.clone())
        );
        let sessions = agent.list_sessions(None, 100).await.unwrap().sessions;
        assert_eq!(
            sessions.len(),
            1,
            "retired checkout must not remain in the session picker"
        );
        assert_eq!(sessions[0].id, expected);
        let turns = server.thread("replacement").await.unwrap().turns;
        assert_eq!(turns.len(), 1, "queued messages must reach the replacement");
        assert!(server.thread("checkout").await.unwrap().turns.is_empty());
    } else {
        registry.client_message(
            connection,
            &json!({"id":2,"method":"thread/start","params":{}}),
        );
        registry.server_message(
            connection,
            &json!({"id":2,"result":{"thread":{"id":"replacement"}}}),
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        while let Ok(event) = events.try_recv() {
            assert!(
                !matches!(event, AgentEvent::SessionReplaced { .. }),
                "cancel and detach must revoke the requested handoff"
            );
        }
    }
}
