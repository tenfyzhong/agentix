mod control;
#[cfg(test)]
mod proxy_tests;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agentix::{
    AgentConfig, Config, ImChannel, LogRotation, LoggingConfig, NetworkConfig, TelegramConfig,
    add_feishu_owner, add_telegram_owner,
};
use agentix_codex::{CodexClient, CodexEndpoint};
use agentix_core::{AgentAdapter, AgentError, ChannelAdapter, Engine, EngineError, SqliteState};
use agentix_feishu::{FeishuAdapter, FeishuOwnerClaimer};
use agentix_pi::{PiFlavor, PiRpcAdapter};
use agentix_telegram::{TelegramAdapter, TelegramOwnerClaimer, TelegramPolicy};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use clap::{Parser, Subcommand};
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
    #[arg(short, long, value_name = "FILE", default_value_os_t = default_config_path())]
    config: PathBuf,
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Run the Agentix bridge until interrupted.
    Serve,
    /// Validate configuration, credentials, and the selected agent transport.
    Doctor,
    /// Use the running Agentix server for local diagnostics and setup.
    Client {
        #[command(subcommand)]
        command: ClientCommand,
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
    let config = Config::load(&config_path)?;
    let _log_guard = init_logging(&config.logging)?;
    match command {
        CliCommand::Serve => serve(config, &config_path).await,
        CliCommand::Doctor => doctor(&config).await,
        CliCommand::Client { command } => client(&config.server.endpoint, command).await,
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

async fn serve(config: Config, config_path: &Path) -> Result<()> {
    let BuiltAgent { adapter, codex } = build_agent(&config.agent).await?;
    let claims = Arc::new(ClaimRegistry::default());
    let channels = build_channels(&config, config_path, claims.clone())?;
    run_service_until_shutdown(
        adapter,
        codex,
        channels,
        config.storage.path,
        config.server.endpoint,
        config_path.to_owned(),
        claims,
        config.notifications.background_turns,
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
    state_path: PathBuf,
    control_endpoint: String,
    config_path: PathBuf,
    claims: Arc<ClaimRegistry>,
    background_turn_notifications: bool,
    channel_shutdown_grace: Duration,
    shutdown_signal: F,
) -> Result<()>
where
    F: Future<Output = Result<()>> + Send,
{
    if let Some(parent) = state_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create state directory {}", parent.display()))?;
    }
    let state = SqliteState::open(&state_path).await?;
    let engine = Arc::new(
        Engine::new(adapter.clone(), state, channels.clone())
            .with_background_turn_notifications(background_turn_notifications),
    );
    let restored = engine.restore_bindings().await?;
    tracing::info!(restored, "restored durable conversation bindings");

    let shutdown = CancellationToken::new();
    let (control_tx, control_rx) = mpsc::channel(32);
    let advertised_control_endpoint = control_endpoint.clone();
    let control_shutdown = shutdown.clone();
    let mut control_task = tokio::spawn(async move {
        control::serve(&control_endpoint, control_tx, control_shutdown).await
    });
    let control_handler_task = tokio::spawn(run_control_handler(
        control_rx,
        adapter.clone(),
        codex,
        claims,
        config_path,
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

    tracing::info!(endpoint = %advertised_control_endpoint, "Agentix is running");
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
    let _ = engine_task.await;
    match engine.prepare_shutdown().await {
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

async fn run_engine_loop(
    engine: Arc<Engine>,
    agent: Arc<dyn AgentAdapter>,
    mut inbound: mpsc::Receiver<agentix_core::InboundEnvelope>,
    shutdown: CancellationToken,
) {
    let mut events = agent.subscribe();
    let mut working_interval = tokio::time::interval(Duration::from_secs(1));
    working_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    working_interval.tick().await;
    let mut inbound_open = true;
    let mut events_open = true;
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = working_interval.tick() => {
                engine.refresh_working_turns().await;
            }
            envelope = inbound.recv(), if inbound_open => {
                let Some(envelope) = envelope else {
                    inbound_open = false;
                    continue;
                };
                if let Err(error) = engine.handle_inbound(envelope).await {
                    if is_empty_rollout_metadata_error(&error) {
                        tracing::debug!(%error, "inbound IM request failed");
                    } else {
                        tracing::warn!(%error, "inbound IM request failed");
                    }
                }
            }
            event = events.recv(), if events_open => match event {
                Ok(event) => {
                    if let Err(error) = engine.handle_agent_event(event).await {
                        tracing::warn!(%error, "agent event failed");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    tracing::warn!(count, "agent event consumer lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    events_open = false;
                }
            }
        }
        if !inbound_open && !events_open {
            break;
        }
    }
}

fn is_empty_rollout_metadata_error(error: &EngineError) -> bool {
    matches!(
        error,
        EngineError::Agent(AgentError::Rejected(message))
            if message.contains("failed to read session metadata")
                && message.contains("rollout at ")
                && message.ends_with(" is empty")
    )
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

    match &config.agent {
        AgentConfig::Codex {
            endpoint,
            command,
            rmux_directory,
        } => {
            let endpoint = CodexEndpoint::parse(endpoint)?;
            let client = CodexClient::connect_with_command_and_rmux_directory(
                endpoint,
                command,
                rmux_directory,
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
        } => doctor_pi(PiFlavor::Pi, command, session_dir)?,
        AgentConfig::OhMyPi {
            command,
            session_dir,
        } => doctor_pi(PiFlavor::OhMyPi, command, session_dir)?,
    }
    tracing::info!("Agentix diagnostics completed");
    Ok(())
}

struct BuiltAgent {
    adapter: Arc<dyn AgentAdapter>,
    codex: Option<CodexClient>,
}

async fn build_agent(config: &AgentConfig) -> Result<BuiltAgent> {
    match config {
        AgentConfig::Codex {
            endpoint,
            command,
            rmux_directory,
        } => {
            let endpoint = CodexEndpoint::parse(endpoint)?;
            let client = CodexClient::connect_with_command_and_rmux_directory(
                endpoint,
                command,
                rmux_directory,
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
        } => Ok(BuiltAgent {
            adapter: Arc::new(PiRpcAdapter::new(PiFlavor::Pi, command, session_dir)),
            codex: None,
        }),
        AgentConfig::OhMyPi {
            command,
            session_dir,
        } => Ok(BuiltAgent {
            adapter: Arc::new(PiRpcAdapter::new(PiFlavor::OhMyPi, command, session_dir)),
            codex: None,
        }),
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
struct MemoryFeishuOwnerClaimer {
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
impl FeishuOwnerClaimer for MemoryFeishuOwnerClaimer {
    async fn claim(&self, code: &str, owner_open_id: &str) -> std::result::Result<bool, String> {
        let now = unix_timestamp().map_err(|error| error.to_string())?;
        if !self.claims.matches(code, now).await {
            return Ok(false);
        }
        let path = self.path.clone();
        let code = code.to_owned();
        let owner_open_id = owner_open_id.to_owned();
        tokio::task::spawn_blocking(move || add_feishu_owner(&path, &owner_open_id))
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

async fn run_control_handler(
    mut calls: mpsc::Receiver<control::ControlCall>,
    agent: Arc<dyn AgentAdapter>,
    codex: Option<CodexClient>,
    claims: Arc<ClaimRegistry>,
    config_path: PathBuf,
) {
    while let Some(call) = calls.recv().await {
        let response = handle_control_request(
            call.request.clone(),
            &agent,
            codex.as_ref(),
            &claims,
            &config_path,
        )
        .await;
        call.respond(response);
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
            Ok(json!({
                "command": format!("/claim {code}"),
                "expiresAt": expires_at,
            }))
        }
    }
}

fn build_channels(
    config: &Config,
    config_path: &Path,
    claims: Arc<ClaimRegistry>,
) -> Result<Vec<Arc<dyn ChannelAdapter>>> {
    let channel: Arc<dyn ChannelAdapter> = match config.channel.kind {
        ImChannel::Telegram => {
            let telegram = config
                .channel
                .telegram
                .as_ref()
                .expect("configuration was validated");
            let mut adapter = TelegramAdapter::with_bot(
                build_telegram_bot(telegram, &config.network)?,
                TelegramPolicy::new(telegram.owner_user_ids.iter().copied()),
            );
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
                adapter = adapter.with_owner_claimer(Arc::new(MemoryFeishuOwnerClaimer {
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

fn doctor_pi(flavor: PiFlavor, command: &Path, session_dir: &Path) -> Result<()> {
    let executable = resolve_executable(command)
        .with_context(|| format!("{} command was not found", flavor.default_command()))?;
    if !session_dir.is_dir() {
        bail!(
            "session directory does not exist: {}",
            session_dir.display()
        );
    }
    let count = agentix_pi::discover_sessions(session_dir)?.len();
    println!(
        "ok: {} at {} with {} discoverable sessions",
        flavor.default_command(),
        executable.display(),
        count
    );
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
    use std::sync::{Arc, Mutex as StdMutex};

    use agentix::{Config, ImChannel};
    use agentix_core::{
        AgentAdapter, AgentError, AgentEvent, ChannelAdapter, ChannelError, ChannelKind,
        ConversationRef, EngineError, HistoryPage, InteractionDecision, MessageRef, OutboundView,
        SessionId, SessionPage, SessionStatus, SessionSummary,
    };
    use async_trait::async_trait;
    use time::format_description::well_known::Rfc3339;
    use tokio::sync::broadcast;
    use tokio_util::sync::CancellationToken;
    use tracing_subscriber::fmt::{format::Writer, time::FormatTime};

    struct LifecycleAgent {
        events: broadcast::Sender<AgentEvent>,
        attached: StdMutex<Vec<String>>,
    }

    impl LifecycleAgent {
        fn new() -> Self {
            let (events, _) = broadcast::channel(8);
            Self {
                events,
                attached: StdMutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl AgentAdapter for LifecycleAgent {
        fn display_name(&self) -> &'static str {
            "Codex"
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
            _text: &str,
        ) -> Result<String, AgentError> {
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

    struct LifecycleChannel {
        views: StdMutex<Vec<OutboundView>>,
        stop_on_shutdown: bool,
    }

    impl LifecycleChannel {
        fn new() -> Self {
            Self {
                views: StdMutex::new(Vec::new()),
                stop_on_shutdown: true,
            }
        }

        fn stubborn() -> Self {
            Self {
                views: StdMutex::new(Vec::new()),
                stop_on_shutdown: false,
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
            self.views.lock().unwrap().push(view.clone());
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
                state_path.clone(),
                format!("tcp://{}", unused_loopback_address()),
                config_path.clone(),
                Arc::new(super::ClaimRegistry::default()),
                true,
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
            directory.path().join("agentix.sqlite3"),
            format!("tcp://{}", unused_loopback_address()),
            directory.path().join("config.toml"),
            Arc::new(super::ClaimRegistry::default()),
            true,
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
        let claimer = super::MemoryFeishuOwnerClaimer {
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

    #[test]
    fn empty_rollout_metadata_errors_are_low_priority() {
        let empty = EngineError::Agent(AgentError::Rejected(
            "-32603: failed to read thread: thread-store internal error: failed to read session metadata /tmp/rollout.jsonl: rollout at /tmp/rollout.jsonl is empty".into(),
        ));
        let other = EngineError::Agent(AgentError::Rejected(
            "-32603: database connection failed".into(),
        ));

        assert!(super::is_empty_rollout_metadata_error(&empty));
        assert!(!super::is_empty_rollout_metadata_error(&other));
    }

    #[test]
    fn builds_only_the_explicitly_selected_channel() {
        for (selected, expected) in [
            ("telegram", ChannelKind::Telegram),
            ("feishu", ChannelKind::Feishu),
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
                    _ => unreachable!(),
                }
            );
            assert_eq!(channels.len(), 1);
            assert_eq!(channels[0].kind(), expected);
        }
    }
}
