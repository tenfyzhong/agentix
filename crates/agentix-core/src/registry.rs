//! Namespaced routing across independently connected local agent runtimes.
use crate::{
    AgentAdapter, AgentError, AgentEvent, AgentKind, HistoryPage, InteractionDecision,
    MultiplexerMutation, MultiplexerMutationResult, MultiplexerSnapshot, QueuedPrompt,
    QueuedPromptPort, SessionCommand, SessionCommandResult, SessionControlPort, SessionId,
    SessionPage, SessionRef, SessionStatus, SessionSummary, WorkspaceRuntimePort,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

struct Route {
    native: Value,
    session: SessionRef,
}
type Routes = Arc<Mutex<HashMap<String, Route>>>;
type KnownSessions = Arc<Mutex<HashSet<crate::NativeSessionId>>>;

pub struct AgentRegistry {
    agents: BTreeMap<AgentKind, Arc<dyn AgentAdapter>>,
    events: broadcast::Sender<AgentEvent>,
    routes: Routes,
    known: BTreeMap<AgentKind, KnownSessions>,
    workspaces: BTreeMap<AgentKind, NamespacedWorkspace>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for AgentRegistry {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl AgentRegistry {
    pub fn new(agents: Vec<(AgentKind, Arc<dyn AgentAdapter>)>) -> Result<Self, AgentError> {
        let mut map = BTreeMap::new();
        for (kind, agent) in agents {
            if map.insert(kind, agent).is_some() {
                return Err(AgentError::Rejected("duplicate agent backend".into()));
            }
        }
        if map.is_empty() {
            return Err(AgentError::Rejected("no agent backends configured".into()));
        }
        let (events, _) = broadcast::channel(1024);
        let routes: Routes = Arc::default();
        let mut tasks = Vec::new();
        let mut known = BTreeMap::new();
        for (&kind, agent) in &map {
            let sessions = KnownSessions::default();
            known.insert(kind, sessions.clone());
            tasks.push(spawn_backend(
                kind,
                agent.clone(),
                events.clone(),
                routes.clone(),
                sessions,
            ));
        }
        let workspaces = map
            .iter()
            .filter(|(_, agent)| agent.workspace_runtime().is_some())
            .map(|(&kind, agent)| {
                (
                    kind,
                    NamespacedWorkspace {
                        kind,
                        agent: agent.clone(),
                    },
                )
            })
            .collect();
        Ok(Self {
            agents: map,
            events,
            routes,
            known,
            workspaces,
            tasks,
        })
    }
    fn target(
        &self,
        session: &SessionId,
    ) -> Result<(SessionRef, &Arc<dyn AgentAdapter>), AgentError> {
        let key = SessionRef::decode(session).ok_or_else(|| {
            AgentError::Rejected("select a qualified session from /sessions".into())
        })?;
        let agent = self.agents.get(&key.agent).ok_or_else(|| {
            AgentError::Unavailable(format!("{} is not configured", key.agent.as_str()))
        })?;
        self.known[&key.agent]
            .lock()
            .unwrap()
            .insert(key.native_id.clone());
        Ok((key, agent))
    }
    fn qualify(kind: AgentKind, session: &mut SessionSummary) {
        let title = session
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .or_else(|| {
                session
                    .preview
                    .as_deref()
                    .and_then(|preview| preview.lines().next())
            })
            .unwrap_or_else(|| session.id.short())
            .chars()
            .take(80)
            .collect::<String>();
        session.id = SessionRef::new(kind, session.id.clone()).encode();
        session.name = Some(format!("{} · {title}", kind.display_name()));
    }
}

#[async_trait]
impl AgentAdapter for AgentRegistry {
    fn display_name(&self) -> &'static str {
        "Agent"
    }
    fn session_backends(&self) -> Vec<AgentKind> {
        self.agents.keys().copied().collect()
    }
    fn session_display_name(&self, session: &SessionId) -> &'static str {
        SessionRef::decode(session).map_or("Agent", |key| key.agent.display_name())
    }
    fn generation(&self) -> u64 {
        1
    }
    fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }
    fn queued_prompts(&self) -> Option<&dyn QueuedPromptPort> {
        Some(self)
    }
    fn session_control(&self) -> Option<&dyn SessionControlPort> {
        Some(self)
    }
    fn workspace_backends(&self) -> Vec<AgentKind> {
        self.workspaces.keys().copied().collect()
    }
    fn workspace_for(&self, kind: Option<AgentKind>) -> Option<&dyn WorkspaceRuntimePort> {
        kind.and_then(|kind| self.workspaces.get(&kind))
            .map(|w| w as &dyn WorkspaceRuntimePort)
            .or_else(|| {
                if kind.is_none() && self.workspaces.len() == 1 {
                    self.workspace_runtime()
                } else {
                    None
                }
            })
    }
    fn workspace_runtime(&self) -> Option<&dyn WorkspaceRuntimePort> {
        self.workspaces
            .values()
            .next()
            .map(|workspace| workspace as &dyn WorkspaceRuntimePort)
    }
    async fn session_capabilities(&self, session: &SessionId) -> crate::SessionCapabilities {
        match self.target(session) {
            Ok((key, agent)) => agent.session_capabilities(key.native_id.adapter_id()).await,
            Err(_) => crate::SessionCapabilities::default(),
        }
    }
    async fn session_access(&self, session: &SessionId) -> crate::SessionAccess {
        match self.target(session) {
            Ok((key, agent)) => agent.session_access(key.native_id.adapter_id()).await,
            Err(_) => crate::SessionAccess::Offline,
        }
    }
    async fn canonical_session(&self, session: &SessionId) -> Result<SessionId, AgentError> {
        if let Some(key) = SessionRef::decode(session) {
            self.target(&key.encode())?;
            return Ok(key.encode());
        }
        let mut found = None;
        for (&kind, agent) in &self.agents {
            let mut cursor = None;
            loop {
                let page = agent.list_sessions(cursor, 100).await?;
                if page.sessions.iter().any(|s| s.id == *session) {
                    if found.is_some() {
                        return Err(AgentError::Rejected(
                            "ambiguous session ID; select its backend in /sessions".into(),
                        ));
                    }
                    found = Some(SessionRef::new(kind, session.clone()).encode());
                    break;
                }
                cursor = page.next_cursor;
                if cursor.is_none() {
                    break;
                }
            }
        }
        found.ok_or_else(|| AgentError::Rejected("unknown session".into()))
    }
    async fn list_sessions(
        &self,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<SessionPage, AgentError> {
        let offset = cursor
            .as_deref()
            .unwrap_or("0")
            .parse::<usize>()
            .map_err(|e| AgentError::Protocol(e.to_string()))?;
        let mut sessions = Vec::new();
        for (&kind, agent) in &self.agents {
            let mut cursor = None;
            let mut seen = HashSet::new();
            loop {
                let page = match agent.list_sessions(cursor, 100).await {
                    Ok(page) => page,
                    Err(error) => {
                        tracing::warn!(?kind, %error, "backend listing unavailable");
                        break;
                    }
                };
                sessions.extend(page.sessions.into_iter().map(|mut s| {
                    self.known[&kind]
                        .lock()
                        .unwrap()
                        .insert(s.id.clone().into());
                    Self::qualify(kind, &mut s);
                    s
                }));
                cursor = page.next_cursor;
                if cursor.as_ref().is_none_or(|c| !seen.insert(c.clone())) {
                    break;
                }
            }
        }
        sessions.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });
        let end = offset.saturating_add(limit as usize).min(sessions.len());
        Ok(SessionPage {
            next_cursor: (end < sessions.len() && limit > 0).then(|| end.to_string()),
            sessions: sessions.drain(offset.min(end)..end).collect(),
        })
    }
    async fn read_history(
        &self,
        session: &SessionId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<HistoryPage, AgentError> {
        let (key, agent) = self.target(session)?;
        agent
            .read_history(key.native_id.adapter_id(), cursor, limit)
            .await
    }
    async fn attach(&self, session: &SessionId) -> Result<(), AgentError> {
        let session = self.canonical_session(session).await?;
        let (key, agent) = self.target(&session)?;
        agent.attach(key.native_id.adapter_id()).await
    }
    async fn is_read_only(&self, session: &SessionId) -> bool {
        match self.target(session) {
            Ok((key, agent)) => agent.is_read_only(key.native_id.adapter_id()).await,
            Err(_) => true,
        }
    }
    async fn is_subagent(&self, session: &SessionId) -> Result<bool, AgentError> {
        let (key, agent) = self.target(session)?;
        agent.is_subagent(key.native_id.adapter_id()).await
    }
    async fn unsubscribe(&self, session: &SessionId) -> Result<(), AgentError> {
        let (key, agent) = self.target(session)?;
        agent.unsubscribe(key.native_id.adapter_id()).await
    }
    async fn start_turn(&self, session: &SessionId, text: &str) -> Result<String, AgentError> {
        let (key, agent) = self.target(session)?;
        agent.start_turn(key.native_id.adapter_id(), text).await
    }
    async fn steer(
        &self,
        session: &SessionId,
        turn: &str,
        text: &str,
    ) -> Result<String, AgentError> {
        let (key, agent) = self.target(session)?;
        agent.steer(key.native_id.adapter_id(), turn, text).await
    }
    async fn interrupt(&self, session: &SessionId, turn: &str) -> Result<(), AgentError> {
        let (key, agent) = self.target(session)?;
        agent.interrupt(key.native_id.adapter_id(), turn).await
    }
    async fn resolve_interaction(&self, decision: InteractionDecision) -> Result<(), AgentError> {
        let token = decision
            .rpc_id
            .as_str()
            .ok_or_else(|| AgentError::Rejected("invalid interaction".into()))?;
        let route = self
            .routes
            .lock()
            .unwrap()
            .remove(token)
            .ok_or_else(|| AgentError::Rejected("interaction expired".into()))?;
        self.agents[&route.session.agent]
            .resolve_interaction(InteractionDecision {
                rpc_id: route.native,
                response: decision.response,
            })
            .await
    }
}
#[async_trait]
impl QueuedPromptPort for AgentRegistry {
    async fn queue_status(&self, session: &SessionId) -> Result<Option<String>, AgentError> {
        let (key, agent) = self.target(session)?;
        agent
            .queued_prompts()
            .ok_or_else(|| AgentError::Rejected("queue unavailable".into()))?
            .queue_status(key.native_id.adapter_id())
            .await
    }
    async fn control_queue(&self, session: &SessionId, action: &str) -> Result<(), AgentError> {
        let (key, agent) = self.target(session)?;
        agent
            .queued_prompts()
            .ok_or_else(|| AgentError::Rejected("queue unavailable".into()))?
            .control_queue(key.native_id.adapter_id(), action)
            .await
    }

    async fn queue_prompt(
        &self,
        session: &SessionId,
        text: &str,
        id: &str,
    ) -> Result<QueuedPrompt, AgentError> {
        let (key, agent) = self.target(session)?;
        agent
            .queued_prompts()
            .ok_or_else(|| AgentError::Rejected("queue unavailable".into()))?
            .queue_prompt(key.native_id.adapter_id(), text, id)
            .await
    }
    async fn list_queued_prompts(
        &self,
        session: &SessionId,
    ) -> Result<Vec<QueuedPrompt>, AgentError> {
        let (key, agent) = self.target(session)?;
        agent
            .queued_prompts()
            .ok_or_else(|| AgentError::Rejected("queue unavailable".into()))?
            .list_queued_prompts(key.native_id.adapter_id())
            .await
    }
}
#[async_trait]
impl SessionControlPort for AgentRegistry {
    async fn run_session_command(
        &self,
        session: &SessionId,
        command: SessionCommand,
    ) -> Result<SessionCommandResult, AgentError> {
        let (key, agent) = self.target(session)?;
        let mut result = agent
            .session_control()
            .ok_or_else(|| AgentError::Rejected("session control unavailable".into()))?
            .run_session_command(key.native_id.adapter_id(), command)
            .await?;
        if let Some(session) = &mut result.replacement_session {
            Self::qualify(key.agent, session);
        }
        Ok(result)
    }
}

struct NamespacedWorkspace {
    kind: AgentKind,
    agent: Arc<dyn AgentAdapter>,
}
#[async_trait]
impl WorkspaceRuntimePort for NamespacedWorkspace {
    fn default_directory(&self) -> String {
        self.agent
            .workspace_runtime()
            .expect("configured runtime")
            .default_directory()
    }
    async fn snapshot(&self) -> Result<Option<MultiplexerSnapshot>, AgentError> {
        let mut snapshot = self
            .agent
            .workspace_runtime()
            .expect("configured runtime")
            .snapshot()
            .await?;
        if let Some(snapshot) = &mut snapshot {
            for session in &mut snapshot.sessions {
                for window in &mut session.windows {
                    for pane in &mut window.panes {
                        if let Some(id) = &mut pane.agent_session {
                            *id = SessionRef::new(self.kind, id.clone()).encode();
                        }
                    }
                }
            }
        }
        Ok(snapshot)
    }
    async fn mutate(
        &self,
        mutation: MultiplexerMutation,
    ) -> Result<MultiplexerMutationResult, AgentError> {
        let mut result = self
            .agent
            .workspace_runtime()
            .expect("configured runtime")
            .mutate(mutation)
            .await?;
        if let Some(session) = &mut result.session {
            session.id = SessionRef::new(self.kind, session.id.clone()).encode();
        }
        Ok(result)
    }
}

fn route_interaction(kind: AgentKind, event: &mut AgentEvent, routes: &Routes) {
    if let AgentEvent::SessionStatusChanged {
        session_id,
        status: SessionStatus::Offline,
    }
    | AgentEvent::SessionExited { session_id } = event
    {
        routes.lock().unwrap().retain(|_, route| {
            route.session.agent != kind || route.session.native_id.as_str() != session_id
        });
    }

    if let AgentEvent::InteractionRequested(request) = event {
        let token = uuid::Uuid::new_v4().to_string();
        routes.lock().unwrap().insert(
            token.clone(),
            Route {
                native: request.rpc_id.clone(),
                session: SessionRef::from_native(
                    kind,
                    crate::NativeSessionId::new(&request.session_id),
                ),
            },
        );
        request.rpc_id = json!(token);
    } else if let AgentEvent::InteractionResolved {
        session_id,
        request_id,
    } = event
    {
        let mut routes = routes.lock().unwrap();
        let key = routes
            .iter()
            .find(|(_, r)| {
                r.session.agent == kind
                    && r.session.native_id.as_str() == session_id
                    && (r.native.as_str() == Some(request_id)
                        || serde_json::from_str::<Value>(request_id).is_ok_and(|id| r.native == id))
            })
            .map(|(key, _)| key.clone());
        if let Some(key) = key {
            routes.remove(&key);
            *request_id = key;
        }
    }
}

fn spawn_backend(
    kind: AgentKind,
    agent: Arc<dyn AgentAdapter>,
    events: broadcast::Sender<AgentEvent>,
    routes: Routes,
    known: KnownSessions,
) -> tokio::task::JoinHandle<()> {
    let mut source = agent.subscribe();
    tokio::spawn(async move {
        loop {
            match source.recv().await {
                Ok(AgentEvent::Disconnected { .. })
                | Err(broadcast::error::RecvError::Lagged(_)) => {
                    routes
                        .lock()
                        .unwrap()
                        .retain(|_, route| route.session.agent != kind);
                    for native in known.lock().unwrap().iter() {
                        let session_id = SessionRef::from_native(kind, native.clone())
                            .encode()
                            .to_string();
                        let _ = events.send(AgentEvent::SessionStatusChanged {
                            session_id,
                            status: SessionStatus::Offline,
                        });
                    }
                }
                Ok(AgentEvent::Connected { .. }) => {
                    if let Ok(page) = agent.list_sessions(None, u32::MAX).await {
                        for session in page.sessions {
                            known.lock().unwrap().insert(session.id.clone().into());
                            let _ = events.send(AgentEvent::SessionResumed {
                                session_id: SessionRef::new(kind, session.id).encode().to_string(),
                            });
                        }
                    }
                }
                Ok(mut event) => {
                    if let Some(session) = event.session_id() {
                        known
                            .lock()
                            .unwrap()
                            .insert(crate::NativeSessionId::new(session));
                    }
                    route_interaction(kind, &mut event, &routes);
                    event.map_session_id(|id| {
                        SessionRef::new(kind, SessionId::new(id))
                            .encode()
                            .to_string()
                    });
                    let _ = events.send(event);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}
