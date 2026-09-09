use agentix_domain::ChannelError;
use serde_json::Value;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::Instant};
use url::Url;

#[derive(Clone)]
pub(crate) struct Api {
    pub client: reqwest::Client,
    pub base: Url,
    bot_token: Arc<str>,
    app_token: Arc<str>,
    limits: Arc<Mutex<Limits>>,
}
#[derive(Default)]
struct Limits {
    cooldowns: HashMap<String, Instant>,
    posts: HashMap<String, Instant>,
}

impl Api {
    pub fn new(client: reqwest::Client, base: Url, bot_token: String, app_token: String) -> Self {
        Self {
            client,
            base,
            bot_token: bot_token.into(),
            app_token: app_token.into(),
            limits: Arc::default(),
        }
    }
    async fn wait_turn(&self, method: &str, body: &Value) {
        loop {
            let wait = {
                let mut limits = self.limits.lock().await;
                let now = Instant::now();
                limits.posts.retain(|_, until| *until > now);
                let channel = body["channel"]
                    .as_str()
                    .filter(|_| method == "chat.postMessage");
                let cooldown = limits.cooldowns.get(method).copied().unwrap_or(now);
                let post = channel
                    .and_then(|channel| limits.posts.get(channel))
                    .copied()
                    .unwrap_or(now);
                let until = cooldown.max(post);
                if until > now {
                    Some(until)
                } else {
                    if let Some(channel) = channel {
                        limits
                            .posts
                            .insert(channel.into(), now + Duration::from_millis(1100));
                    }
                    None
                }
            };
            if let Some(until) = wait {
                tokio::time::sleep_until(until).await;
            } else {
                return;
            }
        }
    }
    pub async fn call(&self, method: &str, body: &Value) -> Result<Value, ChannelError> {
        let url = self
            .base
            .join(method)
            .map_err(|_| ChannelError::InvalidPayload("invalid Slack API URL".into()))?;
        let token = if method == "apps.connections.open" {
            &self.app_token
        } else {
            &self.bot_token
        };
        for attempt in 0..3 {
            self.wait_turn(method, body).await;
            let response = self
                .client
                .post(url.clone())
                .bearer_auth(token.as_ref())
                .json(body)
                .timeout(Duration::from_secs(30))
                .send()
                .await
                .map_err(|error| ChannelError::Transport(error.without_url().to_string()))?;
            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let seconds = response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(1)
                    .min(3600);
                let until = Instant::now() + Duration::from_secs(seconds);
                let mut limits = self.limits.lock().await;
                let current = limits.cooldowns.entry(method.into()).or_insert(until);
                *current = (*current).max(until);
                if attempt < 2 {
                    continue;
                }
                return Err(ChannelError::Transport(
                    "Slack rate limit retry budget exhausted".into(),
                ));
            }
            if !response.status().is_success() {
                return Err(ChannelError::Transport(format!(
                    "Slack HTTP status {}",
                    response.status()
                )));
            }
            let value: Value = response
                .json()
                .await
                .map_err(|_| ChannelError::InvalidPayload("invalid Slack API response".into()))?;
            if value["ok"] != true {
                let code = value["error"].as_str().unwrap_or("unknown_error");
                // Only Slack's symbolic codes are safe to expose. Never echo arbitrary payloads.
                let code = if code.len() <= 80
                    && code
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
                {
                    code
                } else {
                    "unknown_error"
                };
                return Err(ChannelError::Rejected(format!("Slack {method}: {code}")));
            }
            return Ok(value);
        }
        unreachable!("bounded retry loop returns on its last iteration")
    }
}
