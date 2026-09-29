use crate::Cli;
use agentix_memory::{
    Actor, IpcClient, MemoryApi, MemoryLocation, MemoryStore, RequestHandler, context_preview,
};
use anyhow::{Context, Result, bail, ensure};
use clap::Subcommand;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
mod daemon;

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

pub async fn run(cli: &Cli, action: &MemoryCommand) -> Result<Value> {
    let path = cli.config_path()?;
    let location = MemoryLocation::load(&path)?;
    if !location.enabled {
        return match action {
            MemoryCommand::Context { .. } => Ok(json!({"enabled":false,"text":"","items":[]})),
            MemoryCommand::Status | MemoryCommand::Doctor => {
                Ok(json!({"enabled":false,"online":false}))
            }
            _ => bail!("memory is disabled; configure [memory].enabled"),
        };
    }
    if let MemoryCommand::Document { path: note_path } = action {
        return memory_document(&path, &location, note_path).await;
    }
    if matches!(action, MemoryCommand::Doctor) {
        return doctor(&path, &location).await;
    }
    if matches!(action, MemoryCommand::Serve) {
        return daemon::serve(&path, location).await;
    }
    let project = resolve_project(cli, &location, action).await?;
    let request = make_request(cli, action, project.as_deref())?;
    let client = IpcClient::new(&location.path, location.service)?;
    let timeout = if matches!(action, MemoryCommand::Ask { .. }) {
        Duration::from_secs(65)
    } else if matches!(action, MemoryCommand::Context { .. }) {
        Duration::from_millis(location.retrieval.query_timeout_ms + 500)
    } else {
        Duration::from_secs(10)
    };
    let result = client.call_with_timeout(request.clone(), timeout).await;
    match result {
        Ok(mut result) => {
            if let MemoryCommand::Receipt { wait_seconds, .. } = action {
                ensure!(
                    *wait_seconds <= 300,
                    "receipt wait cannot exceed 300 seconds"
                );
                let until = tokio::time::Instant::now() + Duration::from_secs(*wait_seconds);
                while result["complete"] != true
                    && result["states"]["failed"].as_i64().unwrap_or(0) == 0
                    && result["states"]["cancelled"].as_i64().unwrap_or(0) == 0
                    && tokio::time::Instant::now() < until
                {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    result = client.call(request.clone()).await?;
                }
            }
            Ok(result)
        }
        Err(error) if read_only(action) => {
            offline(&location, request, action, &error.to_string()).await
        }
        Err(error) => Err(error),
    }
}

async fn memory_document(
    config_path: &std::path::Path,
    location: &MemoryLocation,
    note_path: &std::path::Path,
) -> Result<Value> {
    use std::path::Component;
    ensure!(
        !note_path.is_absolute()
            && note_path
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "invalid managed memory path"
    );
    let id = note_path
        .file_stem()
        .and_then(|value| value.to_str())
        .context("invalid memory ID")?;
    ensure!(
        id.strip_prefix("mem_")
            .is_some_and(|suffix| suffix.len() == 32
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())),
        "invalid memory ID"
    );
    let config = agentix_task::Config::load(config_path)?;
    let store = MemoryStore::open_read_only(&location.path).await?;
    let (memory, text) = store.rendered_note(id).await?;
    let tasks = agentix_task::Store::open_read_only(&location.task_path).await?;
    let project = tasks.project_result(&memory.project_id).await?;
    let expected = config
        .documents
        .directory
        .join("Projects")
        .join(project.key)
        .join("Memory")
        .join(format!("{id}.md"));
    ensure!(
        note_path == expected,
        "memory path does not match its Project"
    );
    Ok(
        json!({"path":expected,"id":memory.id,"project_id":memory.project_id,"revision":memory.revision,"text":text}),
    )
}

async fn doctor(path: &std::path::Path, location: &MemoryLocation) -> Result<Value> {
    let configuration = match agentix_memory::MemoryConfig::load(path) {
        Ok(config) => {
            let providers = config.providers.iter().map(|(name, provider)| {
                json!({"name":name,"credentials_available":provider.api_key_env.as_ref()
                    .is_none_or(|key| std::env::var(key).is_ok_and(|value| !value.trim().is_empty()))})
            }).collect::<Vec<_>>();
            json!({"valid":true,"model":config.agent.model,"api":config.agent.api,
                "embedding_enabled":config.embedding.enabled,"providers":providers})
        }
        // Do not echo TOML parse excerpts, which can contain sensitive configuration.
        Err(_) => {
            json!({"valid":false,"error":"invalid memory configuration; check model, provider and budget fields"})
        }
    };
    let database = if location.path.exists() {
        match MemoryStore::open_read_only(&location.path).await {
            Ok(_) => json!({"exists":true,"readable":true,"schema_version":1}),
            Err(_) => {
                json!({"exists":true,"readable":false,"error":"incompatible or unreadable memory database"})
            }
        }
    } else {
        json!({"exists":false,"readable":false})
    };
    let client = IpcClient::new(&location.path, location.service)?;
    let status = client
        .call_with_timeout(
            json!({"op":"status","project":null}),
            Duration::from_secs(2),
        )
        .await;
    Ok(
        json!({"enabled":location.enabled,"online":status.is_ok(),"configuration":configuration,
        "database":database,"service":status.ok(),"task_database_exists":location.task_path.exists()}),
    )
}

async fn resolve_project(
    cli: &Cli,
    location: &MemoryLocation,
    action: &MemoryCommand,
) -> Result<Option<String>> {
    if matches!(action, MemoryCommand::Reload) {
        return Ok(None);
    }
    if cli.project.is_none() && matches!(action, MemoryCommand::Status | MemoryCommand::Doctor) {
        return Ok(None);
    }
    ensure!(
        location.task_path.exists(),
        "not_found: task database required to resolve Project"
    );
    let store = agentix_task::Store::open_read_only(&location.task_path).await?;
    let project = if let Some(id) = &cli.project {
        store.project_result(id).await?
    } else {
        let directory = agentix_task::ProjectDirectory::discover(&std::env::current_dir()?)?;
        store
            .project_by_root(&directory.root().to_string_lossy())
            .await?
            .context("not_found: register the current Project or use --project")?
    };
    Ok(Some(project.id))
}

#[allow(clippy::too_many_lines)]
fn make_request(cli: &Cli, action: &MemoryCommand, project: Option<&str>) -> Result<Value> {
    let actor = if cli.executor.is_some() || cli.actor.starts_with("agent:") {
        Actor::Agent
    } else {
        Actor::Human
    };
    let request = match action {
        MemoryCommand::Document { .. } => bail!("document is a local read"),
        MemoryCommand::Status | MemoryCommand::Doctor => json!({"op":"status","project":project}),
        MemoryCommand::Reload => json!({"op":"reload"}),
        MemoryCommand::Sync { after, limit } => {
            json!({"op":"sync","project":project,"after":after,"limit":limit})
        }
        MemoryCommand::ProjectionStatus => json!({"op":"projection_status","project":project}),
        MemoryCommand::Search { query, limit } => {
            json!({"op":"search","project":project,"query":query,"limit":limit})
        }
        MemoryCommand::Show { id, revision } => {
            json!({"op":"show","project":project,"id":id,"revision":revision})
        }
        MemoryCommand::List { after, limit, all } => {
            json!({"op":"list","project":project,"after":after,"limit":limit,"all":all})
        }
        MemoryCommand::Context {
            query,
            turn,
            budget,
        } => {
            json!({"op":"context","project":project,"session":cli.session.as_deref().context("memory context requires --session")?,"turn":turn,"query":query,"budget":budget})
        }
        MemoryCommand::Source {
            receipt_id,
            message,
            offset,
        } => {
            json!({"op":"source","project":project,"receipt_id":receipt_id,"message_id":message,"offset":offset})
        }
        MemoryCommand::Create { file } => {
            json!({"op":"create","project":project,"content":read_content(file)?,"actor":actor})
        }
        MemoryCommand::Update { id, revision, file } => {
            json!({"op":"update","project":project,"id":id,"revision":revision,"content":read_content(file)?,"actor":actor})
        }
        MemoryCommand::Forget {
            id,
            revision,
            reason,
        } => {
            json!({"op":"set_status","project":project,"id":id,"revision":revision,"status":"forgotten","reason":reason,"actor":actor})
        }
        MemoryCommand::SetStatus {
            id,
            status,
            revision,
            reason,
        } => {
            json!({"op":"set_status","project":project,"id":id,"revision":revision,"status":status,"reason":reason,"actor":actor})
        }
        MemoryCommand::Reindex { after, limit } => {
            json!({"op":"reindex","project":project,"after":after,"limit":limit})
        }
        MemoryCommand::Work { id } => json!({"op":"work","project":project,"id":id}),
        MemoryCommand::Retry { id } => json!({"op":"retry","project":project,"id":id}),
        MemoryCommand::Receipt { receipt_id, .. } => {
            json!({"op":"receipt","project":project,"receipt_id":receipt_id})
        }
        MemoryCommand::Backfill { job, offset, limit } => {
            json!({"op":"backfill","project":project,"job":job,"offset":offset,"limit":limit})
        }
        MemoryCommand::Ask { query } => json!({"op":"ask","project":project,"query":query}),
        MemoryCommand::Serve => unreachable!(),
    };
    Ok(request)
}
fn read_content(path: &PathBuf) -> Result<Value> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(256 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 256 * 1024, "memory document exceeds 256 KiB");
    Ok(serde_json::from_slice(&bytes)?)
}
fn read_only(action: &MemoryCommand) -> bool {
    matches!(
        action,
        MemoryCommand::Status
            | MemoryCommand::ProjectionStatus
            | MemoryCommand::Doctor
            | MemoryCommand::Search { .. }
            | MemoryCommand::Show { .. }
            | MemoryCommand::List { .. }
            | MemoryCommand::Context { .. }
            | MemoryCommand::Source { .. }
            | MemoryCommand::Work { .. }
            | MemoryCommand::Receipt { .. }
    )
}
async fn offline(
    location: &MemoryLocation,
    request: Value,
    action: &MemoryCommand,
    error: &str,
) -> Result<Value> {
    if !location.path.exists() && matches!(action, MemoryCommand::Status | MemoryCommand::Doctor) {
        return Ok(json!({"online":false,"enabled":true,"error":error,"initialized":false}));
    }
    let store = MemoryStore::open_read_only(&location.path).await?;
    if let MemoryCommand::Context { query, budget, .. } = action {
        let project = request["project"].as_str().context("missing Project")?;
        let memories = store
            .search(project, query, i64::try_from(location.retrieval.max_items)?)
            .await?;
        let mut value = serde_json::to_value(context_preview(
            project,
            &memories,
            budget
                .unwrap_or(location.retrieval.max_context_bytes)
                .min(location.retrieval.max_context_bytes),
        )?)?;
        value["mode"] = json!("offline_fts");
        return Ok(value);
    }
    let api = MemoryApi::new(store, location.retrieval.clone(), location.service);
    let mut value = api.handle(request).await?;
    if matches!(action, MemoryCommand::Search { .. }) {
        value["mode"] = json!("offline_fts");
    }
    if matches!(action, MemoryCommand::Status | MemoryCommand::Doctor) {
        value["online"] = json!(false);
        value["error"] = json!(error);
    }
    Ok(value)
}
