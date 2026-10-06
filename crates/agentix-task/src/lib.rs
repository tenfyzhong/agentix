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
mod discussion;
mod document_paths;
mod event_maintenance;
mod event_payload;
mod event_retention;
mod event_worker;
mod inbox;
mod inbox_document;
mod inbox_read;
pub use inbox_read::InboxSource;
mod memory_source;
mod model;
pub use memory_source::{MemoryBackfillPage, MemoryJobCancellation, MemorySource};
mod mutations;
mod naming;
mod project;
mod project_archive;
mod project_lookup;
mod project_rename;
mod projection;
mod publication;
mod routing;
mod scoped;
pub use scoped::JobFilter;
mod state_index;
mod store;
mod stored_paths;

pub use config::{Config, DocumentConfig, StorageConfig, expand_home};
pub use model::*;
pub use project::{ProjectDirectory, git_identity};
pub use projection::Service;
pub use store::Store;

#[cfg(test)]
mod event_retention_benchmark;
