//! Codex app-server adapter and per-client Unix, WebSocket, and stdio proxy.

#[cfg(unix)]
mod client;
#[cfg(not(unix))]
mod client_unsupported;
mod endpoint;
#[cfg(unix)]
mod login_environment;
#[cfg(unix)]
mod multiplexer;
#[cfg(unix)]
mod process;
mod protocol;
#[cfg(unix)]
mod quota;

#[cfg(unix)]
pub use client::CodexClient;
#[cfg(not(unix))]
pub use client_unsupported::CodexClient;
pub use endpoint::{CodexEndpoint, EndpointError};
pub use protocol::{ProtocolError, RpcError, ServerMessage, decode_server_frame};

// Reuse the protocol mock in library tests as well as integration tests.
#[cfg(test)]
extern crate self as agentix_codex;

#[cfg(unix)]
mod registry;
#[cfg(unix)]
pub use registry::{ClientBinding, ClientRegistry};

#[cfg(unix)]
mod proxy;
#[cfg(unix)]
mod proxy_address;
#[cfg(unix)]
mod proxy_handshake;
#[cfg(unix)]
mod proxy_http;
#[cfg(unix)]
mod proxy_stdio_io;
#[cfg(unix)]
mod proxy_wire;
#[cfg(unix)]
pub use proxy::{CodexProxy, proxy_stdio};
#[cfg(unix)]
mod upstream;
#[cfg(unix)]
pub use upstream::UpstreamServer;

/// Failure to reserve Agentix's exclusive client-facing listener.
#[derive(Debug, thiserror::Error)]
#[error(
    "cannot bind Codex proxy_endpoint {endpoint}: {source}. This address is reserved for Agentix; do not start app-server on it. Use the upstream endpoint for app-server."
)]
pub struct ProxyBindError {
    pub endpoint: String,
    #[source]
    pub source: std::io::Error,
}

mod proxy_options;
pub use proxy_options::{ProxyOptions, WsAuthMode};
#[cfg(unix)]
mod proxy_auth;

#[cfg(unix)]
mod connection;

#[derive(Debug, thiserror::Error)]
#[error("invalid Codex proxy configuration: {0}")]
pub struct ProxyConfigError(pub String);

/// Startup policy belongs to the Codex connection layer.
#[must_use]
pub fn is_fatal_startup_error(error: &anyhow::Error) -> bool {
    error.is::<ProxyBindError>() || error.is::<ProxyConfigError>()
}
