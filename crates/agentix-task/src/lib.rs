//! Persistent task coordination and read-only document projections.

mod config;
mod deletion;
mod model;
mod mutations;
mod naming;
mod projection;
mod store;

pub use config::{Config, DocumentConfig, DocumentFormat, StorageConfig, expand_home};
pub use model::*;
pub use projection::Service;
pub use store::Store;
