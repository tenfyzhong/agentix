use std::time::Duration;

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::{ProviderConfig, ProviderProtocol};

#[derive(Debug)]
pub(crate) struct ProviderHttpError(pub u16);
impl std::fmt::Display for ProviderHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "provider returned HTTP {}", self.0)
    }
}
impl std::error::Error for ProviderHttpError {}

/// Shared by model, background embedding and online queries for one provider.
pub struct HttpProvider {
    client: reqwest::Client,
    base_url: String,
    authorization: Option<reqwest::header::HeaderValue>,
    pub(crate) protocol: ProviderProtocol,
    permits: Semaphore,
    background: Semaphore,
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
            background: Semaphore::new(config.max_in_flight.saturating_sub(1).max(1)),
        })
    }

    pub(crate) fn identity(&self) -> String {
        format!("{:?}:{}", self.protocol, self.base_url)
    }

    async fn acquire(
        &self,
        query: bool,
    ) -> Result<(
        Option<tokio::sync::SemaphorePermit<'_>>,
        tokio::sync::SemaphorePermit<'_>,
    )> {
        // Acquire the background cap first so queued batches cannot occupy query capacity.
        let background = if query {
            None
        } else {
            Some(self.background.acquire().await?)
        };
        Ok((background, self.permits.acquire().await?))
    }

    pub(crate) async fn post(&self, endpoint: &str, body: Value, timeout: u64) -> Result<Value> {
        self.post_classified(endpoint, body, timeout, false).await
    }

    pub(crate) async fn post_classified(
        &self,
        endpoint: &str,
        body: Value,
        timeout: u64,
        query: bool,
    ) -> Result<Value> {
        tokio::time::timeout(Duration::from_secs(timeout), async {
            let _permit = self.acquire(query).await?;
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
            if !response.status().is_success() {
                return Err(ProviderHttpError(response.status().as_u16()).into());
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(capacity: usize) -> HttpProvider {
        HttpProvider::new(ProviderConfig {
            base_url: "http://127.0.0.1:1".into(),
            api_key_env: None,
            protocol: ProviderProtocol::Openai,
            max_in_flight: capacity,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn background_saturation_reserves_query_capacity_without_exceeding_total() {
        let provider = provider(2);
        let background = provider.acquire(false).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(30), provider.acquire(false))
                .await
                .is_err()
        );
        let query = tokio::time::timeout(Duration::from_millis(100), provider.acquire(true))
            .await
            .unwrap()
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(30), provider.acquire(true))
                .await
                .is_err()
        );
        drop(query);
        drop(background);
        assert!(provider.acquire(false).await.is_ok());
    }

    #[tokio::test]
    async fn single_capacity_query_waits_for_active_request_but_precedes_next_batch() {
        let provider = provider(1);
        let background = provider.acquire(false).await.unwrap();
        let query = provider.acquire(true);
        tokio::pin!(query);
        tokio::select! { result=&mut query => panic!("unexpected permit: {}",result.is_ok()), ()=tokio::time::sleep(Duration::from_millis(10))=>{} }
        let next = provider.acquire(false);
        tokio::pin!(next);
        tokio::select! { result=&mut next => panic!("unexpected permit: {}",result.is_ok()), ()=tokio::time::sleep(Duration::from_millis(10))=>{} }
        drop(background);
        let query = tokio::time::timeout(Duration::from_millis(100), query)
            .await
            .unwrap()
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut next)
                .await
                .is_err()
        );
        drop(query);
        assert!(next.await.is_ok());
    }
    #[tokio::test]
    #[ignore = "provider admission benchmark"]
    async fn query_admission_latency_during_background_saturation() {
        let provider = provider(4);
        let a = provider.acquire(false).await.unwrap();
        let b = provider.acquire(false).await.unwrap();
        let c = provider.acquire(false).await.unwrap();
        let mut samples = Vec::new();
        for _ in 0..100 {
            let start = std::time::Instant::now();
            let permit = provider.acquire(true).await.unwrap();
            samples.push(start.elapsed().as_nanos());
            drop(permit);
        }
        samples.sort_unstable();
        println!(
            "provider admission with 3/4 slots held by background: n=100 p50={}ns p95={}ns p99={}ns",
            samples[49], samples[94], samples[98]
        );
        drop((a, b, c));
    }
}
