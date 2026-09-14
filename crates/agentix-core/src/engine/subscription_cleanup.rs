//! Own best-effort unsubscribe requests after their binding has been removed.
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, watch};
use tokio::task::JoinSet;

use crate::{AgentAdapter, ConversationRef, SessionId};

#[derive(Default)]
pub(super) struct SubscriptionCleanup {
    state: Arc<Mutex<State>>,
    changed: Arc<Notify>,
}

#[derive(Default)]
struct State {
    pending: HashMap<SessionId, watch::Receiver<bool>>,
    workers: JoinSet<()>,
    attachments: HashMap<ConversationRef, Reattachment>,
    ready: VecDeque<Reattachment>,
}

#[derive(Debug, Clone)]
pub struct Reattachment {
    pub(super) conversation: ConversationRef,
    pub(super) session: SessionId,
    owner: String,
    epoch: u64,
    pub(super) id: uuid::Uuid,
}

impl SubscriptionCleanup {
    pub(super) fn cancel_attachment(&self, conversation: &ConversationRef) -> Option<Reattachment> {
        let mut state = self.state.lock().unwrap();
        state
            .ready
            .retain(|request| &request.conversation != conversation);
        state.attachments.remove(conversation)
    }

    pub(super) fn attachment(&self, conversation: &ConversationRef) -> Option<Reattachment> {
        self.state
            .lock()
            .unwrap()
            .attachments
            .get(conversation)
            .cloned()
    }

    fn defer_attachment(&self, request: Reattachment) -> Result<(), super::EngineError> {
        let mut state = self.state.lock().unwrap();
        if state.attachments.len() >= 128 && !state.attachments.contains_key(&request.conversation)
        {
            return Err(super::EngineError::InvalidInput(
                "Too many sessions are reconnecting; try attaching again shortly.".into(),
            ));
        }
        if !state.pending.contains_key(&request.session) {
            state.ready.push_back(request.clone());
            self.changed.notify_one();
        }
        state
            .attachments
            .insert(request.conversation.clone(), request);
        Ok(())
    }

    pub(super) async fn next_attachment(&self) -> Reattachment {
        loop {
            let notified = self.changed.notified();
            if let Some(request) = self.state.lock().unwrap().ready.pop_front() {
                return request;
            }
            notified.await;
        }
    }

    fn take_attachment(&self, request: &Reattachment) -> bool {
        let mut state = self.state.lock().unwrap();
        if state
            .attachments
            .get(&request.conversation)
            .is_some_and(|current| current.id == request.id)
        {
            state.attachments.remove(&request.conversation);
            true
        } else {
            false
        }
    }

    pub(super) fn pending(&self, session: &SessionId) -> Option<watch::Receiver<bool>> {
        self.state.lock().unwrap().pending.get(session).cloned()
    }

    pub(super) fn abort(&self) {
        let mut state = self.state.lock().unwrap();
        state.workers.abort_all();
        state.pending.clear();
        state.attachments.clear();
        state.ready.clear();
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
                let changed = self.changed.clone();
                state.workers.spawn(async move {
                    if let Err(error) = agent.unsubscribe(&session).await {
                        tracing::warn!(%error, %session, "failed to unsubscribe detached session");
                    }
                    if let Some(owner) = owner.upgrade() {
                        let mut state = owner.lock().unwrap();
                        state.pending.remove(&session);
                        let ready = state
                            .attachments
                            .values()
                            .filter(|request| request.session == session)
                            .cloned()
                            .collect::<Vec<_>>();
                        state.ready.extend(ready);
                        changed.notify_one();
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
    pub(super) fn cancel_reattachment(&self, conversation: &ConversationRef) -> bool {
        if let Some(request) = self.sessions.cleanup.cancel_attachment(conversation) {
            self.turns
                .pending_prompts
                .finish_reattachment(request.id, None);
            true
        } else {
            false
        }
    }

    pub(super) async fn defer_subscription_cleanup(
        &self,
        conversation: &crate::ConversationRef,
        session_id: &SessionId,
        owner_id: &str,
    ) -> Result<bool, super::EngineError> {
        if let Some(pending) = self.sessions.cleanup.pending(session_id) {
            self.send_view(
                conversation,
                &crate::OutboundView::text(
                    "Reattaching session",
                    "The previous connection is closing. Reattachment will continue automatically.",
                ),
            )
            .await?;
            if super::pending_prompts::Delivery::current().is_some() {
                self.sessions.cleanup.defer_attachment(Reattachment {
                    conversation: conversation.clone(),
                    session: session_id.clone(),
                    owner: owner_id.to_owned(),
                    epoch: self.sessions.epoch(conversation).await,
                    id: uuid::Uuid::new_v4(),
                })?;
                return Ok(true);
            }
            wait(pending).await;
        }
        Ok(false)
    }

    pub(super) async fn apply_reattachment(
        &self,
        request: Reattachment,
    ) -> Result<(), super::EngineError> {
        if !self.sessions.cleanup.take_attachment(&request) {
            return Ok(());
        }
        if self.sessions.epoch(&request.conversation).await != request.epoch {
            self.turns
                .pending_prompts
                .finish_reattachment(request.id, None);
            return Ok(());
        }
        // Another detach may have started cleanup after this completion was claimed.
        if self.sessions.cleanup.pending(&request.session).is_some() {
            return self.sessions.cleanup.defer_attachment(request);
        }
        let result = self
            .attach(
                &request.conversation,
                &request.owner,
                request.session.clone(),
            )
            .await;
        let epoch = if result.is_ok()
            && self.sessions.current(&request.conversation).await.as_ref() == Some(&request.session)
        {
            Some(self.sessions.epoch(&request.conversation).await)
        } else {
            None
        };
        self.turns
            .pending_prompts
            .finish_reattachment(request.id, epoch);
        result
    }
}
