//! Persistent task coordination and read-only document projections.

mod config;
mod model;
mod mutations;
mod projection;
mod store;

pub use config::{Config, DocumentConfig, DocumentFormat, StorageConfig, expand_home};
pub use model::*;
pub use projection::Service;
pub use store::Store;
