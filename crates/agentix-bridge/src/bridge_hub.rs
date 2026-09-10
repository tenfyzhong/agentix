//! Native session registry for connections accepted by the Agentix control listener.
use crate::bridge::{Connection, Reader, Writer, read_frame};
use crate::{BridgeKind, wire};
use agentix_domain::{AgentError, AgentEvent, SessionId, SessionStatus};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, broadcast};

struct Backend {
    root: PathBuf,
    events: broadcast::Sender<AgentEvent>,
}
pub(super) struct State {
    registrations: Mutex<std::collections::HashMap<(String, SessionId), Arc<Connection>>>,
    pending_registrations: std::sync::Mutex<std::collections::HashSet<(String, SessionId)>>,
    closed: AtomicBool,
    backends: std::sync::Mutex<std::collections::HashMap<String, Backend>>,
}
impl State {
    pub(super) async fn disconnected(&self, agent: &str, session: &SessionId, instance: &str) {
        let mut entries = self.registrations.lock().await;
        let key = (agent.to_owned(), session.clone());
        if entries
            .get(&key)
            .is_some_and(|connection| connection.instance == instance)
        {
            let connection = entries.remove(&key).unwrap();
            connection.online.store(false, Ordering::Release);
            if let Some(backend) = self.backends.lock().unwrap().get(agent) {
                let _ = backend.events.send(AgentEvent::SessionStatusChanged {
                    session_id: session.to_string(),
                    status: SessionStatus::Offline,
                });
            }
        }
    }
}

// Reserve only this identity while its acknowledgement is being written.
// Drop also releases ownership when the accept future is cancelled.
struct RegistrationReservation {
    state: Arc<State>,
    key: (String, SessionId),
}
impl Drop for RegistrationReservation {
    fn drop(&mut self) {
        self.state
            .pending_registrations
            .lock()
            .unwrap()
            .remove(&self.key);
    }
}

pub struct BridgeHub {
    pub(super) state: Arc<State>,
}
fn unavailable(error: impl std::fmt::Display) -> AgentError {
    AgentError::Unavailable(error.to_string())
}
impl Default for BridgeHub {
    fn default() -> Self {
        Self::new()
    }
}
impl BridgeHub {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(State {
                registrations: Mutex::default(),
                pending_registrations: std::sync::Mutex::default(),
                closed: AtomicBool::new(false),
                backends: std::sync::Mutex::default(),
            }),
        }
    }
    /// Accept a registration already read from the local control endpoint.
    /// The supplied stream retains any bytes buffered beyond the first frame.
    pub async fn accept<S>(&self, frame: Value, stream: S) -> Result<(), AgentError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        let (reader, writer) = tokio::io::split(stream);
        accept(
            self.state.clone(),
            frame,
            BufReader::new(Box::new(reader)),
            Box::new(writer),
        )
        .await
    }
    pub async fn shutdown(&self) {
        self.state.closed.store(true, Ordering::Release);
        let connections: Vec<_> = self.state.registrations.lock().await.drain().collect();
        for ((agent, session), connection) in connections {
            connection.shutdown().await;
            if let Some(backend) = self.state.backends.lock().unwrap().get(&agent) {
                let _ = backend.events.send(AgentEvent::SessionStatusChanged {
                    session_id: session.to_string(),
                    status: SessionStatus::Offline,
                });
            }
        }
    }
    pub(super) fn configure(&self, flavor: BridgeKind, root: &Path) {
        self.state
            .backends
            .lock()
            .unwrap()
            .entry(flavor.as_str().into())
            .and_modify(|backend| root.clone_into(&mut backend.root))
            .or_insert_with(|| Backend {
                root: root.to_owned(),
                events: broadcast::channel(1024).0,
            });
    }
    pub(super) fn events(&self, flavor: BridgeKind) -> broadcast::Sender<AgentEvent> {
        self.state.backends.lock().unwrap()[flavor.as_str()]
            .events
            .clone()
    }
    pub(super) async fn connection(
        &self,
        flavor: BridgeKind,
        session: &SessionId,
    ) -> Option<Arc<Connection>> {
        self.state
            .registrations
            .lock()
            .await
            .get(&(flavor.as_str().into(), session.clone()))
            .filter(|connection| connection.online.load(Ordering::Acquire))
            .cloned()
    }
    pub(super) async fn connections(&self, flavor: BridgeKind) -> Vec<Arc<Connection>> {
        self.state
            .registrations
            .lock()
            .await
            .iter()
            .filter(|((agent, _), connection)| {
                agent == flavor.as_str() && connection.online.load(Ordering::Acquire)
            })
            .map(|(_, connection)| connection.clone())
            .collect()
    }
    /// Query the running service without binding or taking ownership of its listener.
    pub async fn live_count(
        endpoint: &str,
        flavor: BridgeKind,
        root: &Path,
    ) -> Result<usize, AgentError> {
        let (reader, mut writer): (Reader, Writer) =
            if let Some(address) = endpoint.strip_prefix("tcp://") {
                let address: std::net::SocketAddr = address.parse().map_err(unavailable)?;
                if !address.ip().is_loopback() {
                    return Err(unavailable(
                        "control TCP endpoint must use a loopback address",
                    ));
                }
                let stream = tokio::net::TcpStream::connect(address)
                    .await
                    .map_err(unavailable)?;
                let (reader, writer) = stream.into_split();
                (BufReader::new(Box::new(reader)), Box::new(writer))
            } else {
                #[cfg(unix)]
                {
                    let path = endpoint
                        .strip_prefix("unix://")
                        .ok_or_else(|| unavailable("invalid control endpoint"))?;
                    let stream = tokio::net::UnixStream::connect(path)
                        .await
                        .map_err(unavailable)?;
                    let (reader, writer) = stream.into_split();
                    (BufReader::new(Box::new(reader)), Box::new(writer))
                }
                #[cfg(not(unix))]
                return Err(unavailable(
                    "control endpoint must use tcp:// on this platform",
                ));
            };
        let request = json!({"id":"inspect","method":"inspect","params":{"version":wire::PROTOCOL_VERSION,"agent":flavor.as_str(),"session_root":root}});
        writer
            .write_all(format!("{request}\n").as_bytes())
            .await
            .map_err(unavailable)?;
        let mut reader = reader;
        let response = tokio::time::timeout(Duration::from_secs(3), read_frame(&mut reader))
            .await
            .map_err(unavailable)??;
        response["result"]["count"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| unavailable("bridge inspection failed"))
    }
}
async fn inspect(state: &State, frame: &Value, writer: &mut Writer) -> Result<(), AgentError> {
    let params = &frame["params"];
    let root = Path::new(params["session_root"].as_str().unwrap_or_default());
    let agent = params["agent"].as_str().unwrap_or_default();
    let configured = state
        .backends
        .lock()
        .unwrap()
        .get(agent)
        .is_some_and(|b| b.root == root);
    let count = state
        .registrations
        .lock()
        .await
        .iter()
        .filter(|((backend, _), connection)| {
            configured && backend == agent && connection.online.load(Ordering::Acquire)
        })
        .count();
    writer
        .write_all(
            format!(
                "{}\n",
                json!({"id":frame["id"],"ok":true,"result":{"count":count}})
            )
            .as_bytes(),
        )
        .await
        .map_err(unavailable)?;
    Ok(())
}
async fn accept(
    state: Arc<State>,
    frame: Value,
    reader: Reader,
    mut writer: Writer,
) -> Result<(), AgentError> {
    let params = &frame["params"];
    if params["version"] != wire::PROTOCOL_VERSION {
        return Err(unavailable("unsupported bridge protocol version"));
    }
    if frame["method"] == "inspect" {
        return inspect(&state, &frame, &mut writer).await;
    }
    let record: wire::Registration = serde_json::from_value(params.clone()).map_err(unavailable)?;
    if frame["method"] != "register"
        || record.session_id.is_empty()
        || record.snapshot.session.id != record.session_id
        || record.snapshot.instance != record.instance
        || record.instance.is_empty()
    {
        return Err(unavailable("invalid bridge registration"));
    }
    let path = Path::new(&record.session_file);
    let events = state
        .backends
        .lock()
        .unwrap()
        .get(&record.agent)
        .filter(|backend| {
            path.is_absolute()
                && !path
                    .components()
                    .any(|part| part == std::path::Component::ParentDir)
                && path.starts_with(&backend.root)
        })
        .map(|backend| backend.events.clone())
        .ok_or_else(|| unavailable("session file is outside the configured backend root"))?;
    let key = (record.agent.clone(), SessionId::new(&record.session_id));
    if !state
        .pending_registrations
        .lock()
        .unwrap()
        .insert(key.clone())
    {
        return Err(unavailable("another instance is registering this session"));
    }
    let _reservation = RegistrationReservation {
        state: state.clone(),
        key: key.clone(),
    };
    {
        let entries = state.registrations.lock().await;
        if state.closed.load(Ordering::Acquire) {
            return Err(unavailable("Agentix is shutting down"));
        }
        if entries.contains_key(&key) {
            return Err(unavailable("another live instance owns this session"));
        }
    }
    tokio::time::timeout(
        Duration::from_secs(3),
        writer.write_all(format!("{}\n", json!({"id":frame["id"],"ok":true})).as_bytes()),
    )
    .await
    .map_err(unavailable)?
    .map_err(unavailable)?;
    let mut entries = state.registrations.lock().await;
    if state.closed.load(Ordering::Acquire) {
        return Err(unavailable("Agentix is shutting down"));
    }
    let connection = Connection::new(
        &record,
        reader,
        writer,
        events.clone(),
        Arc::downgrade(&state),
    );
    entries.insert(key, connection);
    let _ = events.send(AgentEvent::SessionResumed {
        session_id: record.session_id,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn registration(session: &str) -> Value {
        let mut snapshot: Value = serde_json::from_str(include_str!(
            "../../../plugins/agentix-bridge/protocol/snapshot.json"
        ))
        .unwrap();
        snapshot["session"]["id"] = json!(session);
        json!({
            "id": "register", "method": "register", "params": {
                "version": wire::PROTOCOL_VERSION, "agent": "pi", "instance": "contract-instance",
                "pid": 1, "session_id": session, "cwd": "/tmp", "session_file": "/tmp/session.jsonl",
                "snapshot": snapshot
            }
        })
    }

    #[tokio::test]
    async fn stalled_registration_does_not_block_other_sessions_and_cancellation_releases_owner() {
        let hub = Arc::new(BridgeHub::new());
        hub.configure(BridgeKind::Pi, Path::new("/tmp"));
        let (stream, mut peer) = tokio::io::duplex(1);
        let stalled = tokio::spawn({
            let hub = hub.clone();
            async move { hub.accept(registration("stalled"), stream).await }
        });
        // Wait until the acknowledgement write begins and fills the tiny duplex buffer.
        peer.read_u8().await.unwrap();
        let result =
            tokio::time::timeout(Duration::from_millis(100), hub.connections(BridgeKind::Pi)).await;
        assert!(
            result.is_ok(),
            "a registration write held the shared connection registry"
        );

        let (stream, _peer) = tokio::io::duplex(1024);
        tokio::time::timeout(
            Duration::from_millis(100),
            hub.accept(registration("other"), stream),
        )
        .await
        .unwrap()
        .unwrap();
        let (duplicate, _duplicate_peer) = tokio::io::duplex(1024);
        assert!(
            hub.accept(registration("stalled"), duplicate)
                .await
                .is_err()
        );
        stalled.abort();
        let _ = stalled.await;
        let (retry, _retry_peer) = tokio::io::duplex(1024);
        hub.accept(registration("stalled"), retry).await.unwrap();
        hub.shutdown().await;
    }
}
