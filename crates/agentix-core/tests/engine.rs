use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agentix_core::{
    ActionStyle, AgentAdapter, AgentError, AgentEvent, ChannelAdapter, ChannelError, ChannelKind,
    CommandMenu, ConversationRef, Engine, EngineError, HistoryPage, InboundEnvelope,
    InteractionDecision, InteractionKind, InteractionRequest, ItemSummary, MessageRef,
    MultiplexerMutation, MultiplexerMutationResult, MultiplexerPane, MultiplexerSession,
    MultiplexerSnapshot, MultiplexerWindow, OutboundView, QueuedPrompt, QueuedPromptPort,
    SessionCommand, SessionCommandChoice, SessionCommandResult, SessionControlPort, SessionId,
    SessionPage, SessionStatus, SessionSummary, SqliteState, TerminalLocation, ToolSummary,
    TurnStatus, TurnSummary, WorkspaceRuntimePort,
};
use async_trait::async_trait;
use tempfile::tempdir;
use tokio::sync::broadcast;

#[tokio::test]
async fn disconnect_invalidates_listed_idle_sessions_even_after_they_disappear() {
    use agentix_core::{AgentKind, AgentRegistry};
    let pi = Arc::new(FakeAgent::new());
    let registry = AgentRegistry::new(vec![(AgentKind::Pi, pi.clone())]).unwrap();
    registry.list_sessions(None, 10).await.unwrap();
    let mut events = registry.subscribe();
    pi.sessions.lock().unwrap().clear();
    pi.events
        .send(AgentEvent::Disconnected {
            generation: 1,
            reason: "disappeared".into(),
        })
        .unwrap();
    let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        event,
        AgentEvent::SessionStatusChanged {
            status: SessionStatus::Offline,
            ..
        }
    ));
}

#[tokio::test]
async fn unavailable_native_session_notice_names_the_original_host() {
    use agentix_core::{AgentKind, AgentRegistry};
    let mut pi = FakeAgent::new();
    pi.read_only = true;
    let registry = AgentRegistry::new(vec![(AgentKind::Pi, Arc::new(pi))]).unwrap();
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        Arc::new(registry),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    engine
        .handle_inbound(inbound("chat-a", "/attach pi:thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "hello"))
        .await
        .unwrap();
    let body = channel.sent().last().unwrap().1.body.clone();
    assert!(body.contains("Pi"));
    assert!(!body.contains("Codex"));
}

#[tokio::test]
async fn backend_disconnect_expires_only_its_own_interaction_tokens() {
    for transport in [true, false] {
        use agentix_core::{AgentKind, AgentRegistry};
        let pi = Arc::new(FakeAgent::new());
        let omp = Arc::new(FakeAgent::new());
        let registry = AgentRegistry::new(vec![
            (AgentKind::Pi, pi.clone()),
            (AgentKind::Omp, omp.clone()),
        ])
        .unwrap();
        let mut events = registry.subscribe();
        for source in [&pi, &omp] {
            source
                .events
                .send(AgentEvent::InteractionRequested(user_input_request(
                    "same-rpc-id",
                    &serde_json::json!([]),
                )))
                .unwrap();
        }
        let mut tokens = HashMap::new();
        while tokens.len() < 2 {
            if let AgentEvent::InteractionRequested(request) =
                tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
                    .await
                    .unwrap()
                    .unwrap()
            {
                tokens.insert(request.session_id, request.rpc_id);
            }
        }
        assert_ne!(tokens["pi:thr_a"], tokens["omp:thr_a"]);
        let disconnect = if transport {
            AgentEvent::Disconnected {
                generation: 1,
                reason: "lost Pi".into(),
            }
        } else {
            AgentEvent::SessionStatusChanged {
                session_id: "thr_a".into(),
                status: SessionStatus::Offline,
            }
        };
        pi.events.send(disconnect).unwrap();
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
                .await
                .unwrap()
                .unwrap();
            if matches!(
                event,
                AgentEvent::SessionStatusChanged {
                    status: SessionStatus::Offline,
                    ..
                }
            ) {
                break;
            }
        }
        assert!(
            registry
                .resolve_interaction(InteractionDecision {
                    rpc_id: tokens["pi:thr_a"].clone(),
                    response: serde_json::json!({})
                })
                .await
                .is_err()
        );
        registry
            .resolve_interaction(InteractionDecision {
                rpc_id: tokens["omp:thr_a"].clone(),
                response: serde_json::json!({}),
            })
            .await
            .unwrap();
        assert_eq!(
            omp.interaction_decisions.lock().unwrap()[0].rpc_id,
            serde_json::json!("same-rpc-id")
        );
        assert!(pi.interaction_decisions.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn session_picker_filters_by_backend() {
    use agentix_core::{AgentKind, AgentRegistry};
    let registry = AgentRegistry::new(vec![
        (AgentKind::Codex, Arc::new(FakeAgent::new())),
        (AgentKind::Pi, Arc::new(FakeAgent::new())),
    ])
    .unwrap();
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        Arc::new(registry),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    engine
        .handle_inbound(inbound("chat-a", "/sessions pi"))
        .await
        .unwrap();
    let view = channel.sent().last().unwrap().1.clone();
    assert_eq!(view.actions.len(), 2);
    assert!(!view.body.contains("Codex ·"));
    assert!(view.body.contains("Pi ·"));
}

#[tokio::test]
async fn resumed_bridge_reconciles_a_completion_missed_during_disconnect() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::TurnStarted {
            session_id: "thr_a".into(),
            turn_id: "turn_history".into(),
        })
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::SessionStatusChanged {
            session_id: "thr_a".into(),
            status: SessionStatus::Offline,
        })
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::SessionResumed {
            session_id: "thr_a".into(),
        })
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "after reconnect"))
        .await
        .unwrap();
    assert!(
        agent
            .calls()
            .iter()
            .any(|call| call.starts_with("start:thr_a:after reconnect")),
        "{:?}",
        agent.calls()
    );
}

#[tokio::test]
async fn unavailable_backend_does_not_block_healthy_sessions_and_recovers() {
    use agentix_core::{AgentKind, AgentRegistry, DeferredAgent};
    use std::sync::atomic::{AtomicBool, Ordering};
    let ready = Arc::new(AtomicBool::new(false));
    let mut fake = FakeAgent::new();
    fake.subagent = true;
    let source = Arc::new(fake);
    let gate = ready.clone();
    let deferred = DeferredAgent::new("Codex", "/tmp".into(), move || {
        let ready = gate.clone();
        let source = source.clone();
        async move {
            if ready.load(Ordering::Acquire) {
                Ok(source as Arc<dyn AgentAdapter>)
            } else {
                Err(AgentError::Unavailable("offline".into()))
            }
        }
    });
    let registry = AgentRegistry::new(vec![
        (AgentKind::Codex, Arc::new(deferred)),
        (AgentKind::Pi, Arc::new(FakeAgent::new())),
    ])
    .unwrap();
    let first = registry.list_sessions(None, 20).await.unwrap();
    assert!(
        first
            .sessions
            .iter()
            .all(|s| s.id.as_str().starts_with("pi:"))
    );
    assert!(!first.sessions.is_empty());
    ready.store(true, Ordering::Release);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if registry
                .list_sessions(None, 20)
                .await
                .unwrap()
                .sessions
                .iter()
                .any(|s| s.id.as_str() == "codex:thr_a")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        registry
            .is_subagent(&SessionId::new("codex:thr_a"))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn registry_isolates_same_native_session_and_interaction_ids() {
    use agentix_core::{AgentKind, AgentRegistry, SessionKey};
    let pi = FakeAgent::new();
    let omp = FakeAgent::new();
    {
        let mut sessions = pi.sessions.lock().unwrap();
        sessions[0].name = None;
        sessions[0].preview = Some("Pi native preview".into());
    }
    let registry = AgentRegistry::new(vec![
        (AgentKind::Pi, Arc::new(pi.clone())),
        (AgentKind::Omp, Arc::new(omp.clone())),
    ])
    .unwrap();
    let page = registry.list_sessions(None, 20).await.unwrap();
    assert!(page.sessions.iter().any(|s| {
        s.name
            .as_deref()
            .is_some_and(|name| name.contains("Pi native preview"))
    }));
    assert_eq!(page.sessions.len(), 4);
    let ids: std::collections::HashSet<_> = page.sessions.iter().map(|s| &s.id).collect();
    assert_eq!(ids.len(), 4);
    assert!(registry.attach(&SessionId::new("thr_a")).await.is_err());
    let key = SessionKey::new(AgentKind::Pi, SessionId::new("thr_a")).encode();
    registry.attach(&key).await.unwrap();
    assert!(pi.calls.lock().unwrap().iter().any(|c| c.contains("thr_a")));
    assert!(omp.calls.lock().unwrap().is_empty());
    let mut events = registry.subscribe();
    pi.events
        .send(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "turn".into(),
            item_id: "item".into(),
            delta: "pi".into(),
        })
        .unwrap();
    let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.session_id(), Some(key.as_str()));
}

#[tokio::test]
async fn rmux_selects_backend_before_launch_and_keeps_chats_independent() {
    use agentix_core::{AgentKind, AgentRegistry};
    let codex = Arc::new(FakeAgent::new());
    let pi = Arc::new(FakeAgent::new());
    let registry = AgentRegistry::new(vec![
        (AgentKind::Codex, codex.clone()),
        (AgentKind::Pi, pi.clone()),
    ])
    .unwrap();
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        Arc::new(registry),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    engine
        .handle_inbound(inbound("chat-a", "/rmux"))
        .await
        .unwrap();
    assert!(
        channel
            .sent()
            .last()
            .unwrap()
            .1
            .actions
            .iter()
            .any(|a| a.label == "Pi")
    );
    engine
        .handle_inbound(inbound("chat-a", "/rmux pi"))
        .await
        .unwrap();
    assert!(pi.calls().iter().any(|c| c == "mux-snapshot:auto"));
    assert!(codex.calls().is_empty());
    engine
        .handle_inbound(inbound("chat-b", "/rmux codex"))
        .await
        .unwrap();
    assert!(codex.calls().iter().any(|c| c == "mux-snapshot:auto"));
}

#[tokio::test]
async fn registry_workspace_routes_and_qualifies_native_results() {
    use agentix_core::{AgentKind, AgentRegistry};
    let codex = Arc::new(FakeAgent::new());
    let pi = Arc::new(FakeAgent::new());
    let registry = AgentRegistry::new(vec![
        (AgentKind::Codex, codex.clone()),
        (AgentKind::Pi, pi.clone()),
    ])
    .unwrap();
    let workspace = registry.workspace_for(Some(AgentKind::Pi)).unwrap();
    let result = workspace
        .mutate(MultiplexerMutation {
            target: agentix_core::MultiplexerTarget::NewSession {
                name: "test".into(),
                cwd: "/tmp".into(),
            },
            launch_agent: true,
        })
        .await
        .unwrap();
    assert_eq!(result.session.unwrap().id.as_str(), "pi:thr_mux_new");
    assert!(codex.calls().is_empty());
    assert_eq!(pi.calls().len(), 1);
    assert_eq!(
        workspace.snapshot().await.unwrap().unwrap().sessions[0].windows[0].panes[0]
            .agent_session
            .as_ref()
            .unwrap()
            .as_str(),
        "pi:thr_a"
    );
}

#[tokio::test]
async fn registry_rejects_duplicate_backends() {
    use agentix_core::{AgentKind, AgentRegistry};
    assert!(
        AgentRegistry::new(vec![
            (AgentKind::Pi, Arc::new(FakeAgent::new())),
            (AgentKind::Pi, Arc::new(FakeAgent::new())),
        ])
        .is_err()
    );
}

#[derive(Clone)]
struct FakeAgent {
    refresh_count: Arc<std::sync::atomic::AtomicUsize>,
    calls: Arc<Mutex<Vec<String>>>,
    interaction_decisions: Arc<Mutex<Vec<InteractionDecision>>>,
    history_cursors: Arc<Mutex<Vec<Option<String>>>>,
    history_result_cursors: Arc<Mutex<(Option<String>, Option<String>)>>,
    history_turns: Arc<Mutex<Vec<TurnSummary>>>,
    queued_prompts: Arc<Mutex<Vec<QueuedPrompt>>>,
    queue_supported: bool,
    read_only: bool,
    subagent: bool,
    sessions: Arc<Mutex<Vec<SessionSummary>>>,
    rejected_attachments: Arc<Mutex<Vec<SessionId>>>,
    unavailable_attachments: Arc<Mutex<Vec<SessionId>>>,
    start_failures: Arc<Mutex<usize>>,
    events: broadcast::Sender<AgentEvent>,
}

impl FakeAgent {
    fn new() -> Self {
        let (events, _) = broadcast::channel(32);
        Self {
            refresh_count: Arc::default(),
            calls: Arc::new(Mutex::new(Vec::new())),
            interaction_decisions: Arc::new(Mutex::new(Vec::new())),
            history_cursors: Arc::new(Mutex::new(Vec::new())),
            history_result_cursors: Arc::new(Mutex::new((Some("older".into()), None))),
            history_turns: Arc::new(Mutex::new(vec![TurnSummary {
                id: "turn_history".into(),
                status: TurnStatus::Completed,
                user_text: Some("previous question".into()),
                agent_text: Some("previous answer".into()),
                tools: vec![ToolSummary {
                    kind: "commandExecution".into(),
                    label: "cargo test --workspace".into(),
                    status: "completed".into(),
                }],
                items: Vec::new(),
            }])),
            queued_prompts: Arc::new(Mutex::new(Vec::new())),
            queue_supported: false,
            read_only: false,
            subagent: false,
            sessions: Arc::new(Mutex::new(
                [
                    ("thr_a", "Parser cleanup", "/work/parser"),
                    ("thr_b", "Daemon startup", "/work/daemon"),
                ]
                .into_iter()
                .map(|(id, name, cwd)| SessionSummary {
                    id: SessionId::new(id),
                    name: Some(name.into()),
                    preview: None,
                    cwd: Some(cwd.into()),
                    updated_at: Some(1),
                    status: SessionStatus::Idle,
                    terminal: None,
                })
                .collect(),
            )),
            rejected_attachments: Arc::new(Mutex::new(Vec::new())),
            unavailable_attachments: Arc::default(),
            start_failures: Arc::new(Mutex::new(0)),
            events,
        }
    }

    fn with_history(turns: Vec<TurnSummary>) -> Self {
        let agent = Self::new();
        *agent.history_turns.lock().unwrap() = turns;
        agent
    }

    fn with_sessions(sessions: Vec<SessionSummary>) -> Self {
        let agent = Self::new();
        *agent.sessions.lock().unwrap() = sessions;
        agent
    }

    fn with_history_cursors(older: Option<&str>, newer: Option<&str>) -> Self {
        let agent = Self::new();
        *agent.history_result_cursors.lock().unwrap() =
            (older.map(str::to_owned), newer.map(str::to_owned));
        agent
    }

    fn with_queue_support() -> Self {
        Self {
            queue_supported: true,
            ..Self::new()
        }
    }

    fn rejecting_attachment(session_id: &str) -> Self {
        let agent = Self::new();
        agent
            .rejected_attachments
            .lock()
            .unwrap()
            .push(SessionId::new(session_id));
        agent
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn history_cursors(&self) -> Vec<Option<String>> {
        self.history_cursors.lock().unwrap().clone()
    }

    fn interaction_decisions(&self) -> Vec<InteractionDecision> {
        self.interaction_decisions.lock().unwrap().clone()
    }

    fn fail_next_start(&self) {
        *self.start_failures.lock().unwrap() += 1;
    }
}

#[async_trait]
impl AgentAdapter for FakeAgent {
    async fn refresh(&self) -> Result<(), AgentError> {
        self.refresh_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    fn display_name(&self) -> &'static str {
        "Codex"
    }

    fn queued_prompts(&self) -> Option<&dyn QueuedPromptPort> {
        self.queue_supported.then_some(self)
    }

    fn session_control(&self) -> Option<&dyn SessionControlPort> {
        Some(self)
    }

    fn workspace_runtime(&self) -> Option<&dyn WorkspaceRuntimePort> {
        Some(self)
    }

    async fn list_sessions(
        &self,
        _cursor: Option<String>,
        _limit: u32,
    ) -> Result<SessionPage, AgentError> {
        Ok(SessionPage {
            sessions: self.sessions.lock().unwrap().clone(),
            next_cursor: None,
        })
    }

    async fn is_subagent(&self, _session: &SessionId) -> Result<bool, AgentError> {
        Ok(self.subagent)
    }
    async fn is_read_only(&self, _session: &SessionId) -> bool {
        self.read_only
    }

    async fn read_history(
        &self,
        session_id: &SessionId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<HistoryPage, AgentError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("history:{session_id}:{limit}"));
        self.history_cursors.lock().unwrap().push(cursor);
        let turns = self.history_turns.lock().unwrap();
        let start = turns.len().saturating_sub(limit as usize);
        let (older_cursor, newer_cursor) = self.history_result_cursors.lock().unwrap().clone();
        Ok(HistoryPage {
            turns: turns[start..].to_vec(),
            older_cursor,
            newer_cursor,
        })
    }

    async fn attach(&self, session_id: &SessionId) -> Result<(), AgentError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("attach:{session_id}"));
        if self
            .unavailable_attachments
            .lock()
            .unwrap()
            .contains(session_id)
        {
            return Err(AgentError::Unavailable("session bridge is offline".into()));
        }
        if self
            .rejected_attachments
            .lock()
            .unwrap()
            .contains(session_id)
        {
            return Err(AgentError::Rejected(format!(
                "session {session_id} is no longer running"
            )));
        }
        Ok(())
    }

    async fn unsubscribe(&self, session_id: &SessionId) -> Result<(), AgentError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("unsubscribe:{session_id}"));
        Ok(())
    }

    async fn start_turn(&self, session_id: &SessionId, text: &str) -> Result<String, AgentError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("start:{session_id}:{text}"));
        let mut failures = self.start_failures.lock().unwrap();
        if *failures > 0 {
            *failures -= 1;
            return Err(AgentError::Unavailable("temporary failure".into()));
        }
        Ok("turn_new".into())
    }

    async fn steer(
        &self,
        session_id: &SessionId,
        expected_turn_id: &str,
        text: &str,
    ) -> Result<String, AgentError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("steer:{session_id}:{expected_turn_id}:{text}"));
        Ok(expected_turn_id.into())
    }

    async fn interrupt(&self, session_id: &SessionId, turn_id: &str) -> Result<(), AgentError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("stop:{session_id}:{turn_id}"));
        Ok(())
    }

    async fn resolve_interaction(&self, decision: InteractionDecision) -> Result<(), AgentError> {
        self.interaction_decisions.lock().unwrap().push(decision);
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }

    fn generation(&self) -> u64 {
        1
    }
}

#[async_trait]
impl QueuedPromptPort for FakeAgent {
    async fn queue_prompt(
        &self,
        session_id: &SessionId,
        text: &str,
        _client_message_id: &str,
    ) -> Result<QueuedPrompt, AgentError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("queue:{session_id}:{text}"));
        let mut queued = self.queued_prompts.lock().unwrap();
        let prompt = QueuedPrompt {
            id: format!("queued_{}", queued.len() + 1),
            text: text.to_owned(),
        };
        queued.push(prompt.clone());
        Ok(prompt)
    }

    async fn list_queued_prompts(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<QueuedPrompt>, AgentError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("queue-list:{session_id}"));
        Ok(self.queued_prompts.lock().unwrap().clone())
    }
}

#[async_trait]
impl WorkspaceRuntimePort for FakeAgent {
    fn default_directory(&self) -> String {
        "/work/multiplexer".into()
    }

    async fn snapshot(&self) -> Result<Option<MultiplexerSnapshot>, AgentError> {
        self.calls.lock().unwrap().push("mux-snapshot:auto".into());
        Ok(Some(multiplexer_snapshot()))
    }

    async fn mutate(
        &self,
        mutation: MultiplexerMutation,
    ) -> Result<MultiplexerMutationResult, AgentError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("mux-mutate:{mutation:?}"));
        Ok(MultiplexerMutationResult {
            message: if mutation.launch_agent {
                "Codex started in the target pane.".into()
            } else {
                "Shell created.".into()
            },
            session: mutation.launch_agent.then(|| SessionSummary {
                id: SessionId::new("thr_mux_new"),
                name: None,
                preview: None,
                cwd: Some("/work/new".into()),
                updated_at: Some(2),
                status: SessionStatus::Idle,
                terminal: None,
            }),
        })
    }
}

#[async_trait]
impl SessionControlPort for FakeAgent {
    async fn run_session_command(
        &self,
        session_id: &SessionId,
        command: SessionCommand,
    ) -> Result<SessionCommandResult, AgentError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("command:{session_id}:{command:?}"));
        let replacement_session = match command {
            SessionCommand::Fork => Some(SessionSummary {
                id: SessionId::new("thr_fork"),
                name: Some("Parser cleanup (fork)".into()),
                preview: None,
                cwd: Some("/work/parser".into()),
                updated_at: Some(2),
                status: SessionStatus::Idle,
                terminal: None,
            }),
            _ => None,
        };
        Ok(SessionCommandResult {
            title: "Codex command".into(),
            body: format!("Executed {command:?}"),
            replacement_session,
            active_turn: matches!(command, SessionCommand::Review).then(|| "turn_review".into()),
            choices: match command {
                SessionCommand::Model(None) => vec![
                    SessionCommandChoice::new(
                        "GPT-5.6",
                        SessionCommand::Model(Some("gpt-5.6".into())),
                    ),
                    SessionCommandChoice::new(
                        "GPT-5.6 Terra",
                        SessionCommand::Model(Some("gpt-5.6-terra".into())),
                    ),
                ],
                SessionCommand::Reasoning(None) => vec![
                    SessionCommandChoice::new(
                        "Medium",
                        SessionCommand::Reasoning(Some("medium".into())),
                    ),
                    SessionCommandChoice::new(
                        "High",
                        SessionCommand::Reasoning(Some("high".into())),
                    ),
                ],
                _ => Vec::new(),
            },
        })
    }
}

#[derive(Clone, Default)]
struct FakeChannel {
    bot_identity: Option<String>,
    fail_identity: bool,
    channel_kind: Option<ChannelKind>,
    streaming_interval: Option<std::time::Duration>,
    sent: Arc<Mutex<Vec<(ConversationRef, OutboundView)>>>,
    updated: Arc<Mutex<Vec<(MessageRef, OutboundView)>>>,
    disabled_actions: Arc<Mutex<Vec<MessageRef>>>,
    messages: Arc<Mutex<HashMap<MessageRef, OutboundView>>>,
    session_commands: Arc<Mutex<Vec<(ConversationRef, bool)>>>,
    menus: Arc<Mutex<Vec<CommandMenu>>>,
    fail_menu_updates: Arc<Mutex<bool>>,
    task_send_failures: Arc<Mutex<usize>>,
    inbox_send_failures: Arc<Mutex<usize>>,
    inbox_source: Arc<Mutex<Option<InboundEnvelope>>>,
    reject_unchanged_updates: bool,
    next_send_gate: Arc<
        Mutex<
            Option<(
                tokio_util::sync::CancellationToken,
                tokio_util::sync::CancellationToken,
            )>,
        >,
    >,
    next_menu_gate: Arc<
        Mutex<
            Option<(
                tokio_util::sync::CancellationToken,
                tokio_util::sync::CancellationToken,
            )>,
        >,
    >,
}

impl FakeChannel {
    fn sent(&self) -> Vec<(ConversationRef, OutboundView)> {
        self.sent.lock().unwrap().clone()
    }

    fn updated(&self) -> Vec<(MessageRef, OutboundView)> {
        self.updated.lock().unwrap().clone()
    }

    fn disabled_actions(&self) -> Vec<MessageRef> {
        self.disabled_actions.lock().unwrap().clone()
    }

    fn session_commands(&self) -> Vec<(ConversationRef, bool)> {
        self.session_commands.lock().unwrap().clone()
    }

    fn fail_menu_updates(&self) {
        *self.fail_menu_updates.lock().unwrap() = true;
    }
}

#[async_trait]
impl ChannelAdapter for FakeChannel {
    async fn identity(&self) -> Result<Option<String>, ChannelError> {
        if self.fail_identity {
            return Err(ChannelError::Transport("identity unavailable".into()));
        }
        Ok(self.bot_identity.clone())
    }

    async fn read_inbox_message(
        &self,
        _message: &MessageRef,
    ) -> Result<Option<InboundEnvelope>, ChannelError> {
        Ok(self.inbox_source.lock().unwrap().clone())
    }
    fn streaming_update_interval(&self) -> std::time::Duration {
        self.streaming_interval
            .unwrap_or(std::time::Duration::from_secs(1))
    }

    fn kind(&self) -> ChannelKind {
        self.channel_kind.unwrap_or(ChannelKind::Telegram)
    }

    async fn send(
        &self,
        conversation: &ConversationRef,
        view: &OutboundView,
    ) -> Result<MessageRef, ChannelError> {
        let gate = self.next_send_gate.lock().unwrap().take();
        if let Some((entered, release)) = gate {
            entered.cancel();
            release.cancelled().await;
        }
        if view.title == "Inbox submission" {
            let mut failures = self.inbox_send_failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                return Err(ChannelError::Transport(
                    "injected inbox delivery failure".into(),
                ));
            }
        }
        if view.title == "Task update" {
            let mut failures = self.task_send_failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                return Err(ChannelError::Transport(
                    "injected task delivery failure".into(),
                ));
            }
        }
        self.sent
            .lock()
            .unwrap()
            .push((conversation.clone(), view.clone()));
        let message = MessageRef::new(conversation.clone(), format!("m{}", self.sent().len()));
        self.messages
            .lock()
            .unwrap()
            .insert(message.clone(), view.clone());
        Ok(message)
    }

    async fn update(
        &self,
        conversation: &ConversationRef,
        message: &MessageRef,
        view: &OutboundView,
    ) -> Result<(), ChannelError> {
        if self.reject_unchanged_updates && self.messages.lock().unwrap().get(message) == Some(view)
        {
            return Err(ChannelError::Transport("message is not modified".into()));
        }
        self.messages
            .lock()
            .unwrap()
            .insert(message.clone(), view.clone());
        self.updated
            .lock()
            .unwrap()
            .push((message.clone(), view.clone()));
        self.sent
            .lock()
            .unwrap()
            .push((conversation.clone(), view.clone()));
        Ok(())
    }

    async fn disable_actions(&self, message: &MessageRef) -> Result<(), ChannelError> {
        self.disabled_actions.lock().unwrap().push(message.clone());
        Ok(())
    }

    async fn set_command_menu(
        &self,
        conversation: &ConversationRef,
        menu: &CommandMenu,
    ) -> Result<(), ChannelError> {
        let gate = self.next_menu_gate.lock().unwrap().take();
        if let Some((entered, release)) = gate {
            entered.cancel();
            release.cancelled().await;
        }
        if *self.fail_menu_updates.lock().unwrap() {
            return Err(ChannelError::Transport("temporary menu failure".into()));
        }
        self.menus.lock().unwrap().push(menu.clone());
        self.session_commands.lock().unwrap().push((
            conversation.clone(),
            menu.commands
                .iter()
                .any(|command| command.name == "current"),
        ));
        Ok(())
    }
}

fn inbound(chat: &str, text: &str) -> InboundEnvelope {
    InboundEnvelope::text(
        format!("event-{chat}-{text}"),
        ConversationRef::new(ChannelKind::Telegram, chat),
        "owner",
        text,
    )
}

fn inbound_as(chat: &str, owner: &str, text: &str) -> InboundEnvelope {
    InboundEnvelope::text(
        format!("event-{chat}-{owner}-{text}"),
        ConversationRef::new(ChannelKind::Telegram, chat),
        owner,
        text,
    )
}

async fn task_fixture() -> (tempfile::TempDir, Arc<agentix_task::Service>, String) {
    use agentix_task::{Config, DocumentConfig, Service, StorageConfig, WriteOptions};
    use serde_json::json;
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".obsidian")).unwrap();
    let service = Arc::new(
        Service::open(Config {
            schema_version: 1,
            storage: StorageConfig {
                path: dir.path().join("tasks.sqlite3"),
            },
            documents: DocumentConfig {
                root: dir.path().to_owned(),
                directory: "docs".into(),
            },
        })
        .await
        .unwrap(),
    );
    let project = service
        .execute(
            json!({"command":"project.register","root":dir.path(),"name":"demo"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let job=service.execute(json!({"command":"job.create","project":project["id"],"title":"Task board","goal":"Ship"}),WriteOptions::default()).await.unwrap().result;
    let task = service
        .execute(
            json!({"command":"task.add","job":job["id"],"title":"Implement task board"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let id = task["id"].as_str().unwrap().to_owned();
    service
        .execute(
            json!({"command":"task.claim","task":id,"executor":"agent:codex","session":"thr_a"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    service
        .execute(
            json!({"command":"plan.create","task":id,"body":"# Plan"}),
            task_write_options(&service, &id).await,
        )
        .await
        .unwrap();
    service
        .execute(
            json!({"command":"task.start","task":id}),
            task_write_options(&service, &id).await,
        )
        .await
        .unwrap();
    (dir, service, id)
}

#[tokio::test]
async fn qualified_task_sessions_use_native_ids_and_filter_host() {
    use agentix_task::BrowseScope;
    let (_dir, service, _) = task_fixture().await;
    let codex = service
        .store()
        .browse_snapshot(BrowseScope::Session("codex:thr_a"))
        .await
        .unwrap();
    assert_eq!(codex.jobs.len(), 1);
    let pi = service
        .store()
        .browse_snapshot(BrowseScope::Session("pi:thr_a"))
        .await
        .unwrap();
    assert!(pi.jobs.is_empty());
    let page = service
        .store()
        .session_job_page("codex:thr_a", 0, 6)
        .await
        .unwrap();
    assert_eq!(page.jobs.len(), 1);
}

#[tokio::test]
async fn qualified_session_exit_blocks_only_its_native_tasks() {
    let (_dir, service, _) = task_fixture().await;
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        SqliteState::in_memory().await.unwrap(),
        vec![],
    )
    .with_task_board(service.clone());
    engine
        .handle_agent_event(AgentEvent::SessionExited {
            session_id: "pi:thr_a".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        service.store().snapshot().await.unwrap().tasks[0].status,
        agentix_task::TaskStatus::InProgress
    );
    engine
        .handle_agent_event(AgentEvent::SessionExited {
            session_id: "codex:thr_a".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        service.store().snapshot().await.unwrap().tasks[0].status,
        agentix_task::TaskStatus::Blocked
    );
}

#[tokio::test]
async fn task_buttons_follow_claim_plan_start_done_phases() {
    use agentix_task::WriteOptions;
    use serde_json::json;
    let (_dir, service, existing) = task_fixture().await;
    service
        .execute(
            json!({"command":"task.release","task":existing,"reason":"another task"}),
            task_write_options(&service, &existing).await,
        )
        .await
        .unwrap();
    let job = service.store().snapshot().await.unwrap().jobs[0].id.clone();
    let task = service
        .execute(
            json!({"command":"task.add","job":job,"title":"Unplanned"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let id = task["id"].as_str().unwrap();
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone());
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    let claim = task_button(&engine, &channel, id, "Claim").await;
    engine
        .handle_inbound(InboundEnvelope::action(
            "claim-unplanned",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner",
            claim,
        ))
        .await
        .unwrap();
    let view = channel.sent().last().unwrap().1.clone();
    assert!(view.body.contains("PLANNING"));
    assert!(
        !view
            .actions
            .iter()
            .any(|a| matches!(a.label.as_str(), "Done" | "Start"))
    );
    service
        .execute(
            json!({"command":"plan.create","task":id,"body":"# Plan"}),
            task_write_options(&service, id).await,
        )
        .await
        .unwrap();
    let start = task_button(&engine, &channel, id, "Start").await;
    assert!(
        !channel
            .sent()
            .last()
            .unwrap()
            .1
            .actions
            .iter()
            .any(|a| a.label == "Done")
    );
    engine
        .handle_inbound(InboundEnvelope::action(
            "start-planned",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner",
            start,
        ))
        .await
        .unwrap();
    let view = channel.sent().last().unwrap().1.clone();
    assert!(view.body.contains("EXECUTING"));
    assert!(view.actions.iter().any(|a| a.label == "Done"));
    assert!(!view.actions.iter().any(|a| a.label == "Start"));
}

#[tokio::test]
async fn task_board_commands_reasons_and_notifications_follow_bound_session() {
    let (_dir, service, id) = task_fixture().await;
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone());
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-b", "/attach thr_b"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "/jobs"))
        .await
        .unwrap();
    assert!(channel.sent().last().unwrap().1.body.contains("Task board"));
    engine
        .handle_inbound(inbound("chat-a", &format!("/task {id}")))
        .await
        .unwrap();
    let view = channel.sent().last().unwrap().1.clone();
    let block = view
        .actions
        .iter()
        .find(|a| a.label == "Block")
        .unwrap()
        .token
        .clone();
    engine
        .handle_inbound(InboundEnvelope::action(
            "task-block",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner",
            block,
        ))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "Need upstream API"))
        .await
        .unwrap();
    let task = &service.store().snapshot().await.unwrap().tasks[0];
    assert_eq!(task.status.to_string(), "BLOCKED");
    assert_eq!(task.reason.as_deref(), Some("Need upstream API"));
    refresh_and_deliver_tasks(&engine).await.unwrap();
    let notifications: Vec<_> = channel
        .sent()
        .into_iter()
        .filter(|(_, v)| v.title == "Task update")
        .collect();
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].0.conversation_id, "chat-a");
    let count = channel.sent().len();
    refresh_and_deliver_tasks(&engine).await.unwrap();
    assert_eq!(channel.sent().len(), count);
}

#[tokio::test]
async fn missing_task_plan_resumes_planning_without_interrupting_session_lifecycle() {
    let (dir, service, _id) = task_fixture().await;
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        SqliteState::in_memory().await.unwrap(),
        vec![channel],
    )
    .with_task_board(service.clone());
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::SessionExited {
            session_id: "thr_a".into(),
        })
        .await
        .unwrap();
    let plan = service.store().snapshot().await.unwrap().plans[0].clone();
    std::fs::rename(
        service.config().output_dir().join(plan.path),
        dir.path().join("moved-plan.md"),
    )
    .unwrap();
    engine
        .handle_agent_event(AgentEvent::SessionResumed {
            session_id: "thr_a".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        service.store().snapshot().await.unwrap().tasks[0].phase,
        Some(agentix_task::TaskPhase::Planning)
    );
}

#[tokio::test]
async fn task_board_actions_reject_other_sessions_and_exited_session_buttons() {
    let (_dir, service, id) = task_fixture().await;
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone());
    engine
        .handle_inbound(inbound("chat-b", "/attach thr_b"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-b", &format!("/task {id}")))
        .await
        .unwrap();
    assert!(
        channel
            .sent()
            .last()
            .unwrap()
            .1
            .actions
            .iter()
            .all(|a| a.label == "Job")
    );
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", &format!("/task {id}")))
        .await
        .unwrap();
    let token = channel
        .sent()
        .last()
        .unwrap()
        .1
        .actions
        .iter()
        .find(|a| a.label == "Done")
        .unwrap()
        .token
        .clone();
    engine
        .handle_agent_event(AgentEvent::SessionExited {
            session_id: "thr_a".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        service.store().snapshot().await.unwrap().tasks[0]
            .status
            .to_string(),
        "BLOCKED"
    );
    let result = engine
        .handle_inbound(InboundEnvelope::action(
            "old-task-button",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner",
            token,
        ))
        .await;
    assert!(result.is_err());
    engine
        .handle_agent_event(AgentEvent::SessionResumed {
            session_id: "thr_a".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        service.store().snapshot().await.unwrap().tasks[0]
            .status
            .to_string(),
        "IN_PROGRESS"
    );
}

async fn task_write_options(
    service: &agentix_task::Service,
    id: &str,
) -> agentix_task::WriteOptions {
    let state = service.store().snapshot().await.unwrap();
    let lease = state.leases.iter().find(|l| l.task_id == id).unwrap();
    agentix_task::WriteOptions {
        actor_ref: "agent:test".into(),
        session_ref: Some(lease.session_ref.clone()),
        lease_token: Some(lease.token.clone()),
        ..agentix_task::WriteOptions::default()
    }
}

async fn task_button(engine: &Engine, channel: &FakeChannel, id: &str, label: &str) -> String {
    engine
        .handle_inbound(InboundEnvelope::text(
            uuid::Uuid::new_v4().to_string(),
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner",
            format!("/task {id}"),
        ))
        .await
        .unwrap();
    channel
        .sent()
        .last()
        .unwrap()
        .1
        .actions
        .iter()
        .find(|a| a.label == label)
        .unwrap()
        .token
        .clone()
}

#[tokio::test]
async fn task_revision_change_rejects_button_without_changing_session_or_lease() {
    let (_dir, service, id) = task_fixture().await;
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone());
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    let token = task_button(&engine, &channel, &id, "Done").await;
    let options = task_write_options(&service, &id).await;
    service
        .execute(
            serde_json::json!({"command":"task.update","task":id,"title":"Concurrent update"}),
            options.clone(),
        )
        .await
        .unwrap();
    let error = engine
        .handle_inbound(InboundEnvelope::action(
            "stale-task-revision",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner",
            token,
        ))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("revision"), "{error}");
    let state = service.store().snapshot().await.unwrap();
    assert_eq!(state.tasks[0].status, agentix_task::TaskStatus::InProgress);
    assert_eq!(Some(&state.leases[0].token), options.lease_token.as_ref());
}

#[tokio::test]
async fn task_wait_fail_and_job_completion_notify_only_the_bound_session() {
    for (button, event_type, status) in [
        ("Wait", "task.waiting_user", "WAITING_USER"),
        ("Fail", "task.failed", "FAILED"),
        ("Done", "job.pending_review", "DONE"),
    ] {
        let (_dir, service, id) = task_fixture().await;
        let channel = Arc::new(FakeChannel::default());
        let engine = Engine::new(
            Arc::new(FakeAgent::new()),
            SqliteState::in_memory().await.unwrap(),
            vec![channel.clone()],
        )
        .with_task_board(service.clone());
        engine
            .handle_inbound(inbound("chat-a", "/attach thr_a"))
            .await
            .unwrap();
        engine
            .handle_inbound(inbound("chat-b", "/attach thr_b"))
            .await
            .unwrap();
        let token = task_button(&engine, &channel, &id, button).await;
        engine
            .handle_inbound(InboundEnvelope::action(
                "task-action",
                ConversationRef::new(ChannelKind::Telegram, "chat-a"),
                "owner",
                token,
            ))
            .await
            .unwrap();
        if button != "Done" {
            engine
                .handle_inbound(inbound("chat-a", "Specific acceptance reason"))
                .await
                .unwrap();
        }
        assert_eq!(
            service.store().snapshot().await.unwrap().tasks[0]
                .status
                .to_string(),
            status
        );
        refresh_and_deliver_tasks(&engine).await.unwrap();
        let messages: Vec<_> = channel
            .sent()
            .into_iter()
            .filter(|(_, v)| v.title == "Task update")
            .collect();
        assert_eq!(messages.len(), 1, "{event_type}");
        assert_eq!(messages[0].0.conversation_id, "chat-a");
        assert!(messages[0].1.body.contains(event_type));
        if button != "Done" {
            assert!(messages[0].1.body.contains("Specific acceptance reason"));
        }
        refresh_and_deliver_tasks(&engine).await.unwrap();
        assert_eq!(
            channel
                .sent()
                .iter()
                .filter(|(_, v)| v.title == "Task update")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn task_refresh_persists_notifications_without_waiting_for_im_delivery() {
    let (_dir, service, id) = task_fixture().await;
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        state.clone(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone());
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    service
        .execute(
            serde_json::json!({"command":"task.wait","task":id,"reason":"durable"}),
            task_write_options(&service, &id).await,
        )
        .await
        .unwrap();
    *channel.task_send_failures.lock().unwrap() = 1;
    engine.refresh_task_board().await.unwrap();
    assert!(
        !channel
            .sent()
            .iter()
            .any(|(_, view)| view.title == "Task update")
    );
    assert_eq!(
        state.notification_cursor("default", 0).await.unwrap(),
        service.store().latest_sequence().await.unwrap()
    );
    let deliveries = engine.claim_task_notifications(10).await.unwrap();
    assert_eq!(deliveries.len(), 1);
    assert!(
        engine
            .deliver_task_notification(deliveries.into_iter().next().unwrap())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn task_notification_send_failure_retries_after_restart_and_persists_cursor() {
    let (dir, service, id) = task_fixture().await;
    let path = dir.path().join("runtime.sqlite3");
    let channel = Arc::new(FakeChannel::default());
    let runtime_state = SqliteState::open(&path).await.unwrap();
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        runtime_state.clone(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone())
    .with_task_consumer("restart-test".into());
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-b", "/attach thr_b"))
        .await
        .unwrap();
    refresh_and_deliver_tasks(&engine).await.unwrap();
    service
        .execute(
            serde_json::json!({"command":"task.wait","task":id,"reason":"Notify after restart"}),
            task_write_options(&service, &id).await,
        )
        .await
        .unwrap();
    *channel.task_send_failures.lock().unwrap() = 1;
    assert!(refresh_and_deliver_tasks(&engine).await.is_err());
    assert!(!channel.sent().iter().any(|(_, v)| v.title == "Task update"));
    let cursor = runtime_state
        .notification_cursor("restart-test", 0)
        .await
        .unwrap();
    assert_eq!(cursor, service.store().latest_sequence().await.unwrap());
    drop(engine);
    let reopened = Arc::new(
        agentix_task::Service::open(service.config().clone())
            .await
            .unwrap(),
    );
    let channel = Arc::new(FakeChannel::default());
    let runtime_state = SqliteState::open(&path).await.unwrap();
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        runtime_state.clone(),
        vec![channel.clone()],
    )
    .with_task_board(reopened.clone())
    .with_task_consumer("restart-test".into());
    engine.restore_bindings().await.unwrap();
    let retry_at = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
        + 2;
    for notification in runtime_state
        .claim_notifications("restart-test", retry_at, 60, 32)
        .await
        .unwrap()
    {
        engine
            .deliver_task_notification(notification)
            .await
            .unwrap();
    }
    refresh_and_deliver_tasks(&engine).await.unwrap();
    let sent: Vec<_> = channel
        .sent()
        .into_iter()
        .filter(|(_, v)| v.title == "Task update")
        .collect();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0.conversation_id, "chat-a");
    assert!(sent[0].1.body.contains("Notify after restart"));
    drop(engine);
    let channel = Arc::new(FakeChannel::default());
    let runtime_state = SqliteState::open(&path).await.unwrap();
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        runtime_state.clone(),
        vec![channel.clone()],
    )
    .with_task_board(reopened)
    .with_task_consumer("restart-test".into());
    engine.restore_bindings().await.unwrap();
    refresh_and_deliver_tasks(&engine).await.unwrap();
    assert!(!channel.sent().iter().any(|(_, v)| v.title == "Task update"));
}

#[tokio::test]
async fn task_notifications_cross_event_pages_without_skipping_the_final_event() {
    let (_dir, service, id) = task_fixture().await;
    let channel = Arc::new(FakeChannel::default());
    let runtime_state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        runtime_state.clone(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone());
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    let options = task_write_options(&service, &id).await;
    for index in 0..105 {
        service.store().execute(serde_json::json!({"command":"task.update","task":id,"title":format!("Revision {index}")}),options.clone()).await.unwrap();
    }
    service
        .store()
        .execute(
            serde_json::json!({"command":"task.fail","task":id,"reason":"Last event"}),
            options,
        )
        .await
        .unwrap();
    for _ in 0..4 {
        refresh_and_deliver_tasks(&engine).await.unwrap();
    }
    let sent: Vec<_> = channel
        .sent()
        .into_iter()
        .filter(|(_, v)| v.title == "Task update")
        .collect();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].1.body.contains("Last event"));
    assert_eq!(
        runtime_state
            .notification_cursor("default", 0)
            .await
            .unwrap(),
        service.store().latest_sequence().await.unwrap()
    );
}

#[tokio::test]
async fn cancel_task_reason_does_not_change_task_and_unbound_events_are_skipped() {
    let (_dir, service, id) = task_fixture().await;
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone());
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    let token = task_button(&engine, &channel, &id, "Wait").await;
    engine
        .handle_inbound(InboundEnvelope::action(
            "ask-reason",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner",
            token,
        ))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "/cancel"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "Ordinary prompt, not a task reason"))
        .await
        .unwrap();
    assert_eq!(
        service.store().snapshot().await.unwrap().tasks[0].status,
        agentix_task::TaskStatus::InProgress
    );
    engine
        .handle_inbound(inbound("chat-a", "/detach"))
        .await
        .unwrap();
    service
        .execute(
            serde_json::json!({"command":"task.wait","task":id,"reason":"No bound conversation"}),
            task_write_options(&service, &id).await,
        )
        .await
        .unwrap();
    refresh_and_deliver_tasks(&engine).await.unwrap();
    assert!(!channel.sent().iter().any(|(_, v)| v.title == "Task update"));
}

fn user_input_request(request_id: &str, questions: &serde_json::Value) -> InteractionRequest {
    InteractionRequest {
        rpc_id: serde_json::json!(request_id),
        method: "item/tool/requestUserInput".into(),
        session_id: "thr_a".into(),
        turn_id: "turn-plan".into(),
        item_id: Some("item-plan".into()),
        kind: InteractionKind::UserInput,
        title: "Codex needs input".into(),
        detail: String::new(),
        available_decisions: Vec::new(),
        payload: serde_json::json!({"questions": questions}),
        auto_resolution_ms: None,
    }
}

async fn click_action(engine: &Engine, event_id: &str, token: String) {
    engine
        .handle_inbound(InboundEnvelope::action(
            event_id,
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner-42",
            token,
        ))
        .await
        .unwrap();
}

fn multiplexer_snapshot() -> MultiplexerSnapshot {
    MultiplexerSnapshot {
        sessions: vec![MultiplexerSession {
            id: "$1".into(),
            name: "agentix".into(),
            windows: vec![MultiplexerWindow {
                id: "@1".into(),
                index: "0".into(),
                name: "codex:agentix".into(),
                panes: vec![
                    MultiplexerPane {
                        id: "%1".into(),
                        index: "0".into(),
                        active: false,
                        current_command: "codex".into(),
                        cwd: "/work/parser".into(),
                        agent_session: Some(SessionId::new("thr_a")),
                    },
                    MultiplexerPane {
                        id: "%2".into(),
                        index: "1".into(),
                        active: true,
                        current_command: "fish".into(),
                        cwd: "/work/parser".into(),
                        agent_session: None,
                    },
                    MultiplexerPane {
                        id: "%3".into(),
                        index: "2".into(),
                        active: false,
                        current_command: "cargo".into(),
                        cwd: "/work/parser".into(),
                        agent_session: None,
                    },
                ],
            }],
        }],
    }
}

#[tokio::test]
async fn session_picker_uses_titles_instead_of_ids() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound("chat-a", "/sessions"))
        .await
        .unwrap();

    let view = channel.sent().last().unwrap().1.clone();
    assert_eq!(
        view.body,
        "> **1 · Parser cleanup**\n> 🟡 **Status:** Idle\n> 📁 **Workspace:** `/work/parser`\n\n> **2 · Daemon startup**\n> 🟡 **Status:** Idle\n> 📁 **Workspace:** `/work/daemon`"
    );
    assert!(!view.body.contains("thr_a"));
    assert!(!view.body.contains("thr_b"));
    assert_eq!(
        view.actions
            .iter()
            .map(|action| action.label.as_str())
            .collect::<Vec<_>>(),
        vec!["1 · Parser cleanup", "2 · Daemon startup"]
    );
    assert!(
        view.actions
            .iter()
            .all(|action| action.style == ActionStyle::Default)
    );

    engine
        .handle_inbound(InboundEnvelope::action_from_message(
            "attach-by-title",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner",
            view.actions[0].token.clone(),
            MessageRef::new(ConversationRef::new(ChannelKind::Telegram, "chat-a"), "m1"),
        ))
        .await
        .unwrap();
    assert!(agent.calls().contains(&"attach:thr_a".to_string()));
    assert_eq!(
        channel.disabled_actions(),
        vec![MessageRef::new(
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "m1"
        )]
    );
}

#[tokio::test]
async fn session_picker_marks_and_omits_the_current_session_from_actions() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "/sessions"))
        .await
        .unwrap();

    let view = channel.sent().last().unwrap().1.clone();
    assert!(
        view.body
            .contains("**1 · Parser cleanup** · 📎 **Attached**")
    );
    assert!(view.body.contains("**2 · Daemon startup**"));
    assert_eq!(
        view.actions
            .iter()
            .map(|action| action.label.as_str())
            .collect::<Vec<_>>(),
        vec!["2 · Daemon startup"]
    );
}

#[tokio::test]
async fn multiplexer_browser_auto_selects_one_backend_and_navigates_to_panes() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-a");

    engine
        .handle_inbound(inbound("chat-a", "/rmux"))
        .await
        .unwrap();
    let root = channel.sent().last().unwrap().1.clone();
    assert_eq!(root.title, "Terminal · rmux");
    assert!(!root.body.contains("tmux"));
    assert!(agent.calls().contains(&"mux-snapshot:auto".into()));

    let session_token = root
        .actions
        .iter()
        .find(|action| action.label == "agentix")
        .unwrap()
        .token
        .clone();
    engine
        .handle_inbound(InboundEnvelope::action(
            "open-mux-session",
            conversation.clone(),
            "owner",
            session_token,
        ))
        .await
        .unwrap();
    let session = channel.sent().last().unwrap().1.clone();
    assert_eq!(session.title, "rmux · agentix");
    assert!(session.body.contains("**0 · codex:agentix**"));

    let window_token = session
        .actions
        .iter()
        .find(|action| action.label == "0 · codex:agentix")
        .unwrap()
        .token
        .clone();
    engine
        .handle_inbound(InboundEnvelope::action(
            "open-mux-window",
            conversation.clone(),
            "owner",
            window_token,
        ))
        .await
        .unwrap();
    let window = channel.sent().last().unwrap().1.clone();
    assert_eq!(window.title, "rmux · agentix · 0 (codex:agentix)");
    assert!(window.body.contains("**0 · codex** · Codex"));
    assert!(window.body.contains("**1 · fish** · Active · Idle shell"));
    assert!(window.body.contains("**2 · cargo** · Busy"));
    let labels = window
        .actions
        .iter()
        .map(|action| action.label.as_str())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"0 · Attach"));
    assert!(labels.contains(&"1 · Run Codex"));
    assert!(!labels.iter().any(|label| label.starts_with("2 ·")));
    assert!(labels.contains(&"Split ↔ + Codex"));
    assert!(labels.contains(&"Split ↕ + Codex"));
    assert!(!labels.iter().any(|label| label.contains("Shell")));

    let split_token = window
        .actions
        .iter()
        .find(|action| action.label == "Split ↔ + Codex")
        .unwrap()
        .token
        .clone();
    engine
        .handle_inbound(InboundEnvelope::action(
            "split-active-mux-pane",
            conversation,
            "owner",
            split_token,
        ))
        .await
        .unwrap();
    assert!(agent.calls().iter().any(|call| {
        call.contains("SplitPane")
            && call.contains("%2")
            && call.contains("cwd: \"/work/multiplexer\"")
            && call.contains("launch_agent: true")
    }));
}

#[tokio::test]
async fn multiplexer_creation_can_start_codex_and_attach_the_new_thread() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-a");

    engine
        .handle_inbound(inbound("chat-a", "/rmux"))
        .await
        .unwrap();
    let root = channel.sent().last().unwrap().1.clone();
    let new_session_token = root
        .actions
        .iter()
        .find(|action| action.label == "+ Session")
        .unwrap()
        .token
        .clone();
    engine
        .handle_inbound(InboundEnvelope::action(
            "new-mux-session",
            conversation,
            "owner",
            new_session_token,
        ))
        .await
        .unwrap();

    let result = channel.sent().last().unwrap().1.clone();
    assert_eq!(
        result.subtitle.as_deref(),
        Some("Attached · Untitled · thr_mux_")
    );
    assert!(result.body.contains("Codex started"));
    assert!(agent.calls().iter().any(|call| {
        call.contains("NewSession")
            && call.contains("name: \"codex\"")
            && call.contains("cwd: \"/work/multiplexer\"")
            && call.contains("launch_agent: true")
    }));
}

#[tokio::test]
async fn multiplexer_window_creation_uses_defaults_and_starts_codex_immediately() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-a");

    engine
        .handle_inbound(inbound("chat-a", "/rmux"))
        .await
        .unwrap();
    let session_token = channel
        .sent()
        .last()
        .unwrap()
        .1
        .actions
        .iter()
        .find(|action| action.label == "agentix")
        .unwrap()
        .token
        .clone();
    engine
        .handle_inbound(InboundEnvelope::action(
            "open-mux-session",
            conversation.clone(),
            "owner",
            session_token,
        ))
        .await
        .unwrap();
    let new_window_token = channel
        .sent()
        .last()
        .unwrap()
        .1
        .actions
        .iter()
        .find(|action| action.label == "+ Window")
        .unwrap()
        .token
        .clone();
    engine
        .handle_inbound(InboundEnvelope::action(
            "new-mux-window",
            conversation,
            "owner",
            new_window_token,
        ))
        .await
        .unwrap();

    assert!(agent.calls().iter().any(|call| {
        call.contains("NewWindow")
            && call.contains("session_id: \"$1\"")
            && call.contains("name: \"codex\"")
            && call.contains("cwd: \"/work/multiplexer\"")
            && call.contains("launch_agent: true")
    }));
    assert_eq!(
        channel.sent().last().unwrap().1.subtitle.as_deref(),
        Some("Attached · Untitled · thr_mux_")
    );
}

#[tokio::test]
async fn session_picker_abbreviates_the_home_workspace() {
    let home = std::env::var("HOME").expect("the test user should have HOME set");
    let agent = Arc::new(FakeAgent::with_sessions(vec![SessionSummary {
        id: SessionId::new("thr_home"),
        name: Some("Agentix".into()),
        preview: None,
        cwd: Some(format!("{home}/src/agentix")),
        updated_at: Some(1),
        status: SessionStatus::Active,
        terminal: None,
    }]));
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent, state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound("chat-a", "/sessions"))
        .await
        .unwrap();

    let view = channel.sent().last().unwrap().1.clone();
    assert!(view.body.contains("📁 **Workspace:** `~/src/agentix`"));
    assert!(!view.body.contains(&home));
}

#[tokio::test]
async fn session_picker_shows_terminal_multiplexer_location() {
    let agent = Arc::new(FakeAgent::with_sessions(vec![SessionSummary {
        id: SessionId::new("thr_rmux"),
        name: Some("Agentix".into()),
        preview: None,
        cwd: Some("/work/agentix".into()),
        updated_at: Some(1),
        status: SessionStatus::Active,
        terminal: Some(TerminalLocation {
            session: "agentix".into(),
            window_index: "1".into(),
            window_name: "codex:agentix".into(),
            pane_index: "0".into(),
            pane_id: "%36".into(),
        }),
    }]));
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent, state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound("chat-a", "/sessions"))
        .await
        .unwrap();

    let sent = channel.sent();
    let body = &sent.last().unwrap().1.body;
    assert!(body.contains("🖥️ **rmux** · `agentix` · `1` (`codex:agentix`) · `0`"));
    assert!(!body.contains("Terminal:"));
    assert!(!body.contains("Session:"));
    assert!(!body.contains("Window:"));
    assert!(!body.contains("Pane:"));
    assert!(!body.contains("%36"));
}

#[tokio::test]
async fn attach_hydrates_one_turn_then_starts_or_steers() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();

    let calls = agent.calls();
    assert!(calls.contains(&"attach:thr_a".to_string()));
    assert!(calls.contains(&"history:thr_a:1".to_string()));
    assert!(
        channel
            .sent()
            .last()
            .unwrap()
            .1
            .body
            .contains("previous answer")
    );
    assert!(
        !channel
            .sent()
            .last()
            .unwrap()
            .1
            .body
            .contains("cargo test --workspace")
    );

    engine
        .handle_inbound(inbound("chat-a", "first prompt"))
        .await
        .unwrap();
    assert!(
        agent
            .calls()
            .contains(&"start:thr_a:first prompt".to_string())
    );

    engine
        .handle_agent_event(AgentEvent::TurnStarted {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
        })
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "change focus"))
        .await
        .unwrap();
    assert!(
        agent
            .calls()
            .contains(&"steer:thr_a:turn_live:change focus".to_string())
    );
}

#[tokio::test]
async fn failed_inbound_events_can_be_retried_with_the_same_event_id() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel]);
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-a");

    engine
        .handle_inbound(InboundEnvelope::text(
            "attach-before-retry",
            conversation.clone(),
            "owner",
            "/attach thr_a",
        ))
        .await
        .unwrap();
    agent.fail_next_start();
    let retryable =
        || InboundEnvelope::text("retryable-event", conversation.clone(), "owner", "retry me");

    assert!(engine.handle_inbound(retryable()).await.is_err());
    engine.handle_inbound(retryable()).await.unwrap();

    assert_eq!(
        agent
            .calls()
            .iter()
            .filter(|call| call.as_str() == "start:thr_a:retry me")
            .count(),
        2
    );
}

#[tokio::test]
async fn binding_commit_is_not_rolled_back_by_a_menu_side_effect_failure() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    channel.fail_menu_updates();
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel]);

    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "still attached"))
        .await
        .unwrap();

    assert!(
        agent
            .calls()
            .contains(&"start:thr_a:still attached".to_string())
    );
}

#[tokio::test]
async fn attaching_the_current_session_only_sends_a_short_notice() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-a");

    engine
        .handle_inbound(InboundEnvelope::text(
            "attach-first",
            conversation.clone(),
            "owner",
            "/attach thr_a",
        ))
        .await
        .unwrap();
    let calls_after_first_attach = agent.calls();

    engine
        .handle_inbound(InboundEnvelope::text(
            "attach-again",
            conversation,
            "owner",
            "/attach thr_a",
        ))
        .await
        .unwrap();

    assert_eq!(agent.calls(), calls_after_first_attach);
    let sent = channel.sent();
    let notice = &sent.last().unwrap().1;
    assert_eq!(notice.body, "This session is already attached.");
    assert!(!notice.body.contains("previous answer"));
}

#[tokio::test]
async fn attach_uses_the_durable_epoch_after_restart_without_a_saved_binding() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-a");
    for index in 0..5 {
        state
            .attach(
                &conversation,
                &SessionId::new(format!("thr_previous_{index}")),
            )
            .await
            .unwrap();
        state.detach(&conversation).await.unwrap();
    }
    let engine = Engine::new(agent, state.clone(), vec![channel]);

    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();

    assert_eq!(
        state.current_session(&conversation).await.unwrap(),
        Some(SessionId::new("thr_a"))
    );
}

#[tokio::test]
async fn attached_session_commands_are_gated_and_update_the_channel_menu() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound("chat-a", "/status"))
        .await
        .unwrap();
    assert!(
        !agent
            .calls()
            .iter()
            .any(|call| call.starts_with("command:"))
    );
    assert!(
        channel
            .sent()
            .last()
            .unwrap()
            .1
            .body
            .contains("Attach a session")
    );

    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    assert!(channel.session_commands().last().unwrap().1);

    engine
        .handle_inbound(inbound_as("chat-a", "owner-2", "/status"))
        .await
        .unwrap();
    assert!(agent.calls().contains(&"command:thr_a:Status".to_string()));
    assert_eq!(channel.sent().last().unwrap().1.title, "Codex command");

    engine
        .handle_inbound(inbound("chat-a", "/detach"))
        .await
        .unwrap();
    assert!(!channel.session_commands().last().unwrap().1);
}

#[tokio::test]
async fn rename_without_an_argument_collects_the_next_im_message() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "/rename"))
        .await
        .unwrap();
    assert!(
        channel
            .sent()
            .last()
            .unwrap()
            .1
            .body
            .contains("Reply with the new session name")
    );
    engine
        .handle_inbound(inbound("chat-a", "Parser cleanup renamed"))
        .await
        .unwrap();

    assert!(
        agent
            .calls()
            .contains(&"command:thr_a:Rename(Some(\"Parser cleanup renamed\"))".to_string())
    );
    assert!(
        !agent
            .calls()
            .contains(&"command:thr_a:Rename(None)".to_string())
    );
}

#[tokio::test]
async fn help_reflects_whether_a_session_is_attached() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent, state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound_as("chat-a", "owner-2", "/help"))
        .await
        .unwrap();
    let detached = channel.sent().last().unwrap().1.clone();
    assert!(detached.body.contains("/sessions"));
    assert!(detached.body.contains("/attach <thread-id>"));
    assert!(!detached.body.contains("/model"));

    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "/help"))
        .await
        .unwrap();
    let attached = channel.sent().last().unwrap().1.clone();
    assert!(attached.body.contains("/current"));
    assert!(attached.body.contains("/model [id]"));
    assert!(attached.body.contains("/mcp"));
    for view in [&detached, &attached] {
        for command in ["/help", "/cancel", "/sessions", "/rmux", "/attach"] {
            assert!(
                view.body
                    .lines()
                    .any(|line| line.starts_with(&format!("**{command}")) && line.contains("** — ")),
                "missing description for {command}: {}",
                view.body
            );
        }
        for line in view.body.lines().filter(|line| line.starts_with("**/")) {
            let (usage, description) = line
                .split_once(" — ")
                .expect("each command needs a description");
            assert!(usage.starts_with("**/") && usage.ends_with("**"));
            assert!(!description.contains("**"));
            assert!(!description.trim().is_empty());
            assert!(!line.contains(" · /"));
        }
    }
}

#[tokio::test]
async fn unsupported_command_returns_detached_help_and_is_not_retried() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent, state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound("chat-a", "/does-not-exist"))
        .await
        .unwrap();

    let invalid = channel.sent().last().unwrap().1.clone();
    assert_eq!(invalid.title, "Invalid command");
    assert_eq!(invalid.status, agentix_core::ViewStatus::Warning);
    assert!(invalid.body.contains("unknown command: /does-not-exist"));
    assert!(invalid.body.contains("/sessions"));
    assert!(invalid.body.contains("/attach <thread-id>"));
    assert!(!invalid.body.contains("/model"));
    let sent = channel.sent().len();
    engine
        .handle_inbound(inbound("chat-a", "/does-not-exist"))
        .await
        .unwrap();
    assert_eq!(channel.sent().len(), sent);
}

#[tokio::test]
async fn unsupported_command_returns_attached_help_without_switching_the_attachment() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "/new"))
        .await
        .unwrap();

    let invalid = channel.sent().last().unwrap().1.clone();
    assert_eq!(invalid.title, "Invalid command");
    assert_eq!(invalid.status, agentix_core::ViewStatus::Warning);
    assert!(invalid.body.contains("unknown command: /new"));
    assert!(invalid.body.contains("/current"));
    assert!(invalid.body.contains("/model [id]"));
    assert!(invalid.body.contains("/mcp"));
    engine
        .handle_inbound(inbound("chat-a", "/status"))
        .await
        .unwrap();

    let calls = agent.calls();
    assert!(!calls.iter().any(|call| call.contains(":New")));
    assert!(calls.contains(&"command:thr_a:Status".to_string()));
}

#[tokio::test]
async fn model_and_reasoning_choices_can_be_selected_for_the_attached_session() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/model"))
        .await
        .unwrap();

    let model = channel.sent().last().unwrap().1.clone();
    assert_eq!(model.subtitle.as_deref(), Some("Parser cleanup · thr_a"));
    assert_eq!(
        model
            .actions
            .iter()
            .map(|action| action.label.as_str())
            .collect::<Vec<_>>(),
        ["GPT-5.6", "GPT-5.6 Terra"]
    );
    engine
        .handle_inbound(InboundEnvelope::action(
            "select-model",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner-42",
            model.actions[1].token.clone(),
        ))
        .await
        .unwrap();
    assert!(
        agent
            .calls()
            .contains(&"command:thr_a:Model(Some(\"gpt-5.6-terra\"))".to_string())
    );

    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/reasoning"))
        .await
        .unwrap();
    let reasoning = channel.sent().last().unwrap().1.clone();
    assert_eq!(
        reasoning
            .actions
            .iter()
            .map(|action| action.label.as_str())
            .collect::<Vec<_>>(),
        ["Medium", "High"]
    );
    engine
        .handle_inbound(InboundEnvelope::action(
            "select-reasoning",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner-42",
            reasoning.actions[1].token.clone(),
        ))
        .await
        .unwrap();
    assert!(
        agent
            .calls()
            .contains(&"command:thr_a:Reasoning(Some(\"high\"))".to_string())
    );
}

#[tokio::test]
async fn running_codex_turn_queues_follow_ups_and_exposes_them_in_im() {
    let agent = Arc::new(FakeAgent::with_queue_support());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::TurnStarted {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
        })
        .await
        .unwrap();

    engine
        .handle_inbound(inbound("chat-a", "first follow-up"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "second follow-up"))
        .await
        .unwrap();

    let calls = agent.calls();
    assert!(calls.contains(&"queue:thr_a:first follow-up".to_string()));
    assert!(calls.contains(&"queue:thr_a:second follow-up".to_string()));
    assert!(!calls.iter().any(|call| call.starts_with("steer:")));
    let confirmation = channel.sent().last().unwrap().1.clone();
    assert_eq!(confirmation.title, "Codex · Queued");
    assert_eq!(confirmation.subtitle.as_deref(), Some("Position #2"));
    assert!(confirmation.body.contains("> second follow-up"));

    engine
        .handle_inbound(inbound("chat-a", "/queue"))
        .await
        .unwrap();

    let queue = channel.sent().last().unwrap().1.clone();
    assert_eq!(queue.title, "Codex · Parser cleanup · thr_a");
    assert_eq!(queue.subtitle.as_deref(), Some("Queue · 2 messages"));
    assert!(queue.body.contains("> **1**\n> first follow-up"));
    assert!(queue.body.contains("> **2**\n> second follow-up"));
}

#[tokio::test]
async fn attach_hydrates_the_latest_running_turn_with_a_stop_action() {
    let agent = Arc::new(FakeAgent::with_history(vec![TurnSummary {
        id: "turn_running".into(),
        status: TurnStatus::InProgress,
        user_text: Some("keep working".into()),
        agent_text: Some("still running".into()),
        tools: Vec::new(),
        items: Vec::new(),
    }]));
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_a"))
        .await
        .unwrap();

    let running = channel.sent().last().unwrap().1.clone();
    assert_eq!(running.actions.len(), 1);
    assert_eq!(running.actions[0].label, "Stop");

    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "new direction"))
        .await
        .unwrap();
    assert!(
        agent
            .calls()
            .contains(&"steer:thr_a:turn_running:new direction".to_string())
    );

    engine
        .handle_inbound(InboundEnvelope::action(
            "stop-hydrated-turn",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner-42",
            running.actions[0].token.clone(),
        ))
        .await
        .unwrap();
    assert!(
        agent
            .calls()
            .contains(&"stop:thr_a:turn_running".to_string())
    );

    engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_a".into(),
            turn_id: "turn_running".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await
        .unwrap();
    assert_eq!(channel.updated().len(), 1);
}

fn visible_stop_messages(channel: &FakeChannel) -> Vec<MessageRef> {
    channel
        .messages
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, view)| view.actions.iter().any(|action| action.label == "Stop"))
        .map(|(message, _)| message.clone())
        .collect()
}

#[tokio::test]
async fn attach_stop_button_moves_to_only_the_latest_attached_running_turn() {
    for kind in [
        ChannelKind::Telegram,
        ChannelKind::Feishu,
        ChannelKind::Slack,
    ] {
        let agent = Arc::new(FakeAgent::with_history(vec![TurnSummary {
            id: "turn_running".into(),
            status: TurnStatus::InProgress,
            user_text: Some("keep working".into()),
            agent_text: Some("still running".into()),
            tools: Vec::new(),
            items: Vec::new(),
        }]));
        let channel = Arc::new(FakeChannel {
            channel_kind: Some(kind),
            reject_unchanged_updates: true,
            streaming_interval: Some(std::time::Duration::ZERO),
            ..FakeChannel::default()
        });
        let engine = Engine::new(
            agent.clone(),
            SqliteState::in_memory().await.unwrap(),
            vec![channel.clone()],
        );
        let chat = ConversationRef::new(kind, "chat-a");
        let other_chat = ConversationRef::new(kind, "chat-b");
        let commands = [
            (&chat, "/attach thr_a", true),
            (&chat, "/history", true),
            (&chat, "/attach thr_b", true),
            (&chat, "/attach thr_a", true),
            (&other_chat, "/attach thr_a", true),
            (&other_chat, "/detach", false),
            (&chat, "/attach thr_a", true),
            (&chat, "/attach thr_b", false),
        ];
        for (index, (conversation, command, running)) in commands.into_iter().enumerate() {
            if index == 7 {
                agent.history_turns.lock().unwrap()[0].status = TurnStatus::Completed;
            }
            let old_token = visible_stop_messages(&channel).first().map(|message| {
                (
                    message.conversation.clone(),
                    channel.messages.lock().unwrap()[message].actions[0]
                        .token
                        .clone(),
                )
            });
            engine
                .handle_inbound(InboundEnvelope::text(
                    format!("command-{index}"),
                    conversation.clone(),
                    "owner",
                    command,
                ))
                .await
                .unwrap();
            let visible = visible_stop_messages(&channel);
            assert_eq!(visible.len(), usize::from(running), "{kind:?}: {command}");
            if running {
                assert_eq!(&visible[0].conversation, conversation);
            }
            if command != "/history"
                && let Some((old_conversation, token)) = old_token
            {
                let result = engine
                    .handle_inbound(InboundEnvelope::action(
                        format!("stale-stop-{index}"),
                        old_conversation,
                        "owner",
                        token,
                    ))
                    .await;
                assert!(matches!(result, Err(EngineError::InvalidAction)));
            }
        }
        engine
            .handle_agent_event(AgentEvent::AgentMessageDelta {
                session_id: "thr_b".into(),
                turn_id: "turn_running".into(),
                item_id: "late-item".into(),
                delta: "late output".into(),
            })
            .await
            .unwrap();
        assert!(visible_stop_messages(&channel).is_empty());
        assert!(!agent.calls().iter().any(|call| call.starts_with("stop:")));
    }
}

#[tokio::test]
async fn attach_stop_button_does_not_return_on_superseded_turn_updates() {
    let agent = Arc::new(FakeAgent::with_history(vec![TurnSummary {
        id: "turn_old".into(),
        status: TurnStatus::InProgress,
        user_text: Some("old prompt".into()),
        agent_text: Some("old response".into()),
        tools: Vec::new(),
        items: Vec::new(),
    }]));
    let channel = Arc::new(FakeChannel {
        streaming_interval: Some(std::time::Duration::ZERO),
        ..FakeChannel::default()
    });
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::TurnStarted {
            session_id: "thr_a".into(),
            turn_id: "turn_new".into(),
        })
        .await
        .unwrap();
    assert!(visible_stop_messages(&channel).is_empty());
    for turn in ["turn_new", "turn_old"] {
        engine
            .handle_agent_event(AgentEvent::AgentMessageDelta {
                session_id: "thr_a".into(),
                turn_id: turn.into(),
                item_id: format!("item-{turn}"),
                delta: "more output".into(),
            })
            .await
            .unwrap();
        assert_eq!(visible_stop_messages(&channel).len(), 1);
    }
    engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_a".into(),
            turn_id: "turn_old".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "follow up"))
        .await
        .unwrap();
    assert!(
        agent
            .calls()
            .contains(&"steer:thr_a:turn_new:follow up".into())
    );
    engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_a".into(),
            turn_id: "turn_new".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await
        .unwrap();
    assert!(visible_stop_messages(&channel).is_empty());
}

#[tokio::test]
async fn late_output_cannot_restore_stop_on_an_old_turn_after_the_latest_turn_finishes() {
    let channel = Arc::new(FakeChannel {
        streaming_interval: Some(std::time::Duration::ZERO),
        ..FakeChannel::default()
    });
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "start work"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::TurnStarted {
            session_id: "thr_a".into(),
            turn_id: "turn_latest".into(),
        })
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_a".into(),
            turn_id: "turn_latest".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "turn_new".into(),
            item_id: "late-item".into(),
            delta: "late output".into(),
        })
        .await
        .unwrap();
    assert!(visible_stop_messages(&channel).is_empty());
}

#[tokio::test]
async fn attach_and_history_render_one_message_per_turn_with_distinct_speakers() {
    let agent = Arc::new(FakeAgent::with_history(vec![
        TurnSummary {
            id: "turn_first_long_identifier".into(),
            status: TurnStatus::Completed,
            user_text: Some("first question\nwith context".into()),
            agent_text: Some("First **answer**".into()),
            tools: Vec::new(),
            items: Vec::new(),
        },
        TurnSummary {
            id: "turn_second_long_identifier".into(),
            status: TurnStatus::InProgress,
            user_text: Some("second question".into()),
            agent_text: Some("Second answer".into()),
            tools: Vec::new(),
            items: Vec::new(),
        },
    ]));
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();

    let attached = channel.sent();
    assert_eq!(agent.history_cursors(), vec![None]);
    assert!(agent.calls().contains(&"history:thr_a:1".to_string()));
    assert_eq!(attached.len(), 2);
    assert_eq!(attached[0].1.subtitle.as_deref(), Some("Attached"));
    assert_eq!(attached[1].1.title, "Codex · Parser cleanup · thr_a");
    assert_eq!(
        attached[1].1.subtitle.as_deref(),
        Some("Turn turn_sec · Working 0s")
    );
    assert_eq!(attached[1].1.actions.len(), 1);
    assert_eq!(attached[1].1.actions[0].label, "Stop");

    engine
        .handle_inbound(inbound("chat-a", "/history"))
        .await
        .unwrap();

    let sent = channel.sent();
    let history = &sent[2..];
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].1.subtitle.as_deref(), Some("History"));
    assert!(history[1].1.body.contains("first question"));
    assert_eq!(history[2].1.body, attached[1].1.body);
    assert!(history[2].1.actions.is_empty());
}

#[tokio::test]
async fn current_detach_and_lifecycle_messages_show_the_session_title_and_id() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent, state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "/current"))
        .await
        .unwrap();
    assert_eq!(
        channel.sent().last().unwrap().1.title,
        "Codex · Parser cleanup · thr_a"
    );

    engine
        .handle_inbound(inbound("chat-a", "/detach"))
        .await
        .unwrap();
    assert!(
        channel
            .sent()
            .last()
            .unwrap()
            .1
            .body
            .contains("Parser cleanup · thr_a")
    );
}

#[tokio::test]
async fn tool_events_do_not_render_or_appear_in_live_turns() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent, state, vec![channel.clone()]);
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    let before = channel.sent().len();

    engine
        .handle_agent_event(AgentEvent::ItemStarted {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
            item_id: "tool_a".into(),
            kind: "commandExecution".into(),
            label: "cargo test --workspace".into(),
        })
        .await
        .unwrap();
    assert_eq!(channel.sent().len(), before);

    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
            item_id: "message_a".into(),
            delta: "Tests passed.".into(),
        })
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::ItemCompleted {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
            item: ItemSummary {
                id: "tool_a".into(),
                kind: "commandExecution".into(),
                text: Some("verbose command output".into()),
                status: Some("completed".into()),
            },
        })
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await
        .unwrap();

    let sent = channel.sent();
    assert!(sent.last().unwrap().1.body.contains("Tests passed."));
    assert!(sent[before..].iter().all(|(_, view)| {
        !view.body.contains("cargo test --workspace")
            && !view.body.contains("verbose command output")
            && !view.body.contains("Tools")
    }));
}

#[tokio::test]
async fn live_turns_visually_separate_user_input_from_agent_output() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent, state, vec![channel.clone()]);
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();

    engine
        .handle_agent_event(AgentEvent::ItemCompleted {
            session_id: "thr_a".into(),
            turn_id: "turn_live_long_identifier".into(),
            item: ItemSummary {
                id: "user_a".into(),
                kind: "userMessage".into(),
                text: Some("resume\nwith context".into()),
                status: Some("completed".into()),
            },
        })
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "turn_live_long_identifier".into(),
            item_id: "agent_a".into(),
            delta: "Agent **answer**".into(),
        })
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_a".into(),
            turn_id: "turn_live_long_identifier".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await
        .unwrap();

    let view = channel.sent().last().unwrap().1.clone();
    assert_eq!(
        view.subtitle.as_deref(),
        Some("Turn turn_liv · Completed in 0s")
    );
    assert_eq!(
        view.body,
        "**👤 You**\n\n> resume\n> with context\n\n**🤖 Codex**\n\n> Agent **answer**"
    );
    assert!(!view.body.contains("You: "));
    assert!(
        !view
            .subtitle
            .as_deref()
            .unwrap()
            .contains("long_identifier")
    );
}

#[tokio::test]
async fn simultaneous_session_streams_route_by_thread_id() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent, state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-b", "/attach thr_b"))
        .await
        .unwrap();
    let before = channel.sent().len();

    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_b".into(),
            turn_id: "turn_b".into(),
            item_id: "item_b".into(),
            delta: "only for B".into(),
        })
        .await
        .unwrap();

    let sent = channel.sent();
    assert_eq!(sent.len(), before + 1);
    assert_eq!(sent.last().unwrap().0.conversation_id, "chat-b");
    assert!(sent.last().unwrap().1.title.contains("thr_b"));
    assert!(sent.last().unwrap().1.body.contains("only for B"));
}

#[tokio::test]
async fn live_turn_has_owner_bound_stop_action() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
            item_id: "item_a".into(),
            delta: "working".into(),
        })
        .await
        .unwrap();

    let action = channel.sent().last().unwrap().1.actions[0].clone();
    engine
        .handle_inbound(InboundEnvelope::action(
            "callback-1",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner-42",
            action.token,
        ))
        .await
        .unwrap();

    assert!(agent.calls().contains(&"stop:thr_a:turn_live".to_string()));
}

#[tokio::test]
async fn interaction_action_is_bound_to_the_conversation_owner() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent, state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::InteractionRequested(InteractionRequest {
            rpc_id: serde_json::json!("rpc-1"),
            method: "item/commandExecution/requestApproval".into(),
            session_id: "thr_a".into(),
            turn_id: "turn-1".into(),
            item_id: Some("item-1".into()),
            kind: InteractionKind::CommandApproval,
            title: "Run command?".into(),
            detail: "cargo test".into(),
            available_decisions: vec!["accept".into(), "decline".into()],
            payload: serde_json::json!({}),
            auto_resolution_ms: None,
        }))
        .await
        .unwrap();

    let actions = channel.sent().last().unwrap().1.actions.clone();
    engine
        .handle_inbound(InboundEnvelope::action(
            "callback-2",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner-42",
            actions[0].token.clone(),
        ))
        .await
        .unwrap();
    let resolved = channel.updated().last().unwrap().1.clone();
    assert!(resolved.actions.is_empty());
    assert_eq!(resolved.status, agentix_core::ViewStatus::Success);
    assert!(resolved.body.contains("**Selected:** Allow once"));
    let sibling_error = engine
        .handle_inbound(InboundEnvelope::action(
            "callback-3",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner-42",
            actions[1].token.clone(),
        ))
        .await
        .unwrap_err();
    assert!(matches!(sibling_error, EngineError::InvalidAction));
}

#[tokio::test]
async fn external_approval_resolution_clears_buttons_without_inventing_a_decision() {
    for (kind, label) in [
        (ChannelKind::Telegram, "Telegram"),
        (ChannelKind::Feishu, "Feishu"),
        (ChannelKind::Slack, "Slack"),
    ] {
        let agent = Arc::new(FakeAgent::new());
        let channel = Arc::new(FakeChannel {
            channel_kind: Some(kind),
            ..FakeChannel::default()
        });
        let state = SqliteState::in_memory().await.unwrap();
        let engine = Engine::new(agent, state, vec![channel.clone()]);

        engine
            .handle_inbound(InboundEnvelope::text(
                "attach",
                ConversationRef::new(kind, "chat-a"),
                "owner-42",
                "/attach thr_a",
            ))
            .await
            .unwrap();
        engine
            .handle_agent_event(AgentEvent::InteractionRequested(InteractionRequest {
                rpc_id: serde_json::json!(91),
                method: "item/commandExecution/requestApproval".into(),
                session_id: "thr_a".into(),
                turn_id: "turn-1".into(),
                item_id: Some("item-1".into()),
                kind: InteractionKind::CommandApproval,
                title: "Command approval".into(),
                detail: "cargo test".into(),
                available_decisions: vec!["accept".into(), "decline".into()],
                payload: serde_json::json!({}),
                auto_resolution_ms: None,
            }))
            .await
            .unwrap();
        let old_token = channel.sent().last().unwrap().1.actions[0].token.clone();

        engine
            .handle_agent_event(AgentEvent::InteractionResolved {
                session_id: "thr_a".into(),
                request_id: "91".into(),
            })
            .await
            .unwrap();

        let resolved = channel.updated().last().unwrap().1.clone();
        assert!(resolved.actions.is_empty());
        assert_eq!(resolved.status, agentix_core::ViewStatus::Muted);
        assert!(
            resolved
                .body
                .contains(&format!("**Resolved:** Outside {label}"))
        );
        let error = engine
            .handle_inbound(InboundEnvelope::action(
                "stale-approval",
                ConversationRef::new(kind, "chat-a"),
                "owner-42",
                old_token,
            ))
            .await
            .unwrap_err();
        assert!(matches!(error, EngineError::InvalidAction));
    }
}

#[tokio::test]
async fn plan_questions_are_answered_one_at_a_time_with_buttons_and_other_text() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::InteractionRequested(user_input_request(
            "input-1",
            &serde_json::json!([
                {
                    "id": "approach",
                    "header": "Approach",
                    "question": "Which implementation approach?",
                    "options": [
                        {"label": "Fast", "description": "Make the smallest change."},
                        {"label": "Thorough", "description": "Cover every edge case."}
                    ]
                },
                {
                    "id": "details",
                    "header": "Details",
                    "question": "How should rollout work?",
                    "options": [
                        {"label": "Immediate", "description": "Enable it now."},
                        {"label": "Staged", "description": "Enable it gradually."}
                    ]
                }
            ]),
        )))
        .await
        .unwrap();

    let first = channel.sent().last().unwrap().1.clone();
    assert!(first.body.contains("**Question 1 of 2 · Approach**"));
    assert!(first.body.contains("Which implementation approach?"));
    assert_eq!(
        first
            .actions
            .iter()
            .map(|action| action.label.as_str())
            .collect::<Vec<_>>(),
        vec!["Fast", "Thorough", "Other…"]
    );
    let fast = first.actions[0].token.clone();
    click_action(&engine, "choose-fast", fast).await;

    let second = channel.updated().last().unwrap().1.clone();
    assert!(second.body.contains("1. **Approach:** Fast"));
    assert!(second.body.contains("**Question 2 of 2 · Details**"));
    let other = second
        .actions
        .iter()
        .find(|action| action.label == "Other…")
        .unwrap()
        .token
        .clone();
    click_action(&engine, "choose-other", other).await;
    let awaiting_text = channel.updated().last().unwrap().1.clone();
    assert!(awaiting_text.actions.is_empty());
    assert!(awaiting_text.body.contains("Reply with your custom answer"));

    engine
        .handle_inbound(inbound_as(
            "chat-a",
            "owner-42",
            "Use a staged rollout with a feature flag",
        ))
        .await
        .unwrap();

    let decisions = agent.interaction_decisions();
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].response,
        serde_json::json!({
            "answers": {
                "approach": {"answers": ["Fast"]},
                "details": {"answers": ["Use a staged rollout with a feature flag"]}
            }
        })
    );
    let completed = channel.updated().last().unwrap().1.clone();
    assert!(completed.actions.is_empty());
    assert_eq!(completed.status, agentix_core::ViewStatus::Success);
    assert!(
        completed
            .body
            .contains("2. **Details:** Use a staged rollout with a feature flag")
    );
}

#[tokio::test]
async fn cancel_leaves_custom_plan_input_and_restores_its_choices() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);

    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::InteractionRequested(user_input_request(
            "input-cancel",
            &serde_json::json!([{
                "id": "approach",
                "header": "Approach",
                "question": "Which approach?",
                "options": [{"label": "Fast", "description": "Small change."}]
            }]),
        )))
        .await
        .unwrap();
    let other = channel
        .sent()
        .last()
        .unwrap()
        .1
        .actions
        .iter()
        .find(|action| action.label == "Other…")
        .unwrap()
        .token
        .clone();
    click_action(&engine, "choose-other-before-cancel", other).await;

    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/cancel"))
        .await
        .unwrap();

    assert!(agent.interaction_decisions().is_empty());
    let restored = channel.updated().last().unwrap().1.clone();
    assert!(restored.actions.iter().any(|action| action.label == "Fast"));
    assert!(
        restored
            .actions
            .iter()
            .any(|action| action.label == "Other…")
    );
    assert_eq!(
        channel.sent().last().unwrap().1.body,
        "Pending reply cancelled."
    );
}

#[tokio::test]
async fn external_plan_resolution_clears_buttons_and_pending_text_reply() {
    for (kind, label) in [
        (ChannelKind::Telegram, "Telegram"),
        (ChannelKind::Feishu, "Feishu"),
        (ChannelKind::Slack, "Slack"),
    ] {
        let agent = Arc::new(FakeAgent::new());
        let channel = Arc::new(FakeChannel {
            channel_kind: Some(kind),
            ..FakeChannel::default()
        });
        let state = SqliteState::in_memory().await.unwrap();
        let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);

        engine
            .handle_inbound(InboundEnvelope::text(
                "attach",
                ConversationRef::new(kind, "chat-a"),
                "owner-42",
                "/attach thr_a",
            ))
            .await
            .unwrap();
        engine
            .handle_agent_event(AgentEvent::InteractionRequested(user_input_request(
                "input-external",
                &serde_json::json!([{
                    "id": "approach",
                    "header": "Approach",
                    "question": "Which approach?",
                    "options": [{"label": "Fast", "description": "Small change."}]
                }]),
            )))
            .await
            .unwrap();
        let initial = channel.sent().last().unwrap().1.clone();
        let other = initial
            .actions
            .iter()
            .find(|action| action.label == "Other…")
            .unwrap()
            .token
            .clone();
        engine
            .handle_inbound(InboundEnvelope::action(
                "choose-other",
                ConversationRef::new(kind, "chat-a"),
                "owner-42",
                other,
            ))
            .await
            .unwrap();

        engine
            .handle_agent_event(AgentEvent::InteractionResolved {
                session_id: "thr_a".into(),
                request_id: "input-external".into(),
            })
            .await
            .unwrap();
        let resolved = channel.updated().last().unwrap().1.clone();
        assert!(resolved.actions.is_empty());
        assert_eq!(resolved.status, agentix_core::ViewStatus::Muted);
        assert!(
            resolved
                .body
                .contains(&format!("**Resolved:** Outside {label}"))
        );

        engine
            .handle_inbound(InboundEnvelope::text(
                "prompt",
                ConversationRef::new(kind, "chat-a"),
                "owner-42",
                "ordinary prompt",
            ))
            .await
            .unwrap();
        assert!(
            agent
                .calls()
                .contains(&"start:thr_a:ordinary prompt".to_owned())
        );
        assert!(agent.interaction_decisions().is_empty());
    }
}

#[tokio::test]
async fn restore_preserves_offline_bindings_and_turns_until_reconnect() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-a");
    let engine = Engine::new(agent.clone(), state.clone(), vec![channel.clone()]);
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
            item_id: "item".into(),
            delta: "saved response".into(),
        })
        .await
        .unwrap();
    drop(engine);

    agent
        .unavailable_attachments
        .lock()
        .unwrap()
        .push(SessionId::new("thr_a"));
    let restarted = Engine::new(agent.clone(), state.clone(), vec![channel.clone()]);
    assert_eq!(restarted.restore_bindings().await.unwrap(), 1);
    assert_eq!(
        state.current_session(&conversation).await.unwrap(),
        Some(SessionId::new("thr_a"))
    );
    assert!(
        channel
            .sent()
            .iter()
            .any(|(_, view)| view.subtitle.as_deref() == Some("Online · Waiting for agent"))
    );
    assert!(!agent.calls().iter().any(|call| call.starts_with("start:")));

    agent.unavailable_attachments.lock().unwrap().clear();
    *agent.history_turns.lock().unwrap() = vec![TurnSummary {
        id: "turn_live".into(),
        status: TurnStatus::Completed,
        user_text: None,
        agent_text: Some("finished while disconnected".into()),
        tools: Vec::new(),
        items: Vec::new(),
    }];
    restarted
        .handle_agent_event(AgentEvent::SessionResumed {
            session_id: "thr_a".into(),
        })
        .await
        .unwrap();
    assert!(
        channel
            .updated()
            .last()
            .unwrap()
            .1
            .body
            .contains("finished while disconnected")
    );
    restarted
        .handle_inbound(inbound("chat-a", "continue"))
        .await
        .unwrap();
    assert!(agent.calls().contains(&"start:thr_a:continue".to_string()));
}

#[tokio::test]
async fn restore_reopens_persisted_agent_subscriptions() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-a");
    state
        .attach(&conversation, &SessionId::new("thr_a"))
        .await
        .unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);

    assert_eq!(engine.restore_bindings().await.unwrap(), 1);
    assert!(agent.calls().contains(&"attach:thr_a".to_string()));
    assert!(channel.session_commands().last().unwrap().1);
    let online = channel.sent().last().unwrap().1.clone();
    assert_eq!(online.title, "Agentix serve");
    assert_eq!(online.subtitle.as_deref(), Some("Online · Reattached"));
    assert!(online.body.contains("Codex session Parser cleanup · thr_a"));
    engine
        .handle_inbound(inbound("chat-a", "continue"))
        .await
        .unwrap();
    assert!(agent.calls().contains(&"start:thr_a:continue".to_string()));
}

#[tokio::test]
async fn startup_skips_command_cards_and_preserves_explicit_help() {
    for kind in [ChannelKind::Feishu, ChannelKind::Slack] {
        for (session, subtitle) in [
            ("thr_a", "Online · Reattached"),
            ("thr_offline", "Online · Waiting for agent"),
            ("thr_missing", "Online · Detached"),
        ] {
            let agent = Arc::new(FakeAgent::rejecting_attachment("thr_missing"));
            agent
                .unavailable_attachments
                .lock()
                .unwrap()
                .push(SessionId::new("thr_offline"));
            let channel = Arc::new(FakeChannel {
                channel_kind: Some(kind),
                ..FakeChannel::default()
            });
            let state = SqliteState::in_memory().await.unwrap();
            let conversation = ConversationRef::new(kind, "chat-a");
            state
                .attach(&conversation, &SessionId::new(session))
                .await
                .unwrap();
            let engine = Engine::new(agent, state, vec![channel.clone()]);

            engine.restore_bindings().await.unwrap();

            assert!(
                channel.session_commands().is_empty(),
                "startup requested a command card for {kind:?} / {session}"
            );
            let sent = channel.sent();
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].1.title, "Agentix serve");
            assert_eq!(sent[0].1.subtitle.as_deref(), Some(subtitle));
            engine
                .handle_inbound(InboundEnvelope::text(
                    "help",
                    conversation,
                    "owner",
                    "/help",
                ))
                .await
                .unwrap();
            assert_eq!(channel.sent().last().unwrap().1.title, "Agentix commands");
        }
    }
}

#[tokio::test]
async fn deferred_startup_notifications_skip_changed_bindings() {
    for stale_session in ["thr_a", "thr_missing"] {
        let agent = Arc::new(FakeAgent::rejecting_attachment("thr_missing"));
        let channel = Arc::new(FakeChannel::default());
        let state = SqliteState::in_memory().await.unwrap();
        let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-a");
        state
            .attach(&conversation, &SessionId::new(stale_session))
            .await
            .unwrap();
        let engine = Engine::new(agent, state, vec![channel.clone()]);
        let updates = engine.restore_bindings_deferred().await.unwrap();
        assert!(channel.sent().is_empty());
        assert!(channel.session_commands().is_empty());
        // Even reattaching the same session invalidates the old startup notice.
        if stale_session == "thr_a" {
            engine
                .handle_inbound(inbound("chat-a", "/detach"))
                .await
                .unwrap();
        }
        engine
            .handle_inbound(inbound("chat-a", "/attach thr_a"))
            .await
            .unwrap();
        let sent = channel.sent().len();
        let menus = channel.session_commands().len();
        engine.notify_restored_bindings(updates).await.unwrap();
        assert_eq!(channel.sent().len(), sent, "sent a stale startup notice");
        assert_eq!(
            channel.session_commands().len(),
            menus,
            "replaced a newer menu"
        );
    }
}

#[tokio::test]
async fn deferred_startup_notice_rechecks_binding_after_slow_menu() {
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    state
        .attach(
            &ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            &SessionId::new("thr_a"),
        )
        .await
        .unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(FakeAgent::new()),
        state,
        vec![channel.clone()],
    ));
    let updates = engine.restore_bindings_deferred().await.unwrap();
    let entered = tokio_util::sync::CancellationToken::new();
    let release = tokio_util::sync::CancellationToken::new();
    *channel.next_menu_gate.lock().unwrap() = Some((entered.clone(), release.clone()));
    let notifications = tokio::spawn({
        let engine = engine.clone();
        async move { engine.notify_restored_bindings(updates).await }
    });
    entered.cancelled().await;
    engine
        .handle_inbound(inbound("chat-a", "/detach"))
        .await
        .unwrap();
    release.cancel();
    notifications.await.unwrap().unwrap();
    assert!(!channel.session_commands().last().unwrap().1);
    assert!(
        !channel
            .sent()
            .iter()
            .any(|(_, view)| view.title == "Agentix serve")
    );
}

#[tokio::test]
async fn deferred_startup_turn_updates_skip_detached_sessions() {
    let state = SqliteState::in_memory().await.unwrap();
    seed_feishu_turn(&state).await;
    let channel = Arc::new(FakeChannel {
        channel_kind: Some(ChannelKind::Feishu),
        ..FakeChannel::default()
    });
    let engine = Engine::new(Arc::new(FakeAgent::new()), state, vec![channel.clone()]);
    let updates = engine.restore_bindings_deferred().await.unwrap();
    assert!(channel.updated().is_empty());
    assert!(channel.sent().is_empty());
    engine
        .handle_inbound(InboundEnvelope::text(
            "detach",
            ConversationRef::new(ChannelKind::Feishu, "saved-chat"),
            "owner-42",
            "/detach",
        ))
        .await
        .unwrap();
    let rendered_count = channel.updated().len();
    engine.notify_restored_bindings(updates).await.unwrap();
    assert_eq!(
        channel.updated().len(),
        rendered_count,
        "repainted a detached turn"
    );
}

async fn seed_feishu_turn(state: &SqliteState) {
    let channel = Arc::new(FakeChannel {
        channel_kind: Some(ChannelKind::Feishu),
        ..FakeChannel::default()
    });
    let engine = Engine::new(Arc::new(FakeAgent::new()), state.clone(), vec![channel]);
    engine
        .handle_inbound(InboundEnvelope::text(
            "feishu-attach",
            ConversationRef::new(ChannelKind::Feishu, "saved-chat"),
            "owner-42",
            "/attach thr_b",
        ))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_b".into(),
            turn_id: "feishu-turn".into(),
            item_id: "item".into(),
            delta: "Saved Feishu response".into(),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn restore_preserves_disabled_channels_without_subscribing_or_rendering_them() {
    let state = SqliteState::in_memory().await.unwrap();
    seed_feishu_turn(&state).await;
    let telegram = ConversationRef::new(ChannelKind::Telegram, "chat-a");
    state
        .attach(&telegram, &SessionId::new("thr_a"))
        .await
        .unwrap();
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(agent.clone(), state.clone(), vec![channel.clone()]);

    assert_eq!(engine.restore_bindings().await.unwrap(), 1);
    assert!(agent.calls().contains(&"attach:thr_a".into()));
    assert!(!agent.calls().contains(&"attach:thr_b".into()));
    assert!(
        channel
            .sent()
            .iter()
            .all(|(conversation, _)| conversation == &telegram)
    );
    assert_eq!(state.list_bindings().await.unwrap().len(), 2);

    let feishu = Arc::new(FakeChannel {
        channel_kind: Some(ChannelKind::Feishu),
        ..FakeChannel::default()
    });
    let restarted = Engine::new(agent, state, vec![feishu.clone()]);
    assert_eq!(restarted.restore_bindings().await.unwrap(), 1);
    assert_eq!(feishu.updated().len(), 1);
    assert!(feishu.updated()[0].1.body.contains("Saved Feishu response"));
}

#[tokio::test]
async fn resumed_sessions_do_not_reactivate_disabled_channels() {
    let state = SqliteState::in_memory().await.unwrap();
    seed_feishu_turn(&state).await;
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        state,
        vec![Arc::new(FakeChannel::default())],
    );
    engine
        .handle_agent_event(AgentEvent::SessionResumed {
            session_id: "thr_b".into(),
        })
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_b".into(),
            turn_id: "feishu-turn".into(),
            item_id: "item".into(),
            delta: "Later Feishu response".into(),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn shutdown_skips_disabled_channels_and_preserves_their_state() {
    let state = SqliteState::in_memory().await.unwrap();
    seed_feishu_turn(&state).await;
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        state.clone(),
        vec![channel.clone()],
    );

    assert!(
        engine
            .prepare_shutdown_notifications()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(channel.sent().is_empty());
    assert!(channel.updated().is_empty());
    assert_eq!(state.list_bindings().await.unwrap().len(), 1);
}

#[tokio::test]
async fn graceful_shutdown_persists_the_binding_and_detaches_the_im() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-a");
    let engine = Engine::new(agent.clone(), state.clone(), vec![channel.clone()]);
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
            item_id: "item_a".into(),
            delta: "still working".into(),
        })
        .await
        .unwrap();
    assert_eq!(channel.sent().last().unwrap().1.actions.len(), 1);

    let mut notifications = engine.prepare_shutdown_notifications().await.unwrap();
    assert_eq!(notifications.len(), 1);
    engine
        .send_shutdown_notification(notifications.pop().unwrap())
        .await
        .unwrap();
    assert_eq!(
        state.current_session(&conversation).await.unwrap(),
        Some(SessionId::new("thr_a"))
    );
    assert_eq!(
        channel.session_commands().last(),
        Some(&(conversation.clone(), false))
    );
    let offline = channel.sent().last().unwrap().1.clone();
    assert_eq!(offline.title, "Agentix serve");
    assert_eq!(offline.subtitle.as_deref(), Some("Offline · Detached"));
    assert!(
        offline
            .body
            .contains("Codex session Parser cleanup · thr_a")
    );
    assert!(offline.body.contains("Saved"));
    let detached_turn = channel.updated().last().unwrap().1.clone();
    assert_eq!(
        detached_turn.subtitle.as_deref(),
        Some("Turn turn_liv · In progress")
    );
    assert!(detached_turn.actions.is_empty());
    assert!(!agent.calls().contains(&"unsubscribe:thr_a".to_string()));
}

#[tokio::test]
async fn restore_discards_an_attachment_when_its_session_is_no_longer_running() {
    let agent = Arc::new(FakeAgent::rejecting_attachment("thr_missing"));
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-a");
    state
        .attach(&conversation, &SessionId::new("thr_missing"))
        .await
        .unwrap();
    let engine = Engine::new(agent, state.clone(), vec![channel.clone()]);

    assert_eq!(engine.restore_bindings().await.unwrap(), 0);
    assert_eq!(state.current_session(&conversation).await.unwrap(), None);
    assert_eq!(
        channel.session_commands().last(),
        Some(&(conversation, false))
    );
    let online = channel.sent().last().unwrap().1.clone();
    assert_eq!(online.title, "Agentix serve");
    assert_eq!(online.subtitle.as_deref(), Some("Online · Detached"));
    assert!(online.body.contains("thr_miss"));
    assert!(online.body.contains("no longer running"));
}

#[tokio::test]
async fn restart_reuses_the_running_turn_message_and_replaces_its_stop_action() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let directory = tempdir().unwrap();
    let state_path = directory.path().join("state.sqlite3");
    let state = SqliteState::open(&state_path).await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
            item_id: "item_a".into(),
            delta: "working before restart".into(),
        })
        .await
        .unwrap();
    let running_message_id = format!("m{}", channel.sent().len());
    let old_stop_token = channel.sent().last().unwrap().1.actions[0].token.clone();
    drop(engine);

    let state = SqliteState::open(&state_path).await.unwrap();
    let restarted = Engine::new(agent.clone(), state, vec![channel.clone()]);
    assert_eq!(restarted.restore_bindings().await.unwrap(), 1);
    let restored = channel.updated();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].0.message_id, running_message_id);
    assert_eq!(restored[0].1.actions.len(), 1);
    assert_ne!(restored[0].1.actions[0].token, old_stop_token);

    restarted
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await
        .unwrap();
    let completed = channel.updated();
    assert_eq!(completed.len(), 2);
    assert_eq!(completed[1].0.message_id, running_message_id);
    assert!(completed[1].1.body.contains("working before restart"));
    assert!(completed[1].1.actions.is_empty());

    drop(restarted);
    let state = SqliteState::open(&state_path).await.unwrap();
    let after_completion = Engine::new(agent, state, vec![channel.clone()]);
    assert_eq!(after_completion.restore_bindings().await.unwrap(), 1);
    assert_eq!(channel.updated().len(), 2);
}

#[tokio::test]
async fn stream_updates_are_throttled_but_completion_flushes_latest_text() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent, state, vec![channel.clone()]);
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    let before = channel.sent().len();

    for delta in ["first", " second"] {
        engine
            .handle_agent_event(AgentEvent::AgentMessageDelta {
                session_id: "thr_a".into(),
                turn_id: "turn_live".into(),
                item_id: "item_a".into(),
                delta: delta.into(),
            })
            .await
            .unwrap();
    }
    assert_eq!(channel.sent().len(), before + 1);

    engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await
        .unwrap();
    let sent = channel.sent();
    assert_eq!(sent.len(), before + 2);
    assert!(sent.last().unwrap().1.body.contains("first second"));
}

#[tokio::test]
async fn running_turn_elapsed_time_refreshes_without_replacing_stop_action() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent, state, vec![channel.clone()]);
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_a"))
        .await
        .unwrap();

    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "run the tests"))
        .await
        .unwrap();
    let started = channel.sent().last().unwrap().1.clone();
    assert_eq!(
        started.subtitle.as_deref(),
        Some("Turn turn_new · Working 0s")
    );
    assert_eq!(started.actions.len(), 1);
    let stop_token = started.actions[0].token.clone();
    let updates = channel.updated().len();
    assert_eq!(engine.refresh_working_turns().await, 0);
    assert_eq!(channel.updated().len(), updates);

    tokio::time::sleep(std::time::Duration::from_millis(1_050)).await;
    assert_eq!(engine.refresh_working_turns().await, 1);
    let refreshed = channel.sent().last().unwrap().1.clone();
    let refreshed_subtitle = refreshed.subtitle.as_deref().unwrap();
    assert!(refreshed_subtitle.starts_with("Turn turn_new · Working "));
    assert_ne!(refreshed_subtitle, "Turn turn_new · Working 0s");
    assert_eq!(refreshed.actions[0].token, stop_token);

    engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_a".into(),
            turn_id: "turn_new".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await
        .unwrap();
    let completed = channel.sent().last().unwrap().1.clone();
    assert!(
        completed
            .subtitle
            .as_deref()
            .unwrap()
            .starts_with("Turn turn_new · Completed in ")
    );
    assert!(completed.actions.is_empty());

    let updates = channel.updated().len();
    assert_eq!(engine.refresh_working_turns().await, 0);
    assert_eq!(channel.updated().len(), updates);
}

#[tokio::test]
async fn disconnect_invalidates_buttons_from_the_old_agent_connection() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent, state, vec![channel.clone()]);
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
            item_id: "item_a".into(),
            delta: "working".into(),
        })
        .await
        .unwrap();
    let token = channel.sent().last().unwrap().1.actions[0].token.clone();
    engine
        .handle_agent_event(AgentEvent::Disconnected {
            generation: 1,
            reason: "socket lost".into(),
        })
        .await
        .unwrap();

    let error = engine
        .handle_inbound(InboundEnvelope::action(
            "callback-after-reconnect",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner-42",
            token,
        ))
        .await
        .unwrap_err();
    assert!(matches!(error, EngineError::InvalidAction));
}

#[tokio::test]
async fn exited_current_session_notifies_the_im_and_detaches() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let persisted = state.clone();
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-a");
    let engine = Engine::new(agent, state, vec![channel.clone()]);
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
            item_id: "item_a".into(),
            delta: "working".into(),
        })
        .await
        .unwrap();
    let stop_token = channel.sent().last().unwrap().1.actions[0].token.clone();
    engine
        .handle_agent_event(AgentEvent::InteractionRequested(InteractionRequest {
            rpc_id: serde_json::json!(91),
            method: "item/commandExecution/requestApproval".into(),
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
            item_id: Some("item_b".into()),
            kind: InteractionKind::CommandApproval,
            title: "Command approval".into(),
            detail: "cargo test".into(),
            available_decisions: vec!["accept".into(), "decline".into()],
            payload: serde_json::json!({}),
            auto_resolution_ms: None,
        }))
        .await
        .unwrap();
    let approval_token = channel.sent().last().unwrap().1.actions[0].token.clone();

    engine
        .handle_agent_event(AgentEvent::SessionExited {
            session_id: "thr_a".into(),
        })
        .await
        .unwrap();

    let interrupted = channel.updated().last().unwrap().1.clone();
    assert_eq!(interrupted.status, agentix_core::ViewStatus::Warning);
    assert!(
        interrupted
            .subtitle
            .as_deref()
            .unwrap()
            .contains("Interrupted")
    );
    assert!(interrupted.actions.is_empty());
    let notice = channel.sent().last().unwrap().1.clone();
    assert_eq!(notice.title, "Codex session exited");
    assert_eq!(notice.subtitle.as_deref(), Some("Automatically detached"));
    assert!(notice.body.contains("Parser cleanup · thr_a"));
    assert_eq!(notice.status, agentix_core::ViewStatus::Warning);
    assert!(!channel.session_commands().last().unwrap().1);
    assert_eq!(
        persisted.current_session(&conversation).await.unwrap(),
        Some(SessionId::new("thr_a"))
    );

    let current_error = engine
        .handle_inbound(inbound("chat-a", "/current"))
        .await
        .unwrap_err();
    assert!(matches!(current_error, EngineError::NoCurrentSession));
    let action_error = engine
        .handle_inbound(InboundEnvelope::action(
            "callback-after-session-exit",
            conversation,
            "owner-42",
            stop_token,
        ))
        .await
        .unwrap_err();
    assert!(matches!(action_error, EngineError::InvalidAction));
    let approval_error = engine
        .handle_inbound(InboundEnvelope::action(
            "approval-after-session-exit",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner-42",
            approval_token,
        ))
        .await
        .unwrap_err();
    assert!(matches!(approval_error, EngineError::InvalidAction));
}

#[tokio::test]
async fn resumed_codex_session_reattaches_the_previous_im_conversation() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-a");
    let engine = Engine::new(agent.clone(), state.clone(), vec![channel.clone()]);
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::SessionExited {
            session_id: "thr_a".into(),
        })
        .await
        .unwrap();

    engine
        .handle_agent_event(AgentEvent::SessionResumed {
            session_id: "thr_a".into(),
        })
        .await
        .unwrap();

    assert_eq!(
        state.current_session(&conversation).await.unwrap(),
        Some(SessionId::new("thr_a"))
    );
    assert_eq!(
        channel.session_commands().last(),
        Some(&(conversation.clone(), true))
    );
    let notice = channel.sent().last().unwrap().1.clone();
    assert_eq!(notice.title, "Codex session resumed");
    assert_eq!(notice.subtitle.as_deref(), Some("Automatically reattached"));
    assert!(notice.body.contains("Parser cleanup · thr_a"));
    assert_eq!(notice.status, agentix_core::ViewStatus::Success);

    engine
        .handle_inbound(inbound("chat-a", "/current"))
        .await
        .unwrap();
    assert!(
        channel
            .sent()
            .last()
            .unwrap()
            .1
            .title
            .contains("Parser cleanup")
    );
}

#[tokio::test]
async fn attaching_another_session_cancels_the_suspended_session_resume() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-a");
    let engine = Engine::new(agent.clone(), state.clone(), vec![channel.clone()]);
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::SessionExited {
            session_id: "thr_a".into(),
        })
        .await
        .unwrap();

    engine
        .handle_inbound(inbound("chat-a", "/attach thr_b"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::SessionResumed {
            session_id: "thr_a".into(),
        })
        .await
        .unwrap();

    assert!(agent.calls().contains(&"unsubscribe:thr_a".to_owned()));
    assert_eq!(
        state.current_session(&conversation).await.unwrap(),
        Some(SessionId::new("thr_b"))
    );
    engine
        .handle_inbound(inbound("chat-a", "/current"))
        .await
        .unwrap();
    assert!(
        channel
            .sent()
            .last()
            .unwrap()
            .1
            .title
            .contains("Daemon startup")
    );
}

#[tokio::test]
async fn history_older_uses_the_cursor_from_the_previous_page() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel]);
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "/history older"))
        .await
        .unwrap();

    assert_eq!(agent.history_cursors(), vec![None, Some("older".into())]);
}

#[tokio::test]
async fn history_newer_uses_the_cursor_from_the_previous_page() {
    let agent = Arc::new(FakeAgent::with_history_cursors(
        Some("older"),
        Some("newer"),
    ));
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel]);
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "/history newer"))
        .await
        .unwrap();

    assert_eq!(agent.history_cursors(), vec![None, Some("newer".into())]);
}

#[tokio::test]
async fn completion_invalidates_the_previous_stop_button() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent, state, vec![channel.clone()]);
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
            item_id: "item_a".into(),
            delta: "done soon".into(),
        })
        .await
        .unwrap();
    let token = channel.sent().last().unwrap().1.actions[0].token.clone();
    engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await
        .unwrap();

    let error = engine
        .handle_inbound(InboundEnvelope::action(
            "late-stop",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner-42",
            token,
        ))
        .await
        .unwrap_err();
    assert!(matches!(error, EngineError::InvalidAction));
}

#[tokio::test]
async fn unattached_turn_completion_notifies_the_im_and_can_attach_the_session() {
    let agent = Arc::new(FakeAgent::with_history(vec![
        TurnSummary {
            id: "turn_background".into(),
            status: TurnStatus::Completed,
            user_text: Some("finish the background task".into()),
            agent_text: Some("Completed **the task**.\n\nAll checks passed.".into()),
            tools: Vec::new(),
            items: Vec::new(),
        },
        TurnSummary {
            id: "turn_newer".into(),
            status: TurnStatus::InProgress,
            user_text: Some("newer unrelated question".into()),
            agent_text: Some("newer unrelated answer".into()),
            tools: Vec::new(),
            items: Vec::new(),
        },
    ]));
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/help"))
        .await
        .unwrap();
    let before = channel.sent().len();

    engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_b".into(),
            turn_id: "turn_background".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await
        .unwrap();

    let sent = channel.sent();
    assert_eq!(sent.len(), before + 1);
    let notification = &sent.last().unwrap().1;
    assert_eq!(notification.title, "Codex · Daemon startup · thr_b");
    assert_eq!(
        notification.subtitle.as_deref(),
        Some("Background turn turn_bac · Completed")
    );
    assert!(notification.body.contains("> finish the background task"));
    assert!(notification.body.contains("> Completed **the task**."));
    assert!(notification.body.contains("> All checks passed."));
    assert!(!notification.body.contains("newer unrelated"));
    assert_eq!(
        serde_json::to_value(notification.status).unwrap(),
        "background"
    );
    assert!(notification.body.contains("not attached"));
    assert_eq!(notification.actions.len(), 1);
    assert_eq!(notification.actions[0].label, "Attach");
    assert_eq!(notification.actions[0].style, ActionStyle::Primary);

    click_action(
        &engine,
        "attach-background",
        notification.actions[0].token.clone(),
    )
    .await;
    assert!(agent.calls().contains(&"attach:thr_b".to_owned()));
}

#[tokio::test]
async fn disabling_background_notifications_keeps_attached_turns_visible() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_background_turn_notifications(false);
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/help"))
        .await
        .unwrap();
    let before = channel.sent().len();
    for status in [
        TurnStatus::Completed,
        TurnStatus::Failed,
        TurnStatus::Interrupted,
    ] {
        engine
            .handle_agent_event(AgentEvent::TurnCompleted {
                session_id: "thr_b".into(),
                turn_id: "turn_background".into(),
                status,
                error: None,
            })
            .await
            .unwrap();
    }
    assert_eq!(channel.sent().len(), before);
    assert!(
        agent.history_cursors().is_empty(),
        "disabled notices must not fetch content"
    );
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "turn_attached".into(),
            item_id: "item_answer".into(),
            delta: "Attached answer".into(),
        })
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_a".into(),
            turn_id: "turn_attached".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await
        .unwrap();
    let updated = channel.updated();
    assert!(updated.last().unwrap().1.body.contains("Attached answer"));
    assert_eq!(
        updated.last().unwrap().1.status,
        agentix_core::ViewStatus::Success
    );
}

#[tokio::test]
async fn unattached_turn_completion_is_not_notified_twice() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent, state, vec![channel.clone()]);
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/help"))
        .await
        .unwrap();
    let before = channel.sent().len();
    let completed = AgentEvent::TurnCompleted {
        session_id: "thr_b".into(),
        turn_id: "turn_background".into(),
        status: TurnStatus::Completed,
        error: None,
    };

    engine.handle_agent_event(completed.clone()).await.unwrap();
    engine.handle_agent_event(completed).await.unwrap();

    assert_eq!(channel.sent().len(), before + 1);
}

#[tokio::test]
async fn draining_turn_completion_adds_an_attach_button() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state, vec![channel.clone()]);
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::TurnStarted {
            session_id: "thr_a".into(),
            turn_id: "turn_background".into(),
        })
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "turn_background".into(),
            item_id: "item-background".into(),
            delta: "Finished in the background.".into(),
        })
        .await
        .unwrap();
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_b"))
        .await
        .unwrap();

    let completed = AgentEvent::TurnCompleted {
        session_id: "thr_a".into(),
        turn_id: "turn_background".into(),
        status: TurnStatus::Completed,
        error: None,
    };
    engine.handle_agent_event(completed.clone()).await.unwrap();

    let notification = channel.sent().last().unwrap().1.clone();
    assert_eq!(notification.title, "Codex · Parser cleanup · thr_a");
    assert!(notification.body.contains("background session"));
    assert_eq!(notification.status, agentix_core::ViewStatus::Background);
    assert_eq!(notification.actions.len(), 1);
    assert_eq!(notification.actions[0].label, "Attach");

    let sent_after_completion = channel.sent().len();
    engine.handle_agent_event(completed).await.unwrap();
    assert_eq!(channel.sent().len(), sent_after_completion);

    click_action(
        &engine,
        "reattach-background",
        notification.actions[0].token.clone(),
    )
    .await;
    assert_eq!(
        agent
            .calls()
            .iter()
            .filter(|call| call.as_str() == "attach:thr_a")
            .count(),
        2
    );
}

#[tokio::test]
async fn stream_and_working_timer_share_the_channel_interval_but_completion_flushes() {
    let channel = Arc::new(FakeChannel {
        streaming_interval: Some(std::time::Duration::from_secs(5)),
        ..FakeChannel::default()
    });
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "work"))
        .await
        .unwrap();
    let before = channel.updated().len();
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "turn_new".into(),
            item_id: "answer".into(),
            delta: "Buffered output".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        channel.updated().len(),
        before,
        "stream must respect the channel interval"
    );
    assert_eq!(
        engine.refresh_working_turns().await,
        0,
        "timer must not bypass stream pacing"
    );
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    assert_eq!(engine.refresh_working_turns().await, 1);
    assert!(
        channel
            .updated()
            .last()
            .unwrap()
            .1
            .body
            .contains("Buffered output")
    );
    engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_a".into(),
            turn_id: "turn_new".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await
        .unwrap();
    assert_eq!(channel.updated().len(), before + 2);
    assert!(channel.updated().last().unwrap().1.actions.is_empty());
}

#[path = "support/task_board.rs"]
mod task_board;

#[tokio::test]
async fn registry_attachment_persists_qualified_identity_for_unambiguous_input() {
    let pi = Arc::new(FakeAgent::new());
    let registry = Arc::new(
        agentix_core::AgentRegistry::new(vec![(agentix_core::AgentKind::Pi, pi)]).unwrap(),
    );
    let state = SqliteState::in_memory().await.unwrap();
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(registry, state.clone(), vec![channel]);
    engine
        .handle_inbound(inbound("chat", "/attach thr_a"))
        .await
        .unwrap();
    assert_eq!(
        state.list_bindings().await.unwrap()[0].1.as_str(),
        "pi:thr_a"
    );
}

#[tokio::test]
async fn legacy_bindings_migrate_atomically_and_require_an_owner() {
    let state = SqliteState::in_memory().await.unwrap();
    let chat = ConversationRef::new(ChannelKind::Telegram, "chat");
    state
        .attach(&chat, &SessionId::new("native"))
        .await
        .unwrap();
    assert!(state.qualify_sessions(None).await.is_err());
    assert_eq!(state.list_bindings().await.unwrap()[0].1.as_str(), "native");
    state
        .qualify_sessions(Some(agentix_core::AgentKind::Pi))
        .await
        .unwrap();
    state.qualify_sessions(None).await.unwrap();
    assert_eq!(
        state.list_bindings().await.unwrap()[0].1.as_str(),
        "pi:native"
    );
}

#[tokio::test]
async fn session_operations_reject_mutations_but_allow_history_for_read_only_hosts() {
    use agentix_core::SessionOperations;
    let mut agent = FakeAgent::new();
    agent.read_only = true;
    let agent = Arc::new(agent);
    let operations = SessionOperations::new(agent.clone());
    let session = SessionId::new("thr_a");
    assert!(operations.send(&session, "hello", None).await.is_err());
    assert!(operations.stop(&session, "turn").await.is_err());
    assert!(
        operations
            .command(&session, SessionCommand::Compact)
            .await
            .is_err()
    );
    assert!(agent.calls().is_empty());
    assert_eq!(
        operations
            .history(&session, None, 5)
            .await
            .unwrap()
            .turns
            .len(),
        1
    );
}

#[tokio::test]
async fn session_operation_results_remain_typed_until_the_control_boundary() {
    use agentix_core::{SessionOperation, SessionOperationResult, SessionOperations};
    let agent = Arc::new(FakeAgent::new());
    let operations = SessionOperations::new(agent.clone());
    let result = operations
        .execute(SessionOperation::Send {
            session: SessionId::new("thr_a"),
            text: "hello".into(),
            expected_turn: None,
        })
        .await
        .unwrap();
    assert_eq!(
        result,
        SessionOperationResult::Started {
            turn_id: "turn_new".into()
        }
    );
    assert_eq!(agent.calls(), ["start:thr_a:hello"]);
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        serde_json::json!({"turn_id":"turn_new"})
    );
}

#[tokio::test]
async fn registry_routes_events_without_polling_host_lifecycle() {
    let agent = Arc::new(FakeAgent::new());
    let _registry =
        agentix_core::AgentRegistry::new(vec![(agentix_core::AgentKind::Pi, agent.clone())])
            .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert_eq!(
        agent
            .refresh_count
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[tokio::test]
async fn an_event_gap_expires_old_actions_and_recovers_the_latest_bound_history() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "turn_live".into(),
            item_id: "item".into(),
            delta: "old partial output".into(),
        })
        .await
        .unwrap();
    let old_token = channel.sent().last().unwrap().1.actions[0].token.clone();
    {
        let mut history = agent.history_turns.lock().unwrap();
        history[0].id = "turn_live".into();
        history[0].agent_text = Some("recovered final output".into());
        history[0].status = TurnStatus::Completed;
    }
    engine.recover_event_gap().await.unwrap();
    let result = engine
        .handle_inbound(InboundEnvelope::action(
            "stale",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner-42",
            old_token,
        ))
        .await;
    assert!(matches!(result, Err(EngineError::InvalidAction)));
    assert!(
        channel
            .updated()
            .iter()
            .any(|(_, view)| view.body.contains("recovered final output"))
    );
}

#[tokio::test]
async fn pending_session_input_does_not_lock_other_conversations_during_delivery() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let engine = Arc::new(Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    ));
    for (chat, session) in [("chat-a", "thr_a"), ("chat-b", "thr_b")] {
        engine
            .handle_inbound(inbound(chat, &format!("/attach {session}")))
            .await
            .unwrap();
        engine
            .handle_inbound(inbound(chat, "/rename"))
            .await
            .unwrap();
    }
    let entered = tokio_util::sync::CancellationToken::new();
    let release = tokio_util::sync::CancellationToken::new();
    *channel.next_send_gate.lock().unwrap() = Some((entered.clone(), release.clone()));
    let first = tokio::spawn({
        let engine = engine.clone();
        async move { engine.handle_inbound(inbound("chat-a", "First name")).await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.cancelled())
        .await
        .unwrap();
    let independent = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        engine.handle_inbound(inbound("chat-b", "Second name")),
    )
    .await;
    release.cancel();
    first.await.unwrap().unwrap();
    independent
        .expect("pending input state must not be held across IM delivery")
        .unwrap();
    assert!(
        agent
            .calls()
            .contains(&"command:thr_b:Rename(Some(\"Second name\"))".to_owned())
    );
}

#[tokio::test]
async fn dispatch_recomputes_bindings_between_consecutive_transfers() {
    use agentix_core::{DispatchQueue, EngineWork};
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        SqliteState::in_memory().await.unwrap(),
        vec![channel],
    );
    for (chat, command) in [("chat-a", "/attach thr_a"), ("chat-b", "/attach thr_b")] {
        engine.handle_inbound(inbound(chat, command)).await.unwrap();
    }
    let mut queue = DispatchQueue::new(8, 4);
    for (chat, command) in [
        ("chat-a", "/attach thr_b"),
        ("chat-a", "/attach thr_a"),
        ("chat-b", "/current"),
    ] {
        queue
            .try_push(EngineWork::Inbound(inbound(chat, command)))
            .unwrap();
    }
    queue
        .try_push(EngineWork::Event(AgentEvent::QueueChanged {
            session_id: "thr_a".into(),
        }))
        .unwrap();
    let snapshot = engine.dispatch_snapshot().await;
    let first = queue.next_ready(|work| snapshot.scope(work)).unwrap();
    assert!(
        queue.next_ready(|work| snapshot.scope(work)).is_none(),
        "transfer must fence old and new sessions and displaced chat"
    );
    engine.execute_work(first.work).await.unwrap();
    queue.finish(first.id);
    let snapshot = engine.dispatch_snapshot().await;
    let second = queue.next_ready(|work| snapshot.scope(work)).unwrap();
    let detached = queue
        .next_ready(|work| snapshot.scope(work))
        .expect("displaced chat no longer owns the transferred session");
    assert!(
        matches!(detached.work, EngineWork::Inbound(ref envelope) if envelope.conversation.conversation_id == "chat-b")
    );
    assert!(
        queue.next_ready(|work| snapshot.scope(work)).is_none(),
        "event for the second attachment must wait for its binding"
    );
    engine.execute_work(second.work).await.unwrap();
    queue.finish(second.id);
    let snapshot = engine.dispatch_snapshot().await;
    assert!(matches!(
        queue.next_ready(|work| snapshot.scope(work)).unwrap().work,
        EngineWork::Event(_)
    ));
}

#[tokio::test]
async fn dispatch_fork_fences_early_new_session_events_only_on_its_backend() {
    use agentix_core::{AgentKind, AgentRegistry, DispatchQueue, EngineWork};
    let registry = AgentRegistry::new(vec![
        (
            AgentKind::Pi,
            Arc::new(FakeAgent::new()) as Arc<dyn AgentAdapter>,
        ),
        (
            AgentKind::Omp,
            Arc::new(FakeAgent::new()) as Arc<dyn AgentAdapter>,
        ),
    ])
    .unwrap();
    let engine = Engine::new(
        Arc::new(registry),
        SqliteState::in_memory().await.unwrap(),
        vec![Arc::new(FakeChannel::default())],
    );
    engine
        .handle_inbound(inbound("chat-a", "/attach pi:thr_a"))
        .await
        .unwrap();
    let mut queue = DispatchQueue::new(4, 4);
    queue
        .try_push(EngineWork::Inbound(inbound("chat-a", "/fork")))
        .unwrap();
    for session in ["pi:unknown-replacement", "omp:independent"] {
        queue
            .try_push(EngineWork::Event(AgentEvent::QueueChanged {
                session_id: session.into(),
            }))
            .unwrap();
    }
    let snapshot = engine.dispatch_snapshot().await;
    let fork = queue.next_ready(|work| snapshot.scope(work)).unwrap();
    let independent = queue.next_ready(|work| snapshot.scope(work)).unwrap();
    assert!(
        matches!(independent.work, EngineWork::Event(AgentEvent::QueueChanged { ref session_id }) if session_id == "omp:independent")
    );
    assert!(queue.next_ready(|work| snapshot.scope(work)).is_none());
    queue.finish(fork.id);
    assert!(queue.next_ready(|work| snapshot.scope(work)).is_some());
}

#[tokio::test]
async fn dispatch_scopes_opaque_attach_actions_without_consuming_their_authorization() {
    use agentix_core::{DispatchQueue, EngineWork};
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    engine
        .handle_inbound(inbound("chat-b", "/attach thr_b"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "/sessions"))
        .await
        .unwrap();
    let token = channel.sent().last().unwrap().1.actions[1].token.clone();
    let mut queue = DispatchQueue::new(4, 4);
    queue
        .try_push(EngineWork::Event(AgentEvent::QueueChanged {
            session_id: "thr_b".into(),
        }))
        .unwrap();
    queue
        .try_push(EngineWork::Inbound(InboundEnvelope::action(
            "attach-button",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner",
            token,
        )))
        .unwrap();
    queue
        .try_push(EngineWork::Inbound(inbound("chat-b", "/current")))
        .unwrap();
    queue
        .try_push(EngineWork::Event(AgentEvent::QueueChanged {
            session_id: "thr_a".into(),
        }))
        .unwrap();
    let snapshot = engine.dispatch_snapshot().await;
    let old = queue.next_ready(|work| snapshot.scope(work)).unwrap();
    assert!(
        matches!(queue.next_ready(|work| snapshot.scope(work)).unwrap().work, EngineWork::Event(AgentEvent::QueueChanged { ref session_id }) if session_id == "thr_a")
    );
    assert!(queue.next_ready(|work| snapshot.scope(work)).is_none());
    queue.finish(old.id);
    let action = queue.next_ready(|work| snapshot.scope(work)).unwrap();
    assert!(
        queue.next_ready(|work| snapshot.scope(work)).is_none(),
        "button must reserve the displaced conversation"
    );
    engine
        .execute_work(action.work)
        .await
        .expect("routing inspection must not consume the action token");
}

#[tokio::test]
async fn shutdown_preparation_finishes_all_local_state_before_any_im_io() {
    use tokio_util::sync::CancellationToken;
    let state = SqliteState::in_memory().await.unwrap();
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        state.clone(),
        vec![channel.clone()],
    );
    for (chat, session) in [("chat-a", "thr_a"), ("chat-b", "thr_b")] {
        engine
            .handle_inbound(inbound(chat, &format!("/attach {session}")))
            .await
            .unwrap();
    }
    let entered = CancellationToken::new();
    let release = CancellationToken::new();
    *channel.next_menu_gate.lock().unwrap() = Some((entered.clone(), release.clone()));
    let prepared = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        engine.prepare_shutdown_notifications(),
    )
    .await;
    release.cancel();
    let notifications = prepared
        .expect("local shutdown preparation waited for IM")
        .unwrap();
    assert_eq!(notifications.len(), 2);
    assert!(!entered.is_cancelled());
    assert_eq!(state.list_bindings().await.unwrap().len(), 2);
    // The durable bindings survive, but every in-memory route is detached even
    // when no shutdown notification can be delivered.
    for chat in ["chat-a", "chat-b"] {
        assert!(matches!(
            engine.handle_inbound(inbound(chat, "/current")).await,
            Err(EngineError::NoCurrentSession)
        ));
    }
}

async fn refresh_and_deliver_tasks(engine: &Engine) -> Result<(), agentix_core::EngineError> {
    engine.refresh_task_board().await?;
    loop {
        let batch = engine.claim_task_notifications(32).await?;
        if batch.is_empty() {
            return Ok(());
        }
        for notification in batch {
            engine.deliver_task_notification(notification).await?;
        }
    }
}

#[tokio::test]
async fn bot_identity_switch_invalidates_only_its_channel_before_restoration() {
    for kind in [
        ChannelKind::Telegram,
        ChannelKind::Feishu,
        ChannelKind::Slack,
    ] {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let state = SqliteState::open(&path).await.unwrap();
        let conversation = ConversationRef::new(kind, "old-chat");
        let other_kind = if kind == ChannelKind::Slack {
            ChannelKind::Feishu
        } else {
            ChannelKind::Slack
        };
        let other = ConversationRef::new(other_kind, "other-chat");
        let agent = Arc::new(FakeAgent::new());
        let channel = Arc::new(FakeChannel {
            channel_kind: Some(kind),
            bot_identity: Some("bot-a".into()),
            ..FakeChannel::default()
        });
        let engine = Engine::new(agent.clone(), state.clone(), vec![channel.clone()]);
        engine.restore_bindings_deferred().await.unwrap();
        state
            .attach(&conversation, &SessionId::new("thr_a"))
            .await
            .unwrap();
        state
            .attach(&other, &SessionId::new("other-session"))
            .await
            .unwrap();
        drop(engine);
        drop(state);
        let state = SqliteState::open(&path).await.unwrap();
        let same = Engine::new(agent.clone(), state.clone(), vec![channel.clone()]);
        assert_eq!(
            same.restore_bindings_deferred()
                .await
                .unwrap()
                .restored_count(),
            1
        );
        state
            .reject_overloaded("test-consumer", &conversation, "old-event", 1)
            .await
            .unwrap();
        drop(same);
        drop(state);
        let state = SqliteState::open(&path).await.unwrap();
        let changed = Arc::new(FakeChannel {
            channel_kind: Some(kind),
            bot_identity: Some("bot-b".into()),
            ..FakeChannel::default()
        });
        let restarted = Engine::new(agent.clone(), state.clone(), vec![changed.clone()]);
        assert_eq!(restarted.restore_bindings().await.unwrap(), 0);
        assert!(changed.sent().is_empty());
        assert!(
            state
                .claim_notifications("test-consumer", i64::MAX / 2, 60, 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(state.current_session(&conversation).await.unwrap(), None);
        assert_eq!(
            state.current_session(&other).await.unwrap(),
            Some(SessionId::new("other-session"))
        );
        // Switching back must not revive the stale route.
        let back = Engine::new(agent, state.clone(), vec![channel]);
        assert_eq!(back.restore_bindings().await.unwrap(), 0);
    }
}

#[tokio::test]
async fn unknown_legacy_bot_is_detached_but_identity_failure_preserves_state() {
    let state = SqliteState::in_memory().await.unwrap();
    let conversation = ConversationRef::new(ChannelKind::Telegram, "old-chat");
    state
        .attach(&conversation, &SessionId::new("thr_a"))
        .await
        .unwrap();
    let failed = Arc::new(FakeChannel {
        fail_identity: true,
        ..FakeChannel::default()
    });
    let engine = Engine::new(Arc::new(FakeAgent::new()), state.clone(), vec![failed]);
    assert!(engine.restore_bindings_deferred().await.is_err());
    assert!(
        state
            .current_session(&conversation)
            .await
            .unwrap()
            .is_some()
    );
    let known = Arc::new(FakeChannel {
        bot_identity: Some("bot-a".into()),
        ..FakeChannel::default()
    });
    let engine = Engine::new(
        Arc::new(FakeAgent::new()),
        state.clone(),
        vec![known.clone()],
    );
    assert_eq!(engine.restore_bindings().await.unwrap(), 0);
    assert!(known.sent().is_empty());
}

#[tokio::test]
async fn reload_background_notifications_toggle_without_losing_owners_or_deduplication() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let mut engine = Engine::new(agent.clone(), state.clone(), vec![channel.clone()]);
    engine
        .handle_inbound(inbound_as("chat-a", "owner-42", "/help"))
        .await
        .unwrap();
    let initial = channel.sent().len();
    let mut notices = 0;
    for (enabled, turn, expected) in [
        (false, "off", 0),
        (true, "on", 1),
        (true, "on", 1),
        (false, "off2", 1),
        (true, "on2", 2),
    ] {
        let mut next = Engine::new(agent.clone(), state.clone(), vec![channel.clone()])
            .with_background_turn_notifications(enabled);
        next.inherit_runtime(&engine);
        engine = next;
        engine
            .handle_agent_event(AgentEvent::TurnCompleted {
                session_id: "thr_b".into(),
                turn_id: turn.into(),
                status: TurnStatus::Completed,
                error: None,
            })
            .await
            .unwrap();
        notices = expected;
        assert_eq!(channel.sent().len(), initial + notices);
    }
    assert_eq!(notices, 2);
}

#[tokio::test]
async fn reload_output_policy_changes_rendered_sections_during_a_live_turn() {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let state = SqliteState::in_memory().await.unwrap();
    let mut engine = Engine::new(agent.clone(), state.clone(), vec![channel.clone()]);
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::AgentMessageDelta {
            session_id: "thr_a".into(),
            turn_id: "live".into(),
            item_id: "answer".into(),
            delta: "Still working".into(),
        })
        .await
        .unwrap();
    for (index, visible) in [true, false, true].into_iter().enumerate() {
        let mut next = Engine::new(agent.clone(), state.clone(), vec![channel.clone()])
            .with_output(agentix_core::OutputConfig {
                show_reasoning: visible,
                show_tool_calls: visible,
            });
        next.inherit_runtime(&engine);
        engine = next;
        for (kind, text) in [
            ("reasoning", "Think carefully"),
            ("commandExecution", "cargo test"),
        ] {
            engine
                .handle_agent_event(AgentEvent::ItemCompleted {
                    session_id: "thr_a".into(),
                    turn_id: "live".into(),
                    item: ItemSummary {
                        id: format!("{kind}-{index}"),
                        kind: kind.into(),
                        text: Some(format!("{text}-{index}")),
                        status: Some("completed".into()),
                    },
                })
                .await
                .unwrap();
        }
        engine
            .handle_agent_event(AgentEvent::TurnCompleted {
                session_id: "thr_a".into(),
                turn_id: "live".into(),
                status: TurnStatus::Completed,
                error: None,
            })
            .await
            .unwrap();
        let updated = channel.updated();
        let view = &updated.last().unwrap().1;
        assert!(view.body.contains("Still working"));
        assert_eq!(
            view.body.contains(&format!("Think carefully-{index}")),
            visible
        );
        assert_eq!(view.body.contains(&format!("cargo test-{index}")), visible);
    }
}

#[tokio::test]
async fn reload_task_board_preserves_pending_action_input() {
    let (_dir, service, id) = task_fixture().await;
    let channel = Arc::new(FakeChannel::default());
    let agent = Arc::new(FakeAgent::new());
    let state = SqliteState::in_memory().await.unwrap();
    let engine = Engine::new(agent.clone(), state.clone(), vec![channel.clone()])
        .with_task_board(service.clone());
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", &format!("/task {id}")))
        .await
        .unwrap();
    let token = channel
        .sent()
        .last()
        .unwrap()
        .1
        .actions
        .iter()
        .find(|a| a.label == "Block")
        .unwrap()
        .token
        .clone();
    engine
        .handle_inbound(InboundEnvelope::action(
            "block-before-reload",
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner",
            token,
        ))
        .await
        .unwrap();
    let mut next = Engine::new(agent, state, vec![channel]).with_task_board(service.clone());
    next.inherit_runtime(&engine);
    next.handle_inbound(inbound("chat-a", "Reason after reload"))
        .await
        .unwrap();
    let snapshot = service.store().snapshot().await.unwrap();
    assert_eq!(snapshot.tasks[0].status.to_string(), "BLOCKED");
    assert_eq!(
        snapshot.tasks[0].reason.as_deref(),
        Some("Reason after reload")
    );
}

#[tokio::test]
async fn reload_task_board_enable_disable_changes_help_and_commands() {
    let (_dir, service, _) = task_fixture().await;
    let channel = Arc::new(FakeChannel::default());
    let agent = Arc::new(FakeAgent::new());
    let state = SqliteState::in_memory().await.unwrap();
    let mut engine = Engine::new(agent.clone(), state.clone(), vec![channel.clone()]);
    for enabled in [true, false, true] {
        let mut next = Engine::new(agent.clone(), state.clone(), vec![channel.clone()]);
        if enabled {
            next = next.with_task_board(service.clone());
        }
        next.inherit_runtime(&engine);
        engine = next;
        engine
            .handle_inbound(InboundEnvelope::text(
                uuid::Uuid::new_v4().to_string(),
                ConversationRef::new(ChannelKind::Telegram, "chat-a"),
                "owner",
                "/help",
            ))
            .await
            .unwrap();
        assert_eq!(
            channel.sent().last().unwrap().1.body.contains("/dashboard"),
            enabled
        );
        let result = engine
            .handle_inbound(InboundEnvelope::text(
                uuid::Uuid::new_v4().to_string(),
                ConversationRef::new(ChannelKind::Telegram, "chat-a"),
                "owner",
                "/dashboard",
            ))
            .await;
        if enabled {
            result.unwrap();
            assert_eq!(channel.sent().last().unwrap().1.title, "Dashboard");
        } else {
            assert!(result.unwrap_err().to_string().contains("not configured"));
        }
    }
}
