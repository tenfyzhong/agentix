//! A stable control listener supervising replaceable, validated service generations.
use super::{
    BuiltAgent, ClaimRegistry, control, run_control_handler, spawn_startup_notifications,
    wait_for_channel_shutdown,
};
use agentix::{AgentConfig, Config, run_engine_loop};
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
    channels: Vec<Arc<dyn ChannelAdapter>>,
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

struct RunningService {
    prepared: Arc<PreparedService>,
    shutdown: CancellationToken,
    control: mpsc::Sender<control::ControlCall>,
    handler: JoinHandle<()>,
    engine: JoinHandle<()>,
    channels: Vec<JoinHandle<()>>,
    startup: Option<AbortOnDropHandle<()>>,
}

impl RunningService {
    fn start(
        prepared: Arc<PreparedService>,
        updates: Option<RestoredBindings>,
        claims: Arc<ClaimRegistry>,
        path: PathBuf,
    ) -> Self {
        prepared.notification_setting.store(
            prepared.config.notifications.background_turns,
            std::sync::atomic::Ordering::Relaxed,
        );
        let shutdown = CancellationToken::new();
        let (control, calls) = mpsc::channel(32);
        let handler = tokio::spawn(run_control_handler(
            calls,
            prepared.adapter.clone(),
            prepared.codex.clone(),
            claims,
            path,
            shutdown.clone(),
        ));
        let (inbound, receiver) = mpsc::channel(256);
        let channels = prepared
            .channels
            .iter()
            .map(|channel| {
                let channel = channel.clone();
                let inbound = inbound.clone();
                let token = shutdown.clone();
                tokio::spawn(async move {
                    if let Err(error) = channel.run(inbound, token).await {
                        tracing::error!(%error, channel = %channel.kind(), "IM channel stopped");
                    }
                })
            })
            .collect();
        let engine = tokio::spawn(run_engine_loop(
            prepared.engine.clone(),
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
            channels,
            startup,
        }
    }

    async fn stop(self, final_shutdown: bool, grace: Duration) -> Arc<PreparedService> {
        self.shutdown.cancel();
        if let Some(startup) = self.startup {
            startup.abort();
            let _ = startup.await;
        }
        let _ = self.engine.await;
        if final_shutdown {
            match agentix::shutdown_engine(self.prepared.engine.clone(), grace).await {
                Ok(notified) => {
                    tracing::info!(notified, "saved bindings and notified IM conversations");
                }
                Err(error) => {
                    tracing::error!(%error, "failed to finish graceful shutdown preparation");
                }
            }
        }
        wait_for_channel_shutdown(self.channels, grace).await;
        let _ = self.handler.await;
        self.prepared
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn run<B, BF, S>(
    initial: PreparedService,
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
                let prepared = result.context("preparing the replacement timed out").and_then(|result| result);
                match prepared {
                    Err(error) => call.respond(Err(format!("configuration reload failed: {error:#}"))),
                    Ok(prepared) => {
                        // Drain the old generation before restoring the replacement.
                        // Do not detach upstream sessions or send offline notices.
                        let previous = running.stop(false, grace).await;
                        match tokio::time::timeout(RELOAD_TIMEOUT, prepared.engine.restore_bindings_deferred()).await {
                            Ok(Ok(updates)) => {
                                running = RunningService::start(Arc::new(prepared), Some(updates), claims.clone(), path.clone());
                                call.respond(Ok(serde_json::json!({"reloaded": true, "config": path})));
                                tracing::info!(config = %path.display(), "configuration reloaded");
                            }
                            failed => {
                                running = RunningService::start(previous, None, claims.clone(), path.clone());
                                let error = match failed {
                                    Ok(Err(error)) => error.to_string(),
                                    Err(_) => "restoring the replacement timed out".into(),
                                    Ok(Ok(_)) => unreachable!(),
                                };
                                call.respond(Err(format!("configuration reload failed: {error}")));
                            }
                        }
                    }
                }
            }
            call = calls.recv() => {
                let Some(call) = call else { break Err(anyhow::anyhow!("control request channel closed")); };
                if call.request == control::ControlRequest::Reload {
                    if pending.is_some() {
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
    shutdown.cancel();
    running.stop(true, grace).await;
    if !listener_finished {
        let _ = listener.await;
    }
    result
}
