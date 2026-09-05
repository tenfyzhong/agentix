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
    /// Serialize outbound requests so a 429 response freezes every send path,
    /// including adapter clones, before another request can reach Telegram.
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
            let mut state = self.state.lock().await;
            let now = Instant::now();
            let mut deadline = state
                .next_request
                .unwrap_or(now)
                .max(state.blocked_until.unwrap_or(now));
            if let Some(next) = chat.and_then(|chat| state.next_chat.get(&chat)) {
                deadline = deadline.max(*next);
            }
            tokio::time::sleep_until(deadline).await;
            let result = request.send_ref().await;
            let now = Instant::now();
            state.next_request = Some(now + Duration::from_millis(50));
            state.next_chat.retain(|_, next| *next > now);
            if let Some(chat) = chat {
                let interval = if chat.0 < 0 { 3_100 } else { 1_100 };
                state
                    .next_chat
                    .insert(chat, now + Duration::from_millis(interval));
            }
            match result {
                Err(teloxide::RequestError::RetryAfter(delay)) => {
                    // Keep the deadline in shared state even if this future is cancelled.
                    state.blocked_until = Some(now + delay.duration() + Duration::from_millis(100));
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
}
