//! Keep an unacknowledged input owned while releasing the runtime dispatch lane.
use super::{
    AgentError, ConversationRef, DeliveryClass, Engine, EngineError, EngineWork, InboundEnvelope,
    MessageRef, OutboundView, SessionId, TurnStatus, ViewStatus, markdown_quote,
};
use std::{
    collections::{HashMap, VecDeque},
    future::{Future, poll_fn},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::Poll,
};
use tokio::{
    sync::{Notify, watch},
    task::{Id, JoinSet},
};

pub(super) type SendFuture = Pin<Box<dyn Future<Output = Result<String, AgentError>> + Send>>;
const MAX_PENDING: usize = 32;
const MAX_QUEUED: usize = 16;

tokio::task_local! {
    static DELIVERY: Arc<Delivery>;
}

pub(super) struct Delivery {
    event_id: String,
    from_queue: bool,
    deferred: AtomicBool,
}

impl Delivery {
    fn new(event_id: String, from_queue: bool) -> Arc<Self> {
        Arc::new(Self {
            event_id,
            from_queue,
            deferred: AtomicBool::new(false),
        })
    }
    pub(super) fn current() -> Option<Arc<Self>> {
        DELIVERY.try_with(Arc::clone).ok()
    }
    pub(super) fn is_deferred() -> bool {
        Self::current().is_some_and(|delivery| delivery.deferred.load(Ordering::Acquire))
    }
}

#[derive(Debug, Clone)]
pub struct QueuedInput {
    pub(super) session: SessionId,
    pub(super) conversation: ConversationRef,
    prompt: String,
    event_id: String,
    epoch: u64,
    generation: u64,
    queued: bool,
    cancelled: bool,
    sequence: u64,
}

#[derive(Debug)]
pub struct PromptAcknowledged {
    pub(super) session: SessionId,
    pub(super) conversation: ConversationRef,
    id: u64,
    result: Result<String, AgentError>,
}

#[derive(Debug)]
pub struct CardDelivered {
    pub(super) session: SessionId,
    pub(super) conversation: ConversationRef,
    id: u64,
    result: Result<MessageRef, EngineError>,
}

struct PendingCard {
    input: QueuedInput,
    turn: Option<String>,
    final_view: Option<OutboundView>,
    valid: bool,
}

struct Pending {
    id: u64,
    input: QueuedInput,
    message: Option<MessageRef>,
    observed_turn: Option<String>,
    stop: bool,
    stop_applied: bool,
    valid: bool,
    queued: VecDeque<QueuedInput>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    closed: bool,
    pending: HashMap<SessionId, Pending>,
    tasks: JoinSet<PromptAcknowledged>,
    cards: HashMap<u64, PendingCard>,
    card_tasks: JoinSet<CardDelivered>,
    card_keys: HashMap<Id, u64>,
    task_keys: HashMap<Id, (SessionId, ConversationRef, u64)>,
    ready: VecDeque<QueuedInput>,
    owned: HashMap<(super::ChannelKind, String), QueuedInput>,
    receipts: HashMap<(super::ChannelKind, String), watch::Sender<OutboundView>>,
    receipt_tasks: JoinSet<()>,
}

#[derive(Default)]
pub(super) struct PendingPrompts {
    state: Mutex<State>,
    changed: Notify,
}

impl PendingPrompts {
    pub(super) fn cancel_queued_conversation(&self, conversation: &ConversationRef) {
        self.cancel_queued_matching(|input| &input.conversation == conversation);
    }

    fn cancel_queued_matching(&self, matches: impl Fn(&QueuedInput) -> bool) {
        let mut state = self.state.lock().unwrap();
        let inputs = state
            .owned
            .values_mut()
            .filter(|input| input.queued && matches(input))
            .map(|input| {
                input.cancelled = true;
                input.clone()
            })
            .collect::<Vec<_>>();
        for input in inputs {
            if let Some(sender) = state
                .receipts
                .get(&(input.conversation.channel, input.event_id))
            {
                sender.send_replace(OutboundView::text(
                    "Agentix · Input not sent",
                    markdown_quote(&input.prompt),
                ));
            }
        }
    }

    fn feedback(&self, input: &QueuedInput, title: &str) {
        let state = self.state.lock().unwrap();
        if let Some(sender) = state
            .receipts
            .get(&(input.conversation.channel, input.event_id.clone()))
        {
            sender.send_replace(OutboundView::text(title, markdown_quote(&input.prompt)));
        }
    }

    fn forget(&self, input: &QueuedInput, title: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let key = (input.conversation.channel, input.event_id.clone());
        state.owned.remove(&key);
        if let Some(sender) = state.receipts.remove(&key) {
            sender.send_replace(OutboundView::text(title, markdown_quote(&input.prompt)));
            true
        } else {
            false
        }
    }

    pub(super) fn contains(&self, session: &SessionId) -> bool {
        self.state.lock().unwrap().pending.contains_key(session)
    }

    fn start(&self, input: QueuedInput, future: SendFuture) -> Result<(), SendFuture> {
        let mut state = self.state.lock().unwrap();
        if state.closed
            || state.pending.len() >= MAX_PENDING
            || state.cards.len() >= MAX_PENDING
            || state.pending.contains_key(&input.session)
        {
            return Err(future);
        }
        let id = state.next_id;
        state.next_id += 1;
        let session = input.session.clone();
        let conversation = input.conversation.clone();
        let task_key = (session.clone(), conversation.clone(), id);
        let handle = state.tasks.spawn(async move {
            PromptAcknowledged {
                session,
                conversation,
                id,
                result: future.await,
            }
        });
        state.task_keys.insert(handle.id(), task_key);
        state.owned.insert(
            (input.conversation.channel, input.event_id.clone()),
            input.clone(),
        );
        state.pending.insert(
            input.session.clone(),
            Pending {
                id,
                input,
                message: None,
                observed_turn: None,
                stop: false,
                stop_applied: false,
                valid: true,
                queued: VecDeque::new(),
            },
        );
        self.changed.notify_one();
        Ok(())
    }

    pub(super) fn invalidate(&self, session: &SessionId) {
        self.cancel_queued_matching(|input| &input.session == session);
        let mut state = self.state.lock().unwrap();
        if let Some(pending) = state.pending.get_mut(session) {
            pending.valid = false;
        }
        for card in state
            .cards
            .values_mut()
            .filter(|card| &card.input.session == session)
        {
            card.valid = false;
        }
    }

    pub(super) fn observed_turn(&self, session: &SessionId) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .pending
            .get(session)
            .filter(|pending| pending.valid)
            .and_then(|pending| pending.observed_turn.clone())
    }

    pub(super) fn request_stop(&self, session: &SessionId) -> bool {
        self.cancel_queued_matching(|input| &input.session == session);
        let mut state = self.state.lock().unwrap();
        let Some(pending) = state.pending.get_mut(session) else {
            return false;
        };
        pending.stop = true;
        true
    }

    pub(super) fn take_stop(&self, session: &SessionId) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(pending) = state.pending.get_mut(session) else {
            return false;
        };
        if !pending.valid || !pending.stop || pending.stop_applied {
            return false;
        }
        pending.stop_applied = true;
        true
    }

    pub(super) fn abort(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.tasks.abort_all();
        state.card_tasks.abort_all();
        state.receipt_tasks.abort_all();
        state.receipts.clear();
    }

    async fn next(&self) -> EngineWork {
        loop {
            tokio::select! {
                result = poll_fn(|cx| {
                    let mut state = self.state.lock().unwrap();
                    if let Some(input) = state.ready.pop_front() {
                        return Poll::Ready(EngineWork::QueuedInput(input));
                    }
                    match state.card_tasks.poll_join_next_with_id(cx) {
                        Poll::Ready(Some(Ok((task, delivered)))) => {
                            state.card_keys.remove(&task);
                            return Poll::Ready(EngineWork::CardDelivered(delivered));
                        }
                        Poll::Ready(Some(Err(error))) => {
                            let id = state.card_keys.remove(&error.id()).expect("card task metadata");
                            let card = &state.cards[&id];
                            return Poll::Ready(EngineWork::CardDelivered(CardDelivered {
                                session: card.input.session.clone(),
                                conversation: card.input.conversation.clone(), id,
                                result: Err(EngineError::Agent(AgentError::Uncertain(format!("card task stopped: {error}")))),
                            }));
                        }
                        Poll::Ready(None) | Poll::Pending => {}
                    }
                    match state.tasks.poll_join_next_with_id(cx) {
                        Poll::Ready(Some(Ok((task, acknowledged)))) => {
                            state.task_keys.remove(&task);
                            Poll::Ready(EngineWork::PromptAcknowledged(acknowledged))
                        }
                        Poll::Ready(Some(Err(error))) => {
                            let (session, conversation, id) = state.task_keys.remove(&error.id())
                                .expect("pending send task metadata");
                            Poll::Ready(EngineWork::PromptAcknowledged(PromptAcknowledged {
                                session, conversation, id,
                                result: Err(AgentError::Uncertain(format!("input confirmation task stopped: {error}"))),
                            }))
                        }
                        Poll::Ready(None) | Poll::Pending => Poll::Pending,
                    }
                }) => return result,
                () = self.changed.notified() => {}
            }
        }
    }
}

impl Engine {
    pub(super) async fn handle_runtime_inbound(
        &self,
        envelope: InboundEnvelope,
    ) -> Result<(), EngineError> {
        DELIVERY
            .scope(
                Delivery::new(envelope.event_id.clone(), false),
                self.handle_inbound(envelope),
            )
            .await
    }

    pub async fn next_pending_prompt(&self) -> EngineWork {
        self.turns.pending_prompts.next().await
    }

    pub fn abort_pending_prompts(&self) {
        self.menus.abort();
        self.turns.pending_prompts.abort();
    }

    pub async fn cancel_pending_prompts(&self) -> Result<(), EngineError> {
        self.abort_pending_prompts();
        let inputs = self
            .turns
            .pending_prompts
            .state
            .lock()
            .unwrap()
            .owned
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for input in inputs {
            self.state
                .fence_event(input.conversation.channel, &input.event_id)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn defer_prompt(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
        prompt: &str,
        future: SendFuture,
        view: &OutboundView,
        generation: u64,
    ) -> Result<Option<SendFuture>, EngineError> {
        let Some(delivery) = Delivery::current() else {
            return Ok(Some(future));
        };
        let input = QueuedInput {
            session: session.clone(),
            conversation: conversation.clone(),
            prompt: prompt.to_owned(),
            event_id: delivery.event_id.clone(),
            epoch: self.sessions.epoch(conversation).await,
            generation,
            queued: false,
            cancelled: false,
            sequence: 0,
        };
        if let Err(future) = self.turns.pending_prompts.start(input, future) {
            return Ok(Some(future));
        }
        delivery.deferred.store(true, Ordering::Release);
        // Poll the original card request briefly, then retain it without holding
        // the session lane. Never cancel and resend an uncertain channel mutation.
        let channel = self.channel(conversation.channel)?.clone();
        let target = conversation.clone();
        let view = view.clone();
        let mut send = Box::pin(async move {
            channel
                .send(&target, &view)
                .await
                .map_err(EngineError::from)
        });
        if let Ok(result) =
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut send).await
        {
            match result {
                Ok(message) => {
                    if let Some(pending) = self
                        .turns
                        .pending_prompts
                        .state
                        .lock()
                        .unwrap()
                        .pending
                        .get_mut(session)
                    {
                        pending.message = Some(message);
                    }
                }
                Err(error) => tracing::warn!(%error, %session, "failed to show pending input"),
            }
        } else {
            let mut state = self.turns.pending_prompts.state.lock().unwrap();
            let pending = &state.pending[session];
            let id = pending.id;
            let input = pending.input.clone();
            state.cards.insert(
                id,
                PendingCard {
                    input,
                    turn: None,
                    final_view: None,
                    valid: true,
                },
            );
            let session = session.clone();
            let conversation = conversation.clone();
            let handle = state.card_tasks.spawn(async move {
                CardDelivered {
                    session,
                    conversation,
                    id,
                    result: send.await,
                }
            });
            state.card_keys.insert(handle.id(), id);
            self.turns.pending_prompts.changed.notify_one();
        }
        Ok(None)
    }

    pub(super) async fn queue_pending_prompt(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
        prompt: &str,
    ) -> Result<bool, EngineError> {
        let Some(delivery) = Delivery::current() else {
            return Ok(false);
        };
        let epoch = self.sessions.epoch(conversation).await;
        let channel = self.channel(conversation.channel)?.clone();
        {
            let mut state = self.turns.pending_prompts.state.lock().unwrap();
            let queued_count = state
                .owned
                .values()
                .filter(|input| input.queued && &input.session == session)
                .count();
            if !state.pending.contains_key(session) && (delivery.from_queue || queued_count == 0) {
                return Ok(false);
            }
            while state.receipt_tasks.try_join_next().is_some() {}
            let key = (conversation.channel, delivery.event_id.clone());
            if !state.receipts.contains_key(&key)
                && state.receipt_tasks.len() >= MAX_PENDING * MAX_QUEUED
            {
                return Err(EngineError::InvalidInput("Too many input notifications are pending. Try again after channel delivery recovers.".into()));
            }
            if queued_count >= MAX_QUEUED && !state.owned.contains_key(&key) {
                return Err(EngineError::InvalidInput("Too many inputs are waiting for delivery. Try again after the current input is acknowledged.".into()));
            }
            let sequence = if let Some(input) = state.owned.get(&key) {
                input.sequence
            } else {
                let sequence = state.next_id;
                state.next_id += 1;
                sequence
            };
            let input = QueuedInput {
                session: session.clone(),
                conversation: conversation.clone(),
                prompt: prompt.to_owned(),
                event_id: delivery.event_id.clone(),
                epoch,
                generation: self.agent.generation(),
                queued: true,
                cancelled: false,
                sequence,
            };
            if !state.receipts.contains_key(&key) {
                let initial = OutboundView::text("Agentix · Queued", markdown_quote(prompt));
                let (sender, receiver) = watch::channel(initial);
                state.receipts.insert(key.clone(), sender);
                state.receipt_tasks.spawn(run_queued_receipt(
                    channel,
                    conversation.clone(),
                    receiver,
                ));
            }

            state.owned.insert(key, input.clone());
            if let Some(pending) = state.pending.get_mut(session) {
                let position = pending
                    .queued
                    .partition_point(|queued| queued.sequence < input.sequence);
                pending.queued.insert(position, input);
            } else {
                state.ready.push_back(input);
                self.turns.pending_prompts.changed.notify_one();
            }
            delivery.deferred.store(true, Ordering::Release);
        }
        Ok(true)
    }

    pub(super) async fn adopt_pending_prompt(&self, session: &SessionId, turn: &str) {
        let input = self
            .turns
            .pending_prompts
            .state
            .lock()
            .unwrap()
            .pending
            .get(session)
            .filter(|pending| pending.observed_turn.is_none() || pending.message.is_some())
            .map(|pending| pending.input.clone());
        let Some(input) = input else {
            return;
        };
        if input.epoch != self.sessions.epoch(&input.conversation).await
            || input.generation != self.agent.generation()
            || self.sessions.current(&input.conversation).await.as_ref() != Some(session)
        {
            return;
        }
        let adopted = {
            let mut state = self.turns.pending_prompts.state.lock().unwrap();
            state
                .pending
                .get_mut(session)
                .filter(|pending| {
                    pending.valid
                        && pending
                            .observed_turn
                            .as_deref()
                            .is_none_or(|observed| observed == turn)
                })
                .map(|pending| {
                    pending.observed_turn = Some(turn.to_owned());
                    (pending.input.prompt.clone(), pending.message.take())
                })
        };
        if let Some((prompt, message)) = adopted {
            let key = (session.clone(), turn.to_owned());
            let mut buffers = self.turns.buffers.lock().await;
            let buffer = buffers.entry(key.clone()).or_default();
            if buffer.user_text.trim().is_empty() {
                buffer.user_text = prompt;
            }
            drop(buffers);
            if let Some(message) = message {
                self.turns.views.lock().await.entry(key).or_insert(message);
            }
        }
    }

    pub(super) async fn apply_prompt_acknowledged(
        &self,
        acknowledged: PromptAcknowledged,
    ) -> Result<(), EngineError> {
        let mut pending = {
            let mut state = self.turns.pending_prompts.state.lock().unwrap();
            if state
                .pending
                .get(&acknowledged.session)
                .is_none_or(|pending| pending.id != acknowledged.id)
            {
                return Ok(());
            }
            state.pending.remove(&acknowledged.session).unwrap()
        };
        let input = pending.input.clone();
        let current = pending.valid
            && input.generation == self.agent.generation()
            && input.epoch == self.sessions.epoch(&input.conversation).await
            && self.sessions.current(&input.conversation).await.as_ref() == Some(&input.session);
        let accepted = acknowledged.result.is_ok();
        // Transfer accepted follow-ups before any fallible card update. An IM
        // error must not discard input that is already owned by the runtime.
        if accepted && current && !pending.stop {
            self.turns
                .pending_prompts
                .state
                .lock()
                .unwrap()
                .ready
                .extend(std::mem::take(&mut pending.queued));
            self.turns.pending_prompts.changed.notify_one();
        }

        self.record_pending_card_outcome(&acknowledged, &input);
        let mut result = match acknowledged.result {
            Ok(turn) => {
                // Acceptance remains durable even if the user already switched.
                self.state
                    .complete_event(input.conversation.channel, &input.event_id)
                    .await?;
                self.turns.pending_prompts.forget(&input, "Agentix · Sent");
                self.show_acknowledged_prompt(&pending, &turn, current)
                    .await
            }
            Err(error) => {
                let uncertain = matches!(error, AgentError::Uncertain(_));
                if uncertain {
                    self.state
                        .fence_event(input.conversation.channel, &input.event_id)
                        .await?;
                } else {
                    self.state
                        .release_event(input.conversation.channel, &input.event_id)
                        .await?;
                }
                self.turns.pending_prompts.forget(
                    &input,
                    if uncertain {
                        "Agentix · Delivery unconfirmed"
                    } else {
                        "Agentix · Send failed"
                    },
                );
                if let Some(message) = pending.message {
                    let mut view = OutboundView::text(
                        if uncertain {
                            "Agentix · Delivery unconfirmed"
                        } else {
                            "Agentix · Send failed"
                        },
                        format!("{}\n\n{error}", markdown_quote(&input.prompt)),
                    );
                    view.status = if uncertain {
                        ViewStatus::Warning
                    } else {
                        ViewStatus::Error
                    };
                    if let Err(error) = self
                        .channel(input.conversation.channel)?
                        .update(&input.conversation, &message, &view)
                        .await
                    {
                        tracing::warn!(%error, "failed to finalize pending input feedback");
                    }
                }
                Ok(())
            }
        };
        for queued in pending.queued {
            if let Err(error) = self.cancel_queued_input(&queued).await {
                tracing::warn!(%error, "failed to finalize cancelled input");
                if result.is_ok() {
                    result = Err(error);
                }
            }
        }
        result
    }

    fn record_pending_card_outcome(&self, acknowledged: &PromptAcknowledged, input: &QueuedInput) {
        if let Some(card) = self
            .turns
            .pending_prompts
            .state
            .lock()
            .unwrap()
            .cards
            .get_mut(&acknowledged.id)
        {
            if !card.valid {
                return;
            }
            match &acknowledged.result {
                Ok(turn) => card.turn = Some(turn.clone()),
                Err(error) => {
                    let mut view = OutboundView::text(
                        "Agentix · Send failed",
                        format!("{}\n\n{error}", markdown_quote(&input.prompt)),
                    );
                    view.status = if matches!(error, AgentError::Uncertain(_)) {
                        ViewStatus::Warning
                    } else {
                        ViewStatus::Error
                    };
                    if matches!(error, AgentError::Uncertain(_)) {
                        view.title = "Agentix · Delivery unconfirmed".into();
                    }
                    card.final_view = Some(view);
                }
            }
        }
    }

    async fn show_acknowledged_prompt(
        &self,
        pending: &Pending,
        turn: &str,
        current: bool,
    ) -> Result<(), EngineError> {
        let input = &pending.input;
        if pending.stop
            && !pending.stop_applied
            && pending.valid
            && input.generation == self.agent.generation()
        {
            self.restore_cold_turn(&input.session, turn).await?;
            let terminal = self
                .turns
                .buffers
                .lock()
                .await
                .get(&(input.session.clone(), turn.to_owned()))
                .is_some_and(|buffer| {
                    !matches!(buffer.status, TurnStatus::InProgress | TurnStatus::Unknown)
                });
            if !terminal {
                self.operations.stop(&input.session, turn).await?;
            }
        }
        if current {
            self.restore_cold_turn(&input.session, turn).await?;
            let key = (input.session.clone(), turn.to_owned());
            let running = {
                let mut buffers = self.turns.buffers.lock().await;
                let buffer = buffers.entry(key.clone()).or_default();
                if buffer.user_text.trim().is_empty() {
                    buffer.user_text.clone_from(&input.prompt);
                }
                let running = matches!(buffer.status, TurnStatus::InProgress | TurnStatus::Unknown);
                if running {
                    buffer.ensure_started();
                }
                running
            };
            if let Some(message) = pending.message.clone() {
                self.turns.views.lock().await.entry(key).or_insert(message);
            }
            if running
                && self
                    .turns
                    .active_turn(&input.session)
                    .await
                    .as_deref()
                    .is_none_or(|active| active == turn)
            {
                self.turns
                    .set_active(input.session.clone(), turn.to_owned())
                    .await;
            }
            self.render_turn(
                &input.conversation,
                &input.session,
                turn,
                DeliveryClass::Live,
                true,
            )
            .await?;
        } else if let Some(message) = pending.message.clone() {
            let view = OutboundView::text(
                "Agentix · Sent",
                format!(
                    "{}\n\nDelivered to the previous session.",
                    markdown_quote(&input.prompt)
                ),
            );
            if let Err(error) = self
                .channel(input.conversation.channel)?
                .update(&input.conversation, &message, &view)
                .await
            {
                tracing::warn!(%error, "failed to finalize detached input feedback");
            }
        }
        Ok(())
    }

    async fn cancel_queued_input(&self, input: &QueuedInput) -> Result<(), EngineError> {
        self.state
            .complete_event(input.conversation.channel, &input.event_id)
            .await?;
        if self
            .turns
            .pending_prompts
            .forget(input, "Agentix · Input not sent")
        {
            return Ok(());
        }
        self.send_view(
            &input.conversation,
            &OutboundView::text(
                "Agentix · Input not sent",
                format!(
                    "{}\n\nThe session changed, stopped, or did not confirm the preceding input.",
                    markdown_quote(&input.prompt)
                ),
            ),
        )
        .await?;
        Ok(())
    }

    pub(super) async fn apply_queued_input(&self, input: QueuedInput) -> Result<(), EngineError> {
        let cancelled = {
            let state = self.turns.pending_prompts.state.lock().unwrap();
            state.closed
                || state
                    .owned
                    .get(&(input.conversation.channel, input.event_id.clone()))
                    .is_none_or(|owned| owned.cancelled)
        };
        if cancelled
            || input.generation != self.agent.generation()
            || input.epoch != self.sessions.epoch(&input.conversation).await
            || self.sessions.current(&input.conversation).await.as_ref() != Some(&input.session)
        {
            return self.cancel_queued_input(&input).await;
        }
        self.turns
            .pending_prompts
            .feedback(&input, "Agentix · Sending…");
        let delivery = Delivery::new(input.event_id.clone(), true);
        let result = DELIVERY
            .scope(
                delivery.clone(),
                self.send_prompt(&input.conversation, &input.prompt),
            )
            .await;
        match result {
            Ok(()) if delivery.deferred.load(Ordering::Acquire) => Ok(()),
            Ok(()) => {
                self.state
                    .complete_event(input.conversation.channel, &input.event_id)
                    .await?;
                self.turns.pending_prompts.forget(&input, "Agentix · Sent");
                Ok(())
            }
            Err(error) => {
                if matches!(error, EngineError::Agent(AgentError::Uncertain(_))) {
                    self.state
                        .fence_event(input.conversation.channel, &input.event_id)
                        .await?;
                } else {
                    self.state
                        .release_event(input.conversation.channel, &input.event_id)
                        .await?;
                }
                self.turns.pending_prompts.forget(
                    &input,
                    if matches!(error, EngineError::Agent(AgentError::Uncertain(_))) {
                        "Agentix · Delivery unconfirmed"
                    } else {
                        "Agentix · Send failed"
                    },
                );
                Err(error)
            }
        }
    }
}

impl Engine {
    pub(super) async fn freeze_exited_cards(&self, session: &SessionId) {
        let cards = self
            .turns
            .pending_prompts
            .state
            .lock()
            .unwrap()
            .cards
            .iter()
            .filter(|(_, card)| &card.input.session == session)
            .filter_map(|(id, card)| card.turn.clone().map(|turn| (*id, turn)))
            .collect::<Vec<_>>();
        if cards.is_empty() {
            return;
        }
        let label = self.session_label(session).await;
        for (id, turn) in cards {
            let mut buffer = self
                .turns
                .buffers
                .lock()
                .await
                .get(&(session.clone(), turn.clone()))
                .cloned();
            if buffer.is_none() {
                if let Err(error) = self.restore_cold_turn(session, &turn).await {
                    tracing::warn!(%error, "failed to restore pending exit card");
                }
                buffer = self
                    .turns
                    .buffers
                    .lock()
                    .await
                    .get(&(session.clone(), turn.clone()))
                    .cloned();
            }
            if let Some(mut buffer) = buffer {
                if matches!(buffer.status, TurnStatus::InProgress | TurnStatus::Unknown) {
                    buffer.status = TurnStatus::Interrupted;
                }
                let view = super::live_turn_view(
                    self.agent.display_name(),
                    &label,
                    &turn,
                    &buffer,
                    DeliveryClass::Live,
                );
                if let Some(card) = self
                    .turns
                    .pending_prompts
                    .state
                    .lock()
                    .unwrap()
                    .cards
                    .get_mut(&id)
                {
                    card.final_view = Some(view);
                }
            }
        }
    }

    pub(super) async fn hold_pending_card(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
        turn: &str,
    ) -> bool {
        if !self
            .turns
            .pending_prompts
            .state
            .lock()
            .unwrap()
            .cards
            .values()
            .any(|card| card.input.session == *session)
        {
            return false;
        }
        let detached = self.sessions.current(conversation).await.is_none();
        let epoch = self.sessions.epoch(conversation).await;
        let generation = self.agent.generation();
        let mut state = self.turns.pending_prompts.state.lock().unwrap();
        let Some(card) = state.cards.values_mut().find(|card| {
            card.input.session == *session
                && card.input.conversation == *conversation
                && ((!card.valid && detached) || card.input.epoch == epoch)
                && card.input.generation == generation
                && card.turn.as_deref().is_none_or(|known| known == turn)
        }) else {
            return false;
        };
        card.turn = Some(turn.to_owned());
        true
    }

    pub(super) async fn apply_card_delivered(
        &self,
        delivered: CardDelivered,
    ) -> Result<(), EngineError> {
        let card = {
            let mut state = self.turns.pending_prompts.state.lock().unwrap();
            let card = state.cards.remove(&delivered.id);
            if state.closed {
                return Ok(());
            }
            card
        };
        let Some(card) = card else {
            return Ok(());
        };
        let input = card.input;
        let current = card.valid
            && input.generation == self.agent.generation()
            && input.epoch == self.sessions.epoch(&input.conversation).await
            && self.sessions.current(&input.conversation).await.as_ref() == Some(&input.session);
        let message = match delivered.result {
            Ok(message) => message,
            Err(error) => {
                tracing::warn!(%error, "pending card delivery failed");
                // A failed progress notice must not swallow output that already
                // completed while its message ID was unavailable.
                if let Some(view) = card.final_view {
                    self.send_view(&input.conversation, &view).await?;
                } else if current && let Some(turn) = card.turn {
                    self.restore_cold_turn(&input.session, &turn).await?;
                    self.render_turn(
                        &input.conversation,
                        &input.session,
                        &turn,
                        DeliveryClass::Live,
                        true,
                    )
                    .await?;
                }
                return Ok(());
            }
        };
        if let Some(view) = card.final_view {
            self.channel(input.conversation.channel)?
                .update(&input.conversation, &message, &view)
                .await?;
            return Ok(());
        }
        if current {
            if let Some(turn) = card.turn {
                self.restore_cold_turn(&input.session, &turn).await?;
                self.turns
                    .views
                    .lock()
                    .await
                    .insert((input.session.clone(), turn.clone()), message);
                self.render_turn(
                    &input.conversation,
                    &input.session,
                    &turn,
                    DeliveryClass::Live,
                    true,
                )
                .await?;
            } else if let Some(pending) = self
                .turns
                .pending_prompts
                .state
                .lock()
                .unwrap()
                .pending
                .get_mut(&input.session)
                .filter(|pending| pending.id == delivered.id)
            {
                pending.message = Some(message);
            }
        } else {
            let view = OutboundView::text(
                if card.valid {
                    "Agentix · Previous session"
                } else {
                    "Agentix · Session exited"
                },
                format!(
                    "{}\n\nThis input belongs to the previous session.",
                    markdown_quote(&input.prompt)
                ),
            );
            self.channel(input.conversation.channel)?
                .update(&input.conversation, &message, &view)
                .await?;
        }
        Ok(())
    }
}

// Own each channel operation until completion. Only edits to a known message are
// retried; an ambiguous initial send is never repeated automatically.
async fn run_queued_receipt(
    channel: Arc<dyn super::ChannelAdapter>,
    target: ConversationRef,
    mut receiver: watch::Receiver<OutboundView>,
) {
    let mut message = None;
    loop {
        for attempt in 0..3 {
            let view = receiver.borrow_and_update().clone();
            if let Some(message) = &message {
                match channel.update(&target, message, &view).await {
                    Ok(()) => break,
                    Err(error) => {
                        tracing::warn!(%error, attempt, "queued feedback update failed");
                        if attempt < 2 {
                            tokio::time::sleep(std::time::Duration::from_millis(100 << attempt))
                                .await;
                        }
                    }
                }
            } else {
                match channel.send(&target, &view).await {
                    Ok(sent) => message = Some(sent),
                    Err(error) => tracing::warn!(%error, "queued feedback delivery failed"),
                }
                break;
            }
        }
        if receiver.changed().await.is_err() {
            break;
        }
    }
}
