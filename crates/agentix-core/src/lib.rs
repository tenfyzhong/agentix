//! Agentix application services, backend routing and orchestration.
mod command;
mod deferred;
mod dispatch;
mod engine;
mod output;
pub use output::OutputConfig;
mod registry;
mod session;

pub use agentix_domain::*;
pub use agentix_storage::{SqliteState, TaskNotification};
pub use command::{AgentCommand, InputParseError, ParsedInput, parse_input};
pub use deferred::DeferredAgent;
pub use dispatch::{DispatchId, DispatchQueue, DispatchScope, DispatchStatistics, Dispatched};
pub use engine::{
    Engine, EngineDispatchSnapshot, EngineError, EngineResource, EngineWork, RestoredBindings,
    ShutdownNotification,
};
pub use engine::{command_menu, command_menu_for};
pub use registry::AgentRegistry;
pub use session::SessionOperations;
