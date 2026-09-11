//! Owns the proxy listener, client registrations and shared upstream lifetime.
use crate::{CodexEndpoint, CodexProxy, ProxyOptions, UpstreamServer};
use anyhow::Result;
use std::path::Path;

/// Owned only by public client clones, never by the background tasks themselves.
/// The final owner cancels readers, reconnect attempts and lifecycle polling.
pub(crate) struct ClientTasks(pub Vec<tokio::task::JoinHandle<()>>);

impl Drop for ClientTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

pub(crate) struct ConnectionManager {
    pub proxy: CodexProxy,
    _upstream: UpstreamServer,
}
impl ConnectionManager {
    pub async fn start(
        listen: &str,
        upstream: &CodexEndpoint,
        command: &Path,
        options: &ProxyOptions,
    ) -> Result<Self> {
        let listen = CodexEndpoint::parse(listen)?.address();
        if listen == upstream.address() {
            return Err(
                crate::ProxyConfigError("proxy and upstream endpoints must differ".into()).into(),
            );
        }
        if listen == "stdio://" && options != &ProxyOptions::default() {
            return Err(crate::ProxyConfigError(
                "WebSocket authentication requires a ws:// proxy listener".into(),
            )
            .into());
        }
        // Reserve the frontend first. A conflicting listener must never launch an upstream.
        let mut proxy = if listen == "stdio://" {
            None
        } else {
            Some(CodexProxy::bind_with_options(&listen, &upstream.address(), options).await?)
        };
        let server = UpstreamServer::ensure(upstream, command).await?;
        if proxy.is_none() {
            proxy =
                Some(CodexProxy::bind_with_options(&listen, &upstream.address(), options).await?);
        }
        Ok(Self {
            proxy: proxy.unwrap(),
            _upstream: server,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn invalid_stdio_auth_is_rejected_before_starting_upstream() {
        let options = ProxyOptions {
            ws_auth: Some(crate::WsAuthMode::CapabilityToken),
            ws_token_sha256: Some("a".repeat(64)),
            ..ProxyOptions::default()
        };
        let endpoint = CodexEndpoint::parse("unix:///unused-agentix-upstream.sock").unwrap();
        let result = ConnectionManager::start(
            "stdio://",
            &endpoint,
            Path::new("must-not-launch"),
            &options,
        )
        .await;
        let error = result.err().expect("invalid stdio authentication");
        assert!(error.is::<crate::ProxyConfigError>(), "{error:#}");
    }
}
