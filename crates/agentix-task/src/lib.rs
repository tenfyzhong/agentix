//! Persistent task coordination and read-only document projections.

mod browse;
mod browse_page;
pub use browse_page::{JobBrowsePage, JobListItem, TaskBrowsePage, TaskListItem};
mod config;
mod context;
pub use browse::{BrowseScope, ProjectSummary};
mod conversation;
pub use conversation::JobMessage;
mod deletion;
mod inbox;
mod inbox_document;
mod inbox_read;
pub use inbox_read::InboxSource;
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
