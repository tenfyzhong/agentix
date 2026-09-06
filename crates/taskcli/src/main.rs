use std::{
    io::{IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::{Command as Process, ExitCode},
};

use agentix_task::{
    Config, DocumentConfig, DocumentFormat, JobStatus, Service, StorageConfig, WriteOptions,
    expand_home,
};
use anyhow::{Context, Result, bail, ensure};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use serde_json::{Value, json};

#[derive(Parser)]
#[command(
    version,
    about = "Coordinate agent tasks with SQLite and read-only Markdown boards"
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
    /// Print a shell completion script without loading task configuration.
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
    Init(Init),
    Doctor,
    Sync,
    Project {
        #[command(subcommand)]
        action: ProjectCommand,
    },
    Job {
        #[command(subcommand)]
        action: JobCommand,
    },
    Task {
        #[command(subcommand)]
        action: TaskCommand,
    },
    Plan {
        #[command(subcommand)]
        action: PlanCommand,
    },
    Event {
        #[command(subcommand)]
        action: EventCommand,
    },
    Context {
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        job: Option<String>,
    },
    Hook {
        #[command(subcommand)]
        action: HookCommand,
    },
}

#[derive(Args)]
struct Init {
    #[arg(long, value_enum)]
    format: Format,
    #[arg(long, value_hint = clap::ValueHint::DirPath)]
    root: PathBuf,
    #[arg(long, default_value = ".", value_hint = clap::ValueHint::DirPath)]
    directory: PathBuf,
    #[arg(long, value_hint = clap::ValueHint::FilePath)]
    database: Option<PathBuf>,
}
#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Obsidian,
    Markdown,
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// Delete the Project, its work, and its entire generated document directory.
    Delete {
        id: String,
    },
    Register {
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_hint = clap::ValueHint::DirPath)]
        root: Option<PathBuf>,
    },
    List {
        #[arg(long)]
        archived: bool,
    },
    Show {
        id: String,
    },
    Archive {
        id: String,
    },
    Unarchive {
        id: String,
    },
}

#[derive(Args)]
struct JobList {
    #[arg(long,conflicts_with_all=["completed","archived"])]
    active: bool,
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
    /// Delete the Job, its Tasks, and their Plan documents.
    Delete {
        id: String,
    },
    Create {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        title: String,
        #[arg(long, default_value = "")]
        goal: String,
    },
    Update {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        goal: Option<String>,
    },
    List(JobList),
    Show {
        id: String,
    },
    Cancel {
        id: String,
    },
    Archive {
        id: String,
    },
    Unarchive {
        id: String,
    },
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
    Add {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        job: String,
        #[arg(long)]
        title: String,
    },
    Update {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        position: Option<i64>,
    },
    List {
        #[arg(long)]
        job: Option<String>,
        #[arg(long)]
        ready: bool,
        #[arg(long)]
        status: Option<String>,
    },
    Show(TaskId),
    Depend {
        id: String,
        dependency: String,
    },
    Undepend {
        id: String,
        dependency: String,
    },
    Claim(TaskId),
    Start(TaskId),
    Heartbeat(TaskId),
    Release(Reason),
    Block(Reason),
    Wait(Reason),
    Fail(Reason),
    Done(TaskId),
    Cancel(TaskId),
    Retry(TaskId),
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
    Create(PlanBody),
    Revise(PlanBody),
    Show { task: String },
}
#[derive(Subcommand)]
enum EventCommand {
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
    SessionStart,
    SessionEnd,
    Interrupt,
    Heartbeat,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Command::Completions { shell } = &cli.command {
        clap_complete::generate(
            *shell,
            &mut Cli::command(),
            "taskcli",
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
                eprintln!("taskcli: {message}");
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
            .or_else(|| std::env::var_os("TASKCLI_CONFIG").map(PathBuf::from));
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

async fn run(cli: &Cli) -> Result<Value> {
    if let Command::Init(init) = &cli.command {
        return initialize(cli, init).await;
    }
    let service = Service::open(Config::load(&cli.config_path()?)?).await?;
    service.store().reap_expired().await?;
    match &cli.command {
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
            let healthy = missing.is_empty() && rendered >= sequence;
            Ok(response(
                json!({"healthy":healthy,"missing_plans":missing,"sequence":sequence,"rendered_sequence":rendered,"documents":service.config().documents}),
            ))
        }
        Command::Sync => {
            service.sync().await?;
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
        Command::Init(_) | Command::Completions { .. } => unreachable!(),
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
                    .unwrap_or_else(|| PathBuf::from("~/.local/share/taskcli/tasks.sqlite3")),
            )?,
        },
        documents: DocumentConfig {
            format: match init.format {
                Format::Obsidian => DocumentFormat::Obsidian,
                Format::Markdown => DocumentFormat::Markdown,
            },
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
                .snapshot()
                .await?
                .projects
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
            let state = service.store().snapshot().await?;
            Ok(response(json!(state.projects[state.project_index(id)?])))
        }
    }
}

fn git_identity(root: &Path) -> Result<(PathBuf, Option<String>)> {
    let root = root.canonicalize()?;
    let output = Process::new("git")
        .arg("-C")
        .arg(&root)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output();
    if let Ok(output) = output
        && output.status.success()
    {
        let common = PathBuf::from(String::from_utf8(output.stdout)?.trim());
        let common = common.canonicalize()?;
        let root = common
            .parent()
            .context("Git common directory has no parent")?
            .to_owned();
        let remote = Process::new("git")
            .arg("-C")
            .arg(&root)
            .args(["remote", "get-url", "origin"])
            .output()?;
        let remote = remote
            .status
            .success()
            .then(|| String::from_utf8_lossy(&remote.stdout).trim().to_owned());
        Ok((root, remote))
    } else {
        Ok((root, None))
    }
}

async fn resolve_project(cli: &Cli, service: &Service) -> Result<String> {
    let state = service.store().snapshot().await?;
    if let Some(id) = &cli.project {
        return Ok(state.projects[state.project_index(id)?].id.clone());
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
    state
        .projects
        .iter()
        .find(|p| p.root == root)
        .map(|p| p.id.clone())
        .context("register this project first with taskcli project register, or specify --project")
}

async fn job(cli: &Cli, service: &Service, action: &JobCommand) -> Result<Value> {
    match action {
        JobCommand::Delete { id } => {
            mutate(cli, service, json!({"command":"job.delete","job":id})).await
        }
        JobCommand::Create { title, goal, name } => {
            let project = resolve_project(cli, service).await?;
            mutate(
                cli,
                service,
                json!({"command":"job.create","project":project,"title":title,"goal":goal,"name":name}),
            )
            .await
        }
        JobCommand::Update {
            id,
            title,
            goal,
            name,
        } => {
            let mut request = json!({"command":"job.update","job":id});
            if let Some(name) = name {
                request["name"] = json!(name);
            }
            if let Some(title) = title {
                request["title"] = json!(title);
            }
            if let Some(goal) = goal {
                request["goal"] = json!(goal);
            }
            mutate(cli, service, request).await
        }
        JobCommand::List(filters) => {
            let state = service.store().snapshot().await?;
            let pid = cli
                .project
                .as_ref()
                .map(|p| state.project_index(p).map(|i| state.projects[i].id.clone()))
                .transpose()?;
            let from = filters.created_from.as_ref().map(|s| date(s)).transpose()?;
            let to = filters.created_to.as_ref().map(|s| date(s)).transpose()?;
            if let Some(period) = &filters.period {
                ensure!(
                    period.len() == 7 && date(&format!("{period}-01")).is_ok(),
                    "period must be YYYY-MM"
                );
            }
            let jobs: Vec<_> = state
                .jobs
                .into_iter()
                .filter(|j| pid.as_ref().is_none_or(|p| *p == j.project_id))
                .filter(|j| {
                    !filters.active || (j.status == JobStatus::Active && j.archived_at.is_none())
                })
                .filter(|j| !filters.completed || j.status == JobStatus::Completed)
                .filter(|j| !filters.archived || j.archived_at.is_some())
                .filter(|j| {
                    from.is_none_or(|v| j.created_at >= v)
                        && to.is_none_or(|v| j.created_at < v + 86400)
                })
                .filter(|j| {
                    filters.period.as_ref().is_none_or(|p| {
                        j.archived_at.is_some_and(|t| format_date(t).starts_with(p))
                    })
                })
                .collect();
            Ok(response(json!(jobs)))
        }
        JobCommand::Show { id } => {
            let state = service.store().snapshot().await?;
            Ok(response(json!(state.jobs[state.job_index(id)?])))
        }
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
            let state = service.store().snapshot().await?;
            let jid = job
                .as_ref()
                .map(|j| state.job_index(j).map(|i| state.jobs[i].id.clone()))
                .transpose()?;
            let pid = cli
                .project
                .as_ref()
                .map(|p| state.project_index(p).map(|i| state.projects[i].id.clone()))
                .transpose()?;
            if let Some(status) = status {
                ensure!(
                    agentix_task::TaskStatus::ALL
                        .iter()
                        .any(|s| s.to_string() == *status),
                    "invalid task status"
                );
            }
            let tasks: Vec<_> = state
                .tasks
                .iter()
                .filter(|t| {
                    jid.as_ref().is_none_or(|j| *j == t.job_id)
                        && pid.as_ref().is_none_or(|p| *p == t.project_id)
                })
                .filter(|t| status.as_ref().is_none_or(|s| t.status.to_string() == *s))
                .filter(|t| {
                    !*ready
                        || (t.status == agentix_task::TaskStatus::Todo
                            && t.dependencies.iter().all(|d| {
                                state.tasks.iter().any(|x| {
                                    x.id == *d && x.status == agentix_task::TaskStatus::Done
                                })
                            }))
                })
                .collect();
            return Ok(response(json!(tasks)));
        }
        TaskCommand::Show(args) => {
            return Ok(response(
                service.store().snapshot().await?.task_result(&args.id)?,
            ));
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
    let state = service.store().snapshot().await?;
    let task = if let Some(id) = task {
        Some(&state.tasks[state.task_index(id)?])
    } else {
        state
            .leases
            .iter()
            .find(|l| cli.session.as_deref() == Some(l.session_ref.as_str()))
            .and_then(|l| state.tasks.iter().find(|t| t.id == l.task_id))
    };
    let job = job
        .map(|id| state.job_index(id).map(|i| &state.jobs[i]))
        .transpose()?
        .or_else(|| task.and_then(|t| state.jobs.iter().find(|j| j.id == t.job_id)));
    let plan = task.and_then(|t| {
        state
            .plans
            .iter()
            .find(|p| Some(&p.id) == t.current_plan.as_ref())
    });
    let lease = task.and_then(|t| state.leases.iter().find(|l| l.task_id == t.id));
    Ok(response(
        json!({"project_id":job.map(|j|&j.project_id),"job_id":job.map(|j|&j.id),"task_id":task.map(|t|&t.id),"task":task,"lease":lease,"plan_path":plan.map(|p|service.config().output_dir().join(&p.path)),"documents":service.config().documents,"context_owner":"external_agent_team","editable_regions":["Goal","Notes","Plan body"]}),
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
        HookCommand::SessionStart => "session.start",
        HookCommand::SessionEnd => "session.end",
        HookCommand::Interrupt => "session.interrupt",
        HookCommand::Heartbeat => "session.heartbeat",
    };
    mutate(cli, service, json!({"command":command,"session":session})).await
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
