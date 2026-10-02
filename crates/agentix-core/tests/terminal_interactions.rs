use agentix_core::*;
use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

struct Host {
    target: Mutex<TerminalInteractionTarget>,
    events: broadcast::Sender<AgentEvent>,
}
impl Host {
    fn new(pid: u32) -> Self {
        Self {
            target: Mutex::new(TerminalInteractionTarget {
                pid,
                client_id: format!("client-{pid}"),
            }),
            events: broadcast::channel(64).0,
        }
    }
}
#[async_trait]
impl AgentAdapter for Host {
    fn display_name(&self) -> &'static str {
        "Fixture"
    }
    fn generation(&self) -> u64 {
        1
    }
    fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }
    async fn terminal_interaction_target(
        &self,
        _: &SessionId,
    ) -> Option<TerminalInteractionTarget> {
        Some(self.target.lock().unwrap().clone())
    }
    async fn list_sessions(&self, _: Option<String>, _: u32) -> Result<SessionPage, AgentError> {
        Ok(SessionPage {
            sessions: Vec::new(),
            next_cursor: None,
        })
    }
    async fn read_history(
        &self,
        _: &SessionId,
        _: Option<String>,
        _: u32,
    ) -> Result<HistoryPage, AgentError> {
        Ok(HistoryPage {
            turns: Vec::new(),
            older_cursor: None,
            newer_cursor: None,
        })
    }
    async fn attach(&self, _: &SessionId) -> Result<(), AgentError> {
        Ok(())
    }
    async fn unsubscribe(&self, _: &SessionId) -> Result<(), AgentError> {
        Ok(())
    }
    async fn start_turn(&self, _: &SessionId, _: &str) -> Result<String, AgentError> {
        panic!("no prompt delivery")
    }
    async fn steer(&self, _: &SessionId, _: &str, _: &str) -> Result<String, AgentError> {
        panic!("no steering")
    }
    async fn interrupt(&self, _: &SessionId, _: &str) -> Result<(), AgentError> {
        Ok(())
    }
    async fn resolve_interaction(&self, _: InteractionDecision) -> Result<(), AgentError> {
        panic!("terminal answers belong to the shared port")
    }
}

struct Terminal {
    prompt: Mutex<Option<TerminalInteraction>>,
    answers: Mutex<Vec<(AgentKind, u32, TerminalInteractionResponse)>>,
    stall_pid: Option<u32>,
}
impl Terminal {
    fn new(kind: TerminalInteractionKind) -> Self {
        Self {
            prompt: Mutex::new(Some(TerminalInteraction {
                kind,
                title: "A future dialog".into(),
                detail: "Arbitrary native choices".into(),
                choices: if kind == TerminalInteractionKind::Unknown {
                    Vec::new()
                } else {
                    vec!["1. First".into(), "2. Second".into(), "3. Third".into()]
                },
                selected: Some(0),
                fingerprint: "fixture-dialog".into(),
                pane_id: "%7".into(),
            })),
            answers: Mutex::default(),
            stall_pid: None,
        }
    }
}
#[async_trait]
impl TerminalInteractionPort for Terminal {
    async fn inspect(
        &self,
        _: AgentKind,
        target: &TerminalInteractionTarget,
    ) -> Result<Option<TerminalInteraction>, AgentError> {
        if self.stall_pid == Some(target.pid) {
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        }
        Ok(self.prompt.lock().unwrap().clone())
    }
    async fn respond(
        &self,
        agent: AgentKind,
        target: &TerminalInteractionTarget,
        _: &TerminalInteraction,
        response: TerminalInteractionResponse,
    ) -> Result<(), AgentError> {
        self.answers
            .lock()
            .unwrap()
            .push((agent, target.pid, response));
        Ok(())
    }
}
async fn next_request(receiver: &mut broadcast::Receiver<AgentEvent>) -> InteractionRequest {
    loop {
        if let AgentEvent::InteractionRequested(request) = receiver.recv().await.unwrap() {
            return request;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn every_backend_observes_native_choices_without_a_command_and_routes_explicit_answers() {
    let terminal = Arc::new(Terminal::new(TerminalInteractionKind::Choice));
    let kinds = [
        AgentKind::Codex,
        AgentKind::Pi,
        AgentKind::Omp,
        AgentKind::Claude,
    ];
    let registry = AgentRegistry::new(
        kinds
            .iter()
            .enumerate()
            .map(|(i, kind)| {
                (
                    *kind,
                    Arc::new(Host::new(u32::try_from(i).unwrap() + 1)) as Arc<dyn AgentAdapter>,
                )
            })
            .collect(),
    )
    .unwrap()
    .with_terminal_interactions(terminal.clone());
    let mut events = registry.subscribe();
    for kind in kinds {
        registry
            .attach(&SessionId::new(format!("{}:session", kind.as_str())))
            .await
            .unwrap();
    }
    let mut requests = Vec::new();
    for _ in kinds {
        requests.push(next_request(&mut events).await);
    }
    assert!(terminal.answers.lock().unwrap().is_empty());
    for request in requests {
        assert_eq!(request.title, "A future dialog");
        assert_eq!(
            request.available_decisions[..3],
            ["1. First", "2. Second", "3. Third"]
        );
        assert_eq!(request.auto_resolution_ms, None);
        let decision = InteractionDecision {
            rpc_id: request.rpc_id,
            response: json!({"decision":"2. Second"}),
        };
        registry
            .resolve_interaction(decision.clone())
            .await
            .unwrap();
        assert!(registry.resolve_interaction(decision).await.is_err());
    }
    let answers = terminal.answers.lock().unwrap();
    assert_eq!(answers.len(), 4);
    for kind in kinds {
        assert!(
            answers.iter().any(
                |answer| answer.0 == kind && answer.2 == TerminalInteractionResponse::Choice(1)
            )
        );
    }
}

#[tokio::test(start_paused = true)]
async fn terminal_responses_expire_when_the_original_client_changes_or_detaches() {
    for detach in [false, true] {
        let host = Arc::new(Host::new(7));
        let terminal = Arc::new(Terminal::new(TerminalInteractionKind::Choice));
        let registry = AgentRegistry::new(vec![(AgentKind::Pi, host.clone())])
            .unwrap()
            .with_terminal_interactions(terminal.clone());
        let mut events = registry.subscribe();
        let session = SessionId::new("pi:session");
        registry.attach(&session).await.unwrap();
        let request = next_request(&mut events).await;
        if detach {
            registry.unsubscribe(&session).await.unwrap();
        } else {
            host.target.lock().unwrap().client_id = "replacement".into();
        }
        assert!(
            registry
                .resolve_interaction(InteractionDecision {
                    rpc_id: request.rpc_id,
                    response: json!({"decision":"2. Second"})
                })
                .await
                .is_err()
        );
        assert!(terminal.answers.lock().unwrap().is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn unknown_terminal_dialogs_are_visible_but_cannot_send_keys() {
    let terminal = Arc::new(Terminal::new(TerminalInteractionKind::Unknown));
    let registry = AgentRegistry::new(vec![(AgentKind::Claude, Arc::new(Host::new(3)))])
        .unwrap()
        .with_terminal_interactions(terminal.clone());
    let mut events = registry.subscribe();
    registry
        .attach(&SessionId::new("claude:session"))
        .await
        .unwrap();
    let request = next_request(&mut events).await;
    assert!(request.detail.contains("original terminal"));
    assert!(request.detail.contains("%7"));
    registry
        .resolve_interaction(InteractionDecision {
            rpc_id: request.rpc_id,
            response: json!({"decision":"Handled in terminal"}),
        })
        .await
        .unwrap();
    assert!(terminal.answers.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn structured_interactions_replace_terminal_fallbacks_without_duplicate_controls() {
    let host = Arc::new(Host::new(7));
    let terminal = Arc::new(Terminal::new(TerminalInteractionKind::Choice));
    let registry = AgentRegistry::new(vec![(AgentKind::Codex, host.clone())])
        .unwrap()
        .with_terminal_interactions(terminal.clone());
    let mut events = registry.subscribe();
    registry
        .attach(&SessionId::new("codex:session"))
        .await
        .unwrap();
    let native = next_request(&mut events).await;
    host.events
        .send(AgentEvent::InteractionRequested(InteractionRequest {
            rpc_id: json!("server-rpc"),
            method: "protocol/requestApproval".into(),
            session_id: "session".into(),
            turn_id: "turn".into(),
            item_id: None,
            kind: InteractionKind::CommandApproval,
            title: "Protocol approval".into(),
            detail: "Command".into(),
            available_decisions: vec!["accept".into(), "decline".into()],
            payload: json!({}),
            auto_resolution_ms: None,
        }))
        .unwrap();
    let structured = next_request(&mut events).await;
    assert_eq!(structured.method, "protocol/requestApproval");
    assert!(
        registry
            .resolve_interaction(InteractionDecision {
                rpc_id: native.rpc_id,
                response: json!({"decision":"2. Second"})
            })
            .await
            .is_err()
    );
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    assert!(
        events.try_recv().is_err(),
        "no duplicate native interaction while protocol request is pending"
    );
    assert!(terminal.answers.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn locally_dismissed_terminal_dialogs_revoke_im_controls() {
    let terminal = Arc::new(Terminal::new(TerminalInteractionKind::Choice));
    let registry = AgentRegistry::new(vec![(AgentKind::Omp, Arc::new(Host::new(3)))])
        .unwrap()
        .with_terminal_interactions(terminal.clone());
    let mut events = registry.subscribe();
    registry
        .attach(&SessionId::new("omp:session"))
        .await
        .unwrap();
    let request = next_request(&mut events).await;
    *terminal.prompt.lock().unwrap() = None;
    loop {
        if let AgentEvent::InteractionResolved { request_id, .. } = events.recv().await.unwrap() {
            assert_eq!(json!(request_id), request.rpc_id);
            break;
        }
    }
    assert!(
        registry
            .resolve_interaction(InteractionDecision {
                rpc_id: request.rpc_id,
                response: json!({"decision":"2. Second"})
            })
            .await
            .is_err()
    );
    assert!(terminal.answers.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_unresponsive_terminal_does_not_delay_other_agent_interactions() {
    let mut terminal = Terminal::new(TerminalInteractionKind::Choice);
    terminal.stall_pid = Some(1);
    let registry = AgentRegistry::new(vec![
        (AgentKind::Pi, Arc::new(Host::new(1))),
        (AgentKind::Omp, Arc::new(Host::new(2))),
    ])
    .unwrap()
    .with_terminal_interactions(Arc::new(terminal));
    let mut events = registry.subscribe();
    registry.attach(&SessionId::new("pi:slow")).await.unwrap();
    registry.attach(&SessionId::new("omp:fast")).await.unwrap();
    let request =
        tokio::time::timeout(std::time::Duration::from_secs(2), next_request(&mut events))
            .await
            .expect("another agent must not wait for the blocked terminal");
    assert_eq!(request.session_id, "omp:fast");
}
