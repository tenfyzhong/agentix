//! One writer per logical card, shared by every Engine delivery path.
use crate::{
    ChannelAdapter, ChannelError, ChannelKind, CommandMenu, ConversationRef, InboundEnvelope,
    MessageRef, OutboundView,
};
use async_trait::async_trait;
use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use tokio_util::sync::CancellationToken;

use agentix_domain::DeliveryAttempt;

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct CardWrites {
    registry: Mutex<Registry>,
}
#[derive(Default)]
struct Registry {
    cards: HashMap<MessageRef, Arc<Card>>,
    sweep: VecDeque<MessageRef>,
}
impl Registry {
    fn prune(&mut self) {
        if self.sweep.len() < 256 {
            return;
        }
        // Bound work under the shared lock, even after a sustained provider outage.
        for _ in 0..8 {
            let Some(message) = self.sweep.pop_front() else {
                break;
            };
            if let Some(card) = self.cards.get(&message) {
                if Arc::strong_count(card) == 1 && !card.retain.load(Ordering::SeqCst) {
                    self.cards.remove(&message);
                } else {
                    self.sweep.push_back(message);
                }
            }
        }
    }
}
struct Card {
    latest: AtomicU64,
    disabled_through: AtomicU64,
    retain: AtomicBool,
    state: AsyncMutex<CardState>,
}
struct CardState {
    target: Option<MessageRef>,
    replacement_uncertain: bool,
    replaced: bool,
    applied_revision: u64,
}
#[derive(Clone)]
pub(super) struct Revision {
    card: Arc<Card>,
    number: u64,
}
impl Revision {
    pub(super) fn current(&self) -> bool {
        self.card.latest.load(Ordering::SeqCst) == self.number
    }
}
impl CardWrites {
    fn card(&self, message: &MessageRef) -> Arc<Card> {
        let mut registry = self.registry.lock().unwrap();
        // Hold the requested card across pruning; hits do not allocate a cloned key.
        let existing = registry.cards.get(message).cloned();
        registry.prune();
        if let Some(card) = existing {
            return card;
        }
        let card = Arc::new(Card {
            latest: AtomicU64::new(0),
            disabled_through: AtomicU64::new(0),
            retain: AtomicBool::new(false),
            state: AsyncMutex::new(CardState {
                target: Some(message.clone()),
                replacement_uncertain: false,
                replaced: false,
                applied_revision: 0,
            }),
        });
        registry.cards.insert(message.clone(), card.clone());
        registry.sweep.push_back(message.clone());
        card
    }

    pub(super) fn reserve(&self, message: &MessageRef) -> Revision {
        let card = self.card(message);
        let number = card.latest.fetch_add(1, Ordering::SeqCst) + 1;
        Revision { card, number }
    }

    pub(super) async fn update(
        &self,
        channel: &dyn ChannelAdapter,
        revision: &Revision,
        conversation: &ConversationRef,
        view: &OutboundView,
    ) -> Result<bool, ChannelError> {
        self.update_if(channel, revision, conversation, view, || async { true })
            .await
    }

    pub(super) async fn update_if<F, Fut>(
        &self,
        channel: &dyn ChannelAdapter,
        revision: &Revision,
        conversation: &ConversationRef,
        view: &OutboundView,
        valid: F,
    ) -> Result<bool, ChannelError>
    where
        F: Fn() -> Fut + Send + Sync,
        Fut: std::future::Future<Output = bool> + Send,
    {
        if !revision.current() {
            return Ok(false);
        }
        let mut state = revision.card.state.lock().await;
        if !revision.current() || !valid().await {
            return Ok(false);
        }
        if state.replacement_uncertain {
            return Err(ChannelError::Transport(
                "replacement card delivery is uncertain".into(),
            ));
        }
        let mut view = Cow::Borrowed(view);
        if revision.number <= revision.card.disabled_through.load(Ordering::SeqCst)
            && view.actions.iter().any(|action| !action.disabled)
        {
            for action in &mut view.to_mut().actions {
                action.disabled = true;
            }
        }
        if let Some(target) = state.target.clone() {
            let mut attempt = WriteAttempt::new(&mut state, &revision.card.retain, false);
            let result = attempt
                .delivery
                .run(channel.update(conversation, &target, &view))
                .await;
            attempt.finish(&result);
            drop(attempt);
            match result {
                Ok(()) => {
                    state.applied_revision = revision.number;
                    return Ok(true);
                }
                Err(error) if definite(&error) => return Err(error),
                Err(error) => tracing::warn!(%error, "card update uncertain; retiring message"),
            }
        }
        // Never replay an older state if its version or binding changed during I/O.
        if !revision.current() || !valid().await {
            return Ok(false);
        }
        let mut attempt = WriteAttempt::new(&mut state, &revision.card.retain, true);
        let result = attempt
            .delivery
            .run(channel.send(conversation, &view))
            .await;
        attempt.finish(&result);
        drop(attempt);
        let replacement = result?;
        // Register the physical replacement as the same logical card (including callbacks).
        self.registry
            .lock()
            .unwrap()
            .cards
            .insert(replacement.clone(), revision.card.clone());
        state.target = Some(replacement);
        state.replaced = true;
        state.applied_revision = revision.number;
        state.replacement_uncertain = false;
        Ok(true)
    }

    async fn disable(
        &self,
        channel: &dyn ChannelAdapter,
        message: &MessageRef,
    ) -> Result<(), ChannelError> {
        let card = self.card(message);
        let barrier = card.latest.load(Ordering::SeqCst);
        card.disabled_through.fetch_max(barrier, Ordering::SeqCst);
        let mut state = card.state.lock().await;
        if state.applied_revision > barrier {
            return Ok(());
        }
        let Some(target) = state.target.clone() else {
            return Ok(());
        };
        let mut attempt = WriteAttempt::new(&mut state, &card.retain, false);
        let result = attempt.delivery.run(channel.disable_actions(&target)).await;
        attempt.finish(&result);
        result
    }
}

fn definite(error: &ChannelError) -> bool {
    matches!(
        error,
        ChannelError::NotSent(_) | ChannelError::Rejected(_) | ChannelError::InvalidPayload(_)
    )
}

// Cancellation before dispatch must not retire a healthy target. Once dispatched,
// losing the response leaves the tombstone in place to isolate late remote writes.
struct WriteAttempt<'a> {
    state: &'a mut CardState,
    retain: &'a AtomicBool,
    previous: Option<MessageRef>,
    replacement: bool,
    resolved: bool,
    delivery: DeliveryAttempt,
}
impl<'a> WriteAttempt<'a> {
    fn new(state: &'a mut CardState, retain: &'a AtomicBool, replacement: bool) -> Self {
        let previous = state.target.take();
        if replacement {
            state.replacement_uncertain = true;
        }
        retain.store(true, Ordering::SeqCst);
        Self {
            state,
            retain,
            previous,
            replacement,
            resolved: false,
            delivery: DeliveryAttempt::default(),
        }
    }
    fn restore(&mut self) {
        self.state.target = self.previous.take();
        if self.replacement {
            self.state.replacement_uncertain = false;
        }
        self.retain.store(
            self.state.replaced || self.state.target.is_none(),
            Ordering::SeqCst,
        );
    }
    fn finish<T>(&mut self, result: &Result<T, ChannelError>) {
        if result.is_ok() || result.as_ref().is_err_and(definite) {
            self.restore();
        }
        self.resolved = true;
    }
}
impl Drop for WriteAttempt<'_> {
    fn drop(&mut self) {
        if !self.resolved && self.delivery.not_dispatched() {
            self.restore();
        }
    }
}

pub(super) struct OrderedChannel {
    inner: Arc<dyn ChannelAdapter>,
    writes: Arc<CardWrites>,
}
impl OrderedChannel {
    pub(super) fn wrap(
        inner: Arc<dyn ChannelAdapter>,
        writes: Arc<CardWrites>,
    ) -> Arc<dyn ChannelAdapter> {
        Arc::new(Self { inner, writes })
    }
}
#[async_trait]
impl ChannelAdapter for OrderedChannel {
    fn kind(&self) -> ChannelKind {
        self.inner.kind()
    }
    fn streaming_update_interval(&self) -> Duration {
        self.inner.streaming_update_interval()
    }
    async fn prepare_connection(&self) -> Result<(), ChannelError> {
        self.inner.prepare_connection().await
    }
    async fn replace_owners(&self, owners: &[String]) {
        self.inner.replace_owners(owners).await;
    }
    async fn identity(&self) -> Result<Option<String>, ChannelError> {
        self.inner.identity().await
    }
    async fn run(
        &self,
        inbound: mpsc::Sender<InboundEnvelope>,
        shutdown: CancellationToken,
    ) -> Result<(), ChannelError> {
        self.inner.run(inbound, shutdown).await
    }
    async fn send(
        &self,
        conversation: &ConversationRef,
        view: &OutboundView,
    ) -> Result<MessageRef, ChannelError> {
        self.inner.send(conversation, view).await
    }
    async fn update(
        &self,
        conversation: &ConversationRef,
        message: &MessageRef,
        view: &OutboundView,
    ) -> Result<(), ChannelError> {
        let revision = self.writes.reserve(message);
        self.writes
            .update(self.inner.as_ref(), &revision, conversation, view)
            .await
            .map(|_| ())
    }
    async fn disable_actions(&self, message: &MessageRef) -> Result<(), ChannelError> {
        self.writes.disable(self.inner.as_ref(), message).await
    }
    async fn read_inbox_message(
        &self,
        message: &MessageRef,
    ) -> Result<Option<InboundEnvelope>, ChannelError> {
        self.inner.read_inbox_message(message).await
    }
    fn supports_command_menu_sync(&self) -> bool {
        self.inner.supports_command_menu_sync()
    }
    async fn sync_command_menu(
        &self,
        conversation: &ConversationRef,
        menu: &CommandMenu,
    ) -> Result<(), ChannelError> {
        self.inner.sync_command_menu(conversation, menu).await
    }
    async fn set_command_menu(
        &self,
        conversation: &ConversationRef,
        menu: &CommandMenu,
    ) -> Result<(), ChannelError> {
        self.inner.set_command_menu(conversation, menu).await
    }
}
