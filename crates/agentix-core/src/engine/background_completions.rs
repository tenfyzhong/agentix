//! Optional completion reads and IM I/O never reserve an Engine dispatch lane.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use super::card_writes::{CardWrites, Revision};
use futures_util::{StreamExt, stream};
use tokio::task::JoinHandle;

use super::{
    Engine, InteractionCoordinator, SessionService, TurnBuffer, TurnCoordinator,
    background_completion_body, live_turn_view, session_display_label, short_identifier,
    turn_status_label,
};
use crate::{
    ActionButton, AgentAdapter, AgentError, ChannelAdapter, ConversationRef, DeliveryClass,
    MessageRef, OutboundView, OutputConfig, SessionId, TurnStatus, TurnSummary, ViewStatus,
};

type Key = (SessionId, String);
const CONCURRENCY: usize = 4;
const RECIPIENT_CONCURRENCY: usize = 4;
const CAPACITY: usize = 64;
const RECENT_CAPACITY: usize = 256;
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const DELIVERY_TIMEOUT: Duration = Duration::from_mins(1);
const LOADING_DELAY: Duration = Duration::from_millis(100);

/// Bounded optional-work queue diagnostics, independent of the Engine event queue.
#[derive(Debug, Clone, Copy)]
pub struct BackgroundCompletionStatistics {
    pub active: usize,
    pub queued: usize,
    pub rejected: u64,
}

#[derive(Default)]
struct State {
    closed: bool,
    waiting: VecDeque<Request>,
    running: HashMap<Key, JoinHandle<()>>,
    recent: VecDeque<Key>,
    rejected: u64,
}

#[derive(Default)]
pub(super) struct BackgroundCompletions {
    state: Mutex<State>,
}

impl BackgroundCompletions {
    fn submit(self: &Arc<Self>, mut request: Request) {
        let mut state = self.state.lock().unwrap();
        let key = request.key();
        if state.closed
            || state.running.contains_key(&key)
            || state.waiting.iter().any(|request| request.key() == key)
            || state.recent.contains(&key)
        {
            return;
        }
        if state.running.len() + state.waiting.len() >= CAPACITY {
            state.rejected += 1;
            tracing::warn!(target: "agentix::telemetry", rejected = state.rejected,
                "background completion capacity exhausted; optional notice skipped");
            return;
        }
        for recipient in &mut request.recipients {
            recipient.revision = recipient
                .message
                .as_ref()
                .map(|message| request.writes.reserve(message));
        }
        state.waiting.push_back(request);
        self.start(&mut state);
    }

    fn start(self: &Arc<Self>, state: &mut State) {
        while state.running.len() < CONCURRENCY {
            let Some(request) = state.waiting.pop_front() else {
                break;
            };
            let key = request.key();
            let completion = CompletionGuard {
                key: key.clone(),
                owner: Arc::downgrade(self),
            };
            let task = tokio::spawn(async move {
                let _completion = completion;
                request.run().await;
            });
            state.running.insert(key, task);
        }
    }

    pub(super) fn cancel(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.waiting.clear();
        for (_, task) in state.running.drain() {
            task.abort();
        }
    }
}

// Release capacity even if an adapter panics or a task is cancelled.
struct CompletionGuard {
    key: Key,
    owner: Weak<BackgroundCompletions>,
}

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            let mut state = owner.state.lock().unwrap();
            state.running.remove(&self.key);
            if !state.closed {
                state.recent.push_back(self.key.clone());
                if state.recent.len() > RECENT_CAPACITY {
                    state.recent.pop_front();
                }
                owner.start(&mut state);
            }
        }
    }
}

impl Drop for BackgroundCompletions {
    fn drop(&mut self) {
        for (_, task) in self.state.get_mut().unwrap().running.drain() {
            task.abort();
        }
    }
}

struct Recipient {
    conversation: ConversationRef,
    owner: String,
    channel: Arc<dyn ChannelAdapter>,
    action_group: String,
    attach_token: tokio::sync::Mutex<Option<(u64, String)>>,
    message: Option<MessageRef>,
    revision: Option<Revision>,
    // Only a pre-existing draining card can also be owned by the live renderer.
    epoch: Option<u64>,
}

struct Request {
    ready: Option<tokio::sync::oneshot::Receiver<()>>,
    session: SessionId,
    turn: String,
    status: TurnStatus,
    error: Option<String>,
    cached: Option<TurnBuffer>,
    recipients: Vec<Recipient>,
    agent: Arc<dyn AgentAdapter>,
    generation: u64,
    sessions: Arc<SessionService>,
    interactions: Arc<InteractionCoordinator>,
    turns: Weak<TurnCoordinator>,
    output: OutputConfig,
    draining: bool,
    writes: Arc<CardWrites>,
}

impl Request {
    fn key(&self) -> Key {
        (self.session.clone(), self.turn.clone())
    }

    async fn valid(&self, recipient: &Recipient) -> bool {
        self.agent.generation() == self.generation
            && self
                .interactions
                .owners
                .lock()
                .await
                .get(&recipient.conversation)
                == Some(&recipient.owner)
            && match recipient.epoch {
                Some(epoch) => self.sessions.epoch(&recipient.conversation).await == epoch,
                None => true,
            }
    }

    async fn run(mut self) {
        // Reserve ordering before local cleanup, but never start optional I/O
        // until that cleanup succeeds. Dropping the permit cancels this notice.
        if let Some(ready) = self.ready.take()
            && ready.await.is_err()
        {
            return;
        }
        if !self.draining
            && !matches!(
                tokio::time::timeout(READ_TIMEOUT, self.agent.is_subagent(&self.session)).await,
                Ok(Ok(false))
            )
        {
            return;
        }
        if self.agent.generation() != self.generation {
            return;
        }
        self.sessions
            .cache_session_summary(self.agent.clone(), &self.session)
            .await;
        self.sessions.await_background_title(&self.session).await;
        let label = self
            .sessions
            .cache
            .lock()
            .await
            .get(&self.session)
            .map_or_else(
                || short_identifier(self.session.native_str()).to_owned(),
                session_display_label,
            );
        let complete_cache = self.cached.as_ref().is_some_and(|buffer| {
            buffer.answer_complete
                && !buffer.user_text.trim().is_empty()
                && !buffer.agent_text.trim().is_empty()
        });
        if complete_cache {
            self.publish(&self.view(&label, None, false)).await;
            return;
        }
        let read = read_with_retry(self.agent.as_ref(), &self.session, &self.turn);
        tokio::pin!(read);
        let history = tokio::select! {
            result = &mut read => result,
            () = tokio::time::sleep(LOADING_DELAY) => {
                let loading = self.view(&label, None, true);
                let (ready, result) = tokio::sync::watch::channel(None);
                // Both branches remain owned by this request. Delivery waits must
                // never suspend polling the history deadline or its retry.
                let finish_read = async {
                    let history = read.await;
                    ready.send_replace(Some(Arc::new(self.view(&label, history.as_ref(), false))));
                };
                tokio::join!(finish_read, self.publish_loading(&loading, result));
                return;
            }
        };
        self.publish(&self.view(&label, history.as_ref(), false))
            .await;
    }

    fn view(&self, label: &str, history: Option<&TurnSummary>, loading: bool) -> OutboundView {
        let mut buffer = self.cached.clone().unwrap_or_default();
        if let Some(history) = history {
            buffer.merge_summary(history, self.output);
        }
        buffer.set_status(self.status.clone());
        let mut view = live_turn_view(
            self.agent.session_display_name(&self.session),
            label,
            &self.turn,
            &buffer,
            if self.draining {
                DeliveryClass::Draining
            } else {
                DeliveryClass::Live
            },
        );
        if !self.draining {
            view.body = format!(
                "{}\n\n{}",
                background_completion_body(&self.status, None),
                view.body
            );
        }
        let notice = if loading {
            Some("Loading turn content…")
        } else if history.is_none() {
            if buffer.user_text.trim().is_empty() && buffer.agent_text.trim().is_empty() {
                Some("Turn content is unavailable.")
            } else if !buffer.answer_complete
                || buffer.user_text.trim().is_empty()
                || buffer.agent_text.trim().is_empty()
            {
                Some("Additional turn content is unavailable.")
            } else {
                None
            }
        } else {
            None
        };
        if let Some(notice) = notice {
            view.body.push_str(&format!("\n\n{notice}"));
            view.sections.push(agentix_domain::ViewSection {
                body: notice.into(),
                ..agentix_domain::ViewSection::default()
            });
        }
        if let Some(error) = self
            .error
            .as_deref()
            .filter(|error| !error.trim().is_empty())
        {
            for section in view
                .sections
                .iter_mut()
                .filter(|section| section.collapsible)
            {
                section.expanded = Some(false);
            }
            view.body.push_str(&format!(
                "\n\n**Error**\n\n{}",
                super::markdown_quote(error)
            ));
            view.sections.push(agentix_domain::ViewSection {
                title: "Error".into(),
                body: error.into(),
                ..agentix_domain::ViewSection::default()
            });
        }
        view.subtitle = Some(format!(
            "Background turn {} · {}",
            short_identifier(&self.turn),
            turn_status_label(&self.status)
        ));
        view.status = ViewStatus::Background;
        view
    }

    async fn attachment_button(&self, recipient: &Recipient) -> (u64, ActionButton) {
        let bindings = self.sessions.bindings.lock().await;
        let epoch = bindings.epoch(&recipient.conversation);
        let attached = bindings.current_session(&recipient.conversation) == Some(&self.session);
        drop(bindings);
        let mut cached_token = recipient.attach_token.lock().await;
        let mut actions = self.interactions.actions.lock().await;
        // Loading and final views share one single-use action. Do not revoke
        // the visible token before an edit which may fail or be cancelled.
        let token = if attached {
            String::new()
        } else if let Some((previous_epoch, token)) = cached_token.as_ref()
            && *previous_epoch == epoch
        {
            token.clone()
        } else {
            let token = actions.issue(
                crate::ActionScope::new(
                    recipient.conversation.clone(),
                    &recipient.owner,
                    self.generation,
                    epoch,
                    &recipient.action_group,
                ),
                super::UiAction::Attach(self.session.clone()),
            );
            *cached_token = Some((epoch, token.clone()));
            token
        };
        let consumed = !attached && !actions.iter().any(|(registered, _)| registered == token);
        drop(actions);
        drop(cached_token);
        let button = ActionButton {
            disabled: attached || consumed,
            label: if attached { "Attached" } else { "Attach" }.into(),
            token,
            style: crate::ActionStyle::Primary,
        };
        (epoch, button)
    }

    async fn publish(&self, view: &OutboundView) {
        stream::iter(&self.recipients)
            .for_each_concurrent(RECIPIENT_CONCURRENCY, |recipient| async move {
                self.publish_one(recipient, view, None, false).await;
            })
            .await;
    }

    async fn publish_loading(
        &self,
        loading: &OutboundView,
        result: tokio::sync::watch::Receiver<Option<Arc<OutboundView>>>,
    ) {
        stream::iter(&self.recipients)
            .for_each_concurrent(RECIPIENT_CONCURRENCY, |recipient| {
                let mut result = result.clone();
                async move {
                    // Recipients admitted after the query finishes need no loading card.
                    let ready = result.borrow().clone();
                    if let Some(view) = ready {
                        self.publish_one(recipient, &view, None, false).await;
                        return;
                    }
                    // Preserve loading/final order independently for each recipient.
                    // Never cancel an in-flight loading send to race a fresh final send.
                    let card = self.publish_one(recipient, loading, None, false).await;
                    let view = result
                        .wait_for(Option::is_some)
                        .await
                        .expect("request owns the result sender")
                        .clone()
                        .expect("final view is ready");
                    self.publish_one(recipient, &view, card.as_ref(), true)
                        .await;
                }
            })
            .await;
    }

    async fn publish_one(
        &self,
        recipient: &Recipient,
        view: &OutboundView,
        delivered_card: Option<&(MessageRef, Revision)>,
        after_loading: bool,
    ) -> Option<(MessageRef, Revision)> {
        if !self.valid(recipient).await
            || recipient
                .revision
                .as_ref()
                .is_some_and(|revision| !revision.current())
            || delivered_card.is_some_and(|(_, revision)| !revision.current())
        {
            return None;
        }
        let message = delivered_card
            .map(|(message, _)| message)
            .or(recipient.message.as_ref());
        let revision = delivered_card
            .map(|(_, revision)| revision)
            .or(recipient.revision.as_ref());
        // A failed/uncertain initial send must never trigger a duplicate send.
        if after_loading && message.is_none() {
            return None;
        }
        let mut view = view.clone();
        let (epoch, button) = self.attachment_button(recipient).await;
        view.actions.push(button);
        let send = async {
            if let (Some(message), Some(revision)) = (message, revision) {
                if self
                    .writes
                    .update_if(
                        recipient.channel.as_ref(),
                        revision,
                        &recipient.conversation,
                        &view,
                        || async {
                            self.valid(recipient).await
                                && self.sessions.epoch(&recipient.conversation).await == epoch
                        },
                    )
                    .await?
                {
                    Ok::<_, crate::ChannelError>(Some((message.clone(), revision.clone())))
                } else {
                    Ok(None)
                }
            } else {
                let message = agentix_domain::DeliveryAttempt::default()
                    .run(recipient.channel.send(&recipient.conversation, &view))
                    .await?;
                let revision = self.writes.reserve(&message);
                Ok(Some((message, revision)))
            }
        };
        // Updates have an internal write deadline and may need one replacement send.
        match tokio::time::timeout(DELIVERY_TIMEOUT * 3, send).await {
            Ok(Ok(Some(card))) => {
                if let Some(turns) = self.turns.upgrade() {
                    turns
                        .record_background_notification(
                            &recipient.conversation,
                            &self.session,
                            &self.turn,
                        )
                        .await;
                }
                Some(card)
            }
            Ok(Ok(None)) => None,
            Ok(Err(error)) => {
                tracing::warn!(%error, "background completion delivery failed");
                None
            }
            Err(_) => {
                tracing::warn!("background completion delivery timed out");
                None
            }
        }
    }
}

async fn read_with_retry(
    agent: &dyn AgentAdapter,
    session: &SessionId,
    turn: &str,
) -> Option<TurnSummary> {
    for attempt in 0..2 {
        match tokio::time::timeout(READ_TIMEOUT, read_history(agent, session, turn)).await {
            Ok(Ok(turn)) => return turn,
            Ok(Err(error)) => tracing::warn!(%error, attempt, "background content read failed"),
            Err(_) => tracing::warn!(attempt, "background content read timed out"),
        }
        if attempt == 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    None
}

async fn read_history(
    agent: &dyn AgentAdapter,
    session: &SessionId,
    turn: &str,
) -> Result<Option<TurnSummary>, AgentError> {
    let mut cursor = None;
    let mut visited = HashSet::new();
    loop {
        let page = agent.read_history(session, cursor, 20).await?;
        if let Some(found) = page.turns.into_iter().find(|found| found.id == turn) {
            return Ok(Some(found));
        }
        match page.older_cursor {
            Some(next) if visited.insert(next.clone()) => cursor = Some(next),
            _ => return Ok(None),
        }
    }
}

impl Engine {
    #[cfg(test)]
    pub(super) async fn queue_background_completion(
        &self,
        session: &SessionId,
        turn: &str,
        status: &TurnStatus,
        error: Option<&str>,
        draining: Option<&ConversationRef>,
    ) -> Result<(), super::EngineError> {
        if let Some(ready) = self
            .prepare_background_completion(session, turn, status, error, draining)
            .await
        {
            let _ = ready.send(());
        }
        Ok(())
    }

    pub(super) async fn prepare_background_completion(
        &self,
        session: &SessionId,
        turn: &str,
        status: &TurnStatus,
        error: Option<&str>,
        draining: Option<&ConversationRef>,
    ) -> Option<tokio::sync::oneshot::Sender<()>> {
        if draining.is_none() && !self.background_turn_notifications {
            return None;
        }
        let key = (session.clone(), turn.to_owned());
        // Completion handlers have already restored and finalized this turn.
        let cached = self.turns.buffers.lock().await.get(&key).cloned();
        let existing = self.turns.views.lock().await.get(&key).cloned();
        let owners = self.interactions.owners.lock().await.clone();
        let mut recipients = Vec::new();
        for (conversation, owner) in owners {
            if draining.is_some_and(|target| target != &conversation)
                || self
                    .turns
                    .background_notification_delivered(&conversation, session, turn)
                    .await
            {
                continue;
            }
            let Some(channel) = self.transports.get(&conversation.channel) else {
                tracing::debug!(channel = ?conversation.channel, "disabled completion channel skipped");
                continue;
            };
            recipients.push(Recipient {
                channel: channel.clone(),
                attach_token: tokio::sync::Mutex::new(None),
                revision: None,
                action_group: uuid::Uuid::new_v4().simple().to_string(),
                message: if draining.is_some() {
                    existing.clone()
                } else {
                    None
                },
                epoch: if draining.is_some() {
                    Some(self.sessions.epoch(&conversation).await)
                } else {
                    None
                },
                conversation,
                owner,
            });
        }
        if recipients.is_empty() {
            return None;
        }
        let (ready, receiver) = tokio::sync::oneshot::channel();
        self.background.submit(Request {
            ready: Some(receiver),
            session: session.clone(),
            turn: turn.into(),
            status: status.clone(),
            error: error.map(str::to_owned),
            cached,
            recipients,
            agent: self.agent.clone(),
            generation: self.agent.generation(),
            sessions: self.sessions.clone(),
            interactions: self.interactions.clone(),
            turns: Arc::downgrade(&self.turns),
            output: self.output,
            draining: draining.is_some(),
            writes: self.card_writes.clone(),
        });
        Some(ready)
    }

    /// Snapshot optional-work pressure without querying sessions or storage.
    #[must_use]
    pub fn background_completion_statistics(&self) -> BackgroundCompletionStatistics {
        let state = self.background.state.lock().unwrap();
        BackgroundCompletionStatistics {
            active: state.running.len(),
            queued: state.waiting.len(),
            rejected: state.rejected,
        }
    }

    /// Cancel optional completion reads and deliveries when the runtime stops.
    pub fn cancel_background_completions(&self) {
        self.background.cancel();
    }
}
