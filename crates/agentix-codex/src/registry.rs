use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// A live transport connection. PID is absent when the OS cannot establish it.
#[derive(Clone, Debug, Serialize)]
pub struct ClientBinding {
    pub connection_id: u64,
    pub pid: Option<u32>,
    pub client_name: Option<String>,
    pub sessions: Vec<String>,
}

#[derive(Default)]
struct Connection {
    pid: Option<u32>,
    client_name: Option<String>,
    sessions: BTreeSet<String>,
    pending: HashMap<String, (String, Option<String>)>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    connections: BTreeMap<u64, Connection>,
}

/// Connection-owned registrations; neither cwd nor loaded threads imply ownership.
#[derive(Clone)]
pub struct ClientRegistry {
    state: Arc<Mutex<State>>,
    changed: watch::Sender<u64>,
}

impl Default for ClientRegistry {
    fn default() -> Self {
        Self {
            state: Arc::default(),
            changed: watch::channel(0).0,
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

    /// Streamed notifications and unrelated responses need no full JSON value or mutation.
    pub fn server_frame(&self, connection: u64, text: &str) {
        if leading_method(text).is_some() {
            return;
        }
        let Ok(header) = serde_json::from_str::<FrameHeader<'_>>(text) else {
            return;
        };
        if header.method.is_some() {
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
        state.connections.insert(
            id,
            Connection {
                pid,
                ..Connection::default()
            },
        );
        id
    }

    pub fn disconnect(&self, id: u64) {
        self.state.lock().unwrap().connections.remove(&id);
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
                pid: c.pid,
                client_name: c.client_name.clone(),
                sessions: c.sessions.iter().cloned().collect(),
            })
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
        if method == "thread/unsubscribe" {
            if matches!(
                result["status"].as_str(),
                Some("unsubscribed" | "notSubscribed" | "notLoaded")
            ) && let Some(thread) = thread
            {
                c.sessions.remove(&thread);
            }
        } else if let Some(thread) = result["thread"]["id"].as_str() {
            c.sessions.insert(thread.to_owned());
        }
        drop(state);
        self.changed
            .send_modify(|version| *version = version.wrapping_add(1));
    }
}
