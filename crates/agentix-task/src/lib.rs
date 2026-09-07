//! Persistent task coordination and read-only document projections.

mod browse;
mod config;
mod context;
pub use browse::{BrowseScope, ProjectSummary};
mod conversation;
pub use conversation::JobMessage;
mod deletion;
mod inbox;
mod inbox_document;
mod model;
mod mutations;
mod naming;
mod project;
mod projection;
mod publication;
mod scoped;
pub use scoped::JobFilter;
mod store;

pub use config::{Config, DocumentConfig, DocumentFormat, StorageConfig, expand_home};
pub use model::*;
pub use project::git_identity;
pub use projection::Service;
pub use store::Store;
