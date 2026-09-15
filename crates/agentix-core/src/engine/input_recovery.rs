//! Optional input history must not occupy the session's event dispatch lane.
use std::{
    collections::{HashMap, VecDeque},
    future::{Future, poll_fn},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
};

use tokio::{
    sync::Notify,
    task::{AbortHandle, JoinSet},
};

use super::{Engine, EngineError, EngineWork};
use crate::{AgentAdapter, AgentError, ConversationRef, DeliveryClass, SessionId};

type TurnKey = (SessionId, String);
type InputFuture = Pin<Box<dyn Future<Output = Result<Option<String>, AgentError>> + Send>>;
const READ_CONCURRENCY: usize = 8;

#[derive(Debug)]
pub struct RecoveredInput {
    pub(super) session: SessionId,
    pub(super) conversation: ConversationRef,
    id: u64,
    turn: String,
    text: Result<Option<String>, AgentError>,
}

struct Request {
    id: u64,
    conversation: ConversationRef,
    delivery: DeliveryClass,
    epoch: u64,
    generation: u64,
    abort: Option<AbortHandle>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    closed: bool,
    requests: HashMap<TurnKey, Request>,
    waiting: VecDeque<(TurnKey, InputFuture)>,
    reads: JoinSet<RecoveredInput>,
}

#[derive(Default)]
pub(super) struct InputRecovery {
    state: Mutex<State>,
    changed: Notify,
}

impl InputRecovery {
    // Poll immediately available input once to preserve the cheap in-memory path.
    // A pending read is owned here, never awaited by render or its session worker.
    #[allow(clippy::too_many_arguments)]
    fn request(
        &self,
        agent: Arc<dyn AgentAdapter>,
        key: TurnKey,
        conversation: ConversationRef,
        delivery: DeliveryClass,
        epoch: u64,
    ) -> Option<String> {
        let mut state = self.state.lock().unwrap();
        if state.closed || state.requests.contains_key(&key) {
            return None;
        }
        let generation = agent.generation();
        let (session, turn) = key.clone();
        let mut future: InputFuture =
            Box::pin(async move { agent.read_turn_input(&session, &turn).await });
        if state.reads.len() < READ_CONCURRENCY
            && state.waiting.is_empty()
            && let Poll::Ready(result) = future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
        {
            return result.ok().flatten();
        }
        let id = state.next_id;
        state.next_id += 1;
        state.requests.insert(
            key.clone(),
            Request {
                id,
                conversation,
                delivery,
                epoch,
                generation,
                abort: None,
            },
        );
        state.waiting.push_back((key, future));
        Self::start_reads(&mut state);
        self.changed.notify_one();
        None
    }

    fn start_reads(state: &mut State) {
        while state.reads.len() < READ_CONCURRENCY {
            let Some((key, future)) = state.waiting.pop_front() else {
                break;
            };
            let Some(request) = state.requests.get_mut(&key) else {
                continue;
            };
            let id = request.id;
            let conversation = request.conversation.clone();
            request.abort = Some(state.reads.spawn(async move {
                RecoveredInput {
                    session: key.0,
                    turn: key.1,
                    conversation,
                    id,
                    text: future.await,
                }
            }));
        }
    }

    pub(super) fn cancel_session(&self, session: &SessionId) {
        let mut state = self.state.lock().unwrap();
        state.requests.retain(|key, request| {
            if &key.0 != session {
                return true;
            }
            if let Some(abort) = &request.abort {
                abort.abort();
            }
            false
        });
        state.waiting.retain(|(key, _)| &key.0 != session);
        self.changed.notify_one();
    }

    fn cancel_all(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.reads.abort_all();
        state.waiting.clear();
        state.requests.clear();
        self.changed.notify_one();
    }

    async fn next(&self) -> RecoveredInput {
        loop {
            tokio::select! {
                result = poll_fn(|cx| {
                    let mut state = self.state.lock().unwrap();
                    loop {
                        Self::start_reads(&mut state);
                        match state.reads.poll_join_next(cx) {
                            Poll::Ready(Some(Ok(result))) => return Poll::Ready(result),
                            Poll::Ready(Some(Err(error))) => {
                                tracing::debug!(%error, "input recovery read cancelled or failed");
                            }
                            Poll::Pending | Poll::Ready(None) => return Poll::Pending,
                        }
                    }
                }) => return result,
                () = self.changed.notified() => {}
            }
        }
    }

    fn take(&self, input: &RecoveredInput) -> Option<(TurnKey, Request)> {
        let mut state = self.state.lock().unwrap();
        let key = (input.session.clone(), input.turn.clone());
        if state.requests.get(&key)?.id != input.id {
            return None;
        }
        state.requests.remove(&key).map(|request| (key, request))
    }
}

impl Engine {
    pub(super) async fn restore_turn_input(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
        turn: &str,
        delivery: DeliveryClass,
    ) {
        let key = (session.clone(), turn.to_owned());
        let missing = self
            .turns
            .buffers
            .lock()
            .await
            .get(&key)
            .is_some_and(|buffer| buffer.user_text.trim().is_empty());
        if !missing {
            return;
        }
        let epoch = self.sessions.epoch(conversation).await;
        let text = self.turns.input_recovery.request(
            self.agent.clone(),
            key.clone(),
            conversation.clone(),
            delivery,
            epoch,
        );
        if let Some(text) = text
            && let Some(buffer) = self.turns.buffers.lock().await.get_mut(&key)
            && buffer.user_text.trim().is_empty()
        {
            buffer.user_text = text;
        }
    }

    /// Await a completed optional read without reserving a session dispatch lane.
    /// Execute the returned work through the normal dispatcher before applying it.
    pub async fn next_input_recovery(&self) -> EngineWork {
        EngineWork::InputRecovered(self.turns.input_recovery.next().await)
    }

    /// Cancel optional reads when admission stops, including queued requests.
    pub fn cancel_input_recovery(&self) {
        self.turns.input_recovery.cancel_all();
    }

    pub(super) async fn apply_recovered_input(
        &self,
        input: RecoveredInput,
    ) -> Result<(), EngineError> {
        let Some(((session, turn), request)) = self.turns.input_recovery.take(&input) else {
            return Ok(());
        };
        if request.generation != self.agent.generation()
            || request.epoch != self.sessions.epoch(&request.conversation).await
        {
            return Ok(());
        }
        let Some(text) = input
            .text
            .ok()
            .flatten()
            .filter(|text| !text.trim().is_empty())
        else {
            return Ok(());
        };
        let was_cold = self.restore_cold_turn(&session, &turn).await?;
        let changed = {
            let mut buffers = self.turns.buffers.lock().await;
            if let Some(buffer) = buffers.get_mut(&(session.clone(), turn.clone()))
                && buffer.user_text.trim().is_empty()
            {
                buffer.user_text = text;
                true
            } else {
                false
            }
        };
        let result = if changed {
            self.render_turn(
                &request.conversation,
                &session,
                &turn,
                request.delivery,
                true,
            )
            .await
        } else {
            Ok(())
        };
        if was_cold {
            self.archive_turn(&session, &turn).await?;
        }
        result
    }
}
