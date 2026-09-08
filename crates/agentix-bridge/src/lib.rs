//! Original-session host bridges accepted through the shared Agentix control endpoint.
mod bridge;
mod bridge_hub;
mod protocol;
pub mod wire;
pub use agentix_domain::AgentKind as BridgeKind;
pub use bridge::BridgeAdapter;
pub use bridge_hub::BridgeHub;
type PendingResponses = std::collections::HashMap<
    String,
    tokio::sync::oneshot::Sender<Result<serde_json::Value, agentix_domain::AgentError>>,
>;
