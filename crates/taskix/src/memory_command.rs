use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum MemoryCommand {
    /// Run the independent local memory service until interrupted.
    Serve,
    /// Inspect queue, index coverage and provider availability.
    Status,
    /// Inspect service configuration and local availability without a model request.
    Doctor,
    /// Restore one page of read-only project memory notes from the database.
    Sync {
        #[arg(long, default_value = "")]
        after: String,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// Read the authoritative Markdown for a managed vault-relative memory path.
    Document { path: PathBuf },
    /// Inspect pending publication and preserved edit conflicts.
    ProjectionStatus,
    /// Reload service configuration for future work.
    Reload,
    /// Search project memory (offline FTS fallback is available).
    Search {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: i64,
    },
    /// Read one current or historical memory.
    Show {
        id: String,
        #[arg(long)]
        revision: Option<i64>,
    },
    /// List a bounded memory page.
    List {
        #[arg(long, default_value = "")]
        after: String,
        #[arg(long, default_value_t = 20)]
        limit: i64,
        #[arg(long)]
        all: bool,
    },
    /// Retrieve a small context, deduplicated within a session.
    Context {
        query: String,
        #[arg(long)]
        turn: String,
        #[arg(long)]
        budget: Option<usize>,
    },
    /// Inspect original evidence; use --message to read a bounded text page.
    Source {
        receipt_id: String,
        #[arg(long)]
        message: Option<String>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Add a memory from a JSON `MemoryInput` document through the service.
    Create {
        #[arg(long)]
        file: PathBuf,
    },
    /// Update a memory with an explicit expected revision.
    Update {
        id: String,
        #[arg(long)]
        revision: i64,
        #[arg(long)]
        file: PathBuf,
    },
    /// Remove a memory from retrieval and suppress replay of its evidence.
    Forget {
        id: String,
        #[arg(long)]
        revision: i64,
        #[arg(long)]
        reason: String,
    },
    /// Change lifecycle status while preserving history.
    SetStatus {
        id: String,
        status: String,
        #[arg(long)]
        revision: i64,
        #[arg(long)]
        reason: String,
    },
    /// Rebuild a bounded FTS page without changing memory content.
    Reindex {
        #[arg(long, default_value = "")]
        after: String,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Inspect one durable maintenance task and its audit.
    Work { id: i64 },
    /// Retry failed work against its current source revision.
    Retry { id: i64 },
    /// Inspect receipt coverage; optionally wait with a deadline.
    Receipt {
        receipt_id: String,
        #[arg(long, default_value_t = 0)]
        wait_seconds: u64,
    },
    /// Explicitly backfill a bounded page of a historical Job.
    Backfill {
        job: String,
        #[arg(long, default_value_t = 0)]
        offset: i64,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// Run a separately limited, read-only Agent query with citations.
    Ask { query: String },
}
