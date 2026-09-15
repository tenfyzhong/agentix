//! Own best-effort unsubscribe requests after their binding has been removed.
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, watch};
use tokio::task::JoinSet;

use crate::{AgentAdapter, AgentError, ConversationRef, HistoryPage, SessionId, SessionOperations};

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
    preparing: HashMap<uuid::Uuid, (SessionId, watch::Receiver<bool>)>,
    prepared: HashMap<uuid::Uuid, Result<HistoryPage, AgentError>>,
    observed: HashMap<String, (uuid::Uuid, super::attachment_history::AttachmentHistory)>,
}

#[derive(Debug, Clone)]
pub struct Reattachment {
    pub(super) conversation: ConversationRef,
    pub(super) session: SessionId,
    owner: String,
    epoch: u64,
    generation: u64,
    prepared: bool,
    pub(super) id: uuid::Uuid,
}

impl SubscriptionCleanup {
    pub(super) fn cancel_attachment(&self, conversation: &ConversationRef) -> Option<Reattachment> {
        let mut state = self.state.lock().unwrap();
        state
            .ready
            .retain(|request| request.prepared || &request.conversation != conversation);
        let request = state.attachments.remove(conversation)?;
        if state
            .observed
            .get(request.session.as_str())
            .is_some_and(|(id, _)| *id == request.id)
        {
            state.observed.remove(request.session.as_str());
        }
        Some(request)
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

    fn attachment_conversations(&self, session: &SessionId) -> Vec<ConversationRef> {
        self.state
            .lock()
            .unwrap()
            .attachments
            .values()
            .filter(|request| &request.session == session)
            .map(|request| request.conversation.clone())
            .collect()
    }

    pub(super) fn observe(&self, event: &crate::AgentEvent) {
        let Some(session) = event.session_id() else {
            return;
        };
        if let Some((_, observed)) = self.state.lock().unwrap().observed.get_mut(session) {
            observed.observe(event);
        }
    }

    fn has_attachment(&self, session: &SessionId) -> bool {
        self.state
            .lock()
            .unwrap()
            .attachments
            .values()
            .any(|request| &request.session == session)
    }

    fn take_prepared(&self, request: &Reattachment) -> Option<Result<HistoryPage, AgentError>> {
        let mut state = self.state.lock().unwrap();
        let mut result = state.prepared.remove(&request.id)?;
        if state
            .observed
            .get(request.session.as_str())
            .is_some_and(|(id, _)| *id == request.id)
            && let Some((_, observed)) = state.observed.remove(request.session.as_str())
            && let Ok(history) = &mut result
        {
            observed.merge(history);
        }
        state.preparing.remove(&request.id);
        state.ready.retain(|ready| ready.id != request.id);
        Some(result)
    }

    fn prepare(
        &self,
        request: Reattachment,
        agent: Arc<dyn AgentAdapter>,
        operations: SessionOperations,
    ) -> Result<watch::Receiver<bool>, super::EngineError> {
        let mut state = self.state.lock().unwrap();
        while state.workers.try_join_next().is_some() {}
        if state.preparing.len() >= 128 {
            return Err(super::EngineError::InvalidInput(
                "Too many connections are pending; try again shortly.".into(),
            ));
        }
        let mut predecessors = state
            .preparing
            .values()
            .filter(|(session, _)| session == &request.session)
            .map(|(_, receiver)| receiver.clone())
            .collect::<Vec<_>>();
        if let Some(cleanup) = state.pending.get(&request.session) {
            predecessors.push(cleanup.clone());
        }
        let (finished, received) = watch::channel(false);
        state
            .preparing
            .insert(request.id, (request.session.clone(), received.clone()));
        state
            .attachments
            .insert(request.conversation.clone(), request.clone());
        state.observed.insert(
            request.session.to_string(),
            (
                request.id,
                super::attachment_history::AttachmentHistory::default(),
            ),
        );
        let owner = Arc::downgrade(&self.state);
        let changed = self.changed.clone();
        state.workers.spawn(async move {
            for predecessor in predecessors {
                wait(predecessor).await;
            }
            let result = match agent.attach(&request.session).await {
                Ok(()) => operations.history(&request.session, None, 1).await,
                Err(error) => Err(error),
            };
            if let Some(owner) = owner.upgrade() {
                let mut state = owner.lock().unwrap();
                state.prepared.insert(request.id, result);
                state.ready.push_back(request);
                changed.notify_one();
            }
            let _ = finished.send(true);
        });
        Ok(received)
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
        state.preparing.clear();
        state.prepared.clear();
        state.observed.clear();
    }

    pub(super) async fn enqueue(&self, agent: Arc<dyn AgentAdapter>, session: &SessionId) {
        let received = {
            let mut state = self.state.lock().unwrap();
            while state.workers.try_join_next().is_some() {}
            // A pending new owner still needs this subscription. Its completion
            // will either adopt it or clean it up after cancellation/failure.
            if state
                .attachments
                .values()
                .any(|request| &request.session == session)
            {
                return;
            }
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
                let preparations = state
                    .preparing
                    .values()
                    .filter(|(target, _)| target == &session)
                    .map(|(_, receiver)| receiver.clone())
                    .collect::<Vec<_>>();
                state.workers.spawn(async move {
                    for preparation in preparations {
                        wait(preparation).await;
                    }
                    if let Err(error) = agent.unsubscribe(&session).await {
                        tracing::warn!(%error, %session, "failed to unsubscribe detached session");
                    }
                    if let Some(owner) = owner.upgrade() {
                        let mut state = owner.lock().unwrap();
                        state.pending.remove(&session);
                        let ready = state
                            .attachments
                            .values()
                            .filter(|request| request.session == session && !request.prepared)
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
    pub(super) fn cancel_session_attachments(&self, session: &SessionId) {
        for conversation in self.sessions.cleanup.attachment_conversations(session) {
            self.cancel_reattachment(&conversation);
        }
    }

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
                    generation: self.agent.generation(),
                    prepared: false,
                    id: uuid::Uuid::new_v4(),
                })?;
                return Ok(true);
            }
            wait(pending).await;
        }
        Ok(false)
    }

    pub(super) async fn begin_attachment(
        &self,
        conversation: &ConversationRef,
        owner: &str,
        session: SessionId,
    ) -> Result<(), super::EngineError> {
        let request = Reattachment {
            conversation: conversation.clone(),
            session,
            owner: owner.to_owned(),
            epoch: self.sessions.epoch(conversation).await,
            generation: self.agent.generation(),
            prepared: true,
            id: uuid::Uuid::new_v4(),
        };
        self.prepare_attachment(request).await
    }

    async fn prepare_attachment(
        &self,
        mut request: Reattachment,
    ) -> Result<(), super::EngineError> {
        request.prepared = true;
        let ready = match self.sessions.cleanup.prepare(
            request.clone(),
            self.agent.clone(),
            self.operations.clone(),
        ) {
            Ok(ready) => ready,
            Err(error) => {
                self.turns
                    .pending_prompts
                    .finish_reattachment(request.id, None);
                return Err(error);
            }
        };
        self.sessions
            .cache_session_summary(self.agent.clone(), &request.session)
            .await;
        if tokio::time::timeout(Duration::from_millis(50), wait(ready))
            .await
            .is_ok()
        {
            return self.apply_prepared_attachment(request).await;
        }
        self.send_view(&request.conversation, &crate::OutboundView::text(
            "Connecting session",
            "Connecting and loading recent messages. You can send input now, or use /cancel to cancel this connection.",
        )).await?;
        Ok(())
    }

    async fn release_unused_attachment(&self, session: &SessionId) {
        if self.sessions.bound_conversation(session).await.is_none()
            && !self.sessions.cleanup.has_attachment(session)
        {
            self.sessions
                .cleanup
                .enqueue(self.agent.clone(), session)
                .await;
        }
    }

    async fn apply_prepared_attachment(
        &self,
        request: Reattachment,
    ) -> Result<(), super::EngineError> {
        let Some(result) = self.sessions.cleanup.take_prepared(&request) else {
            return Ok(());
        };
        let current = self.sessions.cleanup.take_attachment(&request);
        if !current
            || self.sessions.epoch(&request.conversation).await != request.epoch
            || self.agent.generation() != request.generation
        {
            self.turns
                .pending_prompts
                .finish_reattachment(request.id, None);
            self.release_unused_attachment(&request.session).await;
            return Ok(());
        }
        let result = match result {
            Ok(history) => {
                self.finish_attachment(&request.conversation, &request.session, &history)
                    .await
            }
            Err(error) => {
                self.release_unused_attachment(&request.session).await;
                self.show_attach_failure(
                    &request.conversation,
                    &request.owner,
                    &request.session,
                    &error,
                )
                .await
            }
        };
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

    pub(super) async fn apply_reattachment(
        &self,
        request: Reattachment,
    ) -> Result<(), super::EngineError> {
        if request.prepared {
            return self.apply_prepared_attachment(request).await;
        }
        if !self.sessions.cleanup.take_attachment(&request) {
            return Ok(());
        }
        if self.sessions.epoch(&request.conversation).await != request.epoch
            || self.agent.generation() != request.generation
        {
            self.turns
                .pending_prompts
                .finish_reattachment(request.id, None);
            return Ok(());
        }
        if self.sessions.cleanup.pending(&request.session).is_some() {
            return self.sessions.cleanup.defer_attachment(request);
        }
        self.prepare_attachment(request).await
    }
}
