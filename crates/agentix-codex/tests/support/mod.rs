use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use agentix_codex::CodexEndpoint;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio::sync::{Mutex, broadcast, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

const MOCK_CODEX_CLI_VERSION: &str = "0.153.0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockTurn {
    pub id: String,
    pub status: String,
    pub user_text: String,
    pub agent_text: String,
    pub error: Option<String>,
    pub completed_at: i64,
}

impl MockTurn {
    pub fn completed(
        id: impl Into<String>,
        user_text: impl Into<String>,
        agent_text: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            status: "completed".into(),
            user_text: user_text.into(),
            agent_text: agent_text.into(),
            error: None,
            completed_at: 1_001,
        }
    }

    fn in_progress(id: impl Into<String>, user_text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            status: "inProgress".into(),
            user_text: user_text.into(),
            agent_text: String::new(),
            error: None,
            completed_at: 1_001,
        }
    }

    pub fn in_progress_with_output(
        id: impl Into<String>,
        user_text: impl Into<String>,
        agent_text: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            status: "inProgress".into(),
            user_text: user_text.into(),
            agent_text: agent_text.into(),
            error: None,
            completed_at: 1_001,
        }
    }

    fn as_json(&self) -> Value {
        let mut items = vec![json!({
            "id": format!("{}_user", self.id),
            "type": "userMessage",
            "content": [{"type": "text", "text": self.user_text}]
        })];
        if !self.agent_text.is_empty() {
            items.push(json!({
                "id": format!("{}_agent", self.id),
                "type": "agentMessage",
                "text": self.agent_text
            }));
        }
        json!({
            "id": self.id,
            "items": items,
            "status": self.status,
            "startedAt": 1_000,
            "completedAt": (self.status != "inProgress").then_some(self.completed_at),
            "error": self.error.as_ref().map(|message| json!({"message": message, "codexErrorInfo": null, "additionalDetails": null})),
            "durationMs": (self.status != "inProgress").then_some(1_000)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockThread {
    pub id: String,
    pub name: String,
    pub cwd: String,
    pub model: String,
    pub reasoning_effort: String,
    pub service_tier: Option<String>,
    pub turns: Vec<MockTurn>,
    goal: Option<Value>,
}

impl MockThread {
    pub fn new(id: impl Into<String>, name: impl Into<String>, cwd: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            cwd: cwd.into(),
            model: "gpt-5.6".into(),
            reasoning_effort: "medium".into(),
            service_tier: None,
            turns: Vec::new(),
            goal: None,
        }
    }

    pub fn with_turn(mut self, turn: MockTurn) -> Self {
        self.turns.push(turn);
        self
    }

    fn as_json(&self, include_turns: bool) -> Value {
        let status = if self
            .turns
            .last()
            .is_some_and(|turn| turn.status == "inProgress")
        {
            "active"
        } else {
            "idle"
        };
        let turns = if include_turns {
            self.turns.iter().map(MockTurn::as_json).collect()
        } else {
            Vec::new()
        };
        json!({
            "id": self.id,
            "sessionId": self.id,
            "name": self.name,
            "preview": self.turns.last().map(|turn| turn.user_text.as_str()),
            "cwd": self.cwd,
            "cliVersion": MOCK_CODEX_CLI_VERSION,
            "createdAt": 1_000,
            "updatedAt": 1_000_i64.saturating_add(
                i64::try_from(self.turns.len()).unwrap_or(i64::MAX)
            ),
            "status": if status == "active" {
                json!({"type": status, "activeFlags": []})
            } else {
                json!({"type": status})
            },
            "model": self.model,
            "modelProvider": "openai",
            "reasoningEffort": self.reasoning_effort,
            "ephemeral": false,
            "path": format!("/mock/rollout-{}.jsonl", self.id),
            "projectId": null,
            "source": "cli",
            "turns": turns
        })
    }
}

#[derive(Debug, Clone)]
struct QueuedSubmission {
    id: String,
    text: String,
    client_message_id: String,
}

impl QueuedSubmission {
    fn as_json(&self) -> Value {
        json!({
            "id": self.id,
            "clientUserMessageId": self.client_message_id,
            "input": [{"type": "text", "text": self.text}]
        })
    }
}

#[derive(Default)]
struct ServerState {
    threads: BTreeMap<String, MockThread>,
    queues: HashMap<String, Vec<QueuedSubmission>>,
    request_methods: Vec<String>,
    active_writers: HashSet<String>,
    turn_reads: HashMap<String, usize>,
    results: HashMap<String, Vec<Value>>,
    notifications: Vec<Value>,
    server_requests: Vec<Value>,
    page_size: Option<usize>,
    failures: HashMap<String, VecDeque<(i64, String)>>,
    disconnect_responses: HashMap<String, usize>,
}

#[derive(Clone)]
struct SharedServer {
    state: Arc<Mutex<ServerState>>,
    outbound: broadcast::Sender<Value>,
    pending_interactions: Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>,
    next_interaction_id: Arc<AtomicU64>,
}

pub struct MockCodexAppServer {
    _directory: TempDir,
    socket_path: std::path::PathBuf,
    shared: SharedServer,
    task: JoinHandle<()>,
}

impl MockCodexAppServer {
    pub fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let socket_path = directory.path().join("mock-codex.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let (outbound, _) = broadcast::channel(128);
        let shared = SharedServer {
            state: Arc::new(Mutex::new(ServerState::default())),
            outbound,
            pending_interactions: Arc::new(Mutex::new(HashMap::new())),
            next_interaction_id: Arc::new(AtomicU64::new(10_000)),
        };
        let server = shared.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let connection = server.clone();
                tokio::spawn(async move {
                    let websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
                    serve_connection(websocket, connection).await;
                });
            }
        });
        Self {
            _directory: directory,
            socket_path,
            shared,
            task,
        }
    }

    pub fn endpoint(&self) -> CodexEndpoint {
        CodexEndpoint::from_socket_path(&self.socket_path).unwrap()
    }

    pub async fn add_thread(&self, thread: MockThread) {
        self.shared
            .state
            .lock()
            .await
            .threads
            .insert(thread.id.clone(), thread);
    }

    pub async fn set_page_size(&self, page_size: usize) {
        assert!(page_size > 0);
        self.shared.state.lock().await.page_size = Some(page_size);
    }

    pub async fn fail_next(&self, method: &str, code: i64, message: &str) {
        self.shared
            .state
            .lock()
            .await
            .failures
            .entry(method.into())
            .or_default()
            .push_back((code, message.into()));
    }

    pub async fn disconnect_next_response(&self, method: &str) {
        self.disconnect_responses(method, 1).await;
    }

    pub async fn disconnect_responses(&self, method: &str, count: usize) {
        assert!(count > 0);
        self.shared
            .state
            .lock()
            .await
            .disconnect_responses
            .insert(method.into(), count);
    }

    pub async fn thread(&self, id: &str) -> Option<MockThread> {
        self.shared.state.lock().await.threads.get(id).cloned()
    }

    pub async fn latest_turn_id(&self, thread_id: &str) -> Option<String> {
        self.thread(thread_id)
            .await?
            .turns
            .last()
            .map(|turn| turn.id.clone())
    }

    pub async fn request_methods(&self) -> Vec<String> {
        self.shared.state.lock().await.request_methods.clone()
    }

    pub async fn last_result(&self, method: &str) -> Option<Value> {
        self.shared
            .state
            .lock()
            .await
            .results
            .get(method)
            .and_then(|results| results.last())
            .cloned()
    }

    pub async fn notifications(&self) -> Vec<Value> {
        self.shared.state.lock().await.notifications.clone()
    }

    pub async fn server_requests(&self) -> Vec<Value> {
        self.shared.state.lock().await.server_requests.clone()
    }

    pub fn disconnect_clients(&self) {
        let _ = self
            .shared
            .outbound
            .send(json!({"mockControl": "disconnect"}));
    }

    pub async fn wait_for_request_count(&self, method: &str, expected: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(6), async {
            loop {
                let count = self
                    .request_methods()
                    .await
                    .iter()
                    .filter(|candidate| candidate.as_str() == method)
                    .count();
                if count >= expected {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {expected} {method} requests"));
    }

    pub async fn set_active_writer(&self, thread_id: &str) {
        self.shared
            .state
            .lock()
            .await
            .active_writers
            .insert(thread_id.to_owned());
    }

    pub async fn wait_for_turn_reads(&self, thread_id: &str, expected: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(8), async {
            loop {
                if self
                    .shared
                    .state
                    .lock()
                    .await
                    .turn_reads
                    .get(thread_id)
                    .copied()
                    .unwrap_or(0)
                    >= expected
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {expected} turn reads of {thread_id}"));
    }

    pub async fn complete_turn(&self, thread_id: &str, turn_id: &str, answer: &str) {
        {
            let mut state = self.shared.state.lock().await;
            let thread = state.threads.get_mut(thread_id).unwrap();
            let turn = thread
                .turns
                .iter_mut()
                .find(|turn| turn.id == turn_id)
                .unwrap();
            turn.agent_text = answer.into();
            turn.status = "completed".into();
        }
        self.send_notification(json!({
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": thread_id,
                "turnId": turn_id,
                "itemId": format!("{turn_id}_agent"),
                "delta": answer
            }
        }))
        .await;
        self.send_notification(json!({
            "method": "item/completed",
            "params": {
                "completedAtMs": 1_001,
                "threadId": thread_id,
                "turnId": turn_id,
                "item": {
                    "id": format!("{turn_id}_agent"),
                    "type": "agentMessage",
                    "text": answer
                }
            }
        }))
        .await;
        let turn = self
            .thread(thread_id)
            .await
            .unwrap()
            .turns
            .into_iter()
            .find(|turn| turn.id == turn_id)
            .unwrap();
        self.send_notification(json!({
            "method": "turn/completed",
            "params": {
                "threadId": thread_id,
                "turn": turn.as_json()
            }
        }))
        .await;
    }

    pub async fn set_session_status(&self, thread_id: &str, status: &str) {
        let status = if status == "active" {
            json!({"type": status, "activeFlags": []})
        } else {
            json!({"type": status})
        };
        self.send_notification(json!({
            "method": "thread/status/changed",
            "params": {"threadId": thread_id, "status": status}
        }))
        .await;
    }

    pub async fn emit_tool_lifecycle(
        &self,
        thread_id: &str,
        turn_id: &str,
        item_id: &str,
        command: &str,
    ) {
        let item = |status: &str| {
            json!({
                "id": item_id,
                "type": "commandExecution",
                "command": command,
                "commandActions": [],
                "cwd": "/work",
                "status": status
            })
        };
        self.send_notification(json!({
            "method": "item/started",
            "params": {
                "startedAtMs": 1_000,
                "threadId": thread_id,
                "turnId": turn_id,
                "item": item("inProgress")
            }
        }))
        .await;
        self.send_notification(json!({
            "method": "item/completed",
            "params": {
                "completedAtMs": 1_001,
                "threadId": thread_id,
                "turnId": turn_id,
                "item": item("completed")
            }
        }))
        .await;
    }

    pub async fn resolve_interaction_externally(&self, thread_id: &str, request_id: &str) {
        self.send_notification(json!({
            "method": "serverRequest/resolved",
            "params": {"threadId": thread_id, "requestId": request_id}
        }))
        .await;
    }

    pub async fn send_token_usage(
        &self,
        thread_id: &str,
        total_tokens: u64,
        last_tokens: u64,
        context_window: u64,
    ) {
        self.send_notification(json!({
            "method": "thread/tokenUsage/updated",
            "params": {
                "threadId": thread_id,
                "turnId": "turn_usage",
                "tokenUsage": {
                    "total": {"totalTokens": total_tokens},
                    "last": {"totalTokens": last_tokens},
                    "modelContextWindow": context_window
                }
            }
        }))
        .await;
    }

    pub async fn request_command_approval(
        &self,
        thread_id: &str,
        turn_id: &str,
        item_id: &str,
        command: &str,
    ) -> oneshot::Receiver<Value> {
        self.send_server_request(
            "item/commandExecution/requestApproval",
            json!({
                "threadId": thread_id,
                "turnId": turn_id,
                "itemId": item_id,
                "command": command,
                "cwd": "/work",
                "startedAtMs": 1_000,
                "availableDecisions": ["accept", "decline", "cancel"]
            }),
        )
        .await
    }

    pub async fn request_file_approval(
        &self,
        thread_id: &str,
        turn_id: &str,
        item_id: &str,
        reason: &str,
    ) -> oneshot::Receiver<Value> {
        self.send_server_request(
            "item/fileChange/requestApproval",
            json!({
                "threadId": thread_id,
                "turnId": turn_id,
                "itemId": item_id,
                "reason": reason,
                "startedAtMs": 1_000
            }),
        )
        .await
    }

    pub async fn request_user_input(
        &self,
        thread_id: &str,
        turn_id: &str,
        item_id: &str,
        questions: Value,
    ) -> oneshot::Receiver<Value> {
        self.send_server_request(
            "item/tool/requestUserInput",
            json!({
                "threadId": thread_id,
                "turnId": turn_id,
                "itemId": item_id,
                "questions": questions,
                "isBlocking": true
            }),
        )
        .await
    }

    async fn send_server_request(&self, method: &str, params: Value) -> oneshot::Receiver<Value> {
        let id = self
            .shared
            .next_interaction_id
            .fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.shared
            .pending_interactions
            .lock()
            .await
            .insert(id.to_string(), sender);
        let request = json!({"id": id, "method": method, "params": params});
        self.shared
            .state
            .lock()
            .await
            .server_requests
            .push(request.clone());
        self.shared.outbound.send(request).unwrap();
        receiver
    }

    async fn send_notification(&self, notification: Value) {
        self.shared
            .state
            .lock()
            .await
            .notifications
            .push(notification.clone());
        let _ = self.shared.outbound.send(notification);
    }
}

impl Drop for MockCodexAppServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn update_subscriptions(
    subscriptions: &mut HashSet<String>,
    method: &str,
    params: &Value,
    result: &Value,
) {
    match method {
        "thread/resume" | "thread/start" | "thread/fork" => {
            if let Some(thread_id) = result["thread"]["id"].as_str() {
                subscriptions.insert(thread_id.to_owned());
            }
        }
        "thread/unsubscribe" => {
            if let Some(thread_id) = params["threadId"].as_str() {
                subscriptions.remove(thread_id);
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_lines)]
async fn serve_connection<S>(
    mut websocket: tokio_tungstenite::WebSocketStream<S>,
    server: SharedServer,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut outbound = server.outbound.subscribe();
    let mut subscriptions = HashSet::<String>::new();
    loop {
        tokio::select! {
            frame = websocket.next() => {
                let Some(Ok(Message::Text(text))) = frame else {
                    break;
                };
                let value: Value = serde_json::from_str(&text).unwrap();
                if let Some(method) = value.get("method").and_then(Value::as_str) {
                    if method == "initialized" {
                        continue;
                    }
                    let Some(id) = value.get("id").cloned() else {
                        continue;
                    };
                    let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
                    let disconnect = {
                        let mut state = server.state.lock().await;
                        let disconnect = if let Some(remaining) =
                            state.disconnect_responses.get_mut(method)
                        {
                            *remaining -= 1;
                            true
                        } else {
                            false
                        };
                        if state.disconnect_responses.get(method) == Some(&0) {
                            state.disconnect_responses.remove(method);
                        }
                        if disconnect {
                            state.request_methods.push(method.into());
                            true
                        } else {
                            false
                        }
                    };
                    if disconnect {
                        break;
                    }
                    let (result, notifications) = handle_request(&server, method, &params).await;
                    if let Ok(result) = &result {
                        update_subscriptions(&mut subscriptions, method, &params, result);
                    }
                    let response = match result {
                        Ok(result) => json!({"id": id, "result": result}),
                        Err((code, message)) => {
                            json!({"id": id, "error": {"code": code, "message": message}})
                        }
                    };
                    if websocket.send(Message::Text(response.to_string().into())).await.is_err() {
                        break;
                    }
                    for notification in notifications {
                        if let Some(thread_id) = notification["params"]["threadId"].as_str()
                            && !subscriptions.contains(thread_id)
                        {
                            continue;
                        }
                        if websocket
                            .send(Message::Text(notification.to_string().into()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                } else if let (Some(id), Some(result)) = (value.get("id"), value.get("result"))
                    && let Some(sender) = server
                        .pending_interactions
                        .lock()
                        .await
                        .remove(&id.to_string())
                {
                    let _ = sender.send(result.clone());
                }
            }
            notification = outbound.recv() => {
                match notification {
                    Ok(notification) => {
                        if notification.get("mockControl").and_then(Value::as_str)
                            == Some("disconnect")
                        {
                            break;
                        }
                        if let Some(thread_id) = notification["params"]["threadId"].as_str()
                            && !subscriptions.contains(thread_id)
                        {
                            continue;
                        }
                        if websocket
                            .send(Message::Text(notification.to_string().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn handle_request(
    server: &SharedServer,
    method: &str,
    params: &Value,
) -> (Result<Value, (i64, String)>, Vec<Value>) {
    let mut state = server.state.lock().await;
    state.request_methods.push(method.into());
    let mut notifications = Vec::new();
    if let Some(error) = state.failures.get_mut(method).and_then(VecDeque::pop_front) {
        return (Err(error), notifications);
    }
    let result = match method {
        "initialize" => Ok(json!({
            "codexHome": "/mock/.codex",
            "platformFamily": "unix",
            "platformOs": std::env::consts::OS,
            "userAgent": format!("mock-codex-app-server/{MOCK_CODEX_CLI_VERSION}")
        })),
        "thread/loaded/list" => {
            let ids = state.threads.keys().cloned().collect::<Vec<_>>();
            let (start, end, next_cursor) = page_bounds(params, ids.len(), state.page_size);
            Ok(json!({
                "data": &ids[start..end],
                "nextCursor": next_cursor
            }))
        }
        "thread/read" => with_thread(&state, params, |thread| {
            Ok(json!({
                "thread": thread.as_json(
                    params.get("includeTurns").and_then(Value::as_bool).unwrap_or(false)
                )
            }))
        }),
        "thread/resume" => {
            if let Some(thread_id) = params["threadId"].as_str()
                && state.active_writers.contains(thread_id)
            {
                return (
                    Err((
                        -32600,
                        format!("thread {thread_id} already has an active writer"),
                    )),
                    notifications,
                );
            }
            if params.get("excludeTurns").and_then(Value::as_bool) == Some(true) {
                with_thread(&state, params, |thread| Ok(thread_context_response(thread)))
            } else {
                Err((
                    -32602,
                    "paginated threads must be resumed with excludeTurns: true".into(),
                ))
            }
        }
        "thread/start" => {
            let id = format!("thr_started_{}", state.threads.len() + 1);
            let cwd = string_param(params, "cwd").unwrap_or("/work");
            let mut thread = MockThread::new(&id, "Untitled", cwd);
            if let Some(model) = string_param(params, "model") {
                thread.model = model.into();
            }
            thread.service_tier = string_param(params, "serviceTier").map(str::to_owned);
            let response = thread_context_response(&thread);
            state.threads.insert(id, thread);
            Ok(response)
        }
        "thread/name/set" => {
            let Some(thread_id) = string_param(params, "threadId") else {
                return (invalid_params("threadId"), notifications);
            };
            let Some(name) = string_param(params, "name") else {
                return (invalid_params("name"), notifications);
            };
            let Some(thread) = state.threads.get_mut(thread_id) else {
                return (unknown_thread(thread_id), notifications);
            };
            thread.name = name.into();
            Ok(json!({}))
        }
        "thread/unsubscribe" => {
            with_thread(&state, params, |_| Ok(json!({"status": "unsubscribed"})))
        }
        "thread/compact/start" => with_thread(&state, params, |_| Ok(json!({}))),
        "thread/turns/list" => {
            if let Some(thread_id) = params["threadId"].as_str() {
                *state.turn_reads.entry(thread_id.to_owned()).or_default() += 1;
            }
            with_thread(&state, params, |thread| {
                let turns = thread
                    .turns
                    .iter()
                    .rev()
                    .map(MockTurn::as_json)
                    .collect::<Vec<_>>();
                let (start, end, next_cursor) = page_bounds(params, turns.len(), state.page_size);
                Ok(json!({
                    "data": &turns[start..end],
                    "nextCursor": next_cursor,
                    "backwardsCursor": null
                }))
            })
        }
        "turn/start" => {
            let Some(thread_id) = string_param(params, "threadId") else {
                return (invalid_params("threadId"), notifications);
            };
            let Some(text) = input_text(params) else {
                return (invalid_params("input"), notifications);
            };
            let Some(thread) = state.threads.get_mut(thread_id) else {
                return (unknown_thread(thread_id), notifications);
            };
            let turn_id = format!("turn_{}", thread.turns.len() + 1);
            thread.turns.push(MockTurn::in_progress(&turn_id, text));
            let turn = thread.turns.last().unwrap().as_json();
            notifications.push(json!({
                "method": "turn/started",
                "params": {"threadId": thread_id, "turn": turn}
            }));
            notifications.push(json!({
                "method": "item/completed",
                "params": {
                    "completedAtMs": 1_000,
                    "threadId": thread_id,
                    "turnId": turn_id,
                    "item": {
                        "id": format!("{turn_id}_user"),
                        "type": "userMessage",
                        "content": [{"type": "text", "text": text}]
                    }
                }
            }));
            Ok(json!({"turn": thread.turns.last().unwrap().as_json()}))
        }
        "turn/steer" => {
            let Some(thread_id) = string_param(params, "threadId") else {
                return (invalid_params("threadId"), notifications);
            };
            let Some(turn_id) = string_param(params, "expectedTurnId") else {
                return (invalid_params("expectedTurnId"), notifications);
            };
            let Some(text) = input_text(params) else {
                return (invalid_params("input"), notifications);
            };
            let Some(thread) = state.threads.get_mut(thread_id) else {
                return (unknown_thread(thread_id), notifications);
            };
            let Some(turn) = thread.turns.iter_mut().find(|turn| turn.id == turn_id) else {
                return (
                    Err((-32600, format!("unknown turn: {turn_id}"))),
                    notifications,
                );
            };
            turn.user_text.push('\n');
            turn.user_text.push_str(text);
            Ok(json!({"turnId": turn_id}))
        }
        "turn/interrupt" => {
            let Some(thread_id) = string_param(params, "threadId") else {
                return (invalid_params("threadId"), notifications);
            };
            let Some(turn_id) = string_param(params, "turnId") else {
                return (invalid_params("turnId"), notifications);
            };
            let Some(thread) = state.threads.get_mut(thread_id) else {
                return (unknown_thread(thread_id), notifications);
            };
            let Some(turn) = thread.turns.iter_mut().find(|turn| turn.id == turn_id) else {
                return (
                    Err((-32600, format!("unknown turn: {turn_id}"))),
                    notifications,
                );
            };
            turn.status = "interrupted".into();
            let turn = turn.as_json();
            notifications.push(json!({
                "method": "turn/completed",
                "params": {
                    "threadId": thread_id,
                    "turn": turn
                }
            }));
            Ok(json!({}))
        }
        "thread/queue/add" => {
            let Some(thread_id) = string_param(params, "threadId") else {
                return (invalid_params("threadId"), notifications);
            };
            let Some(text) = input_text(params) else {
                return (invalid_params("input"), notifications);
            };
            let client_message_id = string_param(params, "clientUserMessageId")
                .unwrap_or_default()
                .to_owned();
            let queue = state.queues.entry(thread_id.into()).or_default();
            let submission = QueuedSubmission {
                id: format!("queued_{}", queue.len() + 1),
                text: text.into(),
                client_message_id,
            };
            let value = submission.as_json();
            queue.push(submission);
            notifications.push(json!({
                "method": "thread/queue/changed",
                "params": {"threadId": thread_id}
            }));
            Ok(json!({"queuedSubmission": value}))
        }
        "thread/queue/list" => {
            let thread_id = string_param(params, "threadId").unwrap_or_default();
            let submissions = state
                .queues
                .get(thread_id)
                .into_iter()
                .flatten()
                .map(QueuedSubmission::as_json)
                .collect::<Vec<_>>();
            let (start, end, next_cursor) = page_bounds(params, submissions.len(), state.page_size);
            Ok(json!({
                "data": &submissions[start..end],
                "nextCursor": next_cursor
            }))
        }
        "model/list" => {
            let models = model_list();
            let models = models["data"].as_array().unwrap();
            let (start, end, next_cursor) = page_bounds(params, models.len(), state.page_size);
            Ok(json!({"data": &models[start..end], "nextCursor": next_cursor}))
        }
        "thread/settings/update" => {
            let Some(thread_id) = string_param(params, "threadId") else {
                return (invalid_params("threadId"), notifications);
            };
            let Some(thread) = state.threads.get_mut(thread_id) else {
                return (unknown_thread(thread_id), notifications);
            };
            if let Some(model) = string_param(params, "model") {
                thread.model = model.into();
            }
            if let Some(effort) = string_param(params, "effort") {
                thread.reasoning_effort = effort.into();
            }
            if params.get("serviceTier").is_some() {
                thread.service_tier = string_param(params, "serviceTier").map(str::to_owned);
            }
            Ok(json!({}))
        }
        "thread/goal/get" => with_thread(&state, params, |thread| Ok(json!({"goal": thread.goal}))),
        "thread/goal/set" => {
            let Some(thread_id) = string_param(params, "threadId") else {
                return (invalid_params("threadId"), notifications);
            };
            let Some(thread) = state.threads.get_mut(thread_id) else {
                return (unknown_thread(thread_id), notifications);
            };
            let mut goal = thread.goal.take().unwrap_or_else(|| {
                json!({
                    "objective": "Unnamed goal",
                    "status": "active",
                    "threadId": thread_id,
                    "createdAt": 1_000,
                    "updatedAt": 1_000,
                    "tokensUsed": 0,
                    "timeUsedSeconds": 0,
                    "tokenBudget": null
                })
            });
            if let Some(objective) = string_param(params, "objective") {
                goal["objective"] = json!(objective);
            }
            if let Some(status) = string_param(params, "status") {
                goal["status"] = json!(status);
            }
            thread.goal = Some(goal.clone());
            Ok(json!({"goal": goal}))
        }
        "thread/goal/clear" => {
            let Some(thread_id) = string_param(params, "threadId") else {
                return (invalid_params("threadId"), notifications);
            };
            let Some(thread) = state.threads.get_mut(thread_id) else {
                return (unknown_thread(thread_id), notifications);
            };
            thread.goal = None;
            Ok(json!({"cleared": true}))
        }
        "thread/fork" => {
            let Some(source_id) = string_param(params, "threadId") else {
                return (invalid_params("threadId"), notifications);
            };
            let Some(source) = state.threads.get(source_id).cloned() else {
                return (unknown_thread(source_id), notifications);
            };
            let fork_id = format!("{}_fork", source.id);
            let mut fork = source;
            fork.id.clone_from(&fork_id);
            fork.name.push_str(" (fork)");
            if params.get("excludeTurns").and_then(Value::as_bool) == Some(true) {
                fork.turns.clear();
            }
            let value = fork.as_json(false);
            let mut response = thread_context_response(&fork);
            response["thread"] = value;
            state.threads.insert(fork_id, fork);
            Ok(response)
        }
        "skills/list" => Ok(json!({
            "data": [{
                "cwd": "/work",
                "skills": [{
                    "name": "testing",
                    "description": "Exercises the Agentix integration fixture",
                    "enabled": true,
                    "path": "/mock/skills/testing/SKILL.md",
                    "scope": "repo"
                }],
                "errors": []
            }]
        })),
        "review/start" => Ok(json!({
            "reviewThreadId": string_param(params, "threadId").unwrap_or_default(),
            "turn": MockTurn::in_progress("turn_review", "review changes").as_json()
        })),
        "mcpServerStatus/list" => Ok(json!({
            "data": [{
                "name": "filesystem",
                "runtimeStatus": "connected",
                "authStatus": "unsupported",
                "tools": {
                    "read_file": {"name": "read_file", "inputSchema": {"type": "object"}},
                    "write_file": {"name": "write_file", "inputSchema": {"type": "object"}}
                },
                "resources": [],
                "resourceTemplates": []
            }]
        })),
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    if let Ok(value) = &result {
        state
            .results
            .entry(method.into())
            .or_default()
            .push(value.clone());
    }
    state.notifications.extend(notifications.iter().cloned());
    (result, notifications)
}

fn thread_context_response(thread: &MockThread) -> Value {
    json!({
        "approvalPolicy": "on-request",
        "approvalsReviewer": "user",
        "cwd": thread.cwd,
        "model": thread.model,
        "modelProvider": "openai",
        "reasoningEffort": thread.reasoning_effort,
        "serviceTier": thread.service_tier,
        "runtimeWorkspaceRoots": [thread.cwd],
        "sandbox": {"type": "workspaceWrite"},
        "thread": thread.as_json(false)
    })
}

fn with_thread(
    state: &ServerState,
    params: &Value,
    call: impl FnOnce(&MockThread) -> Result<Value, (i64, String)>,
) -> Result<Value, (i64, String)> {
    let Some(thread_id) = string_param(params, "threadId") else {
        return invalid_params("threadId");
    };
    let Some(thread) = state.threads.get(thread_id) else {
        return unknown_thread(thread_id);
    };
    call(thread)
}

fn string_param<'a>(params: &'a Value, name: &str) -> Option<&'a str> {
    params.get(name).and_then(Value::as_str)
}

fn page_bounds(
    params: &Value,
    total: usize,
    configured_page_size: Option<usize>,
) -> (usize, usize, Option<String>) {
    let start = params
        .get("cursor")
        .and_then(Value::as_str)
        .and_then(|cursor| cursor.strip_prefix("mock:"))
        .and_then(|offset| offset.parse().ok())
        .unwrap_or(0)
        .min(total);
    let requested = params
        .get("limit")
        .and_then(Value::as_u64)
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(usize::MAX);
    let limit = configured_page_size.map_or(requested, |size| size.min(requested));
    let end = start.saturating_add(limit).min(total);
    let next_cursor = (end < total).then(|| format!("mock:{end}"));
    (start, end, next_cursor)
}

fn input_text(params: &Value) -> Option<&str> {
    params
        .get("input")?
        .as_array()?
        .iter()
        .find(|input| input.get("type").and_then(Value::as_str) == Some("text"))?
        .get("text")?
        .as_str()
}

fn invalid_params(field: &str) -> Result<Value, (i64, String)> {
    Err((-32602, format!("missing parameter: {field}")))
}

fn unknown_thread(thread_id: &str) -> Result<Value, (i64, String)> {
    Err((-32600, format!("unknown thread: {thread_id}")))
}

fn model_list() -> Value {
    json!({
        "data": [
            {
                "id": "gpt-5.6",
                "model": "gpt-5.6",
                "displayName": "GPT-5.6",
                "description": "Flagship coding model",
                "hidden": false,
                "isDefault": true,
                "defaultReasoningEffort": "medium",
                "supportedReasoningEfforts": [
                    {"reasoningEffort": "medium", "description": "Balanced"},
                    {"reasoningEffort": "high", "description": "Deep reasoning"}
                ],
                "serviceTiers": [
                    {"id": "fast", "name": "Fast", "description": "Low latency"}
                ],
                "defaultServiceTier": null
            },
            {
                "id": "gpt-5.6-terra",
                "model": "gpt-5.6-terra",
                "displayName": "GPT-5.6 Terra",
                "description": "Balanced coding model",
                "hidden": false,
                "isDefault": false,
                "defaultReasoningEffort": "medium",
                "supportedReasoningEfforts": [
                    {"reasoningEffort": "medium", "description": "Balanced"},
                    {"reasoningEffort": "high", "description": "Deep reasoning"}
                ],
                "serviceTiers": [],
                "defaultServiceTier": null
            }
        ],
        "nextCursor": null
    })
}
