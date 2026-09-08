use std::collections::HashMap;
use std::time::Duration;

use teloxide::requests::{Output, Request};
use teloxide::types::ChatId;
use tokio::sync::Mutex;
use tokio::time::Instant;

#[derive(Default)]
pub(crate) struct RateLimiter {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    next_request: Option<Instant>,
    blocked_until: Option<Instant>,
    next_chat: HashMap<ChatId, Instant>,
}

impl RateLimiter {
    /// Pace and retry the request already admitted by the message center.
    /// Shared deadlines survive cancellation of the current FIFO head.
    pub(crate) async fn send<R>(
        &self,
        request: R,
        method: &str,
        chat: Option<ChatId>,
    ) -> Result<Output<R>, teloxide::RequestError>
    where
        R: Request<Err = teloxide::RequestError>,
    {
        loop {
            self.reserve(chat).await;
            let result = request.send_ref().await;
            let now = Instant::now();
            match result {
                Err(teloxide::RequestError::RetryAfter(delay)) => {
                    // Keep the deadline in shared state even if this future is cancelled.
                    let mut state = self.state.lock().await;
                    let deadline = now + delay.duration() + Duration::from_millis(100);
                    state.blocked_until = Some(
                        state
                            .blocked_until
                            .map_or(deadline, |current| current.max(deadline)),
                    );
                    tracing::warn!(
                        api_method = method,
                        chat_id = chat.map(|chat| chat.0),
                        retry_after_seconds = delay.duration().as_secs(),
                        "Telegram rate limit reached; pausing all outbound requests before retrying"
                    );
                }
                result => return result,
            }
        }
    }

    /// Reserve pacing before sending; neither chat waits nor HTTP retain the lock.
    async fn reserve(&self, chat: Option<ChatId>) {
        loop {
            let deadline = {
                let mut state = self.state.lock().await;
                let now = Instant::now();
                let mut deadline = state
                    .next_request
                    .unwrap_or(now)
                    .max(state.blocked_until.unwrap_or(now));
                if let Some(next) = chat.and_then(|chat| state.next_chat.get(&chat)) {
                    deadline = deadline.max(*next);
                }
                if deadline <= now {
                    state.next_request = Some(now + Duration::from_millis(50));
                    state.next_chat.retain(|_, next| *next > now);
                    if let Some(chat) = chat {
                        let interval = if chat.0 < 0 { 3_100 } else { 1_100 };
                        state
                            .next_chat
                            .insert(chat, now + Duration::from_millis(interval));
                    }
                    return;
                }
                deadline
            };
            tokio::time::sleep_until(deadline).await;
            // A concurrent 429 may have extended the global cooldown while asleep.
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::{Future, IntoFuture, Ready, poll_fn, ready};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Poll;

    use agentix_domain::MessageCenter;
    use teloxide::payloads::LogOut;
    use teloxide::requests::HasPayload;
    use teloxide::types::{Seconds, True};

    use super::*;

    struct RetryOnce {
        payload: LogOut,
        attempts: Arc<AtomicUsize>,
    }

    impl HasPayload for RetryOnce {
        type Payload = LogOut;

        fn payload_ref(&self) -> &LogOut {
            &self.payload
        }

        fn payload_mut(&mut self) -> &mut LogOut {
            &mut self.payload
        }
    }

    impl IntoFuture for RetryOnce {
        type Output = Result<True, teloxide::RequestError>;
        type IntoFuture = Ready<Self::Output>;

        fn into_future(self) -> Self::IntoFuture {
            self.send()
        }
    }

    impl Request for RetryOnce {
        type Err = teloxide::RequestError;
        type Send = Ready<Result<True, Self::Err>>;
        type SendRef = Self::Send;

        fn send(self) -> Self::Send {
            self.send_ref()
        }

        fn send_ref(&self) -> Self::SendRef {
            ready(if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(teloxide::RequestError::RetryAfter(Seconds::from_seconds(1)))
            } else {
                Ok(True)
            })
        }
    }

    #[tokio::test]
    async fn cancelling_head_retains_cooldown_without_detached_retries() {
        let center = MessageCenter::default();
        let limiter = RateLimiter::default();
        let attempts = Arc::new(AtomicUsize::new(0));
        let request = || RetryOnce {
            payload: LogOut::new(),
            attempts: attempts.clone(),
        };
        let mut head = Box::pin(center.outbound(None, limiter.send(request(), "mock", None)));
        // The mock response is ready in the same poll that increments attempts,
        // so a pending poll after that increment has entered the cooldown wait.
        while attempts.load(Ordering::SeqCst) == 0 {
            assert_pending(&mut head).await;
            tokio::task::yield_now().await;
        }
        drop(head);
        let deadline = limiter.state.lock().await.blocked_until.unwrap();
        assert!(deadline > Instant::now());
        let other =
            agentix_domain::ConversationRef::new(agentix_domain::ChannelKind::Telegram, "42");
        let mut next = Box::pin(center.outbound(
            Some(&other),
            limiter.send(request(), "mock", Some(ChatId(42))),
        ));
        assert_pending(&mut next).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(next.await.unwrap(), True);
        assert!(Instant::now() >= deadline);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn chat_pacing_does_not_block_unrelated_chats() {
        let limiter = RateLimiter::default();
        let attempts = Arc::new(AtomicUsize::new(1));
        let request = || RetryOnce {
            payload: LogOut::new(),
            attempts: attempts.clone(),
        };
        limiter
            .send(request(), "mock", Some(ChatId(-1)))
            .await
            .unwrap();
        let mut paced = Box::pin(limiter.send(request(), "mock", Some(ChatId(-1))));
        assert_pending(&mut paced).await;
        tokio::time::timeout(
            Duration::from_secs(1),
            limiter.send(request(), "mock", Some(ChatId(-2))),
        )
        .await
        .expect("another group only waits for global pacing")
        .unwrap();
        assert_pending(&mut paced).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    async fn assert_pending(future: &mut (impl Future + Unpin)) {
        poll_fn(|cx| {
            assert!(Pin::new(&mut *future).poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
}
