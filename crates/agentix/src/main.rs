mod control;
mod control_runtime;
use agentix::run_engine_loop;
use control_runtime::run_control_handler;
#[cfg(all(test, unix))]
mod native_control_tests;
#[cfg(test)]
mod proxy_tests;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agentix::{
    AgentConfig, Config, ImChannel, LogRotation, LoggingConfig, NetworkConfig, TelegramConfig,
    add_feishu_owner, add_telegram_owner,
};
use agentix_bridge::{BridgeAdapter, BridgeHub, BridgeKind};
use agentix_codex::{CodexClient, CodexEndpoint};
use agentix_core::OwnerClaimer;
use agentix_core::{AgentAdapter, AgentError, ChannelAdapter, Engine, SqliteState};
use agentix_feishu::FeishuAdapter;
use agentix_telegram::{TelegramAdapter, TelegramOwnerClaimer, TelegramPolicy};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder as RollingBuilder, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(version, about = "Control local coding-agent sessions from IM")]
struct Cli {
    #[arg(short, long, value_name = "FILE", value_hint = clap::ValueHint::FilePath, default_value_os_t = default_config_path())]
    config: PathBuf,
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Run the Agentix bridge until interrupted.
    Serve {
        #[command(flatten)]
        proxy: agentix_codex::ProxyOptions,
    },
    /// Validate configuration, credentials, and the selected agent transport.
    Doctor,
    /// Use the running Agentix server for local diagnostics and setup.
    Client {
        #[command(subcommand)]
        command: ClientCommand,
    },
    /// Print a shell completion script to stdout.
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Debug, Subcommand)]
enum ClientCommand {
    /// Generate a temporary in-memory owner claim code for the selected IM channel.
    Claim {
        #[arg(long, default_value_t = 10)]
        ttl_minutes: u64,
    },
    /// List sessions available through the running Agentix server.
    Sessions {
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long, default_value_t = 25)]
        limit: u32,
    },
    /// Send a prompt to an original session, or steer its active turn.
    Send {
        session: String,
        text: String,
        #[arg(long)]
        turn: Option<String>,
    },
    /// Stop a turn in an original session.
    Stop { session: String, turn: String },
    /// Read a session's conversation history.
    History {
        session: String,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Run a typed session command supplied as JSON.
    Command {
        session: String,
        #[arg(value_name = "JSON")]
        command: String,
    },
    /// Ask the server to send a raw JSON RPC request to Codex.
    Call {
        method: String,
        #[arg(long, value_name = "JSON", default_value = "{}")]
        params: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let Cli {
        config: config_path,
        command,
    } = Cli::parse();
    if let CliCommand::Completions { shell } = command {
        clap_complete::generate(
            shell,
            &mut Cli::command(),
            "agentix",
            &mut std::io::stdout(),
        );
        return Ok(());
    }
    let mut config = Config::load(&config_path)?;
    let _log_guard = init_logging(&config.logging)?;
    match command {
        CliCommand::Serve { proxy } => {
            config.apply_codex_proxy_options(&proxy)?;
            serve(config, &config_path).await
        }
        CliCommand::Doctor => doctor(&config).await,
        CliCommand::Client { command } => client(&config.server.endpoint, command).await,
        CliCommand::Completions { .. } => {
            unreachable!("completions are generated before loading config")
        }
    }
}

fn init_logging(config: &LoggingConfig) -> Result<Option<WorkerGuard>> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&config.level))
        .context("logging.level is not a valid tracing filter")?;
    let stderr = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_timer(local_time_timer());

    if !config.file.enabled {
        tracing_subscriber::registry()
            .with(filter)
            .with(stderr)
            .try_init()
            .context("failed to initialize logging")?;
        return Ok(None);
    }

    let parent = config
        .file
        .path
        .parent()
        .context("logging.file.path has no parent directory")?;
    let file_name = config
        .file
        .path
        .file_name()
        .context("logging.file.path must include a file name")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create log directory {}", parent.display()))?;
    let rotation = match config.file.rotation {
        LogRotation::Never => Rotation::NEVER,
        LogRotation::Minutely => Rotation::MINUTELY,
        LogRotation::Hourly => Rotation::HOURLY,
        LogRotation::Daily => Rotation::DAILY,
    };
    let appender = RollingBuilder::new()
        .rotation(rotation)
        .filename_prefix(file_name.to_string_lossy())
        .max_log_files(config.file.max_files)
        .build(parent)
        .context("failed to initialize rolling file logging")?;
    let (file_writer, guard) = tracing_appender::non_blocking(appender);
    let file = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(file_writer)
        .with_timer(local_time_timer());
    tracing_subscriber::registry()
        .with(filter)
        .with(stderr)
        .with(file)
        .try_init()
        .context("failed to initialize logging")?;
    Ok(Some(guard))
}

fn local_time_timer() -> impl tracing_subscriber::fmt::time::FormatTime {
    tracing_subscriber::fmt::time::LocalTime::rfc_3339()
}

async fn client(endpoint: &str, command: ClientCommand) -> Result<()> {
    match command {
        ClientCommand::Claim { ttl_minutes } => {
            let result =
                control::request(endpoint, &control::ControlRequest::Claim { ttl_minutes }).await?;
            let command = result
                .get("command")
                .and_then(Value::as_str)
                .context("Agentix claim response did not contain a command")?;
            println!("{command}");
            println!("Valid for {ttl_minutes} minute(s). The code is only shown in this terminal.");
            Ok(())
        }
        ClientCommand::Sessions { cursor, limit } => {
            let result = control::request(
                endpoint,
                &control::ControlRequest::Sessions { cursor, limit },
            )
            .await?;
            print_json(&result)
        }
        ClientCommand::Send {
            session,
            text,
            turn,
        } => {
            session_request(
                endpoint,
                agentix_core::SessionOperation::Send {
                    session: agentix_core::SessionId::new(session),
                    text,
                    expected_turn: turn,
                },
            )
            .await
        }
        ClientCommand::Stop { session, turn } => {
            session_request(
                endpoint,
                agentix_core::SessionOperation::Stop {
                    session: agentix_core::SessionId::new(session),
                    turn,
                },
            )
            .await
        }
        ClientCommand::History {
            session,
            cursor,
            limit,
        } => {
            session_request(
                endpoint,
                agentix_core::SessionOperation::History {
                    session: agentix_core::SessionId::new(session),
                    cursor,
                    limit,
                },
            )
            .await
        }
        ClientCommand::Command { session, command } => {
            let command = serde_json::from_str(&command)
                .context("command must be a valid session command JSON object")?;
            session_request(
                endpoint,
                agentix_core::SessionOperation::Command {
                    session: agentix_core::SessionId::new(session),
                    command,
                },
            )
            .await
        }
        ClientCommand::Call { method, params } => {
            let params: Value =
                serde_json::from_str(&params).with_context(|| "--params must be valid JSON")?;
            let result =
                control::request(endpoint, &control::ControlRequest::Call { method, params })
                    .await?;
            print_json(&result)
        }
    }
}

async fn session_request(endpoint: &str, operation: agentix_core::SessionOperation) -> Result<()> {
    let result = control::request(endpoint, &control::ControlRequest::Session(operation)).await?;
    print_json(&result)
}

fn unix_timestamp() -> Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs())
}

fn print_json(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

async fn build_task_board(config: &Config) -> Result<Option<Arc<agentix_task::Service>>> {
    let service = if let Some(task_board) = config.enabled_task_board() {
        let task_config = agentix_task::Config::load(&task_board.config)?;
        Some(Arc::new(agentix_task::Service::open(task_config).await?))
    } else {
        None
    };
    Ok(service)
}

fn retryable_backend_error(error: anyhow::Error) -> Result<anyhow::Error> {
    if agentix_codex::is_fatal_startup_error(&error) {
        tracing::error!(%error, "Codex proxy_endpoint startup failed; exiting");
        return Err(error);
    }
    Ok(error)
}

async fn serve(config: Config, config_path: &Path) -> Result<()> {
    let started = Instant::now();
    let bridge_hub = if config
        .selected_agents()
        .iter()
        .any(|agent| !matches!(agent, AgentConfig::Codex { .. }))
    {
        Some(Arc::new(BridgeHub::new()))
    } else {
        None
    };
    let mut agents = Vec::new();
    let mut codex = None;
    for agent in config.selected_agents() {
        let built = match build_agent(
            agent,
            config.notifications.background_turns,
            bridge_hub.clone(),
        )
        .await
        {
            Ok(built) => built,
            Err(error) => {
                let error = retryable_backend_error(error)?;
                tracing::warn!(%error, backend = agent.kind().as_str(), "backend startup failed; continuing with retries");
                let retry = agent.clone();
                let bridge_hub = bridge_hub.clone();
                let background = config.notifications.background_turns;
                let directory = match agent {
                    AgentConfig::Codex { rmux_directory, .. }
                    | AgentConfig::Claude { rmux_directory, .. }
                    | AgentConfig::Pi { rmux_directory, .. }
                    | AgentConfig::OhMyPi { rmux_directory, .. } => {
                        rmux_directory.to_string_lossy().into_owned()
                    }
                };
                let adapter = agentix_core::DeferredAgent::new(
                    agent.kind().display_name(),
                    directory,
                    move || {
                        let retry = retry.clone();
                        let bridge_hub = bridge_hub.clone();
                        async move {
                            build_agent(&retry, background, bridge_hub)
                                .await
                                .map(|built| built.adapter)
                                .map_err(|error| AgentError::Unavailable(error.to_string()))
                        }
                    },
                );
                BuiltAgent {
                    adapter: Arc::new(adapter),
                    codex: None,
                }
            }
        };
        if built.codex.is_some() {
            codex = built.codex;
        }
        agents.push((agent.kind(), built.adapter));
    }
    let adapter: Arc<dyn AgentAdapter> = Arc::new(agentix_core::AgentRegistry::new(agents)?);
    tracing::info!(
        phase = "agent_connection",
        elapsed_ms = started.elapsed().as_millis(),
        "startup phase completed"
    );
    let started = Instant::now();
    let claims = Arc::new(ClaimRegistry::default());
    let channels = build_channels(&config, config_path, claims.clone())?;
    let task_board = build_task_board(&config).await?;
    tracing::info!(
        phase = "channel_and_task_setup",
        elapsed_ms = started.elapsed().as_millis(),
        "startup phase completed"
    );
    run_service_until_shutdown(
        adapter,
        codex,
        channels,
        task_board,
        config.storage.path,
        config.server.endpoint,
        bridge_hub,
        config_path.to_owned(),
        claims,
        config.notifications.background_turns,
        config.output,
        Duration::from_secs(5),
        async {
            tokio::signal::ctrl_c()
                .await
                .context("signal handler failed")
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_service_until_shutdown<F>(
    adapter: Arc<dyn AgentAdapter>,
    codex: Option<CodexClient>,
    channels: Vec<Arc<dyn ChannelAdapter>>,
    task_board: Option<Arc<agentix_task::Service>>,
    state_path: PathBuf,
    control_endpoint: String,
    bridge: Option<Arc<BridgeHub>>,
    config_path: PathBuf,
    claims: Arc<ClaimRegistry>,
    background_turn_notifications: bool,
    output: agentix_core::OutputConfig,
    channel_shutdown_grace: Duration,
    shutdown_signal: F,
) -> Result<()>
where
    F: Future<Output = Result<()>> + Send,
{
    let started = Instant::now();
    if let Some(parent) = state_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create state directory {}", parent.display()))?;
    }
    let state = SqliteState::open(&state_path).await?;
    let backends = adapter.session_backends();
    if !backends.is_empty() {
        state
            .qualify_sessions((backends.len() == 1).then(|| backends[0]))
            .await?;
    }
    tracing::info!(
        phase = "state_storage",
        elapsed_ms = started.elapsed().as_millis(),
        "startup phase completed"
    );
    let restore_started = Instant::now();
    let mut engine = Engine::new(adapter.clone(), state, channels.clone())
        .with_background_turn_notifications(background_turn_notifications)
        .with_output(output);
    if let Some(task_board) = task_board {
        engine = engine
            .with_task_board(task_board)
            .with_task_consumer(state_path.to_string_lossy().into_owned());
    }
    let engine = Arc::new(engine);
    let updates = engine.restore_bindings_deferred().await?;
    tracing::info!(
        restored = updates.restored_count(),
        phase = "binding_restore",
        elapsed_ms = restore_started.elapsed().as_millis(),
        "restored durable conversation bindings"
    );

    let shutdown = CancellationToken::new();
    let (control_tx, control_rx) = mpsc::channel(32);
    let advertised_control_endpoint = control_endpoint.clone();
    let control_shutdown = shutdown.clone();
    let mut control_task = tokio::spawn(async move {
        control::serve_with_bridge(&control_endpoint, control_tx, control_shutdown, bridge).await
    });
    let control_handler_task = tokio::spawn(run_control_handler(
        control_rx,
        adapter.clone(),
        codex,
        claims,
        config_path,
        shutdown.clone(),
    ));
    let (inbound_tx, inbound_rx) = mpsc::channel(256);
    let mut channel_tasks = Vec::new();
    for channel in channels {
        let inbound = inbound_tx.clone();
        let token = shutdown.clone();
        channel_tasks.push(tokio::spawn(async move {
            if let Err(error) = channel.run(inbound, token).await {
                tracing::error!(%error, channel = %channel.kind(), "IM channel stopped");
            }
        }));
    }
    drop(inbound_tx);

    let engine_task = tokio::spawn(run_engine_loop(
        engine.clone(),
        adapter,
        inbound_rx,
        shutdown.clone(),
    ));

    let startup_task = spawn_startup_notifications(engine.clone(), updates);

    tracing::info!(endpoint = %advertised_control_endpoint, elapsed_ms = started.elapsed().as_millis(), "Agentix is running");
    tokio::pin!(shutdown_signal);
    let control_failure = tokio::select! {
        signal = &mut shutdown_signal => {
            signal?;
            None
        }
        result = &mut control_task => Some(match result {
            Ok(Ok(())) => anyhow::anyhow!("Agentix control server stopped unexpectedly"),
            Ok(Err(error)) => error,
            Err(error) => anyhow::anyhow!("Agentix control server task failed: {error}"),
        }),
    };
    shutdown.cancel();
    startup_task.abort();
    let _ = startup_task.await;
    let _ = engine_task.await;
    match agentix::shutdown_engine(engine.clone(), channel_shutdown_grace).await {
        Ok(notified) => tracing::info!(notified, "saved bindings and notified IM conversations"),
        Err(error) => tracing::error!(%error, "failed to finish graceful shutdown preparation"),
    }

    wait_for_channel_shutdown(channel_tasks, channel_shutdown_grace).await;
    if control_failure.is_none() {
        let _ = control_task.await;
    }
    let _ = control_handler_task.await;
    control_failure.map_or(Ok(()), Err)
}

fn spawn_startup_notifications(
    engine: Arc<Engine>,
    updates: agentix_core::RestoredBindings,
) -> tokio_util::task::AbortOnDropHandle<()> {
    tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        let started = Instant::now();
        match engine.notify_restored_bindings(updates).await {
            Ok(()) => tracing::info!(
                phase = "restored_im_updates",
                elapsed_ms = started.elapsed().as_millis(),
                "restored conversation presentation completed"
            ),
            Err(error) => {
                tracing::warn!(%error, phase = "restored_im_updates", elapsed_ms = started.elapsed().as_millis(), "failed to present restored conversations");
            }
        }
    }))
}

async fn wait_for_channel_shutdown(
    channel_tasks: Vec<tokio::task::JoinHandle<()>>,
    grace: Duration,
) {
    let deadline = tokio::time::Instant::now() + grace;
    for mut task in channel_tasks {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() || tokio::time::timeout(remaining, &mut task).await.is_err() {
            task.abort();
            let _ = task.await;
        }
    }
}

async fn doctor(config: &Config) -> Result<()> {
    config.validate()?;
    println!("ok: configuration and selected-channel owner policy");
    println!("ok: selected channel credentials are configured");

    if let Some(parent) = config.storage.path.parent()
        && !parent.exists()
    {
        bail!("state directory does not exist: {}", parent.display());
    }
    println!("ok: state path {}", config.storage.path.display());

    for agent in config.selected_agents() {
        match agent {
            AgentConfig::Claude {
                command,
                session_dir,
                ..
            } => {
                doctor_pi(
                    BridgeKind::Claude,
                    command,
                    session_dir,
                    &config.server.endpoint,
                )
                .await?;
            }
            AgentConfig::Codex {
                endpoint,
                proxy_endpoint: _,
                proxy: _,
                command,
                rmux_directory,
            } => {
                let endpoint = CodexEndpoint::parse(endpoint)?;
                let client = CodexClient::connect_with_background_turn_notifications(
                    endpoint,
                    command,
                    rmux_directory,
                    false,
                )
                .await?;
                let page = client.list_sessions(None, 1).await?;
                println!(
                    "ok: Codex WebSocket-over-UDS handshake ({} loaded session sample)",
                    page.sessions.len()
                );
            }
            AgentConfig::Pi {
                command,
                session_dir,
                ..
            } => {
                doctor_pi(
                    BridgeKind::Pi,
                    command,
                    session_dir,
                    &config.server.endpoint,
                )
                .await?;
            }
            AgentConfig::OhMyPi {
                command,
                session_dir,
                ..
            } => {
                doctor_pi(
                    BridgeKind::Omp,
                    command,
                    session_dir,
                    &config.server.endpoint,
                )
                .await?;
            }
        }
    }
    tracing::info!("Agentix diagnostics completed");
    Ok(())
}

struct BuiltAgent {
    adapter: Arc<dyn AgentAdapter>,
    codex: Option<CodexClient>,
}

fn claude_workspace_args() -> Vec<String> {
    // Prompts use rmux input; ordinary plugin loading supplies the bridge hooks.
    Vec::new()
}

async fn build_agent(
    config: &AgentConfig,
    background_turn_notifications: bool,
    bridge_hub: Option<Arc<BridgeHub>>,
) -> Result<BuiltAgent> {
    match config {
        AgentConfig::Claude {
            command,
            session_dir,
            rmux_directory,
        } => Ok(BuiltAgent {
            adapter: Arc::new(
                BridgeAdapter::new(
                    BridgeKind::Claude,
                    bridge_hub.context("native bridge hub is not configured")?,
                    session_dir,
                )
                .with_workspace(command, claude_workspace_args(), rmux_directory),
            ),
            codex: None,
        }),
        AgentConfig::Codex {
            endpoint,
            proxy_endpoint,
            proxy,
            command,
            rmux_directory,
        } => {
            let endpoint = CodexEndpoint::parse(endpoint)?;
            let client = CodexClient::connect_with_proxy_options(
                proxy_endpoint,
                endpoint,
                command,
                rmux_directory,
                background_turn_notifications,
                proxy,
            )
            .await?;
            Ok(BuiltAgent {
                adapter: Arc::new(client.clone()),
                codex: Some(client),
            })
        }
        AgentConfig::Pi {
            command,
            session_dir,
            bridge_extension,
            rmux_directory,
        }
        | AgentConfig::OhMyPi {
            command,
            session_dir,
            bridge_extension,
            rmux_directory,
        } => {
            let flavor = if matches!(config, AgentConfig::Pi { .. }) {
                BridgeKind::Pi
            } else {
                BridgeKind::Omp
            };
            let mut args = vec![
                "--session-dir".into(),
                session_dir.to_string_lossy().into_owned(),
            ];
            if let Some(extension) = bridge_extension {
                args.extend(["-e".into(), extension.to_string_lossy().into_owned()]);
            }
            Ok(BuiltAgent {
                adapter: Arc::new(
                    BridgeAdapter::new(
                        flavor,
                        bridge_hub.context("native bridge hub is not configured")?,
                        session_dir,
                    )
                    .with_workspace(command, args, rmux_directory),
                ),
                codex: None,
            })
        }
    }
}

#[derive(Debug)]
struct PendingClaim {
    code: String,
    expires_at: u64,
}

#[derive(Debug, Default)]
struct ClaimRegistry {
    pending: Mutex<Option<PendingClaim>>,
}

impl ClaimRegistry {
    async fn generate(&self, ttl_minutes: u64, now: u64) -> Result<(String, u64)> {
        if !(1..=1_440).contains(&ttl_minutes) {
            bail!("--ttl-minutes must be between 1 and 1440");
        }
        let expires_at = now
            .checked_add(ttl_minutes * 60)
            .context("claim expiry is out of range")?;
        let code = Uuid::new_v4().simple().to_string()[..12].to_ascii_uppercase();
        *self.pending.lock().await = Some(PendingClaim {
            code: code.clone(),
            expires_at,
        });
        Ok((code, expires_at))
    }

    async fn matches(&self, code: &str, now: u64) -> bool {
        let mut pending = self.pending.lock().await;
        let Some(claim) = pending.as_ref() else {
            return false;
        };
        if now > claim.expires_at {
            *pending = None;
            return false;
        }
        constant_time_eq(&claim.code, code)
    }

    async fn consume(&self, code: &str) {
        let mut pending = self.pending.lock().await;
        if pending
            .as_ref()
            .is_some_and(|claim| constant_time_eq(&claim.code, code))
        {
            *pending = None;
        }
    }
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
}

#[derive(Debug)]
struct MemoryStringOwnerClaimer {
    kind: ImChannel,
    path: PathBuf,
    claims: Arc<ClaimRegistry>,
}

#[derive(Debug)]
struct MemoryTelegramOwnerClaimer {
    path: PathBuf,
    claims: Arc<ClaimRegistry>,
}

#[async_trait]
impl TelegramOwnerClaimer for MemoryTelegramOwnerClaimer {
    async fn claim(&self, code: &str, owner_user_id: u64) -> std::result::Result<bool, String> {
        let now = unix_timestamp().map_err(|error| error.to_string())?;
        if !self.claims.matches(code, now).await {
            return Ok(false);
        }
        let path = self.path.clone();
        let code = code.to_owned();
        tokio::task::spawn_blocking(move || add_telegram_owner(&path, owner_user_id))
            .await
            .map_err(|error| format!("owner config update task failed: {error}"))?
            .map_err(|error| {
                format!(
                    "failed to read or update {}: {error:#}",
                    self.path.display()
                )
            })?;
        self.claims.consume(&code).await;
        Ok(true)
    }
}

#[async_trait]
impl OwnerClaimer for MemoryStringOwnerClaimer {
    async fn claim(&self, code: &str, owner_open_id: &str) -> std::result::Result<bool, String> {
        let now = unix_timestamp().map_err(|error| error.to_string())?;
        if !self.claims.matches(code, now).await {
            return Ok(false);
        }
        let path = self.path.clone();
        let code = code.to_owned();
        let owner_open_id = owner_open_id.to_owned();
        let kind = self.kind;
        tokio::task::spawn_blocking(move || match kind {
            ImChannel::Slack => agentix::add_slack_owner(&path, &owner_open_id),
            ImChannel::Feishu => add_feishu_owner(&path, &owner_open_id),
            ImChannel::Telegram => anyhow::bail!("Telegram requires a numeric owner ID"),
        })
        .await
        .map_err(|error| format!("owner config update task failed: {error}"))?
        .map_err(|error| {
            format!(
                "failed to read or update {}: {error:#}",
                self.path.display()
            )
        })?;
        self.claims.consume(&code).await;
        Ok(true)
    }
}

async fn handle_control_request(
    request: control::ControlRequest,
    agent: &Arc<dyn AgentAdapter>,
    codex: Option<&CodexClient>,
    claims: &ClaimRegistry,
    config_path: &Path,
) -> std::result::Result<Value, String> {
    match request {
        control::ControlRequest::Session(operation) => {
            agentix_core::SessionOperations::new(agent.clone())
                .execute(operation)
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| serde_json::to_value(result).map_err(|error| error.to_string()))
        }
        control::ControlRequest::Sessions { cursor, limit } => agent
            .list_sessions(cursor, limit)
            .await
            .map_err(|error| error.to_string())
            .and_then(|page| serde_json::to_value(page).map_err(|error| error.to_string())),
        control::ControlRequest::Call { method, params } => {
            let codex = codex
                .ok_or_else(|| "client call is only available for the Codex backend".to_owned())?;
            codex
                .request(&method, params)
                .await
                .map_err(|error| error.to_string())
        }
        control::ControlRequest::Claim { ttl_minutes } => {
            let config = Config::load(config_path).map_err(|error| error.to_string())?;
            match config.channel.kind {
                ImChannel::Slack => {
                    if !config
                        .channel
                        .slack
                        .as_ref()
                        .expect("configuration was validated")
                        .owner_user_ids
                        .is_empty()
                    {
                        return Err("Slack already has a configured owner".into());
                    }
                }
                ImChannel::Telegram => {
                    let telegram = config
                        .channel
                        .telegram
                        .as_ref()
                        .expect("configuration was validated");
                    if !telegram.owner_user_ids.is_empty() {
                        return Err("Telegram already has a configured owner".into());
                    }
                }
                ImChannel::Feishu => {
                    let feishu = config
                        .channel
                        .feishu
                        .as_ref()
                        .expect("configuration was validated");
                    if !feishu.owner_open_ids.is_empty() {
                        return Err("Feishu already has a configured owner".into());
                    }
                }
            }
            let now = unix_timestamp().map_err(|error| error.to_string())?;
            let (code, expires_at) = claims
                .generate(ttl_minutes, now)
                .await
                .map_err(|error| error.to_string())?;
            let command = if config.channel.kind == ImChannel::Slack {
                let slack = config
                    .channel
                    .slack
                    .as_ref()
                    .expect("validated Slack config");
                agentix_slack::CommandAffixes::new(&slack.command_prefix, &slack.command_suffix)
                    .and_then(|affixes| affixes.encode("claim"))
                    .map_err(|error| error.to_string())?
            } else {
                "/claim".into()
            };
            Ok(json!({
                "command": format!("{command} {code}"),
                "expiresAt": expires_at,
            }))
        }
    }
}

fn telegram_menu_commands(config: &Config) -> Vec<teloxide::types::BotCommand> {
    let mut commands = agentix_telegram::menu_commands();
    if config.enabled_task_board().is_some() {
        commands.insert(
            1,
            teloxide::types::BotCommand::new("dashboard", "Browse projects and task boards"),
        );
    }
    commands
}

fn build_channels(
    config: &Config,
    config_path: &Path,
    claims: Arc<ClaimRegistry>,
) -> Result<Vec<Arc<dyn ChannelAdapter>>> {
    let channel: Arc<dyn ChannelAdapter> = match config.channel.kind {
        ImChannel::Slack => {
            let slack = config
                .channel
                .slack
                .as_ref()
                .expect("configuration was validated");
            let affixes =
                agentix_slack::CommandAffixes::new(&slack.command_prefix, &slack.command_suffix)?;
            let mut adapter = agentix_slack::SlackAdapter::with_client(
                config.network.http_client(
                    reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)),
                )?,
                "https://slack.com/api/".parse()?,
                slack.bot_token.clone(),
                slack.app_token.clone(),
                slack.owner_user_ids.clone(),
            )?
            .with_command_affixes(affixes.clone());
            if let Some(app_id) = &slack.app_id {
                let mut commands = agentix_core::command_menu(true).commands;
                if config.enabled_task_board().is_some() {
                    commands.extend(
                        [
                            ("dashboard", "Browse projects and task boards"),
                            ("board", "Show this session's task board"),
                            ("jobs", "Browse this session's jobs"),
                            ("inboxes", "Browse this project's inbox"),
                            ("inbox", "Append a requirement to this project's inbox"),
                        ]
                        .map(|(name, description)| {
                            agentix_core::ChannelCommand::new(name, description)
                        }),
                    );
                }
                adapter = adapter.with_command_sync(
                    agentix_slack::SlackCommandSync::new(
                        config
                            .slack_cli_path
                            .clone()
                            .unwrap_or_else(|| PathBuf::from("slack")),
                        app_id.clone(),
                        commands,
                    )
                    .with_command_affixes(affixes),
                );
            }
            if slack.owner_user_ids.is_empty() {
                adapter = adapter.with_owner_claimer(Arc::new(MemoryStringOwnerClaimer {
                    kind: ImChannel::Slack,
                    path: config_path.to_owned(),
                    claims,
                }));
            }
            Arc::new(adapter)
        }
        ImChannel::Telegram => {
            let telegram = config
                .channel
                .telegram
                .as_ref()
                .expect("configuration was validated");
            let mut adapter = TelegramAdapter::with_bot(
                build_telegram_bot(telegram, &config.network)?,
                TelegramPolicy::new(telegram.owner_user_ids.iter().copied()),
            )
            .with_menu_commands(telegram_menu_commands(config));
            if telegram.owner_user_ids.is_empty() {
                adapter = adapter.with_owner_claimer(Arc::new(MemoryTelegramOwnerClaimer {
                    path: config_path.to_owned(),
                    claims,
                }));
            }
            Arc::new(adapter)
        }
        ImChannel::Feishu => {
            let feishu = config
                .channel
                .feishu
                .as_ref()
                .expect("configuration was validated");
            let mut adapter = FeishuAdapter::new(
                feishu.app_id.clone(),
                feishu.app_secret.clone(),
                feishu.owner_open_ids.clone(),
            )?;
            if feishu.owner_open_ids.is_empty() {
                adapter = adapter.with_owner_claimer(Arc::new(MemoryStringOwnerClaimer {
                    kind: ImChannel::Feishu,
                    path: config_path.to_owned(),
                    claims,
                }));
            }
            Arc::new(adapter)
        }
    };
    Ok(vec![channel])
}

fn build_telegram_bot(telegram: &TelegramConfig, network: &NetworkConfig) -> Result<teloxide::Bot> {
    let client = network.http_client(teloxide::net::default_reqwest_settings())?;
    Ok(teloxide::Bot::with_client(telegram.token.clone(), client))
}

async fn doctor_pi(
    flavor: BridgeKind,
    command: &Path,
    session_dir: &Path,
    endpoint: &str,
) -> Result<()> {
    let executable = resolve_executable(command)
        .with_context(|| format!("{} command was not found", flavor.as_str()))?;
    let count = match BridgeHub::live_count(endpoint, flavor, session_dir).await {
        Ok(count) => count,
        Err(error) => {
            println!("note: Agentix bridge listener unavailable: {error}");
            0
        }
    };
    println!(
        "ok: {} at {} with {} live bridges",
        flavor.as_str(),
        executable.display(),
        count
    );
    if count == 0 {
        println!(
            "note: start agentix serve, load agentix-bridge in the original terminal, and check session_dir and AGENTIX_CONTROL_ENDPOINT"
        );
    }
    Ok(())
}

fn resolve_executable(command: &Path) -> Option<PathBuf> {
    if command.components().count() > 1 {
        return command.is_file().then(|| command.to_owned());
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|path| path.join(command))
            .find(|candidate| candidate.is_file())
    })
}

fn default_config_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/agentix/config.toml")
}

#[cfg(test)]
mod tests {
    #[test]
    fn claude_terminal_launch_does_not_require_channels() {
        assert!(super::claude_workspace_args().is_empty());
    }

    use std::path::Path;
    use std::sync::{Arc, Mutex as StdMutex};

    use agentix::{Config, ImChannel};
    use agentix_core::{
        AgentAdapter, AgentError, AgentEvent, ChannelAdapter, ChannelError, ChannelKind,
        ConversationRef, HistoryPage, InteractionDecision, MessageRef, OutboundView, SessionId,
        SessionPage, SessionStatus, SessionSummary,
    };
    use async_trait::async_trait;
    use time::format_description::well_known::Rfc3339;
    use tokio::sync::broadcast;
    use tokio_util::sync::CancellationToken;
    use tracing_subscriber::fmt::{format::Writer, time::FormatTime};

    struct LifecycleAgent {
        events: broadcast::Sender<AgentEvent>,
        attached: StdMutex<Vec<String>>,
        turn_blocked: CancellationToken,
        blocked_calls: std::sync::atomic::AtomicUsize,
        turn_release: CancellationToken,
    }

    impl LifecycleAgent {
        fn new() -> Self {
            let (events, _) = broadcast::channel(8);
            Self {
                events,
                attached: StdMutex::new(Vec::new()),
                turn_blocked: CancellationToken::new(),
                blocked_calls: std::sync::atomic::AtomicUsize::new(0),
                turn_release: CancellationToken::new(),
            }
        }
    }

    #[async_trait]
    impl AgentAdapter for LifecycleAgent {
        fn display_name(&self) -> &'static str {
            "Codex"
        }

        fn capabilities(&self) -> agentix_core::AgentCapabilities {
            agentix_core::AgentCapabilities {
                session_control: true,
                ..agentix_core::AgentCapabilities::default()
            }
        }

        async fn list_sessions(
            &self,
            _cursor: Option<String>,
            _limit: u32,
        ) -> Result<SessionPage, AgentError> {
            Ok(SessionPage {
                sessions: vec![SessionSummary {
                    id: SessionId::new("thr_saved"),
                    name: Some("Saved session".into()),
                    preview: None,
                    cwd: Some("/work/saved".into()),
                    updated_at: None,
                    status: SessionStatus::Idle,
                    terminal: None,
                }],
                next_cursor: None,
            })
        }

        async fn read_history(
            &self,
            _session_id: &SessionId,
            _cursor: Option<String>,
            _limit: u32,
        ) -> Result<HistoryPage, AgentError> {
            Ok(HistoryPage {
                turns: Vec::new(),
                older_cursor: None,
                newer_cursor: None,
            })
        }

        async fn attach(&self, session_id: &SessionId) -> Result<(), AgentError> {
            self.attached.lock().unwrap().push(session_id.to_string());
            Ok(())
        }

        async fn unsubscribe(&self, _session_id: &SessionId) -> Result<(), AgentError> {
            Ok(())
        }

        async fn start_turn(
            &self,
            _session_id: &SessionId,
            text: &str,
        ) -> Result<String, AgentError> {
            if text == "blocked-prompt" {
                self.blocked_calls
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.turn_blocked.cancel();
                self.turn_release.cancelled().await;
            }
            Ok("turn_test".into())
        }

        async fn steer(
            &self,
            _session_id: &SessionId,
            _expected_turn_id: &str,
            _text: &str,
        ) -> Result<String, AgentError> {
            Ok("turn_test".into())
        }

        async fn interrupt(
            &self,
            _session_id: &SessionId,
            _turn_id: &str,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        async fn resolve_interaction(
            &self,
            _decision: InteractionDecision,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
            self.events.subscribe()
        }

        fn generation(&self) -> u64 {
            1
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum StartupOperation {
        Menu,
        Notice,
    }

    struct LifecycleChannel {
        views: StdMutex<Vec<OutboundView>>,
        deliveries: StdMutex<Vec<String>>,
        stop_on_shutdown: bool,
        started: CancellationToken,
        blocked_operation: Option<StartupOperation>,
        blocked_working_conversation: Option<String>,
        blocked_offline_conversation: Option<String>,
        blocked_task_conversation: Option<String>,
        blocked: CancellationToken,
        unblock: CancellationToken,
    }

    impl LifecycleChannel {
        fn new() -> Self {
            Self {
                views: StdMutex::new(Vec::new()),
                deliveries: StdMutex::new(Vec::new()),
                stop_on_shutdown: true,
                started: CancellationToken::new(),
                blocked_operation: None,
                blocked_working_conversation: None,
                blocked_offline_conversation: None,
                blocked_task_conversation: None,
                blocked: CancellationToken::new(),
                unblock: CancellationToken::new(),
            }
        }

        fn stubborn() -> Self {
            Self {
                stop_on_shutdown: false,
                ..Self::new()
            }
        }

        async fn block_startup(&self, operation: StartupOperation) {
            if self.blocked_operation == Some(operation) {
                self.blocked.cancel();
                self.unblock.cancelled().await;
            }
        }
    }

    #[async_trait]
    impl ChannelAdapter for LifecycleChannel {
        fn kind(&self) -> ChannelKind {
            ChannelKind::Telegram
        }

        async fn run(
            &self,
            _inbound: tokio::sync::mpsc::Sender<agentix_core::InboundEnvelope>,
            shutdown: CancellationToken,
        ) -> Result<(), ChannelError> {
            self.started.cancel();
            if self.stop_on_shutdown {
                shutdown.cancelled().await;
            } else {
                std::future::pending::<()>().await;
            }
            Ok(())
        }

        async fn send(
            &self,
            conversation: &ConversationRef,
            view: &OutboundView,
        ) -> Result<MessageRef, ChannelError> {
            if self.blocked_task_conversation.as_deref() == Some(&conversation.conversation_id)
                && view.title == "Task update"
            {
                self.blocked.cancel();
                self.unblock.cancelled().await;
            }
            if view.subtitle.as_deref() == Some("Online · Reattached") {
                self.block_startup(StartupOperation::Notice).await;
            }
            if self.blocked_offline_conversation.as_deref() == Some(&conversation.conversation_id)
                && view.subtitle.as_deref() == Some("Offline · Detached")
            {
                self.blocked.cancel();
                self.unblock.cancelled().await;
            }
            if self.blocked_working_conversation.as_deref() == Some(&conversation.conversation_id)
                && view.status == agentix_core::ViewStatus::Running
            {
                self.blocked.cancel();
                self.unblock.cancelled().await;
            }
            self.views.lock().unwrap().push(view.clone());
            self.deliveries
                .lock()
                .unwrap()
                .push(conversation.conversation_id.clone());
            Ok(MessageRef::new(conversation.clone(), "message-test"))
        }

        async fn update(
            &self,
            _conversation: &ConversationRef,
            _message: &MessageRef,
            view: &OutboundView,
        ) -> Result<(), ChannelError> {
            self.views.lock().unwrap().push(view.clone());
            Ok(())
        }

        async fn set_command_menu(
            &self,
            _conversation: &ConversationRef,
            menu: &agentix_core::CommandMenu,
        ) -> Result<(), ChannelError> {
            if menu.commands.iter().any(|command| command.contextual) {
                self.block_startup(StartupOperation::Menu).await;
            }
            Ok(())
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One production-runtime admission and recovery scenario.
    async fn engine_runtime_conversation_flood_preserves_capacity_for_other_conversations() {
        use agentix_core::{Engine, InboundEnvelope, SqliteState};
        use std::time::Duration;
        let agent = Arc::new(LifecycleAgent::new());
        let channel = Arc::new(LifecycleChannel::new());
        let state = SqliteState::in_memory().await.unwrap();
        let engine = Arc::new(Engine::new(
            agent.clone(),
            state.clone(),
            vec![channel.clone()],
        ));
        let slow = ConversationRef::new(ChannelKind::Telegram, "flood");
        engine
            .handle_inbound(InboundEnvelope::text(
                "attach",
                slow.clone(),
                "owner",
                "/attach thr_saved",
            ))
            .await
            .unwrap();
        channel.deliveries.lock().unwrap().clear();
        channel.views.lock().unwrap().clear();
        let (sender, inbound) = tokio::sync::mpsc::channel(8);
        let shutdown = CancellationToken::new();
        let runtime = tokio::spawn(super::run_engine_loop(
            engine.clone(),
            agent.clone(),
            inbound,
            shutdown.clone(),
        ));
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            sender
                .send(InboundEnvelope::text(
                    "blocked",
                    slow.clone(),
                    "owner",
                    "blocked-prompt",
                ))
                .await
                .unwrap();
            agent.turn_blocked.cancelled().await;
            for i in 0..300 {
                sender
                    .send(InboundEnvelope::text(
                        format!("flood-{i}"),
                        slow.clone(),
                        "owner",
                        "/help",
                    ))
                    .await
                    .unwrap();
            }
            sender
                .send(InboundEnvelope::text(
                    "flood-0",
                    slow.clone(),
                    "owner",
                    "/help",
                ))
                .await
                .unwrap();
            sender
                .send(InboundEnvelope::text(
                    "fast",
                    ConversationRef::new(ChannelKind::Telegram, "fast"),
                    "owner",
                    "/help",
                ))
                .await
                .unwrap();
            loop {
                if channel
                    .deliveries
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|chat| chat == "fast")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            // Quota includes the blocked prompt: only the first 15 help requests
            // were accepted. Rejected requests must never run on replay.
            assert!(
                !state
                    .claim_event(ChannelKind::Telegram, "flood-299")
                    .await
                    .unwrap()
            );
            agent.turn_release.cancel();
            loop {
                if channel
                    .views
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|view| view.title == "Agentix commands")
                    .count()
                    == 16
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            sender
                .send(InboundEnvelope::text("after-drain", slow, "owner", "/help"))
                .await
                .unwrap();
            loop {
                let finished = {
                    let views = channel.views.lock().unwrap();
                    views
                        .iter()
                        .filter(|view| view.title == "Agentix commands")
                        .count()
                        == 17
                        && views.iter().any(|view| view.title == "Session busy")
                };
                if finished {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        agent.turn_release.cancel();
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(3), runtime)
            .await
            .unwrap()
            .unwrap();
        assert!(
            result.is_ok(),
            "one conversation exhausted admission or lost quota after completion"
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One production-runtime durable delivery scenario.
    async fn engine_runtime_delivers_task_outbox_per_conversation_while_one_im_send_is_blocked() {
        use agentix_core::{Engine, SqliteState};
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".obsidian")).unwrap();
        let service = Arc::new(
            agentix_task::Service::open(agentix_task::Config {
                schema_version: 1,
                storage: agentix_task::StorageConfig {
                    path: dir.path().join("tasks.sqlite3"),
                },
                documents: agentix_task::DocumentConfig {
                    root: dir.path().into(),
                    directory: "Tasks".into(),
                },
            })
            .await
            .unwrap(),
        );
        let agent = Arc::new(LifecycleAgent::new());
        let channel = Arc::new(LifecycleChannel {
            blocked_task_conversation: Some("slow".into()),
            ..LifecycleChannel::new()
        });
        let state = SqliteState::in_memory().await.unwrap();
        state.notification_cursor("default", 0).await.unwrap();
        for (sequence, chat) in [(1, "slow"), (2, "slow"), (3, "fast")] {
            state
                .stage_notification(
                    "default",
                    sequence,
                    Some((
                        &ConversationRef::new(ChannelKind::Telegram, chat),
                        &OutboundView::text("Task update", sequence.to_string()),
                    )),
                )
                .await
                .unwrap();
        }
        let engine = Arc::new(
            Engine::new(agent.clone(), state.clone(), vec![channel.clone()])
                .with_task_board(service),
        );
        let (_tx, rx) = tokio::sync::mpsc::channel(8);
        let shutdown = CancellationToken::new();
        let worker = tokio::spawn(agentix::run_engine_loop(
            engine,
            agent,
            rx,
            shutdown.clone(),
        ));
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            channel.blocked.cancelled().await;
            loop {
                if channel
                    .deliveries
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|chat| chat == "fast")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            state
                .stage_notification(
                    "default",
                    4,
                    Some((
                        &ConversationRef::new(ChannelKind::Telegram, "later"),
                        &OutboundView::text("Task update", "4"),
                    )),
                )
                .await
                .unwrap();
            loop {
                if channel
                    .deliveries
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|chat| chat == "later")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert!(
                !channel
                    .deliveries
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|chat| chat == "slow")
            );
            channel.unblock.cancel();
            loop {
                if channel
                    .deliveries
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|chat| *chat == "slow")
                    .count()
                    == 2
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(3), worker)
            .await
            .unwrap()
            .unwrap();
        assert!(
            result.is_ok(),
            "task delivery must progress independently of a stalled conversation"
        );
        let views = channel.views.lock().unwrap();
        let order: Vec<_> = views
            .iter()
            .filter(|view| view.body == "1" || view.body == "2")
            .map(|view| view.body.as_str())
            .collect();
        assert_eq!(order, ["1", "2"]);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn engine_runtime_slow_prompt_keeps_other_conversations_live_and_own_requests_ordered() {
        use agentix_core::{Engine, InboundEnvelope, SqliteState};
        use std::time::Duration;
        let agent = Arc::new(LifecycleAgent::new());
        let channel = Arc::new(LifecycleChannel::new());
        let engine = Arc::new(Engine::new(
            agent.clone(),
            SqliteState::in_memory().await.unwrap(),
            vec![channel.clone()],
        ));
        let slow = ConversationRef::new(ChannelKind::Telegram, "slow");
        let fast = ConversationRef::new(ChannelKind::Telegram, "fast");
        engine
            .handle_inbound(InboundEnvelope::text(
                "attach",
                slow.clone(),
                "owner",
                "/attach thr_saved",
            ))
            .await
            .unwrap();
        engine
            .handle_inbound(InboundEnvelope::text(
                "attach-fast",
                fast.clone(),
                "owner",
                "/attach thr_fast",
            ))
            .await
            .unwrap();
        channel.deliveries.lock().unwrap().clear();
        let (sender, inbound) = tokio::sync::mpsc::channel(8);
        let shutdown = CancellationToken::new();
        let runtime = tokio::spawn(super::run_engine_loop(
            engine,
            agent.clone(),
            inbound,
            shutdown.clone(),
        ));
        sender
            .send(InboundEnvelope::text(
                "slow-prompt",
                slow.clone(),
                "owner",
                "blocked-prompt",
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), agent.turn_blocked.cancelled())
            .await
            .unwrap();
        sender
            .send(InboundEnvelope::text("slow-help", slow, "owner", "/help"))
            .await
            .unwrap();
        sender
            .send(InboundEnvelope::text(
                "fast-prompt",
                fast,
                "owner",
                "independent prompt",
            ))
            .await
            .unwrap();
        let fast_result = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if channel
                    .deliveries
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|id| id == "fast")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        let before_release = channel.deliveries.lock().unwrap().clone();
        agent.turn_release.cancel();
        let ordered = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if channel
                    .deliveries
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|id| *id == "slow")
                    .count()
                    >= 2
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        shutdown.cancel();
        runtime.await.unwrap();
        assert!(
            fast_result.is_ok(),
            "another conversation waited for a slow prompt acknowledgement"
        );
        assert!(
            !before_release.iter().any(|id| id == "slow"),
            "same-conversation request overtook its prompt"
        );
        assert!(ordered.is_ok(), "ordered request did not resume");
    }

    #[tokio::test]
    async fn engine_runtime_shutdown_bounds_waits_and_fences_only_started_inputs() {
        use agentix_core::{Engine, InboundEnvelope, SqliteState};
        use std::time::Duration;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let state = SqliteState::open(&path).await.unwrap();
        let agent = Arc::new(LifecycleAgent::new());
        let engine = Arc::new(Engine::new(
            agent.clone(),
            state.clone(),
            vec![Arc::new(LifecycleChannel::new())],
        ));
        let conversation = ConversationRef::new(ChannelKind::Telegram, "slow");
        engine
            .handle_inbound(InboundEnvelope::text(
                "attach",
                conversation.clone(),
                "owner",
                "/attach thr_saved",
            ))
            .await
            .unwrap();
        let (sender, inbound) = tokio::sync::mpsc::channel(8);
        let shutdown = CancellationToken::new();
        let mut runtime = tokio::spawn(super::run_engine_loop(
            engine,
            agent.clone(),
            inbound,
            shutdown.clone(),
        ));
        sender
            .send(InboundEnvelope::text(
                "started",
                conversation.clone(),
                "owner",
                "blocked-prompt",
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), agent.turn_blocked.cancelled())
            .await
            .unwrap();
        sender
            .send(InboundEnvelope::text(
                "pending",
                conversation,
                "owner",
                "never started",
            ))
            .await
            .unwrap();
        shutdown.cancel();
        let stopped = tokio::time::timeout(Duration::from_secs(2), &mut runtime).await;
        if stopped.is_err() {
            runtime.abort();
            let _ = runtime.await;
        }
        assert!(
            stopped.is_ok(),
            "shutdown waited indefinitely for a host acknowledgement"
        );
        assert!(
            !state
                .claim_event(ChannelKind::Telegram, "started")
                .await
                .unwrap()
        );
        assert!(
            state
                .claim_event(ChannelKind::Telegram, "pending")
                .await
                .unwrap()
        );
        drop(state);
        let state = SqliteState::open(&path).await.unwrap();
        assert!(
            !state
                .claim_event(ChannelKind::Telegram, "started")
                .await
                .unwrap(),
            "restart replays a possibly accepted prompt"
        );
        agent.turn_release.cancel();
    }

    #[tokio::test]
    async fn engine_runtime_working_refreshes_do_not_wait_for_a_slow_conversation() {
        use agentix_core::{Engine, InboundEnvelope, SqliteState};
        use std::time::Duration;
        let agent = Arc::new(LifecycleAgent::new());
        let channel = Arc::new(LifecycleChannel {
            blocked_working_conversation: Some("slow".into()),
            ..LifecycleChannel::new()
        });
        let engine = Arc::new(Engine::new(
            agent.clone(),
            SqliteState::in_memory().await.unwrap(),
            vec![channel.clone()],
        ));
        for (chat, session) in [("slow", "thr_a"), ("fast", "thr_b")] {
            engine
                .handle_inbound(InboundEnvelope::text(
                    format!("attach-{chat}"),
                    ConversationRef::new(ChannelKind::Telegram, chat),
                    "owner",
                    format!("/attach {session}"),
                ))
                .await
                .unwrap();
            engine
                .handle_agent_event(AgentEvent::TurnStarted {
                    session_id: session.into(),
                    turn_id: "running".into(),
                })
                .await
                .unwrap();
        }
        channel.deliveries.lock().unwrap().clear();
        let (_sender, inbound) = tokio::sync::mpsc::channel(8);
        let shutdown = CancellationToken::new();
        let runtime = tokio::spawn(super::run_engine_loop(
            engine,
            agent,
            inbound,
            shutdown.clone(),
        ));
        let refreshed = tokio::time::timeout(Duration::from_secs(3), async {
            channel.blocked.cancelled().await;
            loop {
                if channel
                    .deliveries
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|id| id == "fast")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        channel.unblock.cancel();
        shutdown.cancel();
        runtime.await.unwrap();
        assert!(
            refreshed.is_ok(),
            "one slow working card blocked other sessions' timers"
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn engine_runtime_fixed_load_preserves_isolation_and_admission_bounds() {
        use agentix_core::{Engine, InboundEnvelope, SqliteState};
        use std::{
            sync::atomic::{AtomicUsize, Ordering},
            time::{Duration, Instant},
        };
        let agent = Arc::new(LifecycleAgent::new());
        let channel = Arc::new(LifecycleChannel::new());
        let engine = Arc::new(Engine::new(
            agent.clone(),
            SqliteState::in_memory().await.unwrap(),
            vec![channel.clone()],
        ));
        let slow = |index| ConversationRef::new(ChannelKind::Telegram, format!("slow-{index}"));
        for index in 0..32 {
            engine
                .handle_inbound(InboundEnvelope::text(
                    format!("attach-{index}"),
                    slow(index),
                    "owner",
                    format!("/attach thr_{index}"),
                ))
                .await
                .unwrap();
        }
        channel.deliveries.lock().unwrap().clear();
        let (sender, inbound) = tokio::sync::mpsc::channel(8);
        let shutdown = CancellationToken::new();
        let runtime = tokio::spawn(super::run_engine_loop(
            engine,
            agent.clone(),
            inbound,
            shutdown.clone(),
        ));
        for index in 0..31 {
            sender
                .send(InboundEnvelope::text(
                    format!("blocked-{index}"),
                    slow(index),
                    "owner",
                    "blocked-prompt",
                ))
                .await
                .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            while agent.blocked_calls.load(Ordering::SeqCst) != 31 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let started = Instant::now();
        for index in 0..64 {
            sender
                .send(InboundEnvelope::text(
                    format!("fast-{index}"),
                    ConversationRef::new(ChannelKind::Telegram, format!("fast-{index}")),
                    "owner",
                    "/help",
                ))
                .await
                .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            while channel.deliveries.lock().unwrap().len() != 64 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("31 waiting hosts prevented independent conversations from completing");
        let independent_elapsed = started.elapsed();
        assert!(
            channel
                .deliveries
                .lock()
                .unwrap()
                .iter()
                .all(|chat| chat.starts_with("fast-"))
        );
        sender
            .send(InboundEnvelope::text(
                "blocked-31",
                slow(31),
                "owner",
                "blocked-prompt",
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while agent.blocked_calls.load(Ordering::SeqCst) != 32 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let admitted = Arc::new(AtomicUsize::new(0));
        let producer = tokio::spawn({
            let admitted = admitted.clone();
            async move {
                for index in 0..300 {
                    if sender
                        .send(InboundEnvelope::text(
                            format!("pending-{index}"),
                            slow(index % 32),
                            "owner",
                            "/help",
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    admitted.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
        // 32 active workers + 224 pending operations fill runtime capacity;
        // another eight envelopes fit in the caller's channel. No next send
        // can complete until one of those owned operations is released.
        tokio::time::timeout(Duration::from_secs(3), async {
            while admitted.load(Ordering::SeqCst) < 232 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        assert_eq!(admitted.load(Ordering::SeqCst), 232);
        assert!(!producer.is_finished());
        assert_eq!(agent.blocked_calls.load(Ordering::SeqCst), 32);
        shutdown.cancel();
        agent.turn_release.cancel();
        runtime.await.unwrap();
        producer.await.unwrap();
        eprintln!(
            "Engine fixed load: 31 held prompts, 64 independent replies in {independent_elapsed:?}; 32-worker / 256-operation bounds verified"
        );
    }

    #[tokio::test]
    async fn slow_startup_notifications_do_not_block_control_or_channels() {
        for operation in [StartupOperation::Menu, StartupOperation::Notice] {
            let (_directory, channel, endpoint, shutdown, service) =
                start_with_blocked_notification(operation).await;
            let ready = tokio::time::timeout(std::time::Duration::from_secs(1), async {
                channel.started.cancelled().await;
                loop {
                    if let Ok(page) = super::control::request(
                        &endpoint,
                        &super::control::ControlRequest::Sessions {
                            cursor: None,
                            limit: 1,
                        },
                    )
                    .await
                    {
                        assert_eq!(page["sessions"][0]["id"], "thr_saved");
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await;
            channel.unblock.cancel();
            shutdown.cancel();
            service.await.unwrap().unwrap();
            assert!(ready.is_ok(), "startup IM work blocked service readiness");
        }
    }

    #[tokio::test]
    async fn shutdown_cancels_a_blocked_startup_notification() {
        let (_directory, channel, _endpoint, shutdown, mut service) =
            start_with_blocked_notification(StartupOperation::Notice).await;
        shutdown.cancel();
        let stopped = tokio::time::timeout(std::time::Duration::from_secs(1), &mut service).await;
        if stopped.is_err() {
            service.abort();
            let _ = service.await;
        }
        stopped
            .expect("shutdown waited on an online notification")
            .unwrap()
            .unwrap();
        assert!(
            !channel
                .views
                .lock()
                .unwrap()
                .iter()
                .any(|view| { view.subtitle.as_deref() == Some("Online · Reattached") })
        );
    }

    async fn start_with_blocked_notification(
        operation: StartupOperation,
    ) -> (
        tempfile::TempDir,
        Arc<LifecycleChannel>,
        String,
        CancellationToken,
        tokio::task::JoinHandle<anyhow::Result<()>>,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("agentix.sqlite3");
        let state = agentix_core::SqliteState::open(&state_path).await.unwrap();
        state
            .attach(
                &ConversationRef::new(ChannelKind::Telegram, "chat-saved"),
                &SessionId::new("thr_saved"),
            )
            .await
            .unwrap();
        drop(state);
        let channel = Arc::new(LifecycleChannel {
            blocked_operation: Some(operation),
            ..LifecycleChannel::new()
        });
        let endpoint = format!("tcp://{}", unused_loopback_address());
        let shutdown = CancellationToken::new();
        let service = tokio::spawn(super::run_service_until_shutdown(
            Arc::new(LifecycleAgent::new()),
            None,
            vec![channel.clone()],
            None,
            state_path,
            endpoint.clone(),
            None,
            directory.path().join("config.toml"),
            Arc::new(super::ClaimRegistry::default()),
            true,
            agentix_core::OutputConfig::default(),
            std::time::Duration::from_millis(10),
            {
                let shutdown = shutdown.clone();
                async move {
                    shutdown.cancelled().await;
                    Ok(())
                }
            },
        ));
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            channel.blocked.cancelled(),
        )
        .await
        .unwrap();
        (directory, channel, endpoint, shutdown, service)
    }

    #[test]
    fn log_timestamps_use_the_system_local_offset() {
        let mut output = String::new();
        super::local_time_timer()
            .format_time(&mut Writer::new(&mut output))
            .unwrap();

        let timestamp = time::OffsetDateTime::parse(&output, &Rfc3339).unwrap();
        let local_offset = time::UtcOffset::current_local_offset().unwrap();
        assert_eq!(timestamp.offset(), local_offset);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn service_starts_with_offline_native_binding_and_serves_control_requests() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("agentix.sqlite3");
        let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-saved");
        let state = agentix_core::SqliteState::open(&state_path).await.unwrap();
        state
            .attach(&conversation, &SessionId::new("thr_saved"))
            .await
            .unwrap();
        let hub = Arc::new(super::BridgeHub::new());
        let agent = Arc::new(super::BridgeAdapter::new(
            super::BridgeKind::Claude,
            hub.clone(),
            directory.path(),
        ));
        let channel = Arc::new(LifecycleChannel::new());
        let endpoint = format!("unix://{}", directory.path().join("control.sock").display());
        let shutdown = CancellationToken::new();
        let service = tokio::spawn(super::run_service_until_shutdown(
            agent,
            None,
            vec![channel.clone()],
            None,
            state_path,
            endpoint.clone(),
            Some(hub),
            directory.path().join("config.toml"),
            Arc::new(super::ClaimRegistry::default()),
            true,
            agentix_core::OutputConfig::default(),
            std::time::Duration::from_secs(1),
            {
                let shutdown = shutdown.clone();
                async move {
                    shutdown.cancelled().await;
                    Ok(())
                }
            },
        ));
        let ready = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            channel.started.cancelled().await;
            loop {
                if let Ok(page) = super::control::request(
                    &endpoint,
                    &super::control::ControlRequest::Sessions {
                        cursor: None,
                        limit: 1,
                    },
                )
                .await
                {
                    assert!(page["sessions"].as_array().unwrap().is_empty());
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        shutdown.cancel();
        service.await.unwrap().unwrap();
        assert!(
            ready.is_ok(),
            "offline bridge prevented control listener startup"
        );
        assert_eq!(
            state.current_session(&conversation).await.unwrap(),
            Some(SessionId::new("thr_saved"))
        );
    }

    #[tokio::test]
    async fn shutdown_notifications_isolate_a_slow_conversation_after_local_preparation() {
        use agentix_core::{Engine, InboundEnvelope, SqliteState};
        let channel = Arc::new(LifecycleChannel {
            blocked_offline_conversation: Some("slow".into()),
            ..LifecycleChannel::new()
        });
        let state = SqliteState::in_memory().await.unwrap();
        let engine = Arc::new(Engine::new(
            Arc::new(LifecycleAgent::new()),
            state.clone(),
            vec![channel.clone()],
        ));
        for (chat, session) in [("slow", "thr_a"), ("fast", "thr_b")] {
            engine
                .handle_inbound(InboundEnvelope::text(
                    format!("attach-{chat}"),
                    ConversationRef::new(ChannelKind::Telegram, chat),
                    "owner",
                    format!("/attach {session}"),
                ))
                .await
                .unwrap();
        }
        channel.deliveries.lock().unwrap().clear();
        let notified = agentix::shutdown_engine(engine, std::time::Duration::from_millis(100))
            .await
            .unwrap();
        assert!(channel.blocked.is_cancelled());
        assert_eq!(notified, 1);
        assert_eq!(*channel.deliveries.lock().unwrap(), ["fast"]);
        assert_eq!(state.list_bindings().await.unwrap().len(), 2);
        channel.unblock.cancel();
    }

    #[tokio::test]
    async fn service_shutdown_does_not_wait_indefinitely_for_an_offline_notice() {
        use std::time::Duration;
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.sqlite3");
        let conversation = ConversationRef::new(ChannelKind::Telegram, "saved");
        let state = agentix_core::SqliteState::open(&state_path).await.unwrap();
        state
            .attach(&conversation, &SessionId::new("thr_saved"))
            .await
            .unwrap();
        drop(state);
        let channel = Arc::new(LifecycleChannel {
            blocked_offline_conversation: Some("saved".into()),
            ..LifecycleChannel::new()
        });
        let shutdown = CancellationToken::new();
        let mut service = tokio::spawn(super::run_service_until_shutdown(
            Arc::new(LifecycleAgent::new()),
            None,
            vec![channel.clone()],
            None,
            state_path.clone(),
            format!("tcp://{}", unused_loopback_address()),
            None,
            directory.path().join("config.toml"),
            Arc::new(super::ClaimRegistry::default()),
            true,
            agentix_core::OutputConfig::default(),
            Duration::from_millis(50),
            {
                let shutdown = shutdown.clone();
                async move {
                    shutdown.cancelled().await;
                    Ok(())
                }
            },
        ));
        tokio::time::timeout(Duration::from_secs(2), channel.started.cancelled())
            .await
            .unwrap();
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(2), channel.blocked.cancelled())
            .await
            .unwrap();
        let stopped = tokio::time::timeout(Duration::from_secs(1), &mut service).await;
        channel.unblock.cancel();
        if stopped.is_err() {
            service.await.unwrap().unwrap();
        }
        assert!(
            stopped.is_ok(),
            "offline notification blocked service shutdown"
        );
        let state = agentix_core::SqliteState::open(&state_path).await.unwrap();
        assert_eq!(
            state.current_session(&conversation).await.unwrap(),
            Some(SessionId::new("thr_saved"))
        );
    }

    #[tokio::test]
    async fn service_lifecycle_restores_persisted_bindings_and_notifies_on_shutdown() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("agentix.sqlite3");
        let config_path = directory.path().join("config.toml");
        std::fs::write(&config_path, "unused").unwrap();
        let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-saved");
        let state = agentix_core::SqliteState::open(&state_path).await.unwrap();
        state
            .attach(&conversation, &SessionId::new("thr_saved"))
            .await
            .unwrap();
        drop(state);
        let agent = Arc::new(LifecycleAgent::new());

        for _ in 0..2 {
            let channel = Arc::new(LifecycleChannel::new());
            let shutdown = CancellationToken::new();
            let service = tokio::spawn(super::run_service_until_shutdown(
                agent.clone(),
                None,
                vec![channel.clone()],
                None,
                state_path.clone(),
                format!("tcp://{}", unused_loopback_address()),
                None,
                config_path.clone(),
                Arc::new(super::ClaimRegistry::default()),
                true,
                agentix_core::OutputConfig::default(),
                std::time::Duration::from_secs(5),
                {
                    let shutdown = shutdown.clone();
                    async move {
                        shutdown.cancelled().await;
                        Ok(())
                    }
                },
            ));
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if channel
                        .views
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|view| view.subtitle.as_deref() == Some("Online · Reattached"))
                    {
                        return;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            shutdown.cancel();
            service.await.unwrap().unwrap();
            assert!(
                channel
                    .views
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|view| view.subtitle.as_deref() == Some("Offline · Detached"))
            );
        }

        assert_eq!(
            agent.attached.lock().unwrap().as_slice(),
            ["thr_saved", "thr_saved"]
        );
        let state = agentix_core::SqliteState::open(&state_path).await.unwrap();
        assert_eq!(
            state.current_session(&conversation).await.unwrap(),
            Some(SessionId::new("thr_saved"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn channel_shutdown_returns_when_tasks_finish_before_the_deadline() {
        let completed = CancellationToken::new();
        let task_completed = completed.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(4)).await;
            task_completed.cancel();
        });
        let started = tokio::time::Instant::now();

        super::wait_for_channel_shutdown(vec![task], std::time::Duration::from_millis(10)).await;

        assert!(completed.is_cancelled());
        assert_eq!(started.elapsed(), std::time::Duration::from_millis(4));
    }

    #[tokio::test(start_paused = true)]
    async fn channel_shutdown_aborts_stuck_tasks_after_one_shared_deadline() {
        let mut tasks = vec![tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(4)).await;
        })];
        let mut cancelled = Vec::new();
        let mut abort_handles = Vec::new();
        for _ in 0..2 {
            let token = CancellationToken::new();
            let guard = token.clone().drop_guard();
            let task = tokio::spawn(async move {
                let _guard = guard;
                std::future::pending::<()>().await;
            });
            cancelled.push(token);
            abort_handles.push(task.abort_handle());
            tasks.push(task);
        }
        tokio::task::yield_now().await;
        assert!(cancelled.iter().all(|token| !token.is_cancelled()));
        let started = tokio::time::Instant::now();

        let shutdown = tokio::spawn(super::wait_for_channel_shutdown(
            tasks,
            std::time::Duration::from_millis(10),
        ));
        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_millis(9)).await;
        tokio::task::yield_now().await;
        assert!(!shutdown.is_finished());
        assert!(cancelled.iter().all(|token| !token.is_cancelled()));
        shutdown.await.unwrap();

        assert_eq!(started.elapsed(), std::time::Duration::from_millis(10));
        assert!(cancelled.iter().all(CancellationToken::is_cancelled));
        assert!(
            abort_handles
                .iter()
                .all(tokio::task::AbortHandle::is_finished)
        );
    }

    #[tokio::test]
    async fn service_shutdown_completes_with_a_stuck_channel() {
        let directory = tempfile::tempdir().unwrap();

        super::run_service_until_shutdown(
            Arc::new(LifecycleAgent::new()),
            None,
            vec![Arc::new(LifecycleChannel::stubborn())],
            None,
            directory.path().join("agentix.sqlite3"),
            format!("tcp://{}", unused_loopback_address()),
            None,
            directory.path().join("config.toml"),
            Arc::new(super::ClaimRegistry::default()),
            true,
            agentix_core::OutputConfig::default(),
            std::time::Duration::from_millis(10),
            async { Ok(()) },
        )
        .await
        .unwrap();
    }

    fn unused_loopback_address() -> std::net::SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    }

    #[test]
    fn default_config_is_loaded_from_home_dot_config() {
        let home = std::env::var_os("HOME").expect("the test user should have HOME set");

        assert_eq!(
            super::default_config_path(),
            std::path::PathBuf::from(home).join(".config/agentix/config.toml")
        );
    }

    #[tokio::test]
    async fn claim_registry_keeps_only_a_single_unexpired_in_memory_code() {
        let claims = super::ClaimRegistry::default();
        let (first, expires_at) = claims.generate(1, 100).await.unwrap();
        assert_eq!(expires_at, 160);
        assert!(claims.matches(&first, 160).await);
        assert!(!claims.matches(&first, 161).await);

        let (second, _) = claims.generate(10, 200).await.unwrap();
        assert_ne!(first, second);
        assert!(!claims.matches("WRONG", 201).await);
        assert!(claims.matches(&second, 201).await);
        claims.consume(&second).await;
        assert!(!claims.matches(&second, 201).await);
    }

    #[tokio::test]
    async fn telegram_claim_matches_server_memory_and_persists_only_the_owner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agentix.toml");
        std::fs::write(
            &path,
            r#"[channel]
kind = "telegram"

[channel.telegram]
token = "mock-token"
owner_user_ids = []

[agent]
kind = "codex"

[storage]
path = "/tmp/agentix-test.sqlite3"
"#,
        )
        .unwrap();
        let claims = std::sync::Arc::new(super::ClaimRegistry::default());
        let now = super::unix_timestamp().unwrap();
        let (code, _) = claims.generate(1, now).await.unwrap();
        let claimer = super::MemoryTelegramOwnerClaimer {
            path: path.clone(),
            claims: claims.clone(),
        };

        assert!(
            !agentix_telegram::TelegramOwnerClaimer::claim(&claimer, "WRONG", 42)
                .await
                .unwrap()
        );
        assert!(
            agentix_telegram::TelegramOwnerClaimer::claim(&claimer, &code, 42)
                .await
                .unwrap()
        );

        let persisted = std::fs::read_to_string(path).unwrap();
        assert!(persisted.contains("owner_user_ids = [42]"));
        assert!(!persisted.contains("claim_code"));
        assert!(!persisted.contains("claim_expires"));
        assert!(!claims.matches(&code, now).await);
    }

    #[tokio::test]
    async fn feishu_claim_matches_server_memory_and_persists_only_the_owner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agentix.toml");
        std::fs::write(
            &path,
            r#"[channel]
kind = "feishu"

[channel.feishu]
app_id = "cli_mock"
app_secret = "mock-secret"
owner_open_ids = []

[agent]
kind = "codex"

[storage]
path = "/tmp/agentix-test.sqlite3"
"#,
        )
        .unwrap();
        let claims = std::sync::Arc::new(super::ClaimRegistry::default());
        let now = super::unix_timestamp().unwrap();
        let (code, _) = claims.generate(1, now).await.unwrap();
        let claimer = super::MemoryStringOwnerClaimer {
            kind: ImChannel::Feishu,
            path: path.clone(),
            claims: claims.clone(),
        };

        assert!(
            !agentix_feishu::FeishuOwnerClaimer::claim(&claimer, "WRONG", "ou_owner")
                .await
                .unwrap()
        );
        assert!(
            agentix_feishu::FeishuOwnerClaimer::claim(&claimer, &code, "ou_owner")
                .await
                .unwrap()
        );

        let persisted = std::fs::read_to_string(path).unwrap();
        assert!(persisted.contains("owner_open_ids = [\"ou_owner\"]"));
        assert!(!persisted.contains("claim_code"));
        assert!(!persisted.contains("claim_expires"));
        assert!(!claims.matches(&code, now).await);
    }

    #[tokio::test]
    async fn slack_claim_matches_server_memory_and_persists_only_the_owner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agentix.toml");
        std::fs::write(
            &path,
            r#"[channel]
kind = "slack"

[channel.slack]
bot_token = "bot"
app_token = "app"
owner_user_ids = []

[agent]
kind = "codex"

[storage]
path = "/tmp/agentix-test.sqlite3"
"#,
        )
        .unwrap();
        let claims = std::sync::Arc::new(super::ClaimRegistry::default());
        let now = super::unix_timestamp().unwrap();
        let (code, _) = claims.generate(1, now).await.unwrap();
        let claimer = super::MemoryStringOwnerClaimer {
            kind: ImChannel::Slack,
            path: path.clone(),
            claims: claims.clone(),
        };

        assert!(
            !agentix_slack::SlackOwnerClaimer::claim(&claimer, "WRONG", "U1")
                .await
                .unwrap()
        );
        assert!(
            agentix_slack::SlackOwnerClaimer::claim(&claimer, &code, "U1")
                .await
                .unwrap()
        );

        let persisted = std::fs::read_to_string(path).unwrap();
        assert!(persisted.contains("owner_user_ids = [\"U1\"]"));
        assert!(!persisted.contains("claim_code"));
        assert!(!persisted.contains("claim_expires"));
        assert!(!claims.matches(&code, now).await);
    }

    #[test]
    fn builds_only_the_explicitly_selected_channel() {
        for (selected, expected) in [
            ("telegram", ChannelKind::Telegram),
            ("feishu", ChannelKind::Feishu),
            ("slack", ChannelKind::Slack),
        ] {
            let config = Config::from_toml(&format!(
                r#"
[channel]
kind = "{selected}"

[agent]
kind = "codex"

[storage]
path = "/tmp/agentix-test.sqlite3"

[channel.telegram]
token = "mock-token"
owner_user_ids = [42]

[channel.feishu]
app_id = "cli_mock"
app_secret = "mock-secret"
owner_open_ids = ["ou_owner"]
[channel.slack]
bot_token = "bot"
app_token = "app"
owner_user_ids = ["U1"]
"#
            ))
            .unwrap();
            let channels = super::build_channels(
                &config,
                std::path::Path::new("/tmp/agentix-config.toml"),
                std::sync::Arc::new(super::ClaimRegistry::default()),
            )
            .unwrap();

            assert_eq!(
                config.channel.kind,
                match selected {
                    "telegram" => ImChannel::Telegram,
                    "feishu" => ImChannel::Feishu,
                    "slack" => ImChannel::Slack,
                    _ => unreachable!(),
                }
            );
            assert_eq!(channels.len(), 1);
            assert_eq!(channels[0].kind(), expected);
        }
    }
    fn task_board_test_config(setting: Option<&str>, path: &Path) -> Config {
        let mut source = String::from(
            "[channel]\nkind='telegram'\n[channel.telegram]\ntoken='mock-token'\n\
             [agent]\nkind='codex'\n[storage]\npath='/tmp/agentix.sqlite3'\n",
        );
        if let Some(setting) = setting {
            source.push_str(&format!(
                "\n[task_board]\n{setting}\nconfig='{}'\n",
                path.display()
            ));
        }
        Config::from_toml(&source).unwrap()
    }

    #[tokio::test]
    async fn disabled_task_board_does_not_load_its_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing-taskix.toml");
        for setting in [None, Some(""), Some("enable = false")] {
            let config = task_board_test_config(setting, &path);
            assert!(super::build_task_board(&config).await.unwrap().is_none());
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        }
    }

    #[tokio::test]
    async fn enabled_task_board_requires_configuration_and_opens_its_database() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("taskix.toml");
        let database = directory.path().join("tasks.sqlite3");
        let config = task_board_test_config(Some("enable = true"), &path);
        assert!(super::build_task_board(&config).await.is_err());
        std::fs::write(
            &path,
            format!(
                "schema_version=1\n[storage]\npath='{}'\n\
                 [documents]\nroot='{}'\ndirectory='Tasks'\n",
                database.display(),
                directory.path().display(),
            ),
        )
        .unwrap();
        std::fs::create_dir(directory.path().join(".obsidian")).unwrap();
        let service = super::build_task_board(&config).await.unwrap().unwrap();
        assert_eq!(service.config().storage.path, database);
        assert!(database.is_file());
    }

    #[test]
    fn telegram_default_menu_adds_dashboard_only_when_task_board_is_enabled() {
        for (setting, enabled) in [
            (None, false),
            (Some(""), false),
            (Some("enable = false"), false),
            (Some("enable = true"), true),
        ] {
            let config = task_board_test_config(setting, Path::new("/missing/taskix.toml"));
            let commands = super::telegram_menu_commands(&config);
            let expected = if enabled {
                vec!["sessions", "dashboard", "cancel", "rmux", "help"]
            } else {
                vec!["sessions", "cancel", "rmux", "help"]
            };
            assert_eq!(
                commands
                    .iter()
                    .map(|c| c.command.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(commands.iter().any(|c| c.command == "dashboard"), enabled);
            assert!(
                !commands
                    .iter()
                    .any(|c| matches!(c.command.as_str(), "board" | "jobs" | "tasks"))
            );
        }
    }
}
