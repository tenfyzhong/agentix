//! Codex app-server adapter using its native WebSocket-over-UDS transport.

#[cfg(unix)]
mod client;
#[cfg(not(unix))]
mod client_unsupported;
mod endpoint;
#[cfg(unix)]
mod multiplexer;
#[cfg(unix)]
mod process;
mod protocol;

#[cfg(unix)]
pub use client::CodexClient;
#[cfg(not(unix))]
pub use client_unsupported::CodexClient;
pub use endpoint::{CodexEndpoint, EndpointError};
pub use protocol::{ProtocolError, RpcError, ServerMessage, decode_server_frame};
