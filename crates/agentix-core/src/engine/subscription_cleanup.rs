//! Own best-effort unsubscribe requests after their binding has been removed.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::{AgentAdapter, SessionId};

#[derive(Default)]
pub(super) struct SubscriptionCleanup {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    pending: HashMap<SessionId, watch::Receiver<bool>>,
    workers: JoinSet<()>,
}

impl SubscriptionCleanup {
    pub(super) fn pending(&self, session: &SessionId) -> Option<watch::Receiver<bool>> {
        self.state.lock().unwrap().pending.get(session).cloned()
    }

    pub(super) fn abort(&self) {
        let mut state = self.state.lock().unwrap();
        state.workers.abort_all();
        state.pending.clear();
    }

    pub(super) async fn enqueue(&self, agent: Arc<dyn AgentAdapter>, session: &SessionId) {
        let received = {
            let mut state = self.state.lock().unwrap();
            while state.workers.try_join_next().is_some() {}
            if let Some(received) = state.pending.get(session) {
                received.clone()
            } else {
                if state.pending.len() >= 128 {
                    tracing::warn!(%session, "subscription cleanup capacity reached; retaining remote subscription");
                    return;
                }
                let (finished, received) = watch::channel(false);
                state.pending.insert(session.clone(), received.clone());
                let owner = Arc::downgrade(&self.state);
                let session = session.clone();
                state.workers.spawn(async move {
                    if let Err(error) = agent.unsubscribe(&session).await {
                        tracing::warn!(%error, %session, "failed to unsubscribe detached session");
                    }
                    if let Some(owner) = owner.upgrade() {
                        owner.lock().unwrap().pending.remove(&session);
                    }
                    let _ = finished.send(true);
                });
                received
            }
        };
        let _ = tokio::time::timeout(Duration::from_millis(50), wait(received)).await;
    }
}

pub(super) async fn wait(mut received: watch::Receiver<bool>) {
    while !*received.borrow_and_update() {
        if received.changed().await.is_err() {
            break;
        }
    }
}

impl super::Engine {
    pub(super) async fn wait_subscription_cleanup(
        &self,
        conversation: &crate::ConversationRef,
        session_id: &SessionId,
    ) -> Result<(), super::EngineError> {
        if let Some(pending) = self.sessions.cleanup.pending(session_id) {
            self.send_view(
                conversation,
                &crate::OutboundView::text(
                    "Reattaching session",
                    "The previous connection is closing. Reattachment will continue automatically.",
                ),
            )
            .await?;
            wait(pending).await;
        }
        Ok(())
    }
}
