use super::{DeliveryClass, Engine};
use crate::{
    AgentAdapter, AgentError, AgentEvent, ChannelAdapter, ChannelError, ChannelKind,
    ConversationRef, HistoryPage, InteractionDecision, MessageRef, OutboundView, SessionId,
    SessionPage, SqliteState,
};
use async_trait::async_trait;
use std::{
    future::{Future, poll_fn},
    sync::Arc,
    task::Poll,
    time::Duration,
};
use tokio::sync::broadcast;

// A throttled render must not call the agent or send a channel update.
struct UnusedAgent;

#[async_trait]
impl AgentAdapter for UnusedAgent {
    fn display_name(&self) -> &'static str {
        "Test"
    }
    async fn list_sessions(&self, _: Option<String>, _: u32) -> Result<SessionPage, AgentError> {
        unreachable!()
    }
    async fn read_history(
        &self,
        _: &SessionId,
        _: Option<String>,
        _: u32,
    ) -> Result<HistoryPage, AgentError> {
        unreachable!()
    }
    async fn attach(&self, _: &SessionId) -> Result<(), AgentError> {
        unreachable!()
    }
    async fn unsubscribe(&self, _: &SessionId) -> Result<(), AgentError> {
        unreachable!()
    }
    async fn start_turn(&self, _: &SessionId, _: &str) -> Result<String, AgentError> {
        unreachable!()
    }
    async fn steer(&self, _: &SessionId, _: &str, _: &str) -> Result<String, AgentError> {
        unreachable!()
    }
    async fn interrupt(&self, _: &SessionId, _: &str) -> Result<(), AgentError> {
        unreachable!()
    }
    async fn resolve_interaction(&self, _: InteractionDecision) -> Result<(), AgentError> {
        unreachable!()
    }
    fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        unreachable!()
    }
    fn generation(&self) -> u64 {
        0
    }
}

struct UnusedChannel;

#[async_trait]
impl ChannelAdapter for UnusedChannel {
    fn streaming_update_interval(&self) -> Duration {
        Duration::MAX
    }
    fn kind(&self) -> ChannelKind {
        ChannelKind::Telegram
    }
    async fn send(
        &self,
        _: &ConversationRef,
        _: &OutboundView,
    ) -> Result<MessageRef, ChannelError> {
        unreachable!()
    }
    async fn update(
        &self,
        _: &ConversationRef,
        _: &MessageRef,
        _: &OutboundView,
    ) -> Result<(), ChannelError> {
        unreachable!()
    }
}

#[tokio::test]
async fn throttled_render_does_not_wait_for_the_session_cache() {
    let engine = Engine::new(
        Arc::new(UnusedAgent),
        SqliteState::in_memory().await.unwrap(),
        vec![Arc::new(UnusedChannel)],
    );
    let session = SessionId::new("session");
    let key = (session.clone(), "turn".to_owned());
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat");
    assert!(engine.turns.should_render(&key, false, Duration::MAX).await);
    let cache = engine.sessions.cache.lock().await;
    let mut throttled =
        Box::pin(engine.render_turn(&conversation, &session, "turn", DeliveryClass::Live, false));
    assert!(
        matches!(
            poll_fn(|cx| Poll::Ready(throttled.as_mut().poll(cx))).await,
            Poll::Ready(Ok(()))
        ),
        "throttled updates must skip the session cache"
    );
    let mut forced =
        Box::pin(engine.render_turn(&conversation, &session, "turn", DeliveryClass::Live, true));
    assert!(
        poll_fn(|cx| Poll::Ready(forced.as_mut().poll(cx)))
            .await
            .is_pending(),
        "forced updates must still read the label"
    );
    drop(cache);
}
