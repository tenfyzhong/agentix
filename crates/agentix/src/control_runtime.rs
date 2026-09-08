//! Bounded control request dispatch; the socket listener never waits for host I/O.
use super::{ClaimRegistry, control, handle_control_request};
use agentix_codex::CodexClient;
use agentix_core::{
    AgentAdapter, AgentKind, DispatchQueue, DispatchScope, SessionId, SessionKey, SessionOperation,
};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::{sync::mpsc, task::JoinSet};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Resource {
    Backend(AgentKind),
    Session(SessionId, SessionLane),
    Catalog,
    Claims,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SessionLane {
    Mutation,
    History,
    Interrupt,
}

fn request_scope(
    request: &control::ControlRequest,
    backends: &[AgentKind],
) -> DispatchScope<Resource> {
    match request {
        control::ControlRequest::Session(operation) => {
            let (session, lane) = match operation {
                SessionOperation::Send { session, .. }
                | SessionOperation::Command { session, .. } => (session, SessionLane::Mutation),
                SessionOperation::History { session, .. } => (session, SessionLane::History),
                SessionOperation::Stop { session, .. } => (session, SessionLane::Interrupt),
            };
            let keys = if let Some(key) = SessionKey::decode(session) {
                vec![key]
            } else {
                // Reserve every possible qualified alias without performing a
                // remote lookup on the admission loop. The adapter still resolves
                // or rejects ambiguity when the operation executes.
                backends
                    .iter()
                    .map(|kind| SessionKey::new(*kind, session.clone()))
                    .collect()
            };
            DispatchScope::Access {
                shared: keys
                    .iter()
                    .map(|key| Resource::Backend(key.agent))
                    .collect(),
                exclusive: std::iter::once(Resource::Session(session.clone(), lane))
                    .chain(keys.iter().map(|key| Resource::Session(key.encode(), lane)))
                    .collect(),
            }
        }
        control::ControlRequest::Sessions { .. } => DispatchScope::Access {
            shared: backends.iter().copied().map(Resource::Backend).collect(),
            exclusive: vec![Resource::Catalog],
        },
        control::ControlRequest::Call { .. } => {
            DispatchScope::Keys(vec![Resource::Backend(AgentKind::Codex)])
        }
        control::ControlRequest::Claim { .. } => DispatchScope::Keys(vec![Resource::Claims]),
    }
}

pub(super) async fn run_control_handler(
    mut calls: mpsc::Receiver<control::ControlCall>,
    agent: Arc<dyn AgentAdapter>,
    codex: Option<CodexClient>,
    claims: Arc<ClaimRegistry>,
    config_path: PathBuf,
    shutdown: CancellationToken,
) {
    let backends = agent.session_backends();
    let mut queue = DispatchQueue::new(256, 32);
    let mut workers = JoinSet::new();
    let mut active = HashMap::new();
    let mut open = true;
    let mut telemetry = tokio::time::interval(std::time::Duration::from_secs(30));
    telemetry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        while !shutdown.is_cancelled()
            && let Some(job) = queue
                .next_ready(|call: &control::ControlCall| request_scope(&call.request, &backends))
        {
            let agent = agent.clone();
            let codex = codex.clone();
            let claims = claims.clone();
            let config_path = config_path.clone();
            let id = job.id;
            let worker = workers.spawn(async move {
                let started = std::time::Instant::now();
                let call = job.work;
                let response = handle_control_request(
                    call.request.clone(),
                    &agent,
                    codex.as_ref(),
                    &claims,
                    &config_path,
                )
                .await;
                call.respond(response);
                tracing::debug!(target: "agentix::telemetry", runtime = "control",
                    wait_ms = job.queued_for.as_secs_f64() * 1000.0,
                    execution_ms = started.elapsed().as_secs_f64() * 1000.0, "dispatch completed");
                id
            });
            active.insert(worker.id(), id);
        }
        if !open && queue.is_empty() {
            break;
        }
        tokio::select! {
            biased;
            () = shutdown.cancelled() => break,
            _ = telemetry.tick() => queue.statistics().record("control"),
            completed = workers.join_next_with_id(), if !workers.is_empty() => {
                let task = match completed.expect("an active worker") {
                    Ok((task, _)) => task,
                    Err(error) => {
                        tracing::warn!(%error, "control request worker failed");
                        error.id()
                    }
                };
                if let Some(id) = active.remove(&task) { queue.finish(id); }
            }
            call = calls.recv(), if open && !queue.is_full() => {
                if let Some(call) = call {
                    assert!(queue.try_push(call).is_ok(), "control admission capacity");
                } else { open = false; }
            }
        }
    }
    workers.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(session: &str) -> control::ControlRequest {
        control::ControlRequest::Session(SessionOperation::History {
            session: SessionId::new(session),
            cursor: None,
            limit: 10,
        })
    }

    #[test]
    fn mutations_wait_for_legacy_aliases_while_history_and_stop_remain_available() {
        let mut queue = DispatchQueue::new(4, 4);
        let scope = |request: &control::ControlRequest| request_scope(request, &[AgentKind::Pi]);
        let send = control::ControlRequest::Session(SessionOperation::Send {
            session: SessionId::new("same"),
            text: "pending".into(),
            expected_turn: None,
        });
        let rename = control::ControlRequest::Session(SessionOperation::Command {
            session: SessionId::new("pi:same"),
            command: agentix_core::SessionCommand::Rename(Some("new".into())),
        });
        queue.try_push(send).unwrap();
        queue.try_push(rename.clone()).unwrap();
        queue.try_push(history("pi:same")).unwrap();
        queue
            .try_push(control::ControlRequest::Session(SessionOperation::Stop {
                session: SessionId::new("pi:same"),
                turn: "turn".into(),
            }))
            .unwrap();
        let mutation = queue.next_ready(scope).unwrap();
        assert_eq!(queue.next_ready(scope).unwrap().work, history("pi:same"));
        assert!(matches!(
            queue.next_ready(scope).unwrap().work,
            control::ControlRequest::Session(SessionOperation::Stop { .. })
        ));
        assert!(queue.next_ready(scope).is_none());
        queue.finish(mutation.id);
        assert_eq!(queue.next_ready(scope).unwrap().work, rename);
    }

    #[test]
    fn native_aliases_preserve_order_without_serializing_different_hosts() {
        let mut queue = DispatchQueue::new(4, 4);
        let scope = |request: &control::ControlRequest| {
            request_scope(request, &[AgentKind::Pi, AgentKind::Omp])
        };
        for name in ["oh-my-pi:same", "omp:same", "pi:same"] {
            queue.try_push(history(name)).unwrap();
        }
        let first = queue.next_ready(scope).unwrap();
        let independent = queue.next_ready(scope).unwrap();
        assert_eq!(independent.work, history("pi:same"));
        assert!(queue.next_ready(scope).is_none());
        queue.finish(first.id);
        assert_eq!(queue.next_ready(scope).unwrap().work, history("omp:same"));
    }

    #[test]
    fn raw_codex_call_fences_codex_sessions_but_leaves_native_hosts_available() {
        let mut queue = DispatchQueue::new(4, 4);
        let scope = |request: &control::ControlRequest| {
            request_scope(request, &[AgentKind::Codex, AgentKind::Pi])
        };
        queue.try_push(history("codex:a")).unwrap();
        queue
            .try_push(control::ControlRequest::Call {
                method: "diagnostic".into(),
                params: serde_json::json!({}),
            })
            .unwrap();
        queue.try_push(history("codex:b")).unwrap();
        queue.try_push(history("pi:a")).unwrap();
        let first = queue.next_ready(scope).unwrap();
        assert_eq!(queue.next_ready(scope).unwrap().work, history("pi:a"));
        queue.finish(first.id);
        let raw = queue.next_ready(scope).unwrap();
        assert!(matches!(raw.work, control::ControlRequest::Call { .. }));
        assert!(queue.next_ready(scope).is_none());
        queue.finish(raw.id);
        assert_eq!(queue.next_ready(scope).unwrap().work, history("codex:b"));
    }
}
