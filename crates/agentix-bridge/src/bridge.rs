//! Local IPC to extensions running inside the original terminal process.
use super::bridge_hub::BridgeHub;
use super::{BridgeKind, PendingResponses, wire};
use agentix_domain::{
    AgentAdapter, AgentError, AgentEvent, HistoryPage, InteractionDecision, MultiplexerMutation,
    MultiplexerMutationResult, MultiplexerSnapshot, QueuedPrompt, QueuedPromptPort, SessionCommand,
    SessionCommandChoice, SessionCommandResult, SessionControlPort, SessionId, SessionPage,
    SessionSummary, WorkspaceRuntimePort,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, broadcast, oneshot};

const MAX_FRAME: u64 = wire::MAX_FRAME_BYTES as u64;
pub(super) type Reader = BufReader<Box<dyn AsyncRead + Send + Unpin>>;
pub(super) type Writer = Box<dyn AsyncWrite + Send + Unpin>;
fn unavailable(error: impl std::fmt::Display) -> AgentError {
    AgentError::Unavailable(error.to_string())
}
fn rejected(message: &str) -> AgentError {
    AgentError::Rejected(message.into())
}

pub(super) struct Connection {
    pid: Option<u32>,
    pub(super) instance: String,
    writer: Arc<Mutex<Writer>>,
    pending: Arc<StdMutex<PendingResponses>>,
    hub: std::sync::Weak<super::bridge_hub::State>,
    agent: String,
    session: SessionId,
    snapshot: Arc<StdMutex<wire::SessionInfo>>,
    pub(super) online: Arc<AtomicBool>,
    reader_task: tokio::task::JoinHandle<()>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}
impl Connection {
    pub(super) fn new(
        record: &wire::Registration,
        mut reader: Reader,
        writer: Writer,
        events: broadcast::Sender<AgentEvent>,
        state: std::sync::Weak<super::bridge_hub::State>,
    ) -> Arc<Connection> {
        let instance = record.instance.clone();
        let session = record.session_id.clone();
        let agent = record.agent.clone();
        let initial_seq = record.snapshot.seq;
        let snapshot = Arc::new(StdMutex::new(record.snapshot.clone()));
        let pending = Arc::new(StdMutex::new(PendingResponses::new()));
        let online = Arc::new(AtomicBool::new(true));
        let task = {
            let pending = pending.clone();
            let online = online.clone();
            let instance = instance.clone();
            let state = state.clone();
            tokio::spawn(async move {
                let mut last_seq = initial_seq;
                while let Ok(frame) = read_frame(&mut reader).await {
                    if let Some(id) = frame["id"].as_str() {
                        let tx = pending.lock().unwrap().remove(id);
                        if let Some(tx) = tx {
                            let result = if frame["ok"] == true {
                                Ok(frame["result"].clone())
                            } else {
                                Err(rejected(
                                    frame["error"].as_str().unwrap_or("bridge command failed"),
                                ))
                            };
                            let _ = tx.send(result);
                        }
                    } else {
                        let Ok(frame) = serde_json::from_value::<wire::EventFrame>(frame) else {
                            break;
                        };
                        if frame.instance != instance || frame.seq <= last_seq {
                            continue;
                        }
                        if frame.seq != last_seq + 1 {
                            break;
                        }
                        let event: AgentEvent = frame.event.into();
                        if event.session_id() != Some(session.as_str()) {
                            break;
                        }
                        last_seq = frame.seq;
                        let _ = events.send(event);
                    }
                }
                pending.lock().unwrap().clear();
                if let Some(state) = state.upgrade() {
                    state
                        .disconnected(&agent, &SessionId::new(session), &instance)
                        .await;
                } else {
                    online.store(false, Ordering::Release);
                }
            })
        };
        Arc::new(Connection {
            pid: u32::try_from(record.pid).ok(),
            instance,
            writer: Arc::new(Mutex::new(writer)),
            hub: state,
            agent: record.agent.clone(),
            session: SessionId::new(&record.session_id),
            pending,
            snapshot,
            online,
            reader_task: task,
        })
    }

    pub(super) async fn shutdown(&self) {
        self.online.store(false, Ordering::Release);
        self.reader_task.abort();
        self.pending.lock().unwrap().clear();
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            self.writer.lock().await.shutdown().await
        })
        .await;
    }
    fn poison(&self) {
        self.online.store(false, Ordering::Release);
        self.reader_task.abort();
        self.pending.lock().unwrap().clear();
        let hub = self.hub.clone();
        let agent = self.agent.clone();
        let session = self.session.clone();
        let instance = self.instance.clone();
        let writer = self.writer.clone();
        tokio::spawn(async move {
            if let Some(hub) = hub.upgrade() {
                hub.disconnected(&agent, &session, &instance).await;
            }
            let _ = tokio::time::timeout(Duration::from_secs(1), async {
                writer.lock().await.shutdown().await
            })
            .await;
        });
    }
    async fn request(&self, method: &str, params: Value) -> Result<Value, AgentError> {
        if !self.online.load(Ordering::Acquire) {
            return Err(unavailable("bridge disconnected"));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        let mut guard = PendingRequest {
            connection: self,
            id: id.clone(),
            writing: false,
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut bytes = serde_json::to_vec(&wire::Request {
                id,
                method: method.into(),
                params,
            })
            .map_err(unavailable)?;
            bytes.push(b'\n');
            if bytes.len() > wire::MAX_FRAME_BYTES {
                return Err(AgentError::Protocol(
                    "bridge request exceeds maximum frame size".into(),
                ));
            }
            let mut writer = self.writer.lock().await;
            if !self.online.load(Ordering::Acquire) {
                return Err(unavailable("bridge disconnected"));
            }
            // Cancellation halfway through a frame makes this stream unusable.
            guard.writing = true;
            writer.write_all(&bytes).await.map_err(unavailable)?;
            guard.writing = false;
            drop(writer);
            rx.await.map_err(unavailable)?
        })
        .await
        .map_err(|_| {
            tracing::warn!(target: "agentix::telemetry", backend = %self.agent, method,
                timeout_ms = 10000, "backend request timed out");
            unavailable("bridge request timed out; check session state before retrying")
        })?
    }
}

struct PendingRequest<'a> {
    connection: &'a Connection,
    id: String,
    writing: bool,
}
impl Drop for PendingRequest<'_> {
    fn drop(&mut self) {
        self.connection.pending.lock().unwrap().remove(&self.id);
        if self.writing {
            self.connection.poison();
        }
    }
}

pub struct BridgeAdapter {
    flavor: BridgeKind,
    workspace: Option<agentix_multiplexer::WorkspaceManager>,
    server: Arc<BridgeHub>,
    events: broadcast::Sender<AgentEvent>,
}
impl BridgeAdapter {
    #[must_use]
    pub fn new(
        flavor: BridgeKind,
        server: Arc<BridgeHub>,
        session_root: impl Into<PathBuf>,
    ) -> Self {
        let session_root = session_root.into();
        server.configure(flavor, &session_root);
        let events = server.events(flavor);
        Self {
            flavor,
            workspace: None,
            server,
            events,
        }
    }
    #[must_use]
    pub fn with_workspace(mut self, command: &Path, args: Vec<String>, directory: &Path) -> Self {
        self.workspace = Some(agentix_multiplexer::WorkspaceManager::new(
            std::iter::once(command.to_string_lossy().into_owned())
                .chain(args)
                .collect(),
            directory,
        ));
        self
    }
    #[must_use]
    pub fn with_optional_multiplexer(
        self,
        driver: Option<Arc<dyn agentix_multiplexer::MultiplexerDriver>>,
    ) -> Self {
        match driver {
            Some(driver) => self.with_multiplexer(driver),
            None => self,
        }
    }
    #[must_use]
    pub fn with_multiplexer(self, driver: Arc<dyn agentix_multiplexer::MultiplexerDriver>) -> Self {
        if let Some(workspace) = &self.workspace {
            workspace.set_driver(driver);
        }
        self
    }
    async fn connection(&self, id: &SessionId) -> Result<Arc<Connection>, AgentError> {
        self.server
            .connection(self.flavor, id)
            .await
            .ok_or_else(|| {
                unavailable(
                    "session bridge is offline; install agentix-bridge and reload the terminal",
                )
            })
    }
    pub(crate) async fn request(
        &self,
        id: &SessionId,
        method: &str,
        params: Value,
    ) -> Result<Value, AgentError> {
        self.connection(id).await?.request(method, params).await
    }
}

pub(super) async fn read_frame(reader: &mut Reader) -> Result<Value, AgentError> {
    let mut bytes = Vec::new();
    let count = reader
        .take(MAX_FRAME + 1)
        .read_until(b'\n', &mut bytes)
        .await
        .map_err(unavailable)?;
    if count == 0 || count as u64 > MAX_FRAME || bytes.last() != Some(&b'\n') {
        return Err(unavailable("invalid or closed bridge stream"));
    }
    serde_json::from_slice(&bytes).map_err(unavailable)
}

#[async_trait]
impl AgentAdapter for BridgeAdapter {
    fn display_name(&self) -> &'static str {
        match self.flavor {
            BridgeKind::Claude => "Claude Code",
            BridgeKind::Codex => "Codex",
            BridgeKind::Pi => "Pi",
            BridgeKind::Omp => "OMP",
        }
    }
    fn workspace_runtime(&self) -> Option<&dyn WorkspaceRuntimePort> {
        self.workspace
            .as_ref()
            .map(|_| self as &dyn WorkspaceRuntimePort)
    }
    fn generation(&self) -> u64 {
        1
    }
    fn session_control(&self) -> Option<&dyn SessionControlPort> {
        Some(self)
    }
    fn queued_prompts(&self) -> Option<&dyn QueuedPromptPort> {
        Some(self)
    }
    fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }
    async fn session_capabilities(
        &self,
        session: &SessionId,
    ) -> agentix_domain::SessionCapabilities {
        let Ok(connection) = self.connection(session).await else {
            return agentix_domain::SessionCapabilities::default();
        };
        agentix_domain::SessionCapabilities::from_names(
            connection
                .snapshot
                .lock()
                .unwrap()
                .capabilities
                .iter()
                .map(String::as_str),
        )
    }
    async fn session_access(&self, session: &SessionId) -> agentix_domain::SessionAccess {
        if self.connection(session).await.is_ok() {
            agentix_domain::SessionAccess::Writable
        } else {
            agentix_domain::SessionAccess::Offline
        }
    }
    async fn list_sessions(
        &self,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<SessionPage, AgentError> {
        let connections = self.server.connections(self.flavor).await;
        let mut sessions = Vec::new();
        let locations = match &self.workspace {
            Some(w) => w.process_locations().await.unwrap_or_default(),
            None => std::collections::HashMap::default(),
        };
        let mut pending = tokio::task::JoinSet::new();
        let mut connections = connections.into_iter();
        loop {
            while pending.len() < 16 {
                let Some(connection) = connections.next() else {
                    break;
                };
                pending.spawn(async move {
                    let Ok(value) = connection.request("info", json!({})).await else {
                        return Ok(None);
                    };
                    let info: wire::SessionInfo =
                        serde_json::from_value(value).map_err(unavailable)?;
                    if info.instance != connection.instance
                        || info.session.id != connection.session.as_str()
                    {
                        connection.poison();
                        return Err(AgentError::Protocol(
                            "Bridge metadata changed connection identity".into(),
                        ));
                    }
                    let summary: SessionSummary = info.session.clone().into();
                    *connection.snapshot.lock().unwrap() = info;
                    Ok::<_, AgentError>(Some((connection.pid, summary)))
                });
            }
            let Some(result) = pending.join_next().await else {
                break;
            };
            if let Some((pid, mut summary)) = result.map_err(unavailable)?? {
                summary.terminal = pid.and_then(|pid| locations.get(&pid).cloned());
                sessions.push(summary);
            }
        }
        sessions.sort_by(|a: &SessionSummary, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });
        let offset = cursor
            .as_deref()
            .unwrap_or("0")
            .parse::<usize>()
            .map_err(unavailable)?;
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
        serde_json::from_value::<wire::History>(
            self.request(session, "history", json!({"cursor":cursor,"limit":limit}))
                .await?,
        )
        .map(Into::into)
        .map_err(unavailable)
    }

    async fn attach(&self, session: &SessionId) -> Result<(), AgentError> {
        self.connection(session).await.map(|_| ())
    }
    async fn is_read_only(&self, session: &SessionId) -> bool {
        self.connection(session).await.is_err()
    }
    async fn unsubscribe(&self, _session: &SessionId) -> Result<(), AgentError> {
        Ok(())
    }
    async fn start_turn(&self, session: &SessionId, text: &str) -> Result<String, AgentError> {
        let result = self
            .request(
                session,
                "prompt",
                json!({"text":text,"request_id":uuid::Uuid::new_v4().to_string()}),
            )
            .await?;
        result["turn_id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| rejected("missing turn ID"))
    }
    async fn steer(
        &self,
        session: &SessionId,
        turn: &str,
        text: &str,
    ) -> Result<String, AgentError> {
        self.request(
            session,
            "steer",
            json!({"text":text,"request_id":uuid::Uuid::new_v4().to_string()}),
        )
        .await?;
        Ok(turn.into())
    }
    async fn interrupt(&self, session: &SessionId, _turn: &str) -> Result<(), AgentError> {
        self.request(session, "stop", json!({})).await.map(|_| ())
    }
    async fn resolve_interaction(&self, _decision: InteractionDecision) -> Result<(), AgentError> {
        Err(rejected("terminal dialogs must be answered locally"))
    }
}

#[async_trait]
impl SessionControlPort for BridgeAdapter {
    async fn run_session_command(
        &self,
        session: &SessionId,
        command: SessionCommand,
    ) -> Result<SessionCommandResult, AgentError> {
        let name = command.name();
        if !self.supports_command(session, name).await {
            return Err(rejected("command not supported by this host"));
        }
        let value = match &command {
            SessionCommand::Model(value)
            | SessionCommand::Reasoning(value)
            | SessionCommand::Rename(value) => value.clone(),
            _ => None,
        };
        let result = self
            .request(session, "command", json!({"name":name,"value":value}))
            .await?;
        let mut output = SessionCommandResult::message(
            format!("{} · {name}", self.display_name()),
            result["body"].as_str().unwrap_or_default(),
        );
        for choice in result["choices"].as_array().into_iter().flatten() {
            if let (Some(label), Some(value)) = (choice["label"].as_str(), choice["value"].as_str())
            {
                let command = match name {
                    "model" => SessionCommand::Model(Some(value.into())),
                    "reasoning" => SessionCommand::Reasoning(Some(value.into())),
                    _ => continue,
                };
                output
                    .choices
                    .push(SessionCommandChoice::new(label, command));
            }
        }
        Ok(output)
    }
}
#[async_trait]
impl QueuedPromptPort for BridgeAdapter {
    async fn queue_prompt(
        &self,
        session: &SessionId,
        text: &str,
        id: &str,
    ) -> Result<QueuedPrompt, AgentError> {
        serde_json::from_value::<wire::QueueItem>(
            self.request(session, "queue", json!({"text":text,"request_id":id}))
                .await?,
        )
        .map(Into::into)
        .map_err(unavailable)
    }
    async fn list_queued_prompts(
        &self,
        session: &SessionId,
    ) -> Result<Vec<QueuedPrompt>, AgentError> {
        let state: wire::QueueState =
            serde_json::from_value(self.request(session, "queue_state", json!({})).await?)
                .map_err(unavailable)?;
        Ok(state.items.into_iter().map(Into::into).collect())
    }
    async fn queue_status(&self, session: &SessionId) -> Result<Option<String>, AgentError> {
        let state = self.request(session, "queue_state", json!({})).await?;
        Ok(Some(
            if state["uncertain"].is_object() {
                "Delivery uncertain: inspect history before clearing the queue and resubmitting."
            } else if state["paused"] == true {
                "Paused. Resume when ready."
            } else if self.flavor == BridgeKind::Claude {
                "No unresolved Claude deliveries. Native event queuing is managed by Claude."
            } else {
                "Runs in FIFO order after the active task."
            }
            .into(),
        ))
    }
    async fn control_queue(&self, session: &SessionId, action: &str) -> Result<(), AgentError> {
        if !matches!(action, "resume" | "clear") {
            return Err(rejected("unknown queue action"));
        }
        self.request(session, &format!("queue_{action}"), json!({}))
            .await
            .map(|_| ())
    }
}

#[async_trait]
impl WorkspaceRuntimePort for BridgeAdapter {
    fn multiplexer_kind(&self) -> agentix_domain::MultiplexerKind {
        self.workspace.as_ref().map_or(
            agentix_domain::MultiplexerKind::default(),
            agentix_multiplexer::WorkspaceManager::kind,
        )
    }
    fn default_directory(&self) -> String {
        self.workspace.as_ref().map_or_else(
            || "~".into(),
            |w| w.default_directory().to_string_lossy().into_owned(),
        )
    }
    async fn resolve_directory(&self, input: &str, base: &str) -> Result<String, AgentError> {
        (self
            .workspace
            .as_ref()
            .ok_or_else(|| rejected("multiplexer is not configured"))?)
        .resolve_directory(input, base)
        .await
        .map_err(|e| AgentError::Rejected(e.to_string()))
    }
    async fn list_directories(
        &self,
        directory: &str,
        page: usize,
        show_hidden: bool,
    ) -> Result<agentix_domain::WorkspaceDirectoryPage, AgentError> {
        (self
            .workspace
            .as_ref()
            .ok_or_else(|| rejected("multiplexer is not configured"))?)
        .list_directories(directory, page, show_hidden)
        .await
        .map_err(|e| AgentError::Rejected(e.to_string()))
    }
    async fn snapshot(&self) -> Result<Option<MultiplexerSnapshot>, AgentError> {
        let sessions = self.list_sessions(None, u32::MAX).await?.sessions;
        self.workspace
            .as_ref()
            .ok_or_else(|| rejected("multiplexer is not configured"))?
            .snapshot(&sessions)
            .await
            .map_err(unavailable)
    }
    async fn mutate(
        &self,
        mutation: MultiplexerMutation,
    ) -> Result<MultiplexerMutationResult, AgentError> {
        let workspace = self
            .workspace
            .as_ref()
            .ok_or_else(|| rejected("multiplexer launch is not configured"))?;
        let prepared = workspace.prepare(mutation).await.map_err(unavailable)?;
        let known: HashSet<_> = self
            .list_sessions(None, u32::MAX)
            .await?
            .sessions
            .into_iter()
            .map(|s| s.id)
            .collect();
        let outcome = workspace.execute(&prepared).await.map_err(unavailable)?;
        if !prepared.mutation.launch_agent {
            return Ok(MultiplexerMutationResult {
                pane_id: outcome.location.pane_id.clone(),
                message: "Shell created".into(),
                session: None,
            });
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let page = self.list_sessions(None, u32::MAX).await?;
            if let Some(session) = page.sessions.into_iter().find(|s| {
                !known.contains(&s.id)
                    && s.terminal.as_ref().is_some_and(|t| {
                        t.multiplexer == outcome.location.multiplexer
                            && t.pane_id == outcome.location.pane_id
                    })
            }) {
                self.attach(&session.id).await?;
                return Ok(MultiplexerMutationResult {
                    pane_id: outcome.location.pane_id.clone(),
                    message: format!(
                        "{} started in {} pane {}",
                        self.display_name(),
                        outcome.location.multiplexer,
                        outcome.location.pane_id
                    ),
                    session: Some(session),
                });
            }
            if tokio::time::Instant::now() >= deadline
                || !workspace
                    .pane_exists(&outcome.location)
                    .await
                    .map_err(unavailable)?
            {
                return Err(unavailable(format!(
                    "{} did not register a live bridge in pane {}. Inspect that terminal and ensure the bridge extension is installed. The terminal has been left open.",
                    self.display_name(),
                    outcome.location.pane_id
                )));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

#[cfg(test)]
mod connection_tests {
    use super::*;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    struct StalledWriter;
    impl AsyncWrite for StalledWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Poll::Pending
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
    fn stalled_connection() -> (Arc<Connection>, tokio::io::DuplexStream) {
        let snapshot = serde_json::from_str(include_str!(
            "../../../plugins/agentix-bridge/protocol/snapshot.json"
        ))
        .unwrap();
        let record = wire::Registration {
            version: 2,
            agent: "pi".into(),
            instance: "contract-instance".into(),
            pid: 1,
            session_id: "native-id".into(),
            cwd: "/tmp".into(),
            session_file: "/tmp/session.jsonl".into(),
            snapshot,
        };
        let (reader, remote) = tokio::io::duplex(1024);
        let connection = Connection::new(
            &record,
            BufReader::new(Box::new(reader)),
            Box::new(StalledWriter),
            broadcast::channel(16).0,
            std::sync::Weak::new(),
        );
        (connection, remote)
    }
    #[tokio::test]
    async fn invalid_or_misrouted_events_invalidate_the_connection() {
        for event in [
            json!({"TurnStarted":{"session_id":"native-id"}}),
            json!({"TurnStarted":{"session_id":"other","turn_id":"turn"}}),
        ] {
            let (connection, mut remote) = stalled_connection();
            remote
                .write_all(
                    format!(
                        "{}\n",
                        json!({"instance":"contract-instance","seq":4,"event":event})
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            tokio::task::yield_now().await;
            assert!(!connection.online.load(Ordering::Acquire));
        }
    }
    #[tokio::test(start_paused = true)]
    async fn oversized_requests_are_rejected_before_touching_the_stream() {
        let (connection, _remote) = stalled_connection();
        let result = connection
            .request(
                "prompt",
                json!({"text":"x".repeat(usize::try_from(MAX_FRAME).unwrap())}),
            )
            .await;
        assert!(matches!(result, Err(AgentError::Protocol(_))));
        assert!(connection.online.load(Ordering::Acquire));
    }
    #[tokio::test(start_paused = true)]
    async fn request_deadline_includes_blocked_writes() {
        use tracing::instrument::WithSubscriber;
        let output = tempfile::NamedTempFile::new().unwrap();
        let writer = output.reopen().unwrap();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.try_clone().unwrap())
            .finish();
        let (connection, _remote) = stalled_connection();
        let result = tokio::time::timeout(
            Duration::from_secs(11),
            connection.request("prompt", json!({"text":"hello"})),
        )
        .with_subscriber(subscriber)
        .await;
        assert!(
            result.is_ok(),
            "the request must time out before its outer caller"
        );
        assert!(result.unwrap().is_err());
        let log = std::fs::read_to_string(output.path()).unwrap();
        assert!(log.contains("backend request timed out"), "{log}");
        assert!(log.contains("backend=pi"), "{log}");
        assert!(log.contains("method=\"prompt\""), "{log}");
        assert!(!log.contains("hello"));
    }
    #[tokio::test]
    async fn cancelling_a_request_releases_its_pending_response_slot() {
        let (connection, _remote) = stalled_connection();
        let task = tokio::spawn({
            let connection = connection.clone();
            async move { connection.request("prompt", json!({"text":"hello"})).await }
        });
        tokio::task::yield_now().await;
        assert_eq!(connection.pending.lock().unwrap().len(), 1);
        task.abort();
        let _ = task.await;
        assert_eq!(connection.pending.lock().unwrap().len(), 0);
    }
}
