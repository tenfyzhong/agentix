use std::time::Duration;

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::{ProviderConfig, ProviderProtocol};

/// Shared by model, background embedding and online queries for one provider.
pub struct HttpProvider {
    client: reqwest::Client,
    base_url: String,
    authorization: Option<reqwest::header::HeaderValue>,
    pub(crate) protocol: ProviderProtocol,
    permits: Semaphore,
}

impl HttpProvider {
    pub fn new(config: ProviderConfig) -> Result<Self> {
        let url = reqwest::Url::parse(&config.base_url)?;
        ensure!(
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "invalid: provider URL"
        );
        ensure!(
            (1..=64).contains(&config.max_in_flight),
            "invalid: provider concurrency"
        );
        let authorization = if let Some(name) = config.api_key_env {
            let key = std::env::var(&name).with_context(|| {
                format!("provider key environment variable {name} is unavailable")
            })?;
            ensure!(
                !key.trim().is_empty(),
                "provider key environment variable is empty"
            );
            let mut header = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}"))?;
            header.set_sensitive(true);
            Some(header)
        } else {
            None
        };
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            base_url: config.base_url.trim_end_matches('/').into(),
            authorization,
            protocol: config.protocol,
            permits: Semaphore::new(config.max_in_flight),
        })
    }

    pub(crate) fn identity(&self) -> String {
        format!("{:?}:{}", self.protocol, self.base_url)
    }

    pub(crate) async fn post(&self, endpoint: &str, body: Value, timeout: u64) -> Result<Value> {
        tokio::time::timeout(Duration::from_secs(timeout), async {
            let _permit = self.permits.acquire().await?;
            let body = serde_json::to_vec(&body)?;
            ensure!(
                body.len() <= 2 * 1024 * 1024,
                "model request exceeds byte budget"
            );
            let mut request = self
                .client
                .post(format!("{}/{}", self.base_url, endpoint))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body);
            if let Some(value) = &self.authorization {
                request = request.header(reqwest::header::AUTHORIZATION, value.clone());
            }
            let mut response = request.send().await.context("provider request failed")?;
            ensure!(
                response.status().is_success(),
                "provider returned HTTP {}",
                response.status().as_u16()
            );
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                ensure!(
                    bytes.len() + chunk.len() <= 2 * 1024 * 1024,
                    "model response exceeds byte budget"
                );
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&bytes).context("invalid provider JSON")
        })
        .await
        .context("provider deadline exceeded")?
    }
}
