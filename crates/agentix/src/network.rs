use anyhow::{Context, Result, bail};
use serde::Deserialize;
use url::Url;

/// Global settings for Agentix's outbound network connections.
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    pub proxy: Option<String>,
}

impl std::fmt::Debug for NetworkConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkConfig")
            .field("proxy", &self.proxy.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

impl NetworkConfig {
    /// Apply the global proxy while preserving the caller's timeout and TLS settings.
    pub fn http_client(&self, mut builder: reqwest::ClientBuilder) -> Result<reqwest::Client> {
        self.validate()?;
        if let Some(proxy) = &self.proxy {
            let proxy = reqwest::Proxy::all(proxy)
                .map_err(|_| anyhow::anyhow!("failed to configure network.proxy"))?;
            builder = builder.no_proxy().proxy(proxy);
        }
        builder.build().context("failed to build HTTP client")
    }

    pub fn validate(&self) -> Result<()> {
        let Some(proxy) = &self.proxy else {
            return Ok(());
        };
        let valid = proxy.contains("://")
            && !proxy.chars().any(char::is_whitespace)
            && Url::parse(proxy).is_ok_and(|url| {
                matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h")
                    && url.host_str().is_some_and(|host| !host.is_empty())
                    && url.port() != Some(0)
                    && matches!(url.path(), "" | "/")
                    && url.query().is_none()
                    && url.fragment().is_none()
            });
        if !valid {
            bail!(
                "network.proxy must be an http://, https://, socks5://, or socks5h:// URL with a host and optional port, without a path, query, or fragment"
            );
        }
        Ok(())
    }
}
