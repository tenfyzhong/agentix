//! Pi and Oh My Pi JSONL-RPC adapter primitives.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use agentix_core::{
    AgentAdapter, AgentError, AgentEvent, HistoryPage, InteractionDecision, InteractionKind,
    InteractionRequest, ItemSummary, SessionId, SessionPage, SessionStatus, SessionSummary,
    TurnStatus, TurnSummary,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, broadcast, oneshot};
use walkdir::WalkDir;

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

#[cfg(test)]
#[path = "file_tests.rs"]
mod file_tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiFlavor {
    Pi,
    OhMyPi,
}

impl PiFlavor {
    #[must_use]
    pub const fn default_command(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::OhMyPi => "omp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredSession {
    pub summary: SessionSummary,
    pub path: PathBuf,
}

#[derive(Debug, Error)]
pub enum PiError {
    #[error("session I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("session JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("session header is missing {0}")]
    MissingHeader(&'static str),
}

#[must_use]
pub fn process_args(flavor: PiFlavor, session_path: &str) -> Vec<String> {
    let session_flag = match flavor {
        PiFlavor::Pi => "--session",
        PiFlavor::OhMyPi => "--resume",
    };
    vec![
        "--mode".into(),
        "rpc".into(),
        session_flag.into(),
        session_path.into(),
    ]
}

pub fn discover_sessions(root: &Path) -> Result<Vec<DiscoveredSession>, PiError> {
    Ok(scan_sessions(root, read_session_summary))
}

fn discover_headers(root: &Path) -> Vec<DiscoveredSession> {
    #[cfg(test)]
    file_tests::wait_header_scan(root);
    scan_sessions(root, read_session_header)
}

fn scan_sessions(
    root: &Path,
    read: fn(&Path) -> Result<DiscoveredSession, PiError>,
) -> Vec<DiscoveredSession> {
    let mut sessions = Vec::new();
    if !root.exists() {
        return sessions;
    }
    for entry in WalkDir::new(root).follow_links(false) {
        let Ok(entry) = entry else {
            continue;
        };
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(|value| value.to_str()) != Some("jsonl")
        {
            continue;
        }
        if let Ok(session) = read(entry.path()) {
            sessions.push(session);
        }
    }
    sessions.sort_by_key(|session| std::cmp::Reverse(session.summary.updated_at));
    sessions
}

fn read_session_summary(path: &Path) -> Result<DiscoveredSession, PiError> {
    #[cfg(test)]
    file_tests::SUMMARY_READS
        .lock()
        .unwrap()
        .push(path.to_owned());
    #[cfg(test)]
    file_tests::SUMMARY_THREADS
        .lock()
        .unwrap()
        .insert(path.to_owned(), std::thread::current().id());
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    let lines = BufReader::new(file).lines();
    let mut header = None;
    let mut name = None;
    let mut preview = None;
    for line in lines.map_while(Result::ok) {
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match entry.get("type").and_then(Value::as_str) {
            Some("session") if header.is_none() => header = Some(entry.clone()),
            Some("title") => {
                if let Some(value) = entry
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    name = Some(value.to_owned());
                }
            }
            _ => {}
        }
        if matches!(
            entry.get("type").and_then(Value::as_str),
            Some("session_info" | "sessionInfo")
        ) && let Some(value) = entry.get("name").and_then(Value::as_str)
        {
            name = Some(value.to_owned());
        }
        if entry.get("type").and_then(Value::as_str) == Some("message")
            && entry.pointer("/message/role").and_then(Value::as_str) == Some("user")
        {
            preview = message_text(entry.pointer("/message").unwrap_or(&Value::Null));
        }
    }
    let header = header.ok_or(PiError::MissingHeader("session"))?;
    let mut session = session_from_header(path, &header, &metadata)?;
    session.summary.name = name;
    session.summary.preview = preview;
    Ok(session)
}

fn read_session_header(path: &Path) -> Result<DiscoveredSession, PiError> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(header) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if header.get("type").and_then(Value::as_str) == Some("session") {
            return session_from_header(path, &header, &metadata);
        }
    }
    Err(PiError::MissingHeader("session"))
}

fn session_from_header(
    path: &Path,
    header: &Value,
    metadata: &std::fs::Metadata,
) -> Result<DiscoveredSession, PiError> {
    let id = header
        .get("id")
        .and_then(Value::as_str)
        .ok_or(PiError::MissingHeader("id"))?;
    let updated_at = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_secs()).ok());
    Ok(DiscoveredSession {
        summary: SessionSummary {
            id: SessionId::new(id),
            name: None,
            preview: None,
            cwd: header.get("cwd").and_then(Value::as_str).map(str::to_owned),
            updated_at,
            status: SessionStatus::Idle,
            terminal: None,
        },
        path: path.to_owned(),
    })
}

#[must_use]
pub fn map_event(session_id: &str, turn_id: &str, frame: &Value) -> Option<AgentEvent> {
    match frame.get("type").and_then(Value::as_str)? {
        "agent_start" => Some(AgentEvent::TurnStarted {
            session_id: session_id.into(),
            turn_id: turn_id.into(),
        }),
        "message_update"
            if frame
                .pointer("/assistantMessageEvent/type")
                .and_then(Value::as_str)
                == Some("text_delta") =>
        {
            Some(AgentEvent::AgentMessageDelta {
                session_id: session_id.into(),
                turn_id: turn_id.into(),
                item_id: format!(
                    "message-{}",
                    frame
                        .pointer("/assistantMessageEvent/contentIndex")
                        .and_then(Value::as_u64)
                        .unwrap_or_default()
                ),
                delta: frame
                    .pointer("/assistantMessageEvent/delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
            })
        }
        "tool_execution_start" => Some(AgentEvent::ItemStarted {
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            item_id: frame
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .into(),
            kind: "mcpToolCall".into(),
            label: frame
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .into(),
        }),
        "tool_execution_end" => Some(AgentEvent::ItemCompleted {
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            item: ItemSummary {
                id: frame
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .into(),
                kind: "mcpToolCall".into(),
                status: Some(
                    if frame.get("isError").and_then(Value::as_bool) == Some(true) {
                        "failed"
                    } else {
                        "completed"
                    }
                    .into(),
                ),
                text: None,
            },
        }),
        "agent_settled" | "agent_end"
            if frame.get("type").and_then(Value::as_str) == Some("agent_settled")
                || frame.get("isTerminal").and_then(Value::as_bool) == Some(true) =>
        {
            Some(AgentEvent::TurnCompleted {
                session_id: session_id.into(),
                turn_id: turn_id.into(),
                status: TurnStatus::Completed,
                error: None,
            })
        }
        "extension_ui_request" => Some(AgentEvent::InteractionRequested(extension_interaction(
            session_id, turn_id, frame,
        ))),
        _ => None,
    }
}

fn extension_interaction(session_id: &str, turn_id: &str, frame: &Value) -> InteractionRequest {
    let method = frame
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("input");
    let is_input = matches!(method, "input" | "editor" | "select");
    InteractionRequest {
        rpc_id: frame.get("id").cloned().unwrap_or(Value::Null),
        method: format!("extension_ui/{method}"),
        session_id: session_id.into(),
        turn_id: turn_id.into(),
        item_id: None,
        kind: if is_input {
            InteractionKind::UserInput
        } else {
            InteractionKind::CommandApproval
        },
        title: frame
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Pi needs input")
            .into(),
        detail: frame
            .get("message")
            .or_else(|| frame.get("placeholder"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        available_decisions: if is_input {
            Vec::new()
        } else {
            vec!["accept".into(), "decline".into()]
        },
        payload: if is_input {
            json!({"questions": [{"id": "value"}]})
        } else {
            frame.clone()
        },
        auto_resolution_ms: frame.get("timeout").and_then(Value::as_u64),
    }
}

fn message_text(message: &Value) -> Option<String> {
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        return Some(text.to_owned());
    }
    let text = message
        .get("content")
        .and_then(Value::as_array)?
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

type PendingResponses = HashMap<String, oneshot::Sender<Result<Value, AgentError>>>;

struct RpcProcess {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    pending: Arc<Mutex<PendingResponses>>,
    turn_id: Arc<Mutex<String>>,
}

impl RpcProcess {
    async fn request(&self, mut frame: Value) -> Result<Value, AgentError> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        frame["id"] = Value::String(id.clone());
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), sender);
        if let Err(error) = self.write_frame(&frame).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        tokio::time::timeout(Duration::from_secs(30), receiver)
            .await
            .map_err(|_| AgentError::Unavailable("Pi RPC request timed out".into()))?
            .map_err(|_| AgentError::Unavailable("Pi RPC process stopped".into()))?
    }

    async fn write_frame(&self, frame: &Value) -> Result<(), AgentError> {
        let mut bytes =
            serde_json::to_vec(frame).map_err(|error| AgentError::Protocol(error.to_string()))?;
        bytes.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(&bytes)
            .await
            .map_err(|error| AgentError::Unavailable(error.to_string()))?;
        stdin
            .flush()
            .await
            .map_err(|error| AgentError::Unavailable(error.to_string()))
    }
}

pub struct PiRpcAdapter {
    flavor: PiFlavor,
    command: PathBuf,
    session_root: PathBuf,
    processes: Mutex<HashMap<SessionId, Arc<RpcProcess>>>,
    interaction_routes: Arc<Mutex<HashMap<String, SessionId>>>,
    events: broadcast::Sender<AgentEvent>,
    generation: u64,
}

impl PiRpcAdapter {
    #[must_use]
    pub fn new(
        flavor: PiFlavor,
        command: impl Into<PathBuf>,
        session_root: impl Into<PathBuf>,
    ) -> Self {
        let (events, _) = broadcast::channel(512);
        Self {
            flavor,
            command: command.into(),
            session_root: session_root.into(),
            processes: Mutex::new(HashMap::new()),
            interaction_routes: Arc::new(Mutex::new(HashMap::new())),
            events,
            generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
        }
    }

    async fn session(&self, session_id: &SessionId) -> Result<Arc<RpcProcess>, AgentError> {
        self.processes
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| AgentError::Unavailable(format!("session {session_id} is not attached")))
    }

    async fn locate_session(
        &self,
        session_id: &SessionId,
    ) -> Result<DiscoveredSession, AgentError> {
        let root = self.session_root.clone();
        let id = session_id.clone();
        tokio::task::spawn_blocking(move || {
            discover_headers(&root)
                .into_iter()
                .find(|session| session.summary.id == id)
                .ok_or_else(|| AgentError::Rejected(format!("unknown Pi session {id}")))
        })
        .await
        .map_err(|error| AgentError::Unavailable(error.to_string()))?
    }

    fn spawn_session(
        &self,
        session_id: &SessionId,
        discovered: &DiscoveredSession,
    ) -> Result<Arc<RpcProcess>, AgentError> {
        #[cfg(test)]
        file_tests::record_spawn(&self.session_root);
        let path = discovered
            .path
            .to_str()
            .ok_or_else(|| AgentError::Protocol("session path is not UTF-8".into()))?;
        let mut command = Command::new(&self.command);
        command.args(process_args(self.flavor, path));
        if let Some(cwd) = &discovered.summary.cwd {
            command.current_dir(cwd);
        }
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());
        let mut child = command
            .spawn()
            .map_err(|error| AgentError::Unavailable(error.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AgentError::Unavailable("Pi RPC stdin is unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentError::Unavailable("Pi RPC stdout is unavailable".into()))?;
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let turn_id = Arc::new(Mutex::new(String::new()));
        let process = Arc::new(RpcProcess {
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            pending: Arc::clone(&pending),
            turn_id: Arc::clone(&turn_id),
        });
        tokio::spawn(read_rpc_frames(
            stdout,
            session_id.clone(),
            turn_id,
            pending,
            Arc::clone(&self.interaction_routes),
            self.events.clone(),
        ));
        Ok(process)
    }
}

#[async_trait]
impl AgentAdapter for PiRpcAdapter {
    fn display_name(&self) -> &'static str {
        match self.flavor {
            PiFlavor::Pi => "Pi",
            PiFlavor::OhMyPi => "Oh My Pi",
        }
    }

    async fn list_sessions(
        &self,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<SessionPage, AgentError> {
        let root = self.session_root.clone();
        tokio::task::spawn_blocking(move || {
            let all = discover_headers(&root);
            let offset = cursor
                .as_deref()
                .unwrap_or("0")
                .parse::<usize>()
                .map_err(|error| AgentError::Protocol(error.to_string()))?;
            let end = offset.saturating_add(limit as usize).min(all.len());
            Ok(SessionPage {
                sessions: all[offset.min(all.len())..end]
                    .iter()
                    .filter_map(|session| read_session_summary(&session.path).ok())
                    .map(|session| session.summary)
                    .collect(),
                next_cursor: (end < all.len()).then(|| end.to_string()),
            })
        })
        .await
        .map_err(|error| AgentError::Unavailable(error.to_string()))?
    }

    async fn read_history(
        &self,
        session_id: &SessionId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<HistoryPage, AgentError> {
        let session = self.locate_session(session_id).await?;
        tokio::task::spawn_blocking(move || {
            read_history_file(&session.path, cursor.as_deref(), limit)
        })
        .await
        .map_err(|error| AgentError::Unavailable(error.to_string()))?
        .map_err(|error| AgentError::Protocol(error.to_string()))
    }

    async fn attach(&self, session_id: &SessionId) -> Result<(), AgentError> {
        if self.processes.lock().await.contains_key(session_id) {
            return Ok(());
        }
        let discovered = self.locate_session(session_id).await?;
        let mut processes = self.processes.lock().await;
        if processes.contains_key(session_id) {
            return Ok(());
        }
        let process = self.spawn_session(session_id, &discovered)?;
        processes.insert(session_id.clone(), process);
        Ok(())
    }

    async fn unsubscribe(&self, session_id: &SessionId) -> Result<(), AgentError> {
        if let Some(process) = self.processes.lock().await.remove(session_id) {
            process
                .child
                .lock()
                .await
                .kill()
                .await
                .map_err(|error| AgentError::Unavailable(error.to_string()))?;
        }
        Ok(())
    }

    async fn start_turn(&self, session_id: &SessionId, text: &str) -> Result<String, AgentError> {
        let process = self.session(session_id).await?;
        let turn_id = uuid::Uuid::new_v4().simple().to_string();
        *process.turn_id.lock().await = turn_id.clone();
        process
            .request(json!({"type": "prompt", "message": text}))
            .await?;
        Ok(turn_id)
    }

    async fn steer(
        &self,
        session_id: &SessionId,
        expected_turn_id: &str,
        text: &str,
    ) -> Result<String, AgentError> {
        self.session(session_id)
            .await?
            .request(json!({"type": "steer", "message": text}))
            .await?;
        Ok(expected_turn_id.into())
    }

    async fn interrupt(&self, session_id: &SessionId, _turn_id: &str) -> Result<(), AgentError> {
        self.session(session_id)
            .await?
            .request(json!({"type": "abort"}))
            .await?;
        Ok(())
    }

    async fn resolve_interaction(&self, decision: InteractionDecision) -> Result<(), AgentError> {
        let request_id = decision
            .rpc_id
            .as_str()
            .ok_or_else(|| AgentError::Protocol("Pi interaction id is not a string".into()))?;
        let session_id = self
            .interaction_routes
            .lock()
            .await
            .remove(request_id)
            .ok_or_else(|| AgentError::Rejected("Pi interaction expired".into()))?;
        let process = self.session(&session_id).await?;
        let frame = if let Some(value) = extract_user_answer(&decision.response) {
            json!({"type": "extension_ui_response", "id": request_id, "value": value})
        } else {
            let confirmed = decision
                .response
                .get("decision")
                .and_then(Value::as_str)
                .is_some_and(|value| value == "accept" || value == "acceptForSession");
            json!({"type": "extension_ui_response", "id": request_id, "confirmed": confirmed})
        };
        process.write_frame(&frame).await
    }

    fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }

    fn generation(&self) -> u64 {
        self.generation
    }
}

async fn read_rpc_frames(
    stdout: tokio::process::ChildStdout,
    session_id: SessionId,
    turn_id: Arc<Mutex<String>>,
    pending: Arc<Mutex<PendingResponses>>,
    interaction_routes: Arc<Mutex<HashMap<String, SessionId>>>,
    events: broadcast::Sender<AgentEvent>,
) {
    let mut reader = AsyncBufReader::new(stdout);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        match reader.read_until(b'\n', &mut bytes).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%error, "Pi RPC stdout failed");
                break;
            }
        }
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        let Ok(frame) = serde_json::from_slice::<Value>(&bytes) else {
            tracing::warn!("discarding malformed Pi RPC frame");
            continue;
        };
        if frame.get("type").and_then(Value::as_str) == Some("response") {
            let Some(id) = frame.get("id").and_then(Value::as_str) else {
                continue;
            };
            if let Some(sender) = pending.lock().await.remove(id) {
                let response = if frame.get("success").and_then(Value::as_bool) == Some(true) {
                    Ok(frame.get("data").cloned().unwrap_or(Value::Null))
                } else {
                    Err(AgentError::Rejected(
                        frame
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("Pi rejected the command")
                            .into(),
                    ))
                };
                let _ = sender.send(response);
            }
            continue;
        }
        let active_turn = turn_id.lock().await.clone();
        if let Some(event) = map_event(session_id.as_str(), &active_turn, &frame) {
            if let AgentEvent::InteractionRequested(request) = &event
                && let Some(id) = request.rpc_id.as_str()
            {
                interaction_routes
                    .lock()
                    .await
                    .insert(id.into(), session_id.clone());
            }
            let _ = events.send(event);
        }
    }
    pending.lock().await.clear();
    let _ = events.send(AgentEvent::SessionStatusChanged {
        session_id: session_id.to_string(),
        status: SessionStatus::Offline,
    });
}

fn read_history_file(
    path: &Path,
    cursor: Option<&str>,
    limit: u32,
) -> Result<HistoryPage, PiError> {
    let requested_end = cursor.map(str::parse::<usize>).transpose();
    let window_end = requested_end
        .as_ref()
        .ok()
        .copied()
        .flatten()
        .unwrap_or(usize::MAX);
    let mut total = 0;
    let mut turns = VecDeque::<TurnSummary>::new();
    for line in BufReader::new(File::open(path)?)
        .lines()
        .map_while(Result::ok)
    {
        let entry: Value = serde_json::from_str(&line)?;
        if entry.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let message = entry.get("message").unwrap_or(&Value::Null);
        match message.get("role").and_then(Value::as_str) {
            Some("user") => {
                total += 1;
                if total <= window_end && limit > 0 {
                    if turns.len() == limit as usize {
                        turns.pop_front();
                    }
                    turns.push_back(TurnSummary {
                        id: entry
                            .get("id")
                            .and_then(Value::as_str)
                            .map_or_else(|| format!("turn-{total}"), str::to_owned),
                        status: TurnStatus::Completed,
                        user_text: message_text(message),
                        agent_text: None,
                        tools: Vec::new(),
                        items: Vec::new(),
                    });
                }
            }
            Some("assistant") if total <= window_end => {
                if let Some(turn) = turns.back_mut() {
                    turn.agent_text = message_text(message);
                }
            }
            _ => {}
        }
        #[cfg(test)]
        file_tests::record_history_peak(path, turns.len());
    }
    // Preserve JSON/I/O error precedence over invalid cursor errors.
    let end = requested_end
        .map_err(|error| PiError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, error)))?
        .unwrap_or(total)
        .min(total);
    let start = end.saturating_sub(limit as usize);
    Ok(HistoryPage {
        turns: turns.into_iter().collect(),
        older_cursor: (start > 0).then(|| start.to_string()),
        newer_cursor: (end < total).then(|| total.to_string()),
    })
}

fn extract_user_answer(response: &Value) -> Option<&str> {
    response
        .pointer("/answers/value/answers/0")
        .and_then(Value::as_str)
}
