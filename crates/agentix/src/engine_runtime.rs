//! Bounded Engine admission and owned workers. No remote I/O runs on admission.
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use agentix_core::{
    AgentAdapter, AgentError, ChannelKind, ConversationRef, DispatchId, DispatchQueue, Engine,
    EngineError, EngineWork, InboundEnvelope, SessionId,
};
use tokio::{
    sync::{broadcast, mpsc},
    task::{Id, JoinError, JoinSet},
};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};

const CAPACITY: usize = 256;
const CONCURRENCY: usize = 32;
const CONVERSATION_CAPACITY: usize = 16;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Refresh {
    Working(SessionId, String),
    TaskBoard,
}

struct Worker {
    dispatch: DispatchId,
    refresh: Option<Refresh>,
    inbound: Option<(ConversationRef, String)>,
}

pub async fn run_engine_loop(
    engine: Arc<Engine>,
    agent: Arc<dyn AgentAdapter>,
    mut inbound: mpsc::Receiver<InboundEnvelope>,
    shutdown: CancellationToken,
) {
    let notification_shutdown = shutdown.child_token();
    let notifications = AbortOnDropHandle::new(tokio::spawn(super::notification_runtime::run(
        engine.clone(),
        notification_shutdown.clone(),
    )));
    let (sources_tx, mut sources_rx) = mpsc::channel(32);
    let source_engine = engine.clone();
    let source_poll = AbortOnDropHandle::new(tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(error) = source_engine.observe_delivery_state().await {
                tracing::warn!(%error, "delivery state observation failed");
            }
            match source_engine.poll_inbox_sources().await {
                Ok(edits) => {
                    for envelope in edits {
                        if sources_tx.send(envelope).await.is_err() {
                            return;
                        }
                    }
                }
                Err(error) => tracing::warn!(%error, "Inbox source refresh failed"),
            }
        }
    }));
    let mut events = agent.subscribe();
    let mut working_interval = tokio::time::interval(Duration::from_secs(1));
    working_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    working_interval.tick().await;
    let mut pool = EngineWorkers::new();
    let mut telemetry = tokio::time::interval(Duration::from_secs(30));
    telemetry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let (mut inbound_open, mut events_open, mut sources_open) = (true, true, true);
    loop {
        if shutdown.is_cancelled() {
            break;
        }
        pool.start_ready(&engine, &shutdown).await;
        if !inbound_open && !events_open && pool.queue.is_empty() {
            break;
        }
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = telemetry.tick() => pool.queue.statistics().record("engine"),
            completed = pool.workers.join_next_with_id(), if !pool.workers.is_empty() => {
                let completed = completed.expect("an active worker");
                let failed = completed.is_err();
                pool.retire(&engine, completed).await;
                if failed {
                    // A panic can leave an application transition incomplete.
                    // Stop the service rather than continue with uncertain state.
                    shutdown.cancel();
                    break;
                }
            }
            _ = working_interval.tick() => pool.schedule_refreshes(&engine).await,
            envelope = inbound.recv(), if inbound_open && !pool.queue.is_full() => {
                match envelope {
                    Some(envelope) => if let Err(error) = pool.admit_inbound(&engine, envelope).await {
                        tracing::error!(%error, "failed to persist inbound admission outcome");
                        shutdown.cancel();
                    },
                    None => inbound_open = false,
                }
            }
            envelope = sources_rx.recv(), if sources_open && !pool.queue.is_full() => {
                match envelope {
                    Some(envelope) => if let Err(error) = pool.admit_inbound(&engine, envelope).await {
                        tracing::error!(%error, "failed to persist source admission outcome");
                        shutdown.cancel();
                    },
                    None => sources_open = false,
                }
            }
            event = events.recv(), if events_open && !pool.queue.is_full() => match event {
                Ok(event) => pool.queue.try_push(EngineWork::Event(event)).expect("event capacity"),
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    tracing::warn!(count, "agent event consumer lagged");
                    pool.queue.try_push(EngineWork::Recover).expect("recovery capacity");
                }
                Err(broadcast::error::RecvError::Closed) => events_open = false,
            }
        }
    }
    source_poll.abort();
    let _ = source_poll.await;
    notification_shutdown.cancel();
    let _ = notifications.await;
    // Stop admission immediately, let acknowledged work settle, then cancel
    // only the remaining workers. Pending queue entries have no side effects.
    let _ = tokio::time::timeout(SHUTDOWN_GRACE, async {
        while let Some(completed) = pool.workers.join_next_with_id().await {
            pool.retire(&engine, completed).await;
        }
    })
    .await;
    pool.workers.abort_all();
    while let Some(completed) = pool.workers.join_next_with_id().await {
        pool.retire(&engine, completed).await;
    }
}

/// Persist and detach local state before attempting bounded IM shutdown effects.
pub async fn shutdown_engine(engine: Arc<Engine>, grace: Duration) -> Result<usize, EngineError> {
    let notifications = engine.prepare_shutdown_notifications().await?;
    Ok(notify_shutdown(engine, notifications, grace).await)
}

/// Deliver shutdown effects within one shared deadline. All local state has
/// already been persisted and detached before these best-effort calls begin.
async fn notify_shutdown(
    engine: Arc<Engine>,
    notifications: Vec<agentix_core::ShutdownNotification>,
    grace: Duration,
) -> usize {
    let mut pending = notifications.into_iter();
    let mut workers = JoinSet::new();
    let mut notified = 0;
    let _ = tokio::time::timeout(grace, async {
        loop {
            while workers.len() < CONCURRENCY
                && let Some(notification) = pending.next()
            {
                let engine = engine.clone();
                workers.spawn(async move { engine.send_shutdown_notification(notification).await });
            }
            let Some(completed) = workers.join_next().await else {
                break;
            };
            match completed {
                Ok(Ok(())) => notified += 1,
                Ok(Err(error)) => {
                    tracing::warn!(%error, "failed to notify IM conversation during shutdown");
                }
                Err(error) => tracing::warn!(%error, "shutdown notification worker failed"),
            }
        }
    })
    .await;
    if !workers.is_empty() {
        tracing::warn!(
            remaining = workers.len() + pending.len(),
            "IM shutdown notification deadline reached"
        );
    }
    workers.shutdown().await;
    notified
}

struct EngineWorkers {
    queue: DispatchQueue<agentix_core::EngineResource, EngineWork>,
    workers: JoinSet<()>,
    active: HashMap<Id, Worker>,
    refreshes: HashSet<Refresh>,
    working_cursor: usize,
    inbound_counts: HashMap<ConversationRef, usize>,
    inbound_ids: HashSet<(ChannelKind, String)>,
    overloaded: u64,
}

impl EngineWorkers {
    fn new() -> Self {
        Self {
            queue: DispatchQueue::new(CAPACITY, CONCURRENCY),
            workers: JoinSet::new(),
            active: HashMap::new(),
            refreshes: HashSet::new(),
            working_cursor: 0,
            inbound_counts: HashMap::new(),
            inbound_ids: HashSet::new(),
            overloaded: 0,
        }
    }

    async fn admit_inbound(
        &mut self,
        engine: &Engine,
        envelope: InboundEnvelope,
    ) -> Result<(), EngineError> {
        let id = (envelope.conversation.channel, envelope.event_id.clone());
        if self.inbound_ids.contains(&id) {
            return Ok(());
        }
        if self
            .inbound_counts
            .get(&envelope.conversation)
            .copied()
            .unwrap_or(0)
            >= CONVERSATION_CAPACITY
        {
            if engine
                .reject_overloaded(&envelope, CONVERSATION_CAPACITY)
                .await?
            {
                self.overloaded = self.overloaded.saturating_add(1);
                tracing::debug!(target: "agentix::telemetry", runtime = "engine",
                    overloaded = self.overloaded, "conversation admission rejected");
            }
            return Ok(());
        }
        *self
            .inbound_counts
            .entry(envelope.conversation.clone())
            .or_default() += 1;
        self.inbound_ids.insert(id);
        self.queue
            .try_push(EngineWork::Inbound(envelope))
            .expect("inbound capacity");
        Ok(())
    }

    async fn start_ready(&mut self, engine: &Arc<Engine>, shutdown: &CancellationToken) {
        if shutdown.is_cancelled() || !self.queue.needs_routing() {
            return;
        }
        let snapshot = engine.dispatch_snapshot().await;
        while !shutdown.is_cancelled()
            && let Some(job) = self.queue.next_ready(|work| snapshot.scope(work))
        {
            let refresh = match &job.work {
                EngineWork::Working { session, turn } => {
                    Some(Refresh::Working(session.clone(), turn.clone()))
                }
                EngineWork::TaskBoard => Some(Refresh::TaskBoard),
                _ => None,
            };
            let inbound = match &job.work {
                EngineWork::Inbound(envelope) => {
                    Some((envelope.conversation.clone(), envelope.event_id.clone()))
                }
                _ => None,
            };
            let engine = engine.clone();
            let handle = self.workers.spawn(async move {
                let started = std::time::Instant::now();
                let description = match &job.work {
                    EngineWork::Inbound(_) => "inbound IM request failed",
                    EngineWork::Event(_) => "agent event failed",
                    EngineWork::Working { .. } => "working state refresh failed",
                    EngineWork::Recover => "failed to recover after agent event loss",
                    EngineWork::TaskBoard => "task board refresh failed",
                };
                if let Err(error) = engine.execute_work(job.work).await {
                    if is_empty_rollout_metadata_error(&error) {
                        tracing::debug!(%error, description);
                    } else {
                        tracing::warn!(%error, description);
                    }
                }
                tracing::debug!(target: "agentix::telemetry", runtime = "engine", operation = description,
                    wait_ms = job.queued_for.as_secs_f64() * 1000.0,
                    execution_ms = started.elapsed().as_secs_f64() * 1000.0, "dispatch completed");
            });
            self.active.insert(
                handle.id(),
                Worker {
                    dispatch: job.id,
                    refresh,
                    inbound,
                },
            );
        }
    }

    async fn schedule_refreshes(&mut self, engine: &Engine) {
        if !self.queue.is_full() && self.refreshes.insert(Refresh::TaskBoard) {
            self.queue
                .try_push(EngineWork::TaskBoard)
                .expect("maintenance capacity");
        }
        let mut turns = engine.working_turns().await;
        // Stable round-robin admission prevents a large active set from
        // starving later sessions when only part of it fits in the queue.
        turns.sort_by(|a, b| (a.0.as_str(), &a.1).cmp(&(b.0.as_str(), &b.1)));
        let count = turns.len();
        if count > 0 {
            turns.rotate_left(self.working_cursor % count);
            for (session, turn) in turns {
                if self.queue.is_full() {
                    break;
                }
                self.working_cursor = (self.working_cursor + 1) % count;
                if self
                    .refreshes
                    .insert(Refresh::Working(session.clone(), turn.clone()))
                {
                    self.queue
                        .try_push(EngineWork::Working { session, turn })
                        .expect("maintenance capacity");
                }
            }
        }
    }

    async fn retire(&mut self, engine: &Engine, completed: Result<(Id, ()), JoinError>) {
        let (id, failed) = match completed {
            Ok((id, ())) => (id, false),
            Err(error) => {
                tracing::warn!(%error, "Engine worker stopped before completing");
                (error.id(), true)
            }
        };
        if let Some(worker) = self.active.remove(&id) {
            self.queue.finish(worker.dispatch);
            if let Some(refresh) = worker.refresh {
                self.refreshes.remove(&refresh);
            }
            if let Some((conversation, event_id)) = worker.inbound {
                self.inbound_ids
                    .remove(&(conversation.channel, event_id.clone()));
                let count = self
                    .inbound_counts
                    .get_mut(&conversation)
                    .expect("admitted conversation");
                *count -= 1;
                if *count == 0 {
                    self.inbound_counts.remove(&conversation);
                }
                if !failed {
                    return;
                }
                if let Err(error) = engine.fence_inbound(conversation.channel, &event_id).await {
                    tracing::error!(%error, %event_id, "failed to fence an interrupted inbound request");
                } else {
                    tracing::warn!(%event_id, "interrupted inbound request has an uncertain result; automatic replay is disabled");
                }
            }
        }
    }
}

fn is_empty_rollout_metadata_error(error: &EngineError) -> bool {
    matches!(
        error,
        EngineError::Agent(AgentError::Rejected(message))
            if message.contains("failed to read session metadata")
                && message.contains("rollout at ")
                && message.ends_with(" is empty")
    )
}

#[cfg(test)]
mod tests {
    use agentix_core::{AgentError, EngineError};
    #[test]
    fn empty_rollout_metadata_errors_are_low_priority() {
        let empty = EngineError::Agent(AgentError::Rejected(
            "-32603: failed to read thread: thread-store internal error: failed to read session metadata /tmp/rollout.jsonl: rollout at /tmp/rollout.jsonl is empty".into(),
        ));
        let other = EngineError::Agent(AgentError::Rejected(
            "-32603: database connection failed".into(),
        ));

        assert!(super::is_empty_rollout_metadata_error(&empty));
        assert!(!super::is_empty_rollout_metadata_error(&other));
    }
}
