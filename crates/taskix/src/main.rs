use std::{
    io::{IsTerminal, Read, Write},
    path::PathBuf,
    process::{Command as Process, ExitCode},
};

use agentix_task::{
    Config, DocumentConfig, JobStatus, Service, StorageConfig, WriteOptions, expand_home,
    git_identity,
};
use anyhow::{Context, Result, bail, ensure};
use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use serde_json::{Value, json};

mod obsidian;

#[derive(Parser)]
#[command(
    version,
    about = "Coordinate agent tasks with SQLite and read-only Obsidian boards"
)]
struct Cli {
    #[arg(long, global = true, value_hint = clap::ValueHint::FilePath)]
    config: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true)]
    project: Option<String>,
    #[arg(long, global = true, default_value = "user:cli")]
    actor: String,
    #[arg(long, global = true)]
    executor: Option<String>,
    #[arg(long, global = true)]
    session: Option<String>,
    #[arg(long, global = true)]
    delegated_by: Option<String>,
    #[arg(long, global = true)]
    lease_token: Option<String>,
    #[arg(long, global = true)]
    expect_revision: Option<i64>,
    #[arg(long, global = true)]
    idempotency_key: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Submit, inspect, claim, or cancel human requirements in a Project Inbox.
    Inbox {
        #[command(subcommand)]
        action: InboxCommand,
    },
    /// Print a shell completion script without loading task configuration.
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Create configuration, initialize `SQLite` storage, and generate task documents.
    Init(Init),
    /// Install and configure Obsidian task views in the configured vault.
    Obsidian {
        #[command(subcommand)]
        action: ObsidianCommand,
    },
    /// Check for missing Plan files and documents that are behind the event log.
    Doctor,
    /// Regenerate task documents from the current database state.
    Sync {
        /// Retry only pending document publications without a full rebuild.
        #[arg(long)]
        pending: bool,
    },
    /// Register, inspect, archive, or delete Projects shared across worktrees.
    Project {
        #[command(subcommand)]
        action: ProjectCommand,
    },
    /// Manage Jobs that group Tasks for an independently deliverable requirement.
    Job {
        #[command(subcommand)]
        action: JobCommand,
    },
    /// Manage Task dependencies, ownership leases, and execution status.
    Task {
        #[command(subcommand)]
        action: TaskCommand,
    },
    /// Publish, revise, or inspect the execution Plan in a Task note.
    Plan {
        #[command(subcommand)]
        action: PlanCommand,
    },
    /// Inspect the ordered audit log of task coordination events.
    Event {
        #[command(subcommand)]
        action: EventCommand,
    },
    /// Show Task, Job, lease, Plan, and document context for a session or explicit IDs.
    Context {
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        job: Option<String>,
    },
    /// Handle agent session lifecycle events and maintain Task leases.
    Hook {
        #[command(subcommand)]
        action: HookCommand,
    },
}

#[derive(Subcommand)]
enum ObsidianCommand {
    /// Read the Obsidian connection configuration without loading task records.
    Connection,
    /// Query one registered Task, Job, or Inbox entry by its exact ID.
    Show { id: String },
    /// Query registered notes and authoritative status properties without lease credentials.
    Snapshot,
    /// Install task views and reload the configured Obsidian vault when files change.
    Setup {
        /// Use a local `TaskNotes` release directory instead of downloading the bundled version.
        #[arg(long, value_hint = clap::ValueHint::DirPath)]
        plugin_dir: Option<PathBuf>,
        /// Skip Obsidian CLI calls; close Obsidian before setup and reopen it afterward.
        #[arg(long)]
        no_reload: bool,
    },
}

#[derive(Args)]
struct Init {
    #[arg(long, value_hint = clap::ValueHint::DirPath)]
    root: PathBuf,
    #[arg(long, default_value = ".", value_hint = clap::ValueHint::DirPath)]
    directory: PathBuf,
    #[arg(long, value_hint = clap::ValueHint::FilePath)]
    database: Option<PathBuf>,
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// Delete the Project, its work, and its entire generated document directory.
    Delete { id: String },
    /// Register a Project using its root directory and Git identity when available.
    Register {
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_hint = clap::ValueHint::DirPath)]
        root: Option<PathBuf>,
    },
    /// List unarchived Projects, or archived Projects with --archived.
    List {
        #[arg(long)]
        archived: bool,
    },
    /// Show a Project's identity, root directory, and archival state.
    Show { id: String },
    /// Archive a Project after all of its Jobs are closed.
    Archive { id: String },
    /// Restore an archived Project to the active project views.
    Unarchive { id: String },
}

#[derive(Subcommand)]
enum InboxCommand {
    /// Append one requirement, preserving its complete Markdown content.
    Add {
        #[arg(long)]
        content: String,
    },
    /// Import human edits and show visible Inbox entries in document order.
    List,
    /// Import submissions, cancellations, and withdrawals and repair the document.
    Sync,
    /// Atomically claim the next eligible requirement and create or resume its Job.
    ClaimNext,
    /// Release an owned Inbox lease so another agent can resume its existing Job.
    Release { id: String },
    /// Cancel a requirement and its unfinished work, preserving history.
    Cancel { id: String },
    /// Set a human Inbox status; linked completion requires a pending review.
    SetStatus {
        id: String,
        #[arg(long, value_parser = ["TODO", "ACTIVE", "PENDING_REVIEW", "COMPLETED", "CANCELLED", "IN_PROGRESS", "DONE"])]
        status: String,
    },
}

#[derive(Args)]
#[allow(clippy::struct_excessive_bools)] // Independent clap filter flags with explicit conflicts.
struct JobList {
    #[arg(long,conflicts_with_all=["completed","archived","pending_review"])]
    active: bool,
    #[arg(long, conflicts_with_all = ["completed", "archived"])]
    pending_review: bool,
    #[arg(long, conflicts_with = "archived")]
    completed: bool,
    #[arg(long)]
    archived: bool,
    #[arg(long)]
    period: Option<String>,
    #[arg(long)]
    created_from: Option<String>,
    #[arg(long)]
    created_to: Option<String>,
}
#[derive(Subcommand)]
enum JobCommand {
    /// Resume a pending Job with a supplemental user prompt.
    Followup {
        id: String,
        #[arg(long)]
        prompt: String,
        /// Inbox TODO IDs selected by the agent through semantic matching; repeat for multiple entries.
        #[arg(long = "inbox")]
        inbox_ids: Vec<String>,
    },
    /// Submit an ACTIVE Job for review once all non-cancelled Tasks are DONE.
    Submit { id: String },
    /// Record human acceptance of a Job awaiting review.
    Approve { id: String },
    /// Return a Job awaiting review to ACTIVE, preserving its Tasks.
    Reject(Reason),
    /// Delete the Job, its Tasks, and their Plan documents.
    Delete { id: String },
    /// Create a Job with a title and acceptance goal in the selected Project.
    Create {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        title: String,
        #[arg(long, default_value = "")]
        goal: String,
        /// Original user prompt, preserved verbatim in the Job document.
        #[arg(long, default_value = "")]
        prompt: String,
        /// Inbox TODO IDs selected by the agent through semantic matching; repeat for multiple entries.
        #[arg(long = "inbox")]
        inbox_ids: Vec<String>,
        #[arg(long, value_parser = ["required", "none"], default_value = "required")]
        review_policy: String,
    },
    /// Change a Job's display name, title, acceptance goal, or original prompt.
    Update {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        goal: Option<String>,
        /// Replace the original user prompt; an empty string clears it.
        #[arg(long)]
        prompt: Option<String>,
        /// Inbox TODO IDs selected by the agent through semantic matching; repeat for multiple entries.
        #[arg(long = "inbox")]
        inbox_ids: Vec<String>,
        #[arg(long, value_parser = ["required", "none"])]
        review_policy: Option<String>,
    },
    /// List Jobs, optionally filtered by Project, status, or date.
    List(JobList),
    /// Show a Job's goal, status, and lifecycle metadata.
    Show { id: String },
    /// Cancel a Job and its unfinished Tasks after their leases have been released.
    Cancel { id: String },
    /// Archive a closed Job and move its document to the archive.
    Archive { id: String },
    /// Restore an archived Job document without changing its completion status.
    Unarchive { id: String },
}

#[derive(Args)]
struct Reason {
    id: String,
    #[arg(long)]
    reason: String,
}
#[derive(Args)]
struct TaskId {
    id: String,
}
#[derive(Subcommand)]
enum TaskCommand {
    /// Add a Task to a Job and create its note without publishing a Plan.
    Add {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        job: String,
        #[arg(long)]
        title: String,
    },
    /// Change a Task's display name, title, or board position.
    Update {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        position: Option<i64>,
    },
    /// List Tasks, optionally filtered by Job, Project, status, or readiness.
    List {
        #[arg(long)]
        job: Option<String>,
        #[arg(long)]
        ready: bool,
        #[arg(long)]
        status: Option<String>,
    },
    /// Show a Task's status, dependencies, Plan reference, and current lease.
    Show(TaskId),
    /// Add a prerequisite Task in the same Project before execution has started.
    Depend { id: String, dependency: String },
    /// Remove a prerequisite Task before execution has started.
    Undepend { id: String, dependency: String },
    /// Acquire a planning lease using --executor and --session.
    Claim(TaskId),
    /// Begin execution with the current lease, a published Plan, and DONE prerequisites.
    Start(TaskId),
    /// Renew a Task lease using its owning session and lease token.
    Heartbeat(TaskId),
    /// Release ownership and mark the Task BLOCKED with a handoff reason.
    Release(Reason),
    /// Mark a Task BLOCKED with a reason and release its lease.
    Block(Reason),
    /// Mark a Task `WAITING_USER` with a reason and release its lease.
    Wait(Reason),
    /// Mark a Task FAILED with a reason and release its lease.
    Fail(Reason),
    /// Mark an EXECUTING Task DONE and release its lease.
    Done(TaskId),
    /// Cancel a Task and release its lease.
    Cancel(TaskId),
    /// Return a FAILED Task to TODO so it can be claimed again.
    Retry(TaskId),
    /// Return a DONE or CANCELLED Task to TODO so it can be claimed again.
    Reopen(TaskId),
}
#[derive(Args)]
struct PlanBody {
    task: String,
    #[arg(
        long,
        required_unless_present = "file",
        conflicts_with = "file",
        allow_hyphen_values = true
    )]
    body: Option<String>,
    #[arg(long, value_hint = clap::ValueHint::FilePath)]
    file: Option<PathBuf>,
}
#[derive(Subcommand)]
enum PlanCommand {
    /// Publish a Plan from --body or --file while holding the Task lease.
    Create(PlanBody),
    /// Replace the Plan body in the same Task note while holding its lease.
    Revise(PlanBody),
    /// Show the current Plan's metadata and absolute file path for a Task.
    Show { task: String },
}
#[derive(Subcommand)]
enum EventCommand {
    /// List events after a sequence cursor, optionally filtered by Job and limited in count.
    List {
        #[arg(long)]
        job: Option<String>,
        #[arg(long, default_value_t = 0)]
        after: i64,
        #[arg(long, default_value_t = 100)]
        limit: i64,
    },
}
#[derive(Subcommand)]
enum HookCommand {
    /// Record visible user/assistant messages from a JSON array in the session Job.
    Record {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        job: Option<String>,
    },
    /// Acknowledge turn completion without claiming Inbox work.
    Stop,
    /// Recover the session's Tasks blocked by interruption or lease expiry into planning.
    SessionStart,
    /// Record session shutdown and release its active Task leases.
    SessionEnd,
    /// Release an interrupted session's Task leases while preserving its Plans.
    Interrupt,
    /// Renew all active Task leases owned by the session.
    Heartbeat,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Command::Completions { shell } = &cli.command {
        clap_complete::generate(
            *shell,
            &mut Cli::command(),
            "taskix",
            &mut std::io::stdout(),
        );
        return ExitCode::SUCCESS;
    }
    match run(&cli).await {
        Ok(value) => {
            if cli.json {
                println!("{value}");
            } else {
                print_human(&value["result"]);
                if let Some(warning) = value["projection_pending"].as_str() {
                    eprintln!("projection pending: {warning}");
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            let message = format!("{error:#}");
            if cli.json {
                println!(
                    "{}",
                    json!({"schema_version":1,"ok":false,"error":{"code":error_code(&message),"message":message}})
                );
            } else {
                eprintln!("taskix: {message}");
            }
            ExitCode::FAILURE
        }
    }
}

fn error_code(message: &str) -> &str {
    if message.contains("conflict:") {
        "conflict"
    } else if message.contains("not_found:") {
        "not_found"
    } else {
        "invalid_or_failed"
    }
}
fn response(result: Value) -> Value {
    let mut envelope = json!({"schema_version":1,"ok":true});
    envelope["result"] = result;
    envelope
}
fn print_human(value: &Value) {
    if let Some(items) = value.as_array() {
        for item in items {
            print_human(item);
        }
    } else if let (Some(id), Some(title)) = (
        value["id"].as_str(),
        value["title"].as_str().or_else(|| value["name"].as_str()),
    ) {
        println!("{id}  {}  {title}", value["status"].as_str().unwrap_or(""));
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        );
    }
}

impl Cli {
    fn config_path(&self) -> Result<PathBuf> {
        let path = self
            .config
            .clone()
            .or_else(|| std::env::var_os("TASKIX_CONFIG").map(PathBuf::from));
        path.map_or_else(Config::default_path, |p| expand_home(&p))
    }
    fn options(&self) -> WriteOptions {
        WriteOptions {
            actor_ref: self.executor.clone().unwrap_or_else(|| self.actor.clone()),
            session_ref: self.session.clone(),
            delegated_by: self.delegated_by.clone(),
            lease_token: self.lease_token.clone(),
            expected_revision: self.expect_revision,
            idempotency_key: self.idempotency_key.clone(),
        }
    }
}

async fn setup_obsidian(
    cli: &Cli,
    plugin_dir: Option<&std::path::Path>,
    no_reload: bool,
) -> Result<Value> {
    let path = cli.config_path()?;
    let config = Config::load(&path)?;
    Ok(response(
        obsidian::setup(&config, &path, plugin_dir, no_reload).await?,
    ))
}

async fn run(cli: &Cli) -> Result<Value> {
    if let Command::Init(init) = &cli.command {
        return initialize(cli, init).await;
    }
    if let Command::Obsidian {
        action: ObsidianCommand::Setup {
            plugin_dir,
            no_reload,
        },
    } = &cli.command
    {
        return setup_obsidian(cli, plugin_dir.as_deref(), *no_reload).await;
    }
    let config = Config::load(&cli.config_path()?)?;
    if matches!(
        &cli.command,
        Command::Obsidian {
            action: ObsidianCommand::Connection
        }
    ) {
        return Ok(response(
            json!({"protocol_version":1,"documents":config.documents}),
        ));
    }
    let service = Service::open(config).await?;
    // Point reads must not load all entities through global lease maintenance.
    match &cli.command {
        Command::Task {
            action: TaskCommand::Show(args),
        } => {
            return Ok(response(service.store().task_result(&args.id).await?));
        }
        Command::Obsidian {
            action: ObsidianCommand::Show { id },
        } => {
            return Ok(response(service.obsidian_note(id).await?));
        }
        _ => (),
    }
    service.store().reap_expired().await?;
    match &cli.command {
        Command::Inbox { action } => inbox(cli, &service, action).await,
        Command::Doctor => {
            let state = service.store().snapshot().await?;
            let missing: Vec<_> = state
                .plans
                .iter()
                .filter(|p| !service.config().output_dir().join(&p.path).is_file())
                .map(|p| p.path.clone())
                .collect();
            let sequence = service.store().latest_sequence().await?;
            let rendered = service
                .store()
                .metadata("sequence")
                .await?
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let healthy = missing.is_empty()
                && rendered >= sequence
                && !service.store().has_pending_documents().await?;
            Ok(response(
                json!({"healthy":healthy,"missing_plans":missing,"sequence":sequence,"rendered_sequence":rendered,"documents":service.config().documents}),
            ))
        }
        Command::Sync { pending } => {
            if *pending {
                service.sync_pending_documents().await?;
            } else {
                service.sync().await?;
            }
            Ok(response(json!({"synced":true})))
        }
        Command::Project { action } => project(cli, &service, action).await,
        Command::Job { action } => job(cli, &service, action).await,
        Command::Task { action } => task(cli, &service, action).await,
        Command::Plan { action } => plan(cli, &service, action).await,
        Command::Event {
            action: EventCommand::List { job, after, limit },
        } => {
            let events = service
                .store()
                .events(job.as_deref(), *after, *limit)
                .await?;
            let next = events.last().map_or(*after, |e| e.sequence);
            Ok(response(json!({"events":events,"next_cursor":next})))
        }
        Command::Context { task, job } => {
            context(cli, &service, task.as_deref(), job.as_deref()).await
        }
        Command::Hook { action } => hook(cli, &service, action).await,
        Command::Obsidian {
            action: ObsidianCommand::Snapshot,
        } => Ok(response(service.obsidian_snapshot().await?)),
        Command::Init(_) | Command::Completions { .. } | Command::Obsidian { .. } => unreachable!(),
    }
}

async fn initialize(cli: &Cli, init: &Init) -> Result<Value> {
    let path = cli.config_path()?;
    ensure!(!path.exists(), "config already exists: {}", path.display());
    let config = Config {
        schema_version: 1,
        storage: StorageConfig {
            path: expand_home(
                &init
                    .database
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("~/.local/share/taskix/tasks.sqlite3")),
            )?,
        },
        documents: DocumentConfig {
            root: expand_home(&init.root)?,
            directory: init.directory.clone(),
        },
    };
    config.validate()?;
    let service = Service::open(config.clone()).await?;
    service.sync().await?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?
        .write_all(toml::to_string_pretty(&config)?.as_bytes())?;
    Ok(response(
        json!({"config":path,"database":config.storage.path,"documents":config.output_dir()}),
    ))
}

async fn mutate(cli: &Cli, service: &Service, request: Value) -> Result<Value> {
    let outcome = service.execute(request, cli.options()).await?;
    Ok(
        json!({"schema_version":1,"ok":true,"result":outcome.result,"sequence":outcome.sequence,"projection_pending":outcome.projection_pending}),
    )
}

async fn project(cli: &Cli, service: &Service, action: &ProjectCommand) -> Result<Value> {
    match action {
        ProjectCommand::Delete { id } => {
            mutate(
                cli,
                service,
                json!({"command":"project.delete","project":id}),
            )
            .await
        }
        ProjectCommand::Register { name, root } => {
            let root = root.clone().unwrap_or(std::env::current_dir()?);
            let (root, remote) = git_identity(&root)?;
            let name = name
                .clone()
                .or_else(|| root.file_name().map(|n| n.to_string_lossy().into_owned()))
                .context("project name required")?;
            mutate(
                cli,
                service,
                json!({"command":"project.register","name":name,"root":root,"remote":remote}),
            )
            .await
        }
        ProjectCommand::List { archived } => Ok(response(json!(
            service
                .store()
                .projects()
                .await?
                .into_iter()
                .filter(|p| p.archived_at.is_some() == *archived)
                .collect::<Vec<_>>()
        ))),
        ProjectCommand::Archive { id } => {
            mutate(
                cli,
                service,
                json!({"command":"project.archive","project":id}),
            )
            .await
        }
        ProjectCommand::Unarchive { id } => {
            mutate(
                cli,
                service,
                json!({"command":"project.unarchive","project":id}),
            )
            .await
        }
        ProjectCommand::Show { id } => {
            Ok(response(json!(service.store().project_result(id).await?)))
        }
    }
}

async fn resolve_project(cli: &Cli, service: &Service) -> Result<String> {
    if let Some(id) = &cli.project {
        return Ok(service.store().project_result(id).await?.id);
    }
    let cwd = std::env::current_dir()?;
    let check = Process::new("git")
        .arg("-C")
        .arg(&cwd)
        .args(["rev-parse", "--git-dir"])
        .output()?;
    ensure!(
        check.status.success(),
        "--project is required outside a Git repository"
    );
    let (root, _) = git_identity(&cwd)?;
    let root = root.to_string_lossy();
    service
        .store()
        .project_by_root(&root)
        .await?
        .map(|p| p.id)
        .context("register this project first with taskix project register, or specify --project")
}

async fn job(cli: &Cli, service: &Service, action: &JobCommand) -> Result<Value> {
    match action {
        JobCommand::Followup {
            id,
            prompt,
            inbox_ids,
        } => {
            mutate(
                cli,
                service,
                json!({"command":"job.followup","job":id,"prompt":prompt,"inbox_ids":inbox_ids}),
            )
            .await
        }
        JobCommand::Submit { id } => {
            mutate(cli, service, json!({"command":"job.submit","job":id})).await
        }
        JobCommand::Approve { id } => {
            mutate(cli, service, json!({"command":"job.approve","job":id})).await
        }
        JobCommand::Reject(args) => {
            mutate(
                cli,
                service,
                json!({"command":"job.reject","job":args.id,"reason":args.reason}),
            )
            .await
        }
        JobCommand::Delete { id } => {
            mutate(cli, service, json!({"command":"job.delete","job":id})).await
        }
        JobCommand::Create {
            title,
            goal,
            name,
            prompt,
            inbox_ids,
            review_policy,
        } => {
            let project = resolve_project(cli, service).await?;
            mutate(
                cli,
                service,
                json!({"command":"job.create","project":project,"title":title,"goal":goal,"name":name,"prompt":prompt,"inbox_ids":inbox_ids,"review_policy":review_policy}),
            )
            .await
        }
        JobCommand::Update {
            id,
            title,
            goal,
            name,
            prompt,
            inbox_ids,
            review_policy,
        } => {
            let mut request = json!({"command":"job.update","job":id});
            if !inbox_ids.is_empty() {
                request["inbox_ids"] = json!(inbox_ids);
            }
            if let Some(review_policy) = review_policy {
                request["review_policy"] = json!(review_policy);
            }
            if let Some(name) = name {
                request["name"] = json!(name);
            }
            if let Some(title) = title {
                request["title"] = json!(title);
            }
            if let Some(prompt) = prompt {
                request["prompt"] = json!(prompt);
            }
            if let Some(goal) = goal {
                request["goal"] = json!(goal);
            }
            mutate(cli, service, request).await
        }
        JobCommand::List(filters) => list_jobs(cli, service, filters).await,
        JobCommand::Show { id } => Ok(response(json!(service.store().job_record(id).await?))),
        JobCommand::Cancel { id } => {
            mutate(cli, service, json!({"command":"job.cancel","job":id})).await
        }
        JobCommand::Archive { id } => {
            mutate(cli, service, json!({"command":"job.archive","job":id})).await
        }
        JobCommand::Unarchive { id } => {
            mutate(cli, service, json!({"command":"job.unarchive","job":id})).await
        }
    }
}

async fn list_jobs(cli: &Cli, service: &Service, filters: &JobList) -> Result<Value> {
    let from = filters.created_from.as_ref().map(|s| date(s)).transpose()?;
    let to = filters.created_to.as_ref().map(|s| date(s)).transpose()?;
    if let Some(period) = &filters.period {
        ensure!(
            period.len() == 7 && date(&format!("{period}-01")).is_ok(),
            "period must be YYYY-MM"
        );
    }
    let (archived_from, archived_before) = if let Some(period) = &filters.period {
        let start = date(&format!("{period}-01"))?;
        let day = time::OffsetDateTime::from_unix_timestamp(start)?;
        // Preserve the existing unpadded-year formatting for early dates.
        let days = if format_date(start).starts_with(period) {
            i64::from(time::util::days_in_month(day.month(), day.year()))
        } else {
            0
        };
        (Some(start), Some(start + days * 86400))
    } else {
        (None, None)
    };
    let filter = agentix_task::JobFilter {
        status: if filters.active {
            Some(JobStatus::Active)
        } else if filters.pending_review {
            Some(JobStatus::PendingReview)
        } else if filters.completed {
            Some(JobStatus::Completed)
        } else {
            None
        },
        archived: if filters.active || filters.pending_review {
            Some(false)
        } else if filters.archived {
            Some(true)
        } else {
            None
        },
        created_from: from,
        created_before: to.map(|timestamp| timestamp + 86400),
        archived_from,
        archived_before,
    };
    let jobs = service
        .store()
        .filtered_jobs(cli.project.as_deref(), &filter)
        .await?;
    Ok(response(json!(jobs)))
}

async fn task(cli: &Cli, service: &Service, action: &TaskCommand) -> Result<Value> {
    let request = match action {
        TaskCommand::Add { job, title, name } => {
            json!({"command":"task.add","job":job,"title":title,"name":name})
        }
        TaskCommand::Update {
            id,
            name,
            title,
            position,
        } => {
            let mut r = json!({"command":"task.update","task":id});
            if let Some(name) = name {
                r["name"] = json!(name);
            }
            if let Some(t) = title {
                r["title"] = json!(t);
            }
            if let Some(p) = position {
                r["position"] = json!(p);
            }
            r
        }
        TaskCommand::List { job, ready, status } => {
            let tasks = service
                .store()
                .tasks(
                    job.as_deref(),
                    cli.project.as_deref(),
                    status.as_deref(),
                    *ready,
                )
                .await?;
            return Ok(response(json!(tasks)));
        }
        TaskCommand::Show(args) => {
            return Ok(response(service.store().task_result(&args.id).await?));
        }
        TaskCommand::Depend { id, dependency } => {
            json!({"command":"task.depend","task":id,"dependency":dependency})
        }
        TaskCommand::Undepend { id, dependency } => {
            json!({"command":"task.undepend","task":id,"dependency":dependency})
        }
        TaskCommand::Claim(args) => {
            json!({"command":"task.claim","task":args.id,"executor":cli.executor.as_ref().context("claim requires --executor")?,"session":cli.session.as_ref().context("claim requires --session")?,"delegated_by":cli.delegated_by})
        }
        TaskCommand::Heartbeat(args) => json!({"command":"task.heartbeat","task":args.id}),
        TaskCommand::Start(args) => json!({"command":"task.start","task":args.id}),
        TaskCommand::Done(args) => json!({"command":"task.done","task":args.id}),
        TaskCommand::Cancel(args) => json!({"command":"task.cancel","task":args.id}),
        TaskCommand::Retry(args) => json!({"command":"task.retry","task":args.id}),
        TaskCommand::Reopen(args) => json!({"command":"task.reopen","task":args.id}),
        TaskCommand::Block(args) => {
            json!({"command":"task.block","task":args.id,"reason":args.reason})
        }
        TaskCommand::Wait(args) => {
            json!({"command":"task.wait","task":args.id,"reason":args.reason})
        }
        TaskCommand::Fail(args) => {
            json!({"command":"task.fail","task":args.id,"reason":args.reason})
        }
        TaskCommand::Release(args) => {
            json!({"command":"task.release","task":args.id,"reason":args.reason})
        }
    };
    mutate(cli, service, request).await
}

async fn plan(cli: &Cli, service: &Service, action: &PlanCommand) -> Result<Value> {
    let (command, args) = match action {
        PlanCommand::Create(args) => ("plan.create", args),
        PlanCommand::Revise(args) => ("plan.revise", args),
        PlanCommand::Show { task } => return Ok(response(service.plan(task).await?)),
    };
    let body = match (&args.body, &args.file) {
        (Some(body), _) => body.clone(),
        (_, Some(path)) => std::fs::read_to_string(path)?,
        _ => bail!("Plan body is required"),
    };
    mutate(
        cli,
        service,
        json!({"command":command,"task":args.task,"body":body}),
    )
    .await
}

async fn context(
    cli: &Cli,
    service: &Service,
    task: Option<&str>,
    job: Option<&str>,
) -> Result<Value> {
    let mut value = context_snapshot(cli, service, task, job).await?;
    let mut todos = Vec::new();
    if let Some(project) = value["result"]["project_id"].as_str() {
        let outcome = service
            .execute(
                json!({"command":"inbox.list","project":project}),
                WriteOptions::default(),
            )
            .await?;
        ensure!(
            outcome.projection_pending.is_none(),
            "Inbox synchronization pending: {:?}",
            outcome.projection_pending
        );
        todos = outcome
            .result
            .as_array()
            .context("invalid: Inbox list response")?
            .iter()
            .filter(|entry| entry["status"] == "TODO")
            .cloned()
            .collect();
        // Import can cancel work and revoke leases; return the refreshed assignment.
        value = context_snapshot(cli, service, task, job).await?;
    }
    value["result"]["inbox_todos"] = json!(todos);
    Ok(value)
}

async fn context_snapshot(
    cli: &Cli,
    service: &Service,
    task: Option<&str>,
    job: Option<&str>,
) -> Result<Value> {
    let state = service
        .store()
        .context_snapshot(task, job, cli.session.as_deref())
        .await?;
    let task = if let Some(id) = task {
        Some(&state.tasks[state.task_index(id)?])
    } else {
        state
            .leases
            .iter()
            .find(|l| cli.session.as_deref() == Some(l.session_ref.as_str()))
            .and_then(|l| state.tasks.iter().find(|t| t.id == l.task_id))
    };
    let owned_inbox = state.inboxes.iter().find(|e| {
        e.lease
            .as_ref()
            .is_some_and(|l| cli.session.as_deref() == Some(l.session_ref.as_str()))
    });
    let job = job
        .map(|id| state.job_index(id).map(|i| &state.jobs[i]))
        .transpose()?
        .or_else(|| task.and_then(|t| state.jobs.iter().find(|j| j.id == t.job_id)))
        .or_else(|| {
            owned_inbox.and_then(|e| state.jobs.iter().find(|j| Some(&j.id) == e.job_id.as_ref()))
        });
    let project = if let Some(job) = job {
        Some(state.projects[state.project_index(&job.project_id)?].clone())
    } else if let Some(id) = &cli.project {
        Some(service.store().project_result(id).await?)
    } else {
        let cwd = std::env::current_dir()?;
        match service
            .project_for_session(Some(&cwd), cli.session.as_deref())
            .await?
        {
            Some(project) => Some(project),
            None => {
                service
                    .project_for_session(None, cli.session.as_deref())
                    .await?
            }
        }
    };
    let previous_job = if job.is_none() {
        if let Some((session, project)) = cli.session.as_deref().zip(project.as_ref()) {
            service.store().previous_job(session, &project.id).await?
        } else {
            None
        }
    } else {
        None
    };
    let inbox_path = project
        .as_ref()
        .map(|p| service.inbox_path(p))
        .transpose()?;
    let cancellations = cli.session.as_deref().map_or_else(Vec::new, |session| {
        state.cancelled_inboxes_for_session(session)
    });
    let plan = task.and_then(|t| {
        state
            .plans
            .iter()
            .find(|p| Some(&p.id) == t.current_plan.as_ref())
    });
    let lease = task.and_then(|t| state.leases.iter().find(|l| l.task_id == t.id));
    Ok(response(
        json!({"previous_job":previous_job,"project_id":project.map(|p|p.id),"job_id":job.map(|j|&j.id),"task_id":task.map(|t|&t.id),"task":task,"lease":lease,"plan_path":plan.map(|p|service.config().output_dir().join(&p.path)),"documents":service.config().documents,"context_owner":"external_agent_team","editable_regions":["Goal","Notes","Plan body"],"inbox":owned_inbox,"inbox_path":inbox_path,"inbox_cancellations":cancellations}),
    ))
}

async fn hook(cli: &Cli, service: &Service, action: &HookCommand) -> Result<Value> {
    let mut input = String::new();
    if cli.session.is_none() && !std::io::stdin().is_terminal() {
        std::io::stdin().read_to_string(&mut input)?;
    }
    let event: Value = if input.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&input)?
    };
    let session = cli
        .session
        .as_deref()
        .or_else(|| event["session_id"].as_str())
        .context("hook requires session_id on stdin or --session")?;
    let command = match action {
        HookCommand::Record { file, job } => {
            let messages: Value = serde_json::from_slice(&std::fs::read(file)?)?;
            let mut request =
                json!({"command":"session.record","session":session,"messages":messages});
            if let Some(job) = job {
                request["job"] = json!(job);
            }
            let mut options = cli.options();
            options.session_ref = Some(session.into());
            let outcome = service.execute(request, options).await?;
            return Ok(
                json!({"schema_version":1,"ok":true,"result":outcome.result,"sequence":outcome.sequence,"projection_pending":outcome.projection_pending}),
            );
        }
        // Keep the legacy entrypoint safe for installed plugins that still call it.
        HookCommand::Stop => {
            return Ok(response(
                json!({"claimed":false,"reason":"manual_intake_required"}),
            ));
        }
        HookCommand::SessionStart => "session.start",
        HookCommand::SessionEnd => "session.end",
        HookCommand::Interrupt => "session.interrupt",
        HookCommand::Heartbeat => "session.heartbeat",
    };
    mutate(cli, service, json!({"command":command,"session":session})).await
}

async fn inbox(cli: &Cli, service: &Service, action: &InboxCommand) -> Result<Value> {
    let request = match action {
        InboxCommand::SetStatus { id, status } => {
            json!({"command":"inbox.set-status","inbox":id,"status":status})
        }
        InboxCommand::Cancel { id } => json!({"command":"inbox.cancel","inbox":id}),
        InboxCommand::Release { id } => json!({"command":"inbox.release","inbox":id}),
        _ => {
            let project = resolve_project(cli, service).await?;
            match action {
                InboxCommand::Add { content } => {
                    json!({"command":"inbox.add","project":project,"content":content})
                }
                InboxCommand::List => json!({"command":"inbox.list","project":project}),
                InboxCommand::Sync => json!({"command":"inbox.sync","project":project}),
                InboxCommand::ClaimNext => json!({"command":"inbox.claim-next","project":project}),
                _ => unreachable!(),
            }
        }
    };
    mutate(cli, service, request).await
}

fn date(value: &str) -> Result<i64> {
    let format = time::format_description::parse_borrowed::<2>("[year]-[month]-[day]")?;
    Ok(time::Date::parse(value, &format)?
        .midnight()
        .assume_utc()
        .unix_timestamp())
}
fn format_date(timestamp: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(timestamp)
        .map(|d| format!("{}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn previous_job(
        state: &agentix_task::Snapshot,
        session: &str,
        project: &str,
    ) -> Option<Value> {
        use sqlx::Connection;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.sqlite3");
        let store = agentix_task::Store::open(&path).await.unwrap();
        let mut conn = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO projects(id,data) VALUES(?,'{}')")
            .bind(project)
            .execute(&mut conn)
            .await
            .unwrap();
        for job in &state.jobs {
            sqlx::query("INSERT INTO jobs(id,data) VALUES(?,?)")
                .bind(&job.id)
                .bind(json!(job).to_string())
                .execute(&mut conn)
                .await
                .unwrap();
        }
        store.previous_job(session, project).await.unwrap()
    }

    #[tokio::test]
    async fn previous_job_orders_same_second_followups_and_ignores_late_replies() {
        let make_job = |id: &str, created_at: i64| {
            json!({
                "id": id, "project_id": "project", "title": "Work", "goal": "",
                "status": "PENDING_REVIEW", "revision": 1, "created_at": created_at,
                "updated_at": created_at, "document_path": "job.md", "session_id": "session"
            })
        };
        let mut old = make_job("job_001", 1);
        old["followup_session_id"] = json!("session");
        old["followup_at"] = json!(100);
        old["followup_id"] = json!("message_003");
        old["conversation"] = json!([{
            "id": "followup:message_003", "session_id": "session", "role": "user",
            "text": "Supplement", "recorded_at": 100
        }]);
        let mut state: agentix_task::Snapshot = serde_json::from_value(json!({
            "projects": [], "jobs": [old, make_job("job_002", 100)],
            "tasks": [], "plans": [], "leases": []
        }))
        .unwrap();
        assert_eq!(
            previous_job(&state, "session", "project").await.unwrap()["id"],
            "job_001"
        );
        state
            .jobs
            .push(serde_json::from_value(make_job("job_004", 100)).unwrap());
        assert_eq!(
            previous_job(&state, "session", "project").await.unwrap()["id"],
            "job_004"
        );
        state.jobs[0].updated_at = 101;
        state.jobs[0].conversation.push(agentix_task::JobMessage {
            id: "reply_005".into(),
            session_id: "session".into(),
            role: "assistant".into(),
            text: "Late reply".into(),
            recorded_at: 101,
        });
        assert_eq!(
            previous_job(&state, "session", "project").await.unwrap()["id"],
            "job_004"
        );
    }

    #[test]
    fn every_subcommand_has_a_description_in_short_and_long_help() {
        fn check(command: &mut clap::Command, path: &str, missing: &mut Vec<String>) {
            let about = command.get_about().map(ToString::to_string);
            if let Some(about) = about.filter(|text| !text.trim().is_empty()) {
                assert!(
                    command.render_help().to_string().contains(&about),
                    "{path}: description missing from short help"
                );
                assert!(
                    command.render_long_help().to_string().contains(&about),
                    "{path}: description missing from long help"
                );
            } else {
                missing.push(path.to_owned());
            }
            for child in command.get_subcommands_mut() {
                if child.get_name() != "help" {
                    check(child, &format!("{path} {}", child.get_name()), missing);
                }
            }
        }

        let mut missing = Vec::new();
        check(&mut Cli::command(), "taskix", &mut missing);
        assert!(missing.is_empty(), "missing descriptions: {missing:#?}");
    }
}
