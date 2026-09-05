use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use agentix_core::{
    AgentAdapter, AgentError, AgentEvent, GoalCommand, HistoryPage, InteractionDecision,
    MultiplexerMutation, MultiplexerMutationResult, MultiplexerSnapshot, QueuedPrompt,
    QueuedPromptPort, SessionCommand, SessionCommandChoice, SessionCommandResult,
    SessionControlPort, SessionId, SessionPage, SessionStatus, SessionSummary, TerminalLocation,
    ToolSummary, TurnSummary, WorkspaceRuntimePort,
};
use async_trait::async_trait;
use futures_util::future::try_join_all;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::sync::{Mutex, Notify, broadcast, oneshot};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use crate::endpoint::CodexEndpoint;
use crate::multiplexer::{RmuxManager, started_session};
use crate::process::{
    CodexProcessDiscovery, confirm_exited_sessions, reappeared_sessions,
    select_running_session_ids, session_terminal_locations,
};
use crate::protocol::{
    ModelDescriptor, ModelListResult, ProtocolError, QueueAddResult, QueueListResult,
    QueuedSubmission, RpcError, ServerMessage, TurnStartResult, TurnSteerResult,
    decode_server_frame, item_summary, parse_session_status, parse_turn_status,
};

type Socket = WebSocketStream<UnixStream>;
type Writer = SplitSink<Socket, Message>;
type Reader = SplitStream<Socket>;
type PendingMap = HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>;

struct ConnectionState {
    generation: AtomicU64,
    changed: Notify,
}

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(10);
const DAEMON_READY_TIMEOUT: Duration = Duration::from_secs(5);
const DAEMON_READY_POLL_INTERVAL: Duration = Duration::from_millis(100);
const RUNNING_SESSION_POLL_INTERVAL: Duration = Duration::from_secs(2);
const MULTIPLEXER_SESSION_START_TIMEOUT: Duration = Duration::from_secs(10);
const MULTIPLEXER_SESSION_POLL_INTERVAL: Duration = Duration::from_millis(200);
const PENDING_RESUME_TIMEOUT: Duration = Duration::from_secs(3);
const PENDING_RESUME_POLL_INTERVAL: Duration = Duration::from_millis(50);
const IDEMPOTENT_REQUEST_RETRY_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("failed to connect to Codex socket: {0}")]
    Connect(#[from] std::io::Error),
    #[error("failed to run Codex daemon start command {command}: {source}")]
    DaemonStart {
        command: PathBuf,
        source: std::io::Error,
    },
    #[error("Codex daemon start command timed out: {0}")]
    DaemonStartTimeout(PathBuf),
    #[error("Codex daemon start command {command} exited with {status}: {stderr}")]
    DaemonStartFailed {
        command: PathBuf,
        status: ExitStatus,
        stderr: String,
    },
    #[error("Codex WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("Codex protocol error: {0}")]
    Protocol(#[from] ProtocolError),
    #[error("Codex RPC {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("Codex RPC response channel closed")]
    ResponseClosed,
    #[error("Codex RPC request timed out")]
    Timeout,
    #[error("Codex response has an invalid shape: {0}")]
    InvalidResponse(&'static str),
    #[error("invalid running-session cursor")]
    InvalidCursor,
    #[error("Codex session {0} has no rollout and cannot be attached")]
    NoRollout(SessionId),
    #[error("Codex process discovery failed: {0}")]
    ProcessDiscovery(String),
    #[error("Codex process discovery task failed: {0}")]
    ProcessDiscoveryTask(#[from] tokio::task::JoinError),
}

#[derive(Clone)]
pub struct CodexClient {
    writer: Arc<Mutex<Writer>>,
    pending: Arc<Mutex<PendingMap>>,
    next_id: Arc<AtomicI64>,
    events: broadcast::Sender<AgentEvent>,
    connection: Arc<ConnectionState>,
    subscriptions: Arc<Mutex<HashSet<SessionId>>>,
    process_sessions: Arc<Mutex<HashSet<SessionId>>>,
    exited_process_sessions: Arc<Mutex<HashSet<SessionId>>>,
    pending_resumes: Arc<Mutex<HashSet<SessionId>>>,
    token_usage: Arc<Mutex<HashMap<SessionId, Value>>>,
    process_discovery: Option<CodexProcessDiscovery>,
    rmux: RmuxManager,
}

impl CodexClient {
    pub async fn connect(endpoint: CodexEndpoint) -> Result<Self, ClientError> {
        Self::connect_with_command(endpoint, Path::new("codex")).await
    }

    pub async fn connect_with_command(
        endpoint: CodexEndpoint,
        command: &Path,
    ) -> Result<Self, ClientError> {
        Self::connect_with_command_and_rmux_directory(endpoint, command, Path::new("~")).await
    }

    pub async fn connect_with_command_and_rmux_directory(
        endpoint: CodexEndpoint,
        command: &Path,
        rmux_directory: &Path,
    ) -> Result<Self, ClientError> {
        let process_discovery = CodexProcessDiscovery::for_endpoint(&endpoint);
        let rmux = RmuxManager::new(command, endpoint.socket_path(), rmux_directory);
        let websocket = connect_managed_socket(&endpoint, command).await?;
        let (writer, reader) = websocket.split();
        let writer = Arc::new(Mutex::new(writer));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let subscriptions = Arc::new(Mutex::new(HashSet::new()));
        let process_sessions = Arc::new(Mutex::new(HashSet::new()));
        let exited_process_sessions = Arc::new(Mutex::new(HashSet::new()));
        let pending_resumes = Arc::new(Mutex::new(HashSet::new()));
        let token_usage = Arc::new(Mutex::new(HashMap::new()));
        let (events, _) = broadcast::channel(512);
        let connection = Arc::new(ConnectionState {
            generation: AtomicU64::new(NEXT_GENERATION.fetch_add(1, Ordering::Relaxed)),
            changed: Notify::new(),
        });
        tokio::spawn(read_loop(
            reader,
            Arc::clone(&writer),
            Arc::clone(&pending),
            Arc::clone(&subscriptions),
            Arc::clone(&token_usage),
            events.clone(),
            endpoint,
            Arc::clone(&connection),
        ));

        let client = Self {
            writer,
            pending,
            next_id: Arc::new(AtomicI64::new(2)),
            events,
            connection,
            subscriptions,
            process_sessions,
            exited_process_sessions,
            pending_resumes,
            token_usage,
            process_discovery,
            rmux,
        };
        let _ = client.events.send(AgentEvent::Connected {
            generation: client.connection.generation.load(Ordering::Acquire),
        });
        tokio::spawn(monitor_running_sessions(client.clone()));
        Ok(client)
    }

    pub async fn list_sessions(
        &self,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<SessionPage, ClientError> {
        let Some(discovery) = self.process_discovery.clone() else {
            return self.list_loaded_sessions(cursor, limit).await;
        };
        let (loaded_ids, _) = self.loaded_session_ids(None, None).await?;
        let loaded = self.read_sessions(&loaded_ids).await?;
        let snapshot = discovery
            .discover()
            .await
            .map_err(|error| ClientError::ProcessDiscovery(error.to_string()))?;
        let selected = select_running_session_ids(&loaded, &snapshot);
        let terminal_locations = session_terminal_locations(&loaded, &snapshot);
        let loaded_ids = loaded_ids.into_iter().collect::<HashSet<_>>();
        let missing_ids = selected
            .difference(&loaded_ids)
            .cloned()
            .collect::<Vec<_>>();
        let mut sessions = loaded
            .into_iter()
            .filter(|session| selected.contains(&session.id))
            .collect::<Vec<_>>();
        let mut direct_sessions = self.read_sessions(&missing_ids).await?;
        for session in &mut direct_sessions {
            if session.status == SessionStatus::NotLoaded {
                session.status = SessionStatus::Unknown;
            }
        }
        sessions.extend(direct_sessions);
        for session in &mut sessions {
            session.terminal = terminal_locations.get(&session.id).cloned();
        }
        sessions.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.id.as_str().cmp(left.id.as_str()))
        });
        page_running_sessions(&sessions, cursor.as_deref(), limit)
    }

    async fn list_loaded_sessions(
        &self,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<SessionPage, ClientError> {
        let (ids, next_cursor) = self.loaded_session_ids(cursor, Some(limit)).await?;
        let sessions = self.read_sessions(&ids).await?;
        Ok(SessionPage {
            sessions,
            next_cursor,
        })
    }

    async fn loaded_session_ids(
        &self,
        cursor: Option<String>,
        limit: Option<u32>,
    ) -> Result<(Vec<SessionId>, Option<String>), ClientError> {
        let result = self
            .request(
                "thread/loaded/list",
                json!({
                    "cursor": cursor,
                    "limit": limit
                }),
            )
            .await?;
        let data = result
            .get("data")
            .and_then(Value::as_array)
            .ok_or(ClientError::InvalidResponse("thread/loaded/list data"))?;
        let ids = data
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(SessionId::new)
                    .ok_or(ClientError::InvalidResponse("loaded thread id"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((
            ids,
            result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ))
    }

    async fn read_sessions(&self, ids: &[SessionId]) -> Result<Vec<SessionSummary>, ClientError> {
        let sessions = try_join_all(ids.iter().map(|id| async move {
            let thread = self.read_thread(id, false).await?;
            if !thread_has_rollout(&thread) {
                return Ok(None);
            }
            parse_session_summary(&thread).map(Some)
        }))
        .await?;
        Ok(sessions.into_iter().flatten().collect())
    }

    async fn read_thread(
        &self,
        session_id: &SessionId,
        include_turns: bool,
    ) -> Result<Value, ClientError> {
        self.request_after_reconnect(
            "thread/read",
            json!({
                "threadId": session_id.as_str(),
                "includeTurns": include_turns
            }),
        )
        .await?
        .get("thread")
        .cloned()
        .ok_or(ClientError::InvalidResponse("thread/read thread"))
    }

    async fn wait_for_rmux_session(
        &self,
        location: &TerminalLocation,
        known_sessions: &HashSet<SessionId>,
        cwd: &Path,
    ) -> Result<SessionSummary, AgentError> {
        let deadline = tokio::time::Instant::now() + MULTIPLEXER_SESSION_START_TIMEOUT;
        loop {
            let discovery_error = match self.list_sessions(None, u32::MAX).await {
                Ok(page) => {
                    if let Some(session) =
                        started_session(&page.sessions, location, known_sessions, cwd)
                    {
                        let mut session = session.clone();
                        session.terminal = Some(location.clone());
                        return Ok(session);
                    }
                    None
                }
                Err(error) => Some(error.to_string()),
            };

            let pane_exists = RmuxManager::pane_exists(location)
                .await
                .map_err(|error| AgentError::Rejected(error.to_string()))?;
            if !pane_exists {
                return Err(AgentError::Rejected(format!(
                    "Codex exited before creating a session in rmux pane {}",
                    location.pane_id
                )));
            }
            if tokio::time::Instant::now() >= deadline {
                let detail = discovery_error
                    .map(|error| format!("; last discovery error: {error}"))
                    .unwrap_or_default();
                return Err(AgentError::Rejected(format!(
                    "timed out waiting for Codex to create a session in rmux pane {}{}",
                    location.pane_id, detail
                )));
            }
            tokio::time::sleep(MULTIPLEXER_SESSION_POLL_INTERVAL).await;
        }
    }

    async fn resume_pending_session(&self, session_id: &SessionId) -> Result<(), ClientError> {
        if !self.pending_resumes.lock().await.contains(session_id) {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now() + PENDING_RESUME_TIMEOUT;
        loop {
            if self.try_resume_pending_session(session_id).await? {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ClientError::NoRollout(session_id.clone()));
            }
            tokio::time::sleep(PENDING_RESUME_POLL_INTERVAL).await;
        }
    }

    async fn try_resume_pending_session(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, ClientError> {
        if !self.pending_resumes.lock().await.contains(session_id) {
            return Ok(true);
        }
        match self
            .request_after_reconnect("thread/resume", thread_resume_params(session_id))
            .await
        {
            Ok(_) => {
                self.pending_resumes.lock().await.remove(session_id);
                Ok(true)
            }
            Err(error) if is_rollout_initializing(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn resume_exited_session(&self, session_id: &SessionId) -> Result<bool, ClientError> {
        if !self
            .exited_process_sessions
            .lock()
            .await
            .contains(session_id)
        {
            return Ok(false);
        }
        let provisional = match self
            .request_after_reconnect("thread/resume", thread_resume_params(session_id))
            .await
        {
            Ok(_) => false,
            Err(error) if is_rollout_initializing(&error) => true,
            Err(error) => return Err(error),
        };
        if !self.exited_process_sessions.lock().await.remove(session_id) {
            return Ok(false);
        }
        self.subscriptions.lock().await.insert(session_id.clone());
        if provisional {
            self.pending_resumes.lock().await.insert(session_id.clone());
        } else {
            self.pending_resumes.lock().await.remove(session_id);
        }
        let _ = self.events.send(AgentEvent::SessionResumed {
            session_id: session_id.to_string(),
        });
        Ok(true)
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        let write_result = self
            .writer
            .lock()
            .await
            .send(Message::Text(
                json!({"id": id, "method": method, "params": params})
                    .to_string()
                    .into(),
            ))
            .await;
        if let Err(error) = write_result {
            self.pending.lock().await.remove(&id);
            return Err(ClientError::WebSocket(error));
        }
        let response = tokio::time::timeout(Duration::from_secs(30), receiver)
            .await
            .map_err(|_| ClientError::Timeout)?
            .map_err(|_| ClientError::ResponseClosed)?;
        response.map_err(|error| ClientError::Rpc {
            code: error.code,
            message: error.message,
        })
    }

    async fn request_after_reconnect(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, ClientError> {
        let deadline = tokio::time::Instant::now() + IDEMPOTENT_REQUEST_RETRY_TIMEOUT;
        loop {
            let generation = self.connection.generation.load(Ordering::Acquire);
            let result = tokio::time::timeout_at(deadline, self.request(method, params.clone()))
                .await
                .map_err(|_| ClientError::Timeout)?;
            match result {
                Err(ClientError::ResponseClosed | ClientError::WebSocket(_)) => {
                    self.wait_for_reconnect(generation, deadline).await?;
                }
                result => return result,
            }
        }
    }

    async fn wait_for_reconnect(
        &self,
        generation: u64,
        deadline: tokio::time::Instant,
    ) -> Result<(), ClientError> {
        tokio::time::timeout_at(deadline, async {
            loop {
                let notified = self.connection.changed.notified();
                if self.connection.generation.load(Ordering::Acquire) != generation {
                    return;
                }
                notified.await;
            }
        })
        .await
        .map_err(|_| ClientError::Timeout)
    }

    pub async fn respond(&self, id: Value, result: Value) -> Result<(), ClientError> {
        self.writer
            .lock()
            .await
            .send(Message::Text(
                json!({"id": id, "result": result}).to_string().into(),
            ))
            .await?;
        Ok(())
    }

    async fn paged_history(
        &self,
        session_id: &SessionId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<HistoryPage, ClientError> {
        let result = self
            .request_after_reconnect(
                "thread/turns/list",
                json!({
                    "threadId": session_id.as_str(),
                    "cursor": cursor,
                    "limit": limit,
                    "sortDirection": "desc",
                    "itemsView": "full"
                }),
            )
            .await?;
        history_from_result(&result)
    }

    async fn stable_history(
        &self,
        session_id: &SessionId,
        limit: u32,
    ) -> Result<HistoryPage, ClientError> {
        let result = self
            .request_after_reconnect(
                "thread/read",
                json!({"threadId": session_id.as_str(), "includeTurns": true}),
            )
            .await?;
        let turns = result
            .get("thread")
            .and_then(|thread| thread.get("turns"))
            .and_then(Value::as_array)
            .ok_or(ClientError::InvalidResponse("thread/read turns"))?;
        let start = turns.len().saturating_sub(limit as usize);
        let parsed = turns[start..]
            .iter()
            .map(parse_turn_summary)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(HistoryPage {
            turns: parsed,
            older_cursor: (start > 0).then(|| format!("fallback:{start}")),
            newer_cursor: None,
        })
    }

    async fn available_models(&self) -> Result<Vec<ModelDescriptor>, ClientError> {
        let mut models = Vec::new();
        let mut cursor = None;
        loop {
            let result = self
                .request(
                    "model/list",
                    json!({
                        "cursor": cursor,
                        "limit": 100,
                        "includeHidden": false
                    }),
                )
                .await?;
            let page: ModelListResult = decode_result(result, "model/list result")?;
            models.extend(page.data);
            let next_cursor = page.next_cursor;
            if next_cursor.is_none() {
                break;
            }
            if next_cursor == cursor {
                return Err(ClientError::InvalidResponse("model/list repeated cursor"));
            }
            cursor = next_cursor;
        }
        Ok(models)
    }

    async fn fork_thread(
        &self,
        source_session: &SessionId,
    ) -> Result<SessionCommandResult, ClientError> {
        let result = self
            .request(
                "thread/fork",
                json!({
                    "threadId": source_session.as_str(),
                    "excludeTurns": true
                }),
            )
            .await?;
        self.replacement_result(result, "Forked session", "Forked and attached the session.")
            .await
    }

    async fn replacement_result(
        &self,
        result: Value,
        title: &str,
        body: &str,
    ) -> Result<SessionCommandResult, ClientError> {
        let thread = result
            .get("thread")
            .ok_or(ClientError::InvalidResponse("thread command response"))?;
        let session = parse_session_summary(thread)?;
        self.subscriptions.lock().await.insert(session.id.clone());
        Ok(Self::replacement_session_result(session, title, body))
    }

    fn replacement_session_result(
        session: SessionSummary,
        title: &str,
        body: &str,
    ) -> SessionCommandResult {
        SessionCommandResult {
            title: format!("Codex · {title}"),
            body: body.into(),
            replacement_session: Some(session),
            active_turn: None,
            choices: Vec::new(),
        }
    }

    async fn model_command(
        &self,
        session_id: &SessionId,
        requested: Option<String>,
    ) -> Result<SessionCommandResult, ClientError> {
        let models = self.available_models().await?;
        if let Some(requested) = requested {
            let selected = models.iter().find(|model| {
                model.id.as_deref() == Some(requested.as_str())
                    || model.model.as_deref() == Some(requested.as_str())
            });
            let selected = selected.ok_or_else(|| ClientError::Rpc {
                code: -32602,
                message: format!("unknown model: {requested}"),
            })?;
            let model = selected
                .identifier()
                .ok_or(ClientError::InvalidResponse("model identifier"))?;
            self.request(
                "thread/settings/update",
                json!({"threadId": session_id.as_str(), "model": model}),
            )
            .await?;
            return Ok(SessionCommandResult::message(
                "Codex · Model",
                format!("Model changed to `{model}` for subsequent turns."),
            ));
        }

        let thread = self.read_thread(session_id, false).await?;
        let current = thread
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("default");
        let available = models
            .iter()
            .filter_map(|model| model.id.as_deref().or(model.model.as_deref()))
            .map(|model| format!("- `{model}`"))
            .collect::<Vec<_>>()
            .join("\n");
        let choices = models
            .iter()
            .filter_map(|model| {
                let id = model.id.as_deref().or(model.model.as_deref())?;
                let label = model.display_name.as_deref().unwrap_or(id);
                Some(SessionCommandChoice::new(
                    label,
                    SessionCommand::Model(Some(id.to_owned())),
                ))
            })
            .collect();
        Ok(SessionCommandResult {
            title: "Codex · Model".into(),
            body: format!("**Current:** `{current}`\n\n**Available models**\n\n{available}"),
            replacement_session: None,
            active_turn: None,
            choices,
        })
    }

    async fn reasoning_command(
        &self,
        session_id: &SessionId,
        requested: Option<String>,
    ) -> Result<SessionCommandResult, ClientError> {
        let thread = self.read_thread(session_id, false).await?;
        let models = self.available_models().await?;
        let current_model = thread.get("model").and_then(Value::as_str);
        let supported = models
            .iter()
            .find(|model| {
                model.id.as_deref() == current_model || model.model.as_deref() == current_model
            })
            .into_iter()
            .flat_map(|model| &model.supported_reasoning_efforts)
            .map(|effort| effort.reasoning_effort.as_str())
            .collect::<Vec<_>>();

        if let Some(requested) = requested {
            if !supported.is_empty() && !supported.contains(&requested.as_str()) {
                return Err(ClientError::Rpc {
                    code: -32602,
                    message: format!("unsupported reasoning effort: {requested}"),
                });
            }
            self.request(
                "thread/settings/update",
                json!({"threadId": session_id.as_str(), "effort": requested}),
            )
            .await?;
            return Ok(SessionCommandResult::message(
                "Codex · Reasoning",
                format!("Reasoning effort changed to `{requested}` for subsequent turns."),
            ));
        }

        let current = thread
            .get("reasoningEffort")
            .and_then(Value::as_str)
            .unwrap_or("default");
        let available = if supported.is_empty() {
            "Not advertised by the current model.".into()
        } else {
            supported
                .iter()
                .map(|effort| format!("`{effort}`"))
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let choices = supported
            .iter()
            .map(|effort| {
                SessionCommandChoice::new(
                    title_case(effort),
                    SessionCommand::Reasoning(Some((*effort).to_owned())),
                )
            })
            .collect();
        Ok(SessionCommandResult {
            title: "Codex · Reasoning".into(),
            body: format!("**Current:** `{current}`\n\n**Available:** {available}"),
            replacement_session: None,
            active_turn: None,
            choices,
        })
    }

    async fn plan_command(
        &self,
        session_id: &SessionId,
        enabled: bool,
    ) -> Result<SessionCommandResult, ClientError> {
        let thread = self.read_thread(session_id, false).await?;
        let model = thread
            .get("model")
            .and_then(Value::as_str)
            .ok_or(ClientError::InvalidResponse("thread model"))?;
        let effort = thread
            .get("reasoningEffort")
            .cloned()
            .unwrap_or(Value::Null);
        let mode = if enabled { "plan" } else { "default" };
        self.request(
            "thread/settings/update",
            json!({
                "threadId": session_id.as_str(),
                "collaborationMode": {
                    "mode": mode,
                    "settings": {
                        "model": model,
                        "reasoning_effort": effort,
                        "developer_instructions": null
                    }
                }
            }),
        )
        .await?;
        Ok(SessionCommandResult::message(
            "Codex · Plan",
            if enabled {
                "Plan mode enabled for subsequent turns. Use `/plan off` to return to default mode."
            } else {
                "Plan mode disabled for subsequent turns."
            },
        ))
    }

    async fn fast_command(
        &self,
        session_id: &SessionId,
        requested: Option<bool>,
    ) -> Result<SessionCommandResult, ClientError> {
        let context = self
            .request("thread/resume", thread_resume_params(session_id))
            .await?;
        let current_model = context
            .get("model")
            .and_then(Value::as_str)
            .ok_or(ClientError::InvalidResponse("thread model"))?;
        let models = self.available_models().await?;
        let model = models
            .iter()
            .find(|model| model.identifier() == Some(current_model))
            .ok_or(ClientError::InvalidResponse("current model descriptor"))?;
        let fast = model
            .service_tiers
            .iter()
            .find(|tier| tier.id == "fast")
            .ok_or_else(|| ClientError::Rpc {
                code: -32602,
                message: format!("model {current_model} does not support Fast mode"),
            })?;
        let enabled = requested.unwrap_or_else(|| {
            context.get("serviceTier").and_then(Value::as_str) != Some(fast.id.as_str())
        });
        self.request(
            "thread/settings/update",
            json!({
                "threadId": session_id.as_str(),
                "serviceTier": if enabled { Value::String(fast.id.clone()) } else { Value::Null }
            }),
        )
        .await?;
        Ok(SessionCommandResult::message(
            "Codex · Fast",
            if enabled {
                format!("Fast mode enabled using `{}`.", fast.name)
            } else {
                "Fast mode disabled; the model's default service tier is active.".into()
            },
        ))
    }

    async fn clear_command(
        &self,
        session_id: &SessionId,
        name: Option<String>,
    ) -> Result<SessionCommandResult, ClientError> {
        let context = self
            .request("thread/resume", thread_resume_params(session_id))
            .await?;
        let result = self
            .request(
                "thread/start",
                json!({
                    "cwd": context.get("cwd").cloned().unwrap_or(Value::Null),
                    "model": context.get("model").cloned().unwrap_or(Value::Null),
                    "approvalPolicy": context.get("approvalPolicy").cloned().unwrap_or(Value::Null),
                    "sandbox": context.get("sandbox").cloned().unwrap_or(Value::Null),
                    "serviceTier": context.get("serviceTier").cloned().unwrap_or(Value::Null)
                }),
            )
            .await?;
        let thread = result
            .get("thread")
            .ok_or(ClientError::InvalidResponse("thread/start thread"))?;
        let new_id = thread
            .get("id")
            .and_then(Value::as_str)
            .ok_or(ClientError::InvalidResponse("thread/start thread id"))?;
        if let Some(name) = &name {
            self.request("thread/name/set", json!({"threadId": new_id, "name": name}))
                .await?;
        }
        if let Some(effort) = context.get("reasoningEffort").and_then(Value::as_str) {
            self.request(
                "thread/settings/update",
                json!({"threadId": new_id, "effort": effort}),
            )
            .await?;
        }
        let mut session = parse_session_summary(thread)?;
        session.name = name.or(session.name);
        self.subscriptions.lock().await.insert(session.id.clone());
        self.pending_resumes.lock().await.insert(session.id.clone());
        Ok(Self::replacement_session_result(
            session,
            "Clear",
            "Started and attached a fresh session.",
        ))
    }

    async fn rename_command(
        &self,
        session_id: &SessionId,
        name: Option<String>,
    ) -> Result<SessionCommandResult, ClientError> {
        let name = name.ok_or_else(|| ClientError::Rpc {
            code: -32602,
            message: "usage: /rename <name>".into(),
        })?;
        self.request(
            "thread/name/set",
            json!({"threadId": session_id.as_str(), "name": name}),
        )
        .await?;
        Ok(SessionCommandResult::message(
            "Codex · Rename",
            format!("Session renamed to **{name}**."),
        ))
    }

    async fn diff_command(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionCommandResult, ClientError> {
        let thread = self.read_thread(session_id, false).await?;
        let cwd = thread
            .get("cwd")
            .and_then(Value::as_str)
            .ok_or(ClientError::InvalidResponse("thread cwd"))?;
        let inside = Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .current_dir(cwd)
            .output()
            .await;
        let Ok(inside) = inside else {
            return Ok(SessionCommandResult::message(
                "Codex · Diff",
                format!("`{}` is not a Git worktree.", abbreviate_home(cwd)),
            ));
        };
        if !inside.status.success() {
            return Ok(SessionCommandResult::message(
                "Codex · Diff",
                format!("`{}` is not a Git worktree.", abbreviate_home(cwd)),
            ));
        }
        let mut diff = String::new();
        for arguments in [
            ["diff", "--cached", "--no-ext-diff", "--color=never"],
            ["diff", "--no-ext-diff", "--color=never", ""],
        ] {
            let arguments = arguments
                .iter()
                .filter(|argument| !argument.is_empty())
                .copied()
                .collect::<Vec<_>>();
            let output = Command::new("git")
                .args(arguments)
                .current_dir(cwd)
                .output()
                .await?;
            diff.push_str(&String::from_utf8_lossy(&output.stdout));
        }
        let untracked = Command::new("git")
            .args(["ls-files", "--others", "--exclude-standard", "-z"])
            .current_dir(cwd)
            .output()
            .await?;
        let null_device = if cfg!(windows) { "NUL" } else { "/dev/null" };
        for path in String::from_utf8_lossy(&untracked.stdout)
            .split('\0')
            .filter(|path| !path.is_empty())
        {
            let output = Command::new("git")
                .args([
                    "diff",
                    "--no-index",
                    "--no-ext-diff",
                    "--color=never",
                    "--",
                    null_device,
                    path,
                ])
                .current_dir(cwd)
                .output()
                .await?;
            diff.push_str(&String::from_utf8_lossy(&output.stdout));
        }
        Ok(SessionCommandResult::message(
            "Codex · Diff",
            if diff.is_empty() {
                "No staged, unstaged, or untracked changes.".into()
            } else {
                format!("```diff\n{}\n```", diff.trim_end())
            },
        ))
    }

    async fn goal_command(
        &self,
        session_id: &SessionId,
        command: GoalCommand,
    ) -> Result<SessionCommandResult, ClientError> {
        match command {
            GoalCommand::Show => {
                let result = self
                    .request("thread/goal/get", json!({"threadId": session_id.as_str()}))
                    .await?;
                Ok(SessionCommandResult::message(
                    "Codex · Goal",
                    render_goal(result.get("goal")),
                ))
            }
            GoalCommand::Set(objective) => {
                let result = self
                    .request(
                        "thread/goal/set",
                        json!({"threadId": session_id.as_str(), "objective": objective}),
                    )
                    .await?;
                Ok(SessionCommandResult::message(
                    "Codex · Goal",
                    render_goal(result.get("goal")),
                ))
            }
            GoalCommand::Pause | GoalCommand::Resume => {
                let status = if matches!(command, GoalCommand::Pause) {
                    "paused"
                } else {
                    "active"
                };
                let result = self
                    .request(
                        "thread/goal/set",
                        json!({"threadId": session_id.as_str(), "status": status}),
                    )
                    .await?;
                Ok(SessionCommandResult::message(
                    "Codex · Goal",
                    render_goal(result.get("goal")),
                ))
            }
            GoalCommand::Clear => {
                self.request(
                    "thread/goal/clear",
                    json!({"threadId": session_id.as_str()}),
                )
                .await?;
                Ok(SessionCommandResult::message(
                    "Codex · Goal",
                    "The thread goal was cleared.",
                ))
            }
        }
    }

    async fn skills_command(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionCommandResult, ClientError> {
        let thread = self.read_thread(session_id, false).await?;
        let cwd = thread
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let result = self
            .request("skills/list", json!({"cwds": [cwd], "forceReload": false}))
            .await?;
        let skills = result
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|entry| {
                entry
                    .get("skills")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter(|skill| skill.get("enabled").and_then(Value::as_bool) != Some(false))
            .filter_map(|skill| {
                let name = skill.get("name")?.as_str()?;
                let scope = skill
                    .get("scope")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                Some(format!("- **{name}** · `{scope}`"))
            })
            .collect::<Vec<_>>();
        Ok(SessionCommandResult::message(
            "Codex · Skills",
            if skills.is_empty() {
                "No enabled skills are available for this workspace.".into()
            } else {
                skills.join("\n")
            },
        ))
    }

    async fn review_command(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionCommandResult, ClientError> {
        let result: TurnStartResult = decode_result(
            self.request(
                "review/start",
                json!({
                    "threadId": session_id.as_str(),
                    "target": {"type": "uncommittedChanges"}
                }),
            )
            .await?,
            "review/start result",
        )?;
        Ok(SessionCommandResult {
            title: "Codex · Review".into(),
            body: "Reviewing staged, unstaged, and untracked changes.".into(),
            replacement_session: None,
            active_turn: Some(result.turn.id),
            choices: Vec::new(),
        })
    }

    async fn status_command(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionCommandResult, ClientError> {
        let context = self
            .request("thread/resume", thread_resume_params(session_id))
            .await?;
        let thread = context
            .get("thread")
            .ok_or(ClientError::InvalidResponse("thread/resume thread"))?;
        let goal = self
            .request("thread/goal/get", json!({"threadId": session_id.as_str()}))
            .await?;
        let name = thread
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Untitled");
        let cwd = thread
            .get("cwd")
            .and_then(Value::as_str)
            .map_or_else(|| "unknown".into(), abbreviate_home);
        let model = thread
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("default");
        let effort = thread
            .get("reasoningEffort")
            .and_then(Value::as_str)
            .unwrap_or("default");
        let status = thread
            .get("status")
            .and_then(|status| status.get("type").or(Some(status)))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let approval = context
            .get("approvalPolicy")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let sandbox = context
            .get("sandbox")
            .and_then(|sandbox| sandbox.get("type"))
            .and_then(Value::as_str)
            .map_or_else(|| "unknown".into(), kebab_case);
        let service_tier = context
            .get("serviceTier")
            .and_then(Value::as_str)
            .unwrap_or("default");
        let writable_roots = context
            .get("runtimeWorkspaceRoots")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(abbreviate_home)
            .map(|root| format!("`{root}`"))
            .collect::<Vec<_>>();
        let usage = self.token_usage.lock().await.get(session_id).cloned();
        let (context_usage, total_usage) = usage.as_ref().map_or_else(
            || ("not reported yet".into(), "not reported yet".into()),
            |usage| {
                let total = usage
                    .get("total")
                    .and_then(|total| total.get("totalTokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let last = usage
                    .get("last")
                    .and_then(|last| last.get("totalTokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let window = usage
                    .get("modelContextWindow")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let context =
                    if let Some(percent_tenths) = last.saturating_mul(1_000).checked_div(window) {
                        format!(
                            "{last} / {window} tokens ({}.{:01}%)",
                            percent_tenths / 10,
                            percent_tenths % 10
                        )
                    } else {
                        format!("{last} tokens")
                    };
                (context, format!("{total} total"))
            },
        );
        Ok(SessionCommandResult::message(
            "Codex · Status",
            format!(
                "**Session:** {name}\n**ID:** `{}`\n**State:** `{status}`\n**Directory:** `{cwd}`\n**Model:** `{model}`\n**Reasoning:** `{effort}`\n**Service tier:** `{service_tier}`\n**Approval:** `{approval}`\n**Sandbox:** `{sandbox}`\n**Writable roots:** {}\n**Context:** {context_usage}\n**Tokens:** {total_usage}\n\n**Goal**\n\n{}",
                session_id.as_str(),
                if writable_roots.is_empty() {
                    "none".into()
                } else {
                    writable_roots.join(", ")
                },
                render_goal(goal.get("goal"))
            ),
        ))
    }

    async fn mcp_command(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionCommandResult, ClientError> {
        let result = self
            .request(
                "mcpServerStatus/list",
                json!({
                    "threadId": session_id.as_str(),
                    "detail": "toolsAndAuthOnly",
                    "limit": 100
                }),
            )
            .await?;
        let servers = result
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|server| {
                let name = server.get("name")?.as_str()?;
                let runtime = server
                    .get("runtimeStatus")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let auth = server
                    .get("authStatus")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let tools = server
                    .get("tools")
                    .and_then(Value::as_object)
                    .map_or(0, serde_json::Map::len);
                Some(format!(
                    "- **{name}** · `{runtime}` · auth `{auth}` · {tools} tools"
                ))
            })
            .collect::<Vec<_>>();
        Ok(SessionCommandResult::message(
            "Codex · MCP",
            if servers.is_empty() {
                "No MCP servers are configured for this session.".into()
            } else {
                servers.join("\n")
            },
        ))
    }
}

async fn monitor_running_sessions(client: CodexClient) {
    let mut missing_counts = HashMap::new();
    loop {
        tokio::time::sleep(RUNNING_SESSION_POLL_INTERVAL).await;
        let watched = client.process_sessions.lock().await.clone();
        let running = match discover_running_sessions(&client).await {
            Ok(running) => running,
            Err(error) => {
                tracing::warn!(%error, "failed to inspect running Codex sessions");
                continue;
            }
        };
        let pending_resumes = client.pending_resumes.lock().await.clone();
        for session in pending_resumes.intersection(&running) {
            if let Err(error) = client.try_resume_pending_session(session).await {
                tracing::warn!(%error, session = %session, "failed to subscribe to started Codex session");
            }
        }
        let exited = client.exited_process_sessions.lock().await.clone();
        for session in reappeared_sessions(&exited, &running) {
            if let Err(error) = client.resume_exited_session(&session).await {
                tracing::warn!(%error, session = %session, "failed to resume returned Codex session");
            }
        }
        let online = watched.difference(&exited).cloned().collect::<HashSet<_>>();
        for session in confirm_exited_sessions(&online, &running, &mut missing_counts) {
            client.subscriptions.lock().await.remove(&session);
            client.pending_resumes.lock().await.remove(&session);
            client
                .exited_process_sessions
                .lock()
                .await
                .insert(session.clone());
            missing_counts.remove(&session);
            let _ = client.events.send(AgentEvent::SessionExited {
                session_id: session.to_string(),
            });
            if let Err(error) = client
                .request("thread/unsubscribe", json!({"threadId": session.as_str()}))
                .await
            {
                tracing::warn!(%error, session = %session, "failed to release exited Codex session");
            }
        }
        // Transport subscriptions are independent of IM bindings. Keep observing
        // unbound sessions so their terminal events can reach the engine.
        let subscribed = client.subscriptions.lock().await.clone();
        for session in running.difference(&subscribed) {
            if !exited.contains(session)
                && let Err(error) = client.attach(session).await
            {
                tracing::warn!(%error, session = %session, "failed to observe background Codex session");
            }
        }
    }
}

async fn discover_running_sessions(
    client: &CodexClient,
) -> Result<HashSet<SessionId>, ClientError> {
    let mut running = HashSet::new();
    let mut cursor = None;
    loop {
        let page = client.list_sessions(cursor, u32::MAX).await?;
        running.extend(page.sessions.into_iter().map(|session| session.id));
        cursor = page.next_cursor;
        if cursor.is_none() {
            return Ok(running);
        }
    }
}

async fn connect_managed_socket(
    endpoint: &CodexEndpoint,
    command: &Path,
) -> Result<Socket, ClientError> {
    match connect_socket(endpoint).await {
        Ok(websocket) => Ok(websocket),
        Err(ClientError::Connect(error))
            if endpoint.codex_home().is_some() && daemon_can_fix(&error) =>
        {
            tracing::info!(
                socket = %endpoint.socket_path().display(),
                command = %command.display(),
                "Codex app-server is unavailable; starting its managed daemon"
            );
            start_daemon(command).await?;
            wait_for_managed_socket(endpoint).await
        }
        Err(error) => Err(error),
    }
}

async fn start_daemon(command: &Path) -> Result<(), ClientError> {
    let mut process = Command::new(command);
    process
        .args(["app-server", "daemon", "start"])
        .kill_on_drop(true);
    let output = tokio::time::timeout(DAEMON_START_TIMEOUT, process.output())
        .await
        .map_err(|_| ClientError::DaemonStartTimeout(command.to_owned()))?
        .map_err(|source| ClientError::DaemonStart {
            command: command.to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(ClientError::DaemonStartFailed {
            command: command.to_owned(),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(())
}

async fn wait_for_managed_socket(endpoint: &CodexEndpoint) -> Result<Socket, ClientError> {
    let deadline = tokio::time::Instant::now() + DAEMON_READY_TIMEOUT;
    loop {
        match connect_socket(endpoint).await {
            Ok(websocket) => return Ok(websocket),
            Err(ClientError::Connect(error))
                if daemon_can_fix(&error) && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(DAEMON_READY_POLL_INTERVAL).await;
            }
            Err(error) => return Err(error),
        }
    }
}

fn daemon_can_fix(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}

async fn connect_socket(endpoint: &CodexEndpoint) -> Result<Socket, ClientError> {
    let stream = UnixStream::connect(endpoint.socket_path()).await?;
    let (mut websocket, _) = tokio_tungstenite::client_async("ws://localhost/", stream).await?;
    let initialize_id = 1;
    websocket
        .send(Message::Text(
            json!({
                "id": initialize_id,
                "method": "initialize",
                "params": {
                    "clientInfo": {
                        "name": "agentix",
                        "title": "Agentix",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": {
                        "experimentalApi": true
                    }
                }
            })
            .to_string()
            .into(),
        ))
        .await?;
    wait_for_initialize(&mut websocket, initialize_id).await?;
    websocket
        .send(Message::Text(
            json!({"method": "initialized", "params": {}})
                .to_string()
                .into(),
        ))
        .await?;
    Ok(websocket)
}

fn page_running_sessions(
    sessions: &[SessionSummary],
    cursor: Option<&str>,
    limit: u32,
) -> Result<SessionPage, ClientError> {
    let start = cursor.map_or(Ok(0), |cursor| {
        cursor
            .strip_prefix("running:")
            .and_then(|offset| offset.parse::<usize>().ok())
            .ok_or(ClientError::InvalidCursor)
    })?;
    let end = start.saturating_add(limit as usize).min(sessions.len());
    let page = sessions
        .get(start..end)
        .ok_or(ClientError::InvalidCursor)?
        .to_vec();
    Ok(SessionPage {
        sessions: page,
        next_cursor: (end < sessions.len() && end > start).then(|| format!("running:{end}")),
    })
}

#[async_trait]
impl AgentAdapter for CodexClient {
    fn display_name(&self) -> &'static str {
        "Codex"
    }

    fn queued_prompts(&self) -> Option<&dyn QueuedPromptPort> {
        Some(self)
    }

    fn session_control(&self) -> Option<&dyn SessionControlPort> {
        Some(self)
    }

    fn workspace_runtime(&self) -> Option<&dyn WorkspaceRuntimePort> {
        Some(self)
    }

    async fn list_sessions(
        &self,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<SessionPage, AgentError> {
        self.list_sessions(cursor, limit).await.map_err(agent_error)
    }

    async fn read_history(
        &self,
        session_id: &SessionId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<HistoryPage, AgentError> {
        match self.paged_history(session_id, cursor, limit).await {
            Ok(page) => Ok(page),
            Err(ClientError::Rpc { code: -32601, .. }) => self
                .stable_history(session_id, limit)
                .await
                .map_err(agent_error),
            Err(error)
                if is_thread_unmaterialized(&error)
                    && self.pending_resumes.lock().await.contains(session_id) =>
            {
                Ok(HistoryPage {
                    turns: Vec::new(),
                    older_cursor: None,
                    newer_cursor: None,
                })
            }
            Err(error) => Err(agent_error(error)),
        }
    }

    async fn attach(&self, session_id: &SessionId) -> Result<(), AgentError> {
        let thread = self
            .read_thread(session_id, false)
            .await
            .map_err(agent_error)?;
        if !thread_has_rollout(&thread) {
            return Err(agent_error(ClientError::NoRollout(session_id.clone())));
        }
        match self
            .request_after_reconnect("thread/resume", thread_resume_params(session_id))
            .await
        {
            Ok(_) => {
                self.pending_resumes.lock().await.remove(session_id);
                self.exited_process_sessions.lock().await.remove(session_id);
            }
            Err(error) if is_rollout_initializing(&error) => {
                let running = self
                    .list_sessions(None, u32::MAX)
                    .await
                    .map_err(agent_error)?
                    .sessions
                    .into_iter()
                    .any(|session| session.id == *session_id);
                if !running {
                    return Err(agent_error(error));
                }
                self.pending_resumes.lock().await.insert(session_id.clone());
            }
            Err(error) => return Err(agent_error(error)),
        }
        self.subscriptions.lock().await.insert(session_id.clone());
        if self.process_discovery.is_some() {
            self.process_sessions
                .lock()
                .await
                .insert(session_id.clone());
        }
        Ok(())
    }

    async fn unsubscribe(&self, session_id: &SessionId) -> Result<(), AgentError> {
        self.subscriptions.lock().await.remove(session_id);
        self.process_sessions.lock().await.remove(session_id);
        self.exited_process_sessions.lock().await.remove(session_id);
        self.pending_resumes.lock().await.remove(session_id);
        self.request(
            "thread/unsubscribe",
            json!({"threadId": session_id.as_str()}),
        )
        .await
        .map(|_| ())
        .map_err(agent_error)
    }

    async fn start_turn(&self, session_id: &SessionId, text: &str) -> Result<String, AgentError> {
        let result: TurnStartResult = decode_result(
            self.request(
                "turn/start",
                json!({
                    "threadId": session_id.as_str(),
                    "input": [{"type": "text", "text": text}]
                }),
            )
            .await
            .map_err(agent_error)?,
            "turn/start result",
        )
        .map_err(agent_error)?;
        let turn_id = result.turn.id;
        self.resume_pending_session(session_id)
            .await
            .map_err(agent_error)?;
        Ok(turn_id)
    }

    async fn steer(
        &self,
        session_id: &SessionId,
        expected_turn_id: &str,
        text: &str,
    ) -> Result<String, AgentError> {
        let result: TurnSteerResult = decode_result(
            self.request(
                "turn/steer",
                json!({
                    "threadId": session_id.as_str(),
                    "expectedTurnId": expected_turn_id,
                    "input": [{"type": "text", "text": text}]
                }),
            )
            .await
            .map_err(agent_error)?,
            "turn/steer result",
        )
        .map_err(agent_error)?;
        Ok(result.turn_id)
    }

    async fn interrupt(&self, session_id: &SessionId, turn_id: &str) -> Result<(), AgentError> {
        self.request(
            "turn/interrupt",
            json!({"threadId": session_id.as_str(), "turnId": turn_id}),
        )
        .await
        .map(|_| ())
        .map_err(agent_error)
    }

    async fn resolve_interaction(&self, decision: InteractionDecision) -> Result<(), AgentError> {
        self.respond(decision.rpc_id, decision.response)
            .await
            .map_err(agent_error)
    }

    fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }

    fn generation(&self) -> u64 {
        self.connection.generation.load(Ordering::Acquire)
    }
}

#[async_trait]
impl QueuedPromptPort for CodexClient {
    async fn queue_prompt(
        &self,
        session_id: &SessionId,
        text: &str,
        client_message_id: &str,
    ) -> Result<QueuedPrompt, AgentError> {
        let result: QueueAddResult = decode_result(
            self.request(
                "thread/queue/add",
                json!({
                    "threadId": session_id.as_str(),
                    "input": [{"type": "text", "text": text}],
                    "clientUserMessageId": client_message_id
                }),
            )
            .await
            .map_err(agent_error)?,
            "thread/queue/add result",
        )
        .map_err(agent_error)?;
        Ok(queued_prompt(result.queued_submission))
    }

    async fn list_queued_prompts(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<QueuedPrompt>, AgentError> {
        let mut cursor = None;
        let mut prompts = Vec::new();
        loop {
            let result = self
                .request(
                    "thread/queue/list",
                    json!({
                        "threadId": session_id.as_str(),
                        "cursor": cursor,
                        "limit": 100
                    }),
                )
                .await
                .map_err(agent_error)?;
            let page: QueueListResult =
                decode_result(result, "thread/queue/list result").map_err(agent_error)?;
            prompts.extend(page.data.into_iter().map(queued_prompt));
            let next_cursor = page.next_cursor;
            if next_cursor.is_none() {
                break;
            }
            if next_cursor == cursor {
                return Err(AgentError::Protocol(
                    "queue response repeated its cursor".into(),
                ));
            }
            cursor = next_cursor;
        }
        Ok(prompts)
    }
}

#[async_trait]
impl WorkspaceRuntimePort for CodexClient {
    fn default_directory(&self) -> String {
        self.rmux.default_directory().to_string_lossy().into_owned()
    }

    async fn snapshot(&self) -> Result<Option<MultiplexerSnapshot>, AgentError> {
        let sessions = self
            .list_sessions(None, 100)
            .await
            .map_err(agent_error)?
            .sessions;
        RmuxManager::snapshot(&sessions)
            .await
            .map_err(|error| AgentError::Rejected(error.to_string()))
    }

    async fn mutate(
        &self,
        mutation: MultiplexerMutation,
    ) -> Result<MultiplexerMutationResult, AgentError> {
        let prepared = RmuxManager::prepare(mutation)
            .await
            .map_err(|error| AgentError::Rejected(error.to_string()))?;
        let launch_codex = prepared.mutation.launch_codex;
        let known_sessions = if launch_codex {
            self.loaded_session_ids(None, None)
                .await
                .map_err(agent_error)?
                .0
                .into_iter()
                .collect()
        } else {
            HashSet::new()
        };
        let outcome = self
            .rmux
            .execute(&prepared)
            .await
            .map_err(|error| AgentError::Rejected(error.to_string()))?;
        let session = if launch_codex {
            let session = self
                .wait_for_rmux_session(&outcome.location, &known_sessions, &prepared.cwd)
                .await?;
            self.subscriptions.lock().await.insert(session.id.clone());
            self.process_sessions
                .lock()
                .await
                .insert(session.id.clone());
            self.pending_resumes.lock().await.insert(session.id.clone());
            Some(session)
        } else {
            None
        };
        let location = outcome.location;
        let message = if session.is_some() {
            format!(
                "Codex started in `rmux · {} · {} ({}) · {}`.",
                location.session, location.window_index, location.window_name, location.pane_index
            )
        } else {
            format!(
                "Shell created in `rmux · {} · {} ({}) · {}`.",
                location.session, location.window_index, location.window_name, location.pane_index
            )
        };
        Ok(MultiplexerMutationResult { message, session })
    }
}

#[async_trait]
impl SessionControlPort for CodexClient {
    async fn run_session_command(
        &self,
        session_id: &SessionId,
        command: SessionCommand,
    ) -> Result<SessionCommandResult, AgentError> {
        let result = match command {
            SessionCommand::Compact => self
                .request(
                    "thread/compact/start",
                    json!({"threadId": session_id.as_str()}),
                )
                .await
                .map(|_| {
                    SessionCommandResult::message("Codex · Compact", "Context compaction started.")
                }),
            SessionCommand::Fork => self.fork_thread(session_id).await,
            SessionCommand::Fast(enabled) => self.fast_command(session_id, enabled).await,
            SessionCommand::Clear(name) => self.clear_command(session_id, name).await,
            SessionCommand::Exit => Ok(SessionCommandResult::message(
                "Codex · Exit",
                "Detached from the session.",
            )),
            SessionCommand::Diff => self.diff_command(session_id).await,
            SessionCommand::Rename(name) => self.rename_command(session_id, name).await,
            SessionCommand::Model(model) => self.model_command(session_id, model).await,
            SessionCommand::Reasoning(effort) => self.reasoning_command(session_id, effort).await,
            SessionCommand::Skills => self.skills_command(session_id).await,
            SessionCommand::Plan { enabled, .. } => self.plan_command(session_id, enabled).await,
            SessionCommand::Goal(command) => self.goal_command(session_id, command).await,
            SessionCommand::Review => self.review_command(session_id).await,
            SessionCommand::Status => self.status_command(session_id).await,
            SessionCommand::Mcp => self.mcp_command(session_id).await,
        };
        result.map_err(agent_error)
    }
}

async fn wait_for_initialize(
    websocket: &mut Socket,
    initialize_id: i64,
) -> Result<(), ClientError> {
    while let Some(frame) = websocket.next().await {
        let frame = frame?;
        let Message::Text(text) = frame else {
            continue;
        };
        let value: Value = serde_json::from_str(&text)
            .map_err(|_| ClientError::InvalidResponse("initialize JSON"))?;
        match decode_server_frame(&value)? {
            ServerMessage::Response { id, result } if id == json!(initialize_id) => {
                return result.map(|_| ()).map_err(|error| ClientError::Rpc {
                    code: error.code,
                    message: error.message,
                });
            }
            _ => {}
        }
    }
    Err(ClientError::ResponseClosed)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn read_loop(
    mut reader: Reader,
    writer: Arc<Mutex<Writer>>,
    pending: Arc<Mutex<PendingMap>>,
    subscriptions: Arc<Mutex<HashSet<SessionId>>>,
    token_usage: Arc<Mutex<HashMap<SessionId, Value>>>,
    events: broadcast::Sender<AgentEvent>,
    endpoint: CodexEndpoint,
    connection: Arc<ConnectionState>,
) {
    loop {
        let disconnected_generation = connection.generation.load(Ordering::Acquire);
        let mut disconnect_reason = "Codex socket closed".to_owned();
        while let Some(frame) = reader.next().await {
            match frame {
                Ok(Message::Text(text)) => {
                    let Ok(value) = serde_json::from_str::<Value>(&text) else {
                        tracing::warn!("discarding malformed Codex JSON frame");
                        continue;
                    };
                    if value.get("method").and_then(Value::as_str)
                        == Some("thread/tokenUsage/updated")
                        && let Some(params) = value.get("params")
                        && let Some(thread_id) = params.get("threadId").and_then(Value::as_str)
                        && let Some(usage) = params.get("tokenUsage")
                    {
                        token_usage
                            .lock()
                            .await
                            .insert(SessionId::new(thread_id), usage.clone());
                    }
                    match decode_server_frame(&value) {
                        Ok(ServerMessage::Response { id, result }) => {
                            if let Some(id) = id.as_i64()
                                && let Some(sender) = pending.lock().await.remove(&id)
                            {
                                let _ = sender.send(result);
                            }
                        }
                        Ok(ServerMessage::Event(event)) => {
                            let _ = events.send(event);
                        }
                        Ok(ServerMessage::Interaction(request)) => {
                            let _ = events.send(AgentEvent::InteractionRequested(request));
                        }
                        Ok(ServerMessage::Ignored) => {}
                        Err(error) => tracing::warn!(%error, "discarding invalid Codex frame"),
                    }
                }
                Ok(Message::Ping(payload)) => {
                    let _ = writer.lock().await.send(Message::Pong(payload)).await;
                }
                Ok(Message::Close(frame)) => {
                    disconnect_reason =
                        frame.map_or(disconnect_reason, |frame| frame.reason.to_string());
                    break;
                }
                Ok(_) => {}
                Err(error) => {
                    disconnect_reason = error.to_string();
                    break;
                }
            }
        }
        pending.lock().await.clear();
        let _ = events.send(AgentEvent::Disconnected {
            generation: disconnected_generation,
            reason: disconnect_reason,
        });

        let mut retry_delay = Duration::from_millis(100);
        let websocket = loop {
            match connect_socket(&endpoint).await {
                Ok(websocket) => break websocket,
                Err(error) => {
                    tracing::warn!(%error, ?retry_delay, "Codex reconnect failed");
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(5));
                }
            }
        };
        let (new_writer, new_reader) = websocket.split();
        let subscribed = subscriptions
            .lock()
            .await
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        {
            let mut active_writer = writer.lock().await;
            *active_writer = new_writer;
            for (index, session) in subscribed.iter().enumerate() {
                let id = -i64::try_from(index + 1).unwrap_or(i64::MAX);
                if let Err(error) = active_writer
                    .send(Message::Text(
                        json!({
                            "id": id,
                            "method": "thread/resume",
                            "params": thread_resume_params(session)
                        })
                        .to_string()
                        .into(),
                    ))
                    .await
                {
                    tracing::warn!(%error, session = %session, "failed to restore Codex subscription");
                }
            }
        }
        reader = new_reader;
        let connected_generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        connection
            .generation
            .store(connected_generation, Ordering::Release);
        connection.changed.notify_waiters();
        let _ = events.send(AgentEvent::Connected {
            generation: connected_generation,
        });
    }
}

fn thread_resume_params(session_id: &SessionId) -> Value {
    json!({
        "threadId": session_id.as_str(),
        "excludeTurns": true
    })
}

fn parse_session_summary(value: &Value) -> Result<SessionSummary, ClientError> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or(ClientError::InvalidResponse("thread id"))?;
    Ok(SessionSummary {
        id: SessionId::new(id),
        name: value.get("name").and_then(Value::as_str).map(str::to_owned),
        preview: value
            .get("preview")
            .and_then(Value::as_str)
            .map(str::to_owned),
        cwd: value.get("cwd").and_then(Value::as_str).map(str::to_owned),
        updated_at: value.get("updatedAt").and_then(Value::as_i64),
        status: parse_session_status(value.get("status")),
        terminal: None,
    })
}

fn queued_prompt(submission: QueuedSubmission) -> QueuedPrompt {
    let text = submission
        .input
        .into_iter()
        .filter(|item| item.kind == "text")
        .filter_map(|item| item.text)
        .collect::<Vec<_>>()
        .join("\n");
    QueuedPrompt {
        id: submission.id,
        text: if text.is_empty() {
            "[non-text input]".to_owned()
        } else {
            text
        },
    }
}

fn decode_result<T: DeserializeOwned>(value: Value, shape: &'static str) -> Result<T, ClientError> {
    serde_json::from_value(value).map_err(|_| ClientError::InvalidResponse(shape))
}

fn render_goal(goal: Option<&Value>) -> String {
    let Some(goal) = goal.filter(|goal| !goal.is_null()) else {
        return "No goal is set. Use `/goal <objective>` to create one.".into();
    };
    let objective = goal
        .get("objective")
        .and_then(Value::as_str)
        .unwrap_or("Unnamed goal");
    let status = goal
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let tokens = goal.get("tokensUsed").and_then(Value::as_i64).unwrap_or(0);
    let elapsed = goal
        .get("timeUsedSeconds")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let budget = goal
        .get("tokenBudget")
        .and_then(Value::as_i64)
        .map_or_else(|| "unlimited".into(), |budget| budget.to_string());
    format!(
        "**Objective:** {objective}\n**Status:** `{status}`\n**Usage:** {tokens} tokens · {elapsed}s\n**Budget:** {budget}"
    )
}

fn abbreviate_home(path: &str) -> String {
    let Ok(home) = std::env::var("HOME") else {
        return path.into();
    };
    if path == home {
        return "~".into();
    }
    path.strip_prefix(&format!("{home}/"))
        .map_or_else(|| path.into(), |suffix| format!("~/{suffix}"))
}

fn title_case(value: &str) -> String {
    let mut characters = value.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + characters.as_str()
    })
}

fn kebab_case(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 4);
    for character in value.chars() {
        if character.is_uppercase() {
            if !output.is_empty() {
                output.push('-');
            }
            output.extend(character.to_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

fn thread_has_rollout(thread: &Value) -> bool {
    if thread.get("ephemeral").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    match thread.get("path") {
        Some(Value::Null) => false,
        Some(Value::String(path)) => !path.is_empty(),
        _ => true,
    }
}

fn is_rollout_initializing(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Rpc { code: -32600, message }
            if message.starts_with("no rollout found for thread id ")
    )
}

fn is_thread_unmaterialized(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Rpc { code: -32600, message }
            if message.contains(" is not materialized yet")
    )
}

fn history_from_result(result: &Value) -> Result<HistoryPage, ClientError> {
    let data = result
        .get("data")
        .and_then(Value::as_array)
        .ok_or(ClientError::InvalidResponse("thread/turns/list data"))?;
    let mut turns = data
        .iter()
        .map(parse_turn_summary)
        .collect::<Result<Vec<_>, _>>()?;
    turns.reverse();
    Ok(HistoryPage {
        turns,
        older_cursor: result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned),
        newer_cursor: result
            .get("backwardsCursor")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn parse_turn_summary(value: &Value) -> Result<TurnSummary, ClientError> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or(ClientError::InvalidResponse("turn id"))?;
    let mut user_text = Vec::new();
    let mut agent_text = Vec::new();
    let mut tools = Vec::new();
    let mut items = Vec::new();
    for item in value
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let summary = item_summary(item, "history")?;
        match summary.kind.as_str() {
            "userMessage" => user_text.extend(summary.text.clone()),
            "agentMessage" => agent_text.extend(summary.text.clone()),
            "commandExecution" | "fileChange" | "mcpToolCall" => tools.push(ToolSummary {
                kind: summary.kind.clone(),
                label: item
                    .get("command")
                    .map_or_else(|| summary.kind.clone(), std::string::ToString::to_string),
                status: summary.status.clone().unwrap_or_else(|| "unknown".into()),
            }),
            _ => {}
        }
        items.push(summary);
    }
    Ok(TurnSummary {
        id: id.to_owned(),
        status: parse_turn_status(value.get("status").and_then(Value::as_str)),
        user_text: (!user_text.is_empty()).then(|| user_text.join("\n")),
        agent_text: (!agent_text.is_empty()).then(|| agent_text.join("\n")),
        tools,
        items,
    })
}

fn agent_error(error: ClientError) -> AgentError {
    match error {
        ClientError::Connect(error) => AgentError::Unavailable(error.to_string()),
        ClientError::Rpc { code, message } => AgentError::Rejected(format!("{code}: {message}")),
        ClientError::NoRollout(_) => AgentError::Rejected(error.to_string()),
        other => AgentError::Protocol(other.to_string()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::Duration;

    use agentix_core::{AgentAdapter, AgentEvent, SessionId};
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use tempfile::tempdir;
    use tokio::net::UnixListener;
    use tokio_tungstenite::tungstenite::Message;

    use super::CodexClient;
    use crate::CodexEndpoint;

    #[tokio::test]
    async fn first_turn_resumes_a_provisionally_attached_empty_session() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("codex.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            initialize(&mut websocket).await;

            let start = next_json(&mut websocket).await;
            assert_eq!(start["method"], "turn/start");
            send_result(
                &mut websocket,
                &start["id"],
                json!({"turn": {"id": "turn_first"}}),
            )
            .await;

            let first_resume = next_json(&mut websocket).await;
            assert_eq!(first_resume["method"], "thread/resume");
            assert_eq!(first_resume["params"]["excludeTurns"], true);
            send_error(
                &mut websocket,
                &first_resume["id"],
                -32600,
                "no rollout found for thread id thr_empty",
            )
            .await;

            let second_resume = next_json(&mut websocket).await;
            assert_eq!(second_resume["method"], "thread/resume");
            assert_eq!(second_resume["params"]["excludeTurns"], true);
            send_result(&mut websocket, &second_resume["id"], json!({})).await;
        });

        let client = CodexClient::connect(CodexEndpoint::from_socket_path(&socket).unwrap())
            .await
            .unwrap();
        let session = SessionId::new("thr_empty");
        client.pending_resumes.lock().await.insert(session.clone());

        assert_eq!(
            client.start_turn(&session, "hello").await.unwrap(),
            "turn_first"
        );
        assert!(!client.pending_resumes.lock().await.contains(&session));
        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn unsubscribe_stops_process_watching_even_if_upstream_already_unsubscribed() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("codex.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            initialize(&mut websocket).await;
            let unsubscribe = next_json(&mut websocket).await;
            assert_eq!(unsubscribe["method"], "thread/unsubscribe");
            send_error(
                &mut websocket,
                &unsubscribe["id"],
                -32600,
                "thread is not subscribed",
            )
            .await;
        });

        let client = CodexClient::connect(CodexEndpoint::from_socket_path(&socket).unwrap())
            .await
            .unwrap();
        let session = SessionId::new("thr_exited");
        client.subscriptions.lock().await.insert(session.clone());
        client.process_sessions.lock().await.insert(session.clone());
        client
            .exited_process_sessions
            .lock()
            .await
            .insert(session.clone());
        client.pending_resumes.lock().await.insert(session.clone());

        assert!(client.unsubscribe(&session).await.is_err());
        assert!(!client.subscriptions.lock().await.contains(&session));
        assert!(!client.process_sessions.lock().await.contains(&session));
        assert!(
            !client
                .exited_process_sessions
                .lock()
                .await
                .contains(&session)
        );
        assert!(!client.pending_resumes.lock().await.contains(&session));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn resumed_empty_session_restores_a_provisional_subscription() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("codex.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            initialize(&mut websocket).await;
            let resume = next_json(&mut websocket).await;
            assert_eq!(resume["method"], "thread/resume");
            send_error(
                &mut websocket,
                &resume["id"],
                -32600,
                "no rollout found for thread id thr_empty",
            )
            .await;
        });

        let client = CodexClient::connect(CodexEndpoint::from_socket_path(&socket).unwrap())
            .await
            .unwrap();
        let session = SessionId::new("thr_empty");
        client
            .exited_process_sessions
            .lock()
            .await
            .insert(session.clone());
        let mut events = client.subscribe();

        assert!(client.resume_exited_session(&session).await.unwrap());
        assert!(client.subscriptions.lock().await.contains(&session));
        assert!(client.pending_resumes.lock().await.contains(&session));
        assert!(
            !client
                .exited_process_sessions
                .lock()
                .await
                .contains(&session)
        );
        loop {
            if events.recv().await.unwrap()
                == (AgentEvent::SessionResumed {
                    session_id: "thr_empty".into(),
                })
            {
                break;
            }
        }
        server.await.unwrap();
    }

    async fn initialize<S>(websocket: &mut tokio_tungstenite::WebSocketStream<S>)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let initialize = next_json(websocket).await;
        send_result(websocket, &initialize["id"], json!({})).await;
        assert_eq!(next_json(websocket).await["method"], "initialized");
    }

    async fn next_json<S>(websocket: &mut tokio_tungstenite::WebSocketStream<S>) -> Value
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let message = websocket.next().await.unwrap().unwrap();
        serde_json::from_str(message.to_text().unwrap()).unwrap()
    }

    async fn send_result<S>(
        websocket: &mut tokio_tungstenite::WebSocketStream<S>,
        id: &Value,
        result: Value,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        websocket
            .send(Message::Text(
                json!({"id": id, "result": result}).to_string().into(),
            ))
            .await
            .unwrap();
    }

    async fn send_error<S>(
        websocket: &mut tokio_tungstenite::WebSocketStream<S>,
        id: &Value,
        code: i64,
        message: &str,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        websocket
            .send(Message::Text(
                json!({"id": id, "error": {"code": code, "message": message}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
    }
}
