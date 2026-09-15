use agentix_domain::{AgentEvent, InteractionRequest};
mod content;
mod questions;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// A live transport connection. PID is absent when the OS cannot establish it.
#[derive(Clone, Debug, Serialize)]
pub struct ClientBinding {
    pub connection_id: u64,
    pub client_id: String,
    pub pid: Option<u32>,
    pub client_name: Option<String>,
    pub sessions: Vec<String>,
}

#[derive(Default)]
struct Connection {
    client_id: String,
    previous: Option<(String, Instant)>,
    fresh: Option<(String, Instant)>,
    pid: Option<u32>,
    client_name: Option<String>,
    sessions: BTreeSet<String>,
    pending: HashMap<String, (String, Option<String>)>,
}

#[derive(Default)]
struct State {
    questions: HashMap<String, (InteractionRequest, BTreeSet<u64>)>,
    resolved_questions: std::collections::VecDeque<String>,
    content_sequence: u64,
    content_versions: HashMap<String, (u64, u64)>,
    next_id: u64,
    sequence: u64,
    lifecycle: std::collections::VecDeque<(u64, AgentEvent)>,
    connections: BTreeMap<u64, Connection>,
}

/// Connection-owned registrations; neither cwd nor loaded threads imply ownership.
#[derive(Clone)]
pub struct ClientRegistry {
    state: Arc<Mutex<State>>,
    changed: watch::Sender<u64>,
    content_changed: watch::Sender<u64>,
    completion_changed: watch::Sender<u64>,
}

impl Default for ClientRegistry {
    fn default() -> Self {
        Self {
            state: Arc::default(),
            changed: watch::channel(0).0,
            content_changed: watch::channel(0).0,
            completion_changed: watch::channel(0).0,
        }
    }
}

#[derive(Deserialize)]
struct FrameHeader<'a> {
    #[serde(borrow)]
    method: Option<std::borrow::Cow<'a, str>>,
    id: Option<Value>,
}

// Notifications normally begin with method. A read-only observer can stop as
// soon as that field proves no lifecycle response is possible. Anything that
// could mutate ownership still goes through complete JSON validation below.
fn leading_method(text: &str) -> Option<String> {
    let object = text.trim_start().strip_prefix('{')?;
    let mut keys = serde_json::Deserializer::from_str(object).into_iter::<String>();
    if keys.next()?.ok()? != "method" {
        return None;
    }
    let value = object[keys.byte_offset()..]
        .trim_start()
        .strip_prefix(':')?;
    serde_json::Deserializer::from_str(value)
        .into_iter::<String>()
        .next()?
        .ok()
}

impl ClientRegistry {
    /// Observe lifecycle requests without allocating unrelated payloads.
    pub fn client_frame(&self, connection: u64, text: &str) {
        let Ok(header) = serde_json::from_str::<FrameHeader<'_>>(text) else {
            return;
        };
        if header.method.is_none() {
            self.observe_question_answer(connection, text);
        }
        if matches!(
            header.method.as_deref(),
            Some(
                "initialize"
                    | "thread/start"
                    | "thread/resume"
                    | "thread/fork"
                    | "thread/unsubscribe"
            )
        ) && let Ok(message) = serde_json::from_str(text)
        {
            self.client_message(connection, &message);
        }
    }

    /// Observe owned content notifications without allocating their payloads.
    pub fn server_frame(&self, connection: u64, text: &str) {
        if let Some(method) = leading_method(text) {
            self.observe_question_frame(connection, &method, text);
            self.observe_content_frame(connection, &method, text);
            return;
        }
        let Ok(header) = serde_json::from_str::<FrameHeader<'_>>(text) else {
            return;
        };
        if let Some(method) = header.method {
            self.observe_question_frame(connection, &method, text);
            self.observe_content_frame(connection, &method, text);
            return;
        }
        let Some(id) = header.id else { return };
        let tracked = self
            .state
            .lock()
            .unwrap()
            .connections
            .get(&connection)
            .is_some_and(|c| c.pending.contains_key(&id.to_string()));
        if tracked && let Ok(message) = serde_json::from_str(text) {
            self.server_message(connection, &message);
        }
    }

    fn observe_content_frame(&self, connection: u64, method: &str, text: &str) {
        if !(method.starts_with("item/")
            || method.starts_with("turn/")
            || method.starts_with("thread/status/"))
        {
            return;
        }
        if self
            .state
            .lock()
            .unwrap()
            .connections
            .get(&connection)
            .is_none_or(|c| c.sessions.is_empty())
        {
            return;
        }
        let Some(session) = content::session_hint(text) else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        if !state
            .connections
            .get(&connection)
            .is_some_and(|c| c.sessions.contains(&session))
        {
            return;
        }
        state.content_sequence = state.content_sequence.wrapping_add(1);
        let sequence = state.content_sequence;
        let versions = state.content_versions.entry(session).or_default();
        versions.0 = sequence;
        if method == "turn/completed" {
            versions.1 = sequence;
        }
        drop(state);
        self.content_changed.send_replace(sequence);
        if method == "turn/completed" {
            self.completion_changed.send_replace(sequence);
        }
    }

    pub(crate) fn subscribe_content(&self) -> watch::Receiver<u64> {
        self.content_changed.subscribe()
    }

    pub(crate) fn subscribe_completions(&self) -> watch::Receiver<u64> {
        self.completion_changed.subscribe()
    }

    pub(crate) fn content_versions(&self) -> HashMap<String, (u64, u64)> {
        self.state.lock().unwrap().content_versions.clone()
    }

    pub(crate) fn wake(&self) {
        self.changed
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    pub(crate) fn replacement_timeout(&self) -> Option<Duration> {
        self.state
            .lock()
            .unwrap()
            .connections
            .values()
            .filter_map(|c| c.previous.as_ref())
            .filter_map(|(_, when)| Duration::from_mins(2).checked_sub(when.elapsed()))
            .min()
    }

    #[must_use]
    pub fn session_terminals(
        &self,
        panes: &HashMap<u32, agentix_domain::TerminalLocation>,
    ) -> HashMap<String, agentix_domain::TerminalLocation> {
        let mut terminals = HashMap::new();
        for client in self.snapshot() {
            if let Some(terminal) = client.pid.and_then(|pid| panes.get(&pid)) {
                for session in client.sessions {
                    terminals.entry(session).or_insert_with(|| terminal.clone());
                }
            }
        }
        terminals
    }

    #[must_use]
    pub fn connect(&self, pid: Option<u32>) -> u64 {
        let mut state = self.state.lock().unwrap();
        state.next_id += 1;
        let id = state.next_id;
        let client_id = process_identity(pid)
            .unwrap_or_else(|| format!("connection:{}:{id}", std::process::id()));
        state.connections.insert(
            id,
            Connection {
                client_id,
                pid,
                ..Connection::default()
            },
        );
        id
    }

    pub fn disconnect(&self, id: u64) {
        let mut state = self.state.lock().unwrap();
        state.connections.remove(&id);
        prune_content_versions(&mut state);
        drop(state);
        self.changed
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<ClientBinding> {
        self.state
            .lock()
            .unwrap()
            .connections
            .iter()
            .map(|(&id, c)| ClientBinding {
                connection_id: id,
                client_id: c.client_id.clone(),
                pid: c.pid,
                client_name: c.client_name.clone(),
                sessions: c.sessions.iter().cloned().collect(),
            })
            .collect()
    }

    /// Keep the old attachment during a native switch only while its transport
    /// remains connected. Closing the transport always ends this candidate.
    #[must_use]
    pub fn awaiting_replacement(&self, session: &str) -> bool {
        self.state.lock().unwrap().connections.values().any(|c| {
            c.previous
                .as_ref()
                .is_some_and(|(id, when)| id == session && when.elapsed() < Duration::from_mins(2))
        })
    }

    #[must_use]
    pub fn lifecycle_since(&self, sequence: u64) -> Vec<(u64, AgentEvent)> {
        self.state
            .lock()
            .unwrap()
            .lifecycle
            .iter()
            .filter(|(id, _)| *id > sequence)
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    pub fn client_message(&self, connection: u64, message: &Value) {
        let mut state = self.state.lock().unwrap();
        let Some(c) = state.connections.get_mut(&connection) else {
            return;
        };
        let Some(method) = message["method"].as_str() else {
            return;
        };
        if method == "initialize" {
            c.client_name = message["params"]["clientInfo"]["name"]
                .as_str()
                .map(str::to_owned);
        }
        if matches!(
            method,
            "thread/start" | "thread/resume" | "thread/fork" | "thread/unsubscribe"
        ) && let Some(id) = message
            .get("id")
            .filter(|id| id.is_string() || id.is_number())
        {
            if method == "thread/start" && message["params"]["ephemeral"] == true {
                return;
            }
            c.pending.insert(
                id.to_string(),
                (
                    method.to_owned(),
                    message["params"]["threadId"].as_str().map(str::to_owned),
                ),
            );
        }
    }

    pub fn server_message(&self, connection: u64, message: &Value) {
        // A server request can reuse a client request's id. It is not its response.
        if message.get("method").is_some() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        let Some(c) = state.connections.get_mut(&connection) else {
            return;
        };
        let Some(id) = message.get("id") else { return };
        let Some((method, thread)) = c.pending.remove(&id.to_string()) else {
            return;
        };
        let Some(result) = message.get("result") else {
            return;
        };
        let mut events = Vec::new();
        if method == "thread/unsubscribe" {
            if matches!(
                result["status"].as_str(),
                Some("unsubscribed" | "notSubscribed" | "notLoaded")
            ) && let Some(thread) = thread
                && c.sessions.remove(&thread)
            {
                c.previous = Some((thread.clone(), Instant::now()));
                if let Some((fresh, when)) = c.fresh.take()
                    && fresh != thread
                    && when.elapsed() < Duration::from_mins(2)
                {
                    c.previous = None;
                    events.push(AgentEvent::SessionSwitchStarted {
                        session_id: thread.clone(),
                        client_id: c.client_id.clone(),
                    });
                    events.push(AgentEvent::SessionReplaced {
                        session_id: thread,
                        replacement_session_id: fresh,
                        client_id: c.client_id.clone(),
                    });
                }
            }
        } else if let Some(thread) = result["thread"]["id"].as_str() {
            if result["thread"]["ephemeral"] == true {
                return;
            }
            c.sessions.insert(thread.to_owned());
            if method == "thread/start" && result["thread"]["source"].get("subAgent").is_none() {
                if let Some((previous, when)) = c.previous.take()
                    && when.elapsed() < Duration::from_mins(2)
                    && previous != thread
                {
                    events.push(AgentEvent::SessionSwitchStarted {
                        session_id: previous.clone(),
                        client_id: c.client_id.clone(),
                    });
                    events.push(AgentEvent::SessionReplaced {
                        session_id: previous,
                        replacement_session_id: thread.into(),
                        client_id: c.client_id.clone(),
                    });
                } else {
                    c.fresh = Some((thread.into(), Instant::now()));
                }
            } else if method == "thread/resume" {
                c.previous = None;
                c.fresh = None;
            }
        }
        prune_content_versions(&mut state);
        for event in events {
            state.sequence += 1;
            let sequence = state.sequence;
            state.lifecycle.push_back((sequence, event));
            while state.lifecycle.len() > 1024 {
                state.lifecycle.pop_front();
            }
        }
        drop(state);
        self.changed
            .send_modify(|version| *version = version.wrapping_add(1));
    }
}

fn prune_content_versions(state: &mut State) {
    let owned = state
        .connections
        .values()
        .flat_map(|c| c.sessions.iter())
        .collect::<BTreeSet<_>>();
    state
        .content_versions
        .retain(|session, _| owned.contains(session));
}

fn process_identity(pid: Option<u32>) -> Option<String> {
    let pid = pid?;
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .output()
        .ok()?;
    let started = String::from_utf8(output.stdout).ok()?;
    let started = started.trim();
    (!started.is_empty()).then(|| format!("process:{pid}:{started}"))
}
