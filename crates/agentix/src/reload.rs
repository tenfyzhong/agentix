//! A stable control listener supervising replaceable, validated service generations.
use super::{
    BuiltAgent, ClaimRegistry, control, spawn_startup_notifications, wait_for_channel_shutdown,
};
use agentix::{AgentConfig, Config, run_engine_loop_with_config};
use agentix_bridge::BridgeHub;
use agentix_codex::{CodexClient, ProxyOptions};
use agentix_core::{AgentAdapter, ChannelAdapter, Engine, RestoredBindings, SqliteState};
use anyhow::{Context, Result, bail};
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};

const RELOAD_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct PreparedService {
    pub config: Config,
    pub adapter: Arc<dyn AgentAdapter>,
    pub backends: Vec<(AgentConfig, BuiltAgent)>,
    pub notification_setting: Arc<std::sync::atomic::AtomicBool>,
    codex: Option<CodexClient>,
    engine: Arc<Engine>,
    pub channels: Vec<Arc<dyn ChannelAdapter>>,
    identities: std::collections::HashMap<agentix_core::ChannelKind, Option<String>>,
}

impl PreparedService {
    pub async fn new(
        config: Config,
        adapter: Arc<dyn AgentAdapter>,
        codex: Option<CodexClient>,
        channels: Vec<Arc<dyn ChannelAdapter>>,
        task_board: Option<Arc<agentix_task::Service>>,
        backends: Vec<(AgentConfig, BuiltAgent)>,
    ) -> Result<Self> {
        let started = Instant::now();
        if let Some(parent) = config.storage.path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("failed to create state directory {}", parent.display())
            })?;
        }
        let state = SqliteState::open(&config.storage.path).await?;
        let kinds = adapter.session_backends();
        if !kinds.is_empty() {
            state
                .qualify_sessions((kinds.len() == 1).then(|| kinds[0]))
                .await?;
        }
        tracing::info!(
            phase = "state_storage",
            elapsed_ms = started.elapsed().as_millis(),
            "startup phase completed"
        );
        let mut engine = Engine::new(adapter.clone(), state, channels.clone())
            .with_multiplexer_kind(config.multiplexer.resolved_kind)
            .with_background_turn_notifications(config.notifications.background_turns)
            .with_output(config.output);
        if let Some(task_board) = task_board {
            engine = engine
                .with_task_board(task_board)
                .with_task_consumer(config.storage.path.to_string_lossy().into_owned());
        }
        Ok(Self {
            notification_setting: Arc::new(std::sync::atomic::AtomicBool::new(
                config.notifications.background_turns,
            )),
            config,
            adapter,
            backends,
            codex,
            engine: Arc::new(engine),
            channels,
            identities: std::collections::HashMap::new(),
        })
    }
}

pub(super) fn load_candidate(
    path: &Path,
    current: &Config,
    proxy: &ProxyOptions,
) -> Result<Config> {
    let mut next = Config::load(path)?;
    next.apply_codex_proxy_options(proxy)?;
    next.multiplexer.resolved_kind = current.multiplexer.resolved_kind;
    if next.multiplexer != current.multiplexer {
        bail!("changing multiplexer requires restarting agentix serve");
    }
    if next.server.endpoint != current.server.endpoint {
        bail!("changing server.endpoint requires restarting agentix serve");
    }
    if next.storage.path != current.storage.path {
        bail!("changing storage.path requires restarting agentix serve");
    }
    if next.logging != current.logging {
        bail!("changing logging requires restarting agentix serve");
    }
    // A running Codex owns its proxy socket and child process. Rebinding it during
    // preparation would conflict with the live instance and disconnect terminals.
    for old in current.selected_agents() {
        if matches!(old, AgentConfig::Codex { .. }) && !next.selected_agents().contains(&old) {
            bail!(
                "changing or removing the running Codex backend requires restarting agentix serve"
            );
        }
    }
    Ok(next)
}

struct RunningChannel {
    adapter: Arc<dyn ChannelAdapter>,
    shutdown: CancellationToken,
    task: JoinHandle<()>,
}

impl RunningChannel {
    fn start(
        adapter: Arc<dyn ChannelAdapter>,
        inbound: mpsc::Sender<agentix_core::InboundEnvelope>,
    ) -> Self {
        let shutdown = CancellationToken::new();
        let task = tokio::spawn({
            let channel = adapter.clone();
            let token = shutdown.clone();
            async move {
                if let Err(error) = channel.run(inbound, token).await {
                    tracing::error!(%error, channel = %channel.kind(), "IM channel stopped");
                }
            }
        });
        Self {
            adapter,
            shutdown,
            task,
        }
    }
}

struct RunningService {
    prepared: Arc<PreparedService>,
    shutdown: CancellationToken,
    control: mpsc::Sender<control::ControlCall>,
    handler: JoinHandle<()>,
    engine: JoinHandle<()>,
    inbound: mpsc::Sender<agentix_core::InboundEnvelope>,
    settings: tokio::sync::watch::Sender<Arc<Engine>>,
    control_settings: tokio::sync::watch::Sender<super::control_runtime::ControlConfig>,
    channels: Vec<RunningChannel>,
    startup: Option<AbortOnDropHandle<()>>,
}

impl RunningService {
    fn start(
        prepared: Arc<PreparedService>,
        updates: Option<RestoredBindings>,
        claims: Arc<ClaimRegistry>,
        path: PathBuf,
    ) -> Self {
        let shutdown = CancellationToken::new();
        let (control, calls) = mpsc::channel(32);
        let (control_settings, control_snapshots) =
            tokio::sync::watch::channel((prepared.adapter.clone(), prepared.codex.clone()));
        let handler = tokio::spawn(super::control_runtime::run_control_handler_with_config(
            calls,
            control_snapshots,
            claims,
            path,
            shutdown.clone(),
        ));
        let (inbound, receiver) = mpsc::channel(256);
        let channels = prepared
            .channels
            .iter()
            .map(|channel| RunningChannel::start(channel.clone(), inbound.clone()))
            .collect();
        let (settings, snapshots) = tokio::sync::watch::channel(prepared.engine.clone());
        let engine = tokio::spawn(run_engine_loop_with_config(
            snapshots,
            prepared.adapter.clone(),
            receiver,
            shutdown.clone(),
        ));
        let startup =
            updates.map(|updates| spawn_startup_notifications(prepared.engine.clone(), updates));
        Self {
            prepared,
            shutdown,
            control,
            handler,
            engine,
            inbound,
            settings,
            control_settings,
            channels,
            startup,
        }
    }

    async fn replace(&mut self, prepared: PreparedService, grace: Duration) {
        // The admission queues and their workers survive the switch. Only changed
        // transports are stopped; old snapshots stay alive until their work finishes.
        let mut retained = Vec::new();
        let mut retiring = Vec::new();
        for channel in self.channels.drain(..) {
            if !channel.task.is_finished()
                && prepared
                    .channels
                    .iter()
                    .any(|next| Arc::ptr_eq(next, &channel.adapter))
            {
                retained.push(channel);
            } else {
                channel.shutdown.cancel();
                retiring.push(channel.task);
            }
        }
        // Avoid competing consumers (notably Telegram getUpdates) on one bot.
        // The engine continues draining the shared inbound queue during handoff.
        wait_for_channel_shutdown(retiring, grace).await;
        for channel in &prepared.channels {
            if !retained
                .iter()
                .any(|running| Arc::ptr_eq(channel, &running.adapter))
            {
                retained.push(RunningChannel::start(channel.clone(), self.inbound.clone()));
            }
        }
        let owners = configured_owners(&prepared.config);
        for channel in &prepared.channels {
            channel.replace_owners(&owners).await;
        }
        prepared.notification_setting.store(
            prepared.config.notifications.background_turns,
            std::sync::atomic::Ordering::Relaxed,
        );
        self.settings.send_replace(prepared.engine.clone());
        self.control_settings
            .send_replace((prepared.adapter.clone(), prepared.codex.clone()));
        self.prepared = Arc::new(prepared);
        self.channels = retained;
    }

    async fn stop(self, grace: Duration) {
        self.shutdown.cancel();
        if let Some(startup) = self.startup {
            startup.abort();
            let _ = startup.await;
        }
        let _ = self.engine.await;
        match agentix::shutdown_engine(self.prepared.engine.clone(), grace).await {
            Ok(notified) => {
                tracing::info!(notified, "saved bindings and notified IM conversations");
            }
            Err(error) => tracing::error!(%error, "failed to finish graceful shutdown preparation"),
        }
        let tasks = self
            .channels
            .into_iter()
            .map(|channel| {
                channel.shutdown.cancel();
                channel.task
            })
            .collect();
        wait_for_channel_shutdown(tasks, grace).await;
        let _ = self.handler.await;
    }
}

fn configured_owners(config: &Config) -> Vec<String> {
    match config.channel.kind {
        agentix_core::ChannelKind::Telegram => config
            .channel
            .telegram
            .as_ref()
            .unwrap()
            .owner_user_ids
            .iter()
            .map(ToString::to_string)
            .collect(),
        agentix_core::ChannelKind::Feishu => config
            .channel
            .feishu
            .as_ref()
            .unwrap()
            .owner_open_ids
            .clone(),
        agentix_core::ChannelKind::Slack => config
            .channel
            .slack
            .as_ref()
            .unwrap()
            .owner_user_ids
            .clone(),
    }
}

async fn prepare_reload(
    mut next: PreparedService,
    previous: Arc<PreparedService>,
) -> Result<PreparedService> {
    next.identities = previous.identities.clone();
    for channel in &next.channels {
        if previous
            .channels
            .iter()
            .any(|old| Arc::ptr_eq(old, channel))
        {
            continue;
        }
        channel.prepare_connection().await?;
        let identity = channel.identity().await?;
        if let Some(old_identity) = previous.identities.get(&channel.kind())
            && &identity != old_identity
        {
            bail!("changing the bot identity requires restarting agentix serve");
        }
        next.identities.insert(channel.kind(), identity);
    }
    Arc::get_mut(&mut next.engine)
        .context("replacement engine is already running")?
        .inherit_runtime(&previous.engine);
    Ok(next)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn run<B, BF, S>(
    mut initial: PreparedService,
    path: PathBuf,
    proxy: ProxyOptions,
    bridge: Option<Arc<BridgeHub>>,
    claims: Arc<ClaimRegistry>,
    mut build: B,
    signal: S,
    grace: Duration,
) -> Result<()>
where
    B: FnMut(Config, Arc<PreparedService>) -> BF,
    BF: Future<Output = Result<PreparedService>>,
    S: Future<Output = Result<()>> + Send,
{
    let started = Instant::now();
    let updates = initial.engine.restore_bindings_deferred().await?;
    for channel in &initial.channels {
        initial
            .identities
            .insert(channel.kind(), channel.identity().await?);
    }
    tracing::info!(
        restored = updates.restored_count(),
        phase = "binding_restore",
        elapsed_ms = started.elapsed().as_millis(),
        "restored durable conversation bindings"
    );
    let endpoint = initial.config.server.endpoint.clone();
    let shutdown = CancellationToken::new();
    let (sender, mut calls) = mpsc::channel(32);
    let mut listener = tokio::spawn({
        let shutdown = shutdown.clone();
        let endpoint = endpoint.clone();
        async move { control::serve_with_bridge(&endpoint, sender, shutdown, bridge).await }
    });
    let mut running = RunningService::start(
        Arc::new(initial),
        Some(updates),
        claims.clone(),
        path.clone(),
    );
    let mut pending = None::<(
        control::ControlCall,
        std::pin::Pin<Box<tokio::time::Timeout<BF>>>,
    )>;
    let mut validating = None::<(
        control::ControlCall,
        std::pin::Pin<Box<dyn Future<Output = Result<PreparedService>> + Send>>,
    )>;
    tokio::pin!(signal);
    tracing::info!(%endpoint, "Agentix is running");
    let mut listener_finished = false;
    let result = loop {
        tokio::select! {
            biased;
            result = &mut signal => break result,
            () = running.shutdown.cancelled() => break Err(anyhow::anyhow!("Agentix engine stopped unexpectedly")),
            result = &mut listener => {
                listener_finished = true;
                break match result {
                    Ok(Err(error)) => Err(error),
                    other => Err(anyhow::anyhow!("Agentix control server stopped unexpectedly: {other:?}")),
                };
            }
            result = async { pending.as_mut().expect("pending preparation").1.as_mut().await }, if pending.is_some() => {
                let (call, _) = pending.take().expect("completed preparation");
                match result.context("preparing the replacement timed out").and_then(|result| result) {
                    Err(error) => call.respond(Err(format!("configuration reload failed: {error:#}"))),
                    Ok(prepared) => {
                        let previous = running.prepared.clone();
                        validating = Some((call, Box::pin(async move {
                            tokio::time::timeout(RELOAD_TIMEOUT, prepare_reload(prepared, previous)).await.context("validating the replacement timed out")?
                        })));
                    }
                }
            }
            result = async { validating.as_mut().expect("pending validation").1.as_mut().await }, if validating.is_some() => {
                let (call, _) = validating.take().expect("completed validation");
                match result {
                    Err(error) => call.respond(Err(format!("configuration reload failed: {error:#}"))),
                    Ok(prepared) => {
                        running.replace(prepared, grace).await;
                        call.respond(Ok(serde_json::json!({"reloaded": true, "config": path})));
                        tracing::info!(config = %path.display(), "configuration reloaded");
                    }
                }
            }
            call = calls.recv() => {
                let Some(call) = call else { break Err(anyhow::anyhow!("control request channel closed")); };
                if call.request == control::ControlRequest::Reload {
                    if pending.is_some() || validating.is_some() {
                        call.respond(Err("configuration reload is already in progress".into()));
                        continue;
                    }
                    match load_candidate(&path, &running.prepared.config, &proxy) {
                        Err(error) => call.respond(Err(format!("configuration reload failed: {error:#}"))),
                        Ok(config) => {
                            let future = build(config, running.prepared.clone());
                            pending = Some((call, Box::pin(tokio::time::timeout(RELOAD_TIMEOUT, future))));
                        }
                    }
                } else if let Err(error) = running.control.try_send(call) {
                    error.into_inner().respond(Err("Agentix control queue is busy; retry the request".into()));
                }
            }
        }
    };
    drop(pending);
    drop(validating);
    shutdown.cancel();
    running.stop(grace).await;
    if !listener_finished {
        let _ = listener.await;
    }
    result
}

/// Compare connection settings, excluding the authorization policy updated in place.
pub(super) fn same_channel_connection(old: &Config, next: &Config) -> bool {
    if old.channel.kind != next.channel.kind || old.network.proxy != next.network.proxy {
        return false;
    }
    match old.channel.kind {
        agentix_core::ChannelKind::Telegram => {
            match (&old.channel.telegram, &next.channel.telegram) {
                (Some(a), Some(b)) => a.token == b.token,
                _ => false,
            }
        }
        agentix_core::ChannelKind::Feishu => match (&old.channel.feishu, &next.channel.feishu) {
            (Some(a), Some(b)) => a.app_id == b.app_id && a.app_secret == b.app_secret,
            _ => false,
        },
        agentix_core::ChannelKind::Slack => match (&old.channel.slack, &next.channel.slack) {
            (Some(a), Some(b)) => {
                a.app_id == b.app_id
                    && a.bot_token == b.bot_token
                    && a.app_token == b.app_token
                    && a.command_prefix == b.command_prefix
                    && a.command_suffix == b.command_suffix
                    && old.slack_cli_path == next.slack_cli_path
            }
            _ => false,
        },
    }
}

#[cfg(test)]
#[path = "reload_tests.rs"]
mod tests;
