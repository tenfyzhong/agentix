use super::*;
use agentix_core::{
    ChannelError, ChannelKind, ConversationRef, InboundEnvelope, MessageRef, OutboundView,
};
use async_trait::async_trait;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Default)]
struct Traffic {
    active: AtomicUsize,
    peak: AtomicUsize,
    delivered: Mutex<Vec<String>>,
}

struct ReceiverChannel {
    traffic: Arc<Traffic>,
    source: Arc<tokio::sync::Mutex<mpsc::Receiver<InboundEnvelope>>>,
    fail: Arc<AtomicBool>,
}

#[async_trait]
impl ChannelAdapter for ReceiverChannel {
    fn kind(&self) -> ChannelKind {
        ChannelKind::Telegram
    }
    async fn run(
        &self,
        inbound: mpsc::Sender<InboundEnvelope>,
        shutdown: CancellationToken,
    ) -> std::result::Result<(), ChannelError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(ChannelError::Rejected(
                "socket handshake failed after preflight".into(),
            ));
        }
        let active = self.traffic.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.traffic.peak.fetch_max(active, Ordering::SeqCst);
        let mut source = self.source.lock().await;
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                message = source.recv() => match message {
                    Some(message) => { inbound.send(message).await.unwrap(); }
                    None => break,
                }
            }
        }
        self.traffic.active.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }
    async fn send(
        &self,
        conversation: &ConversationRef,
        _: &OutboundView,
    ) -> std::result::Result<MessageRef, ChannelError> {
        self.traffic
            .delivered
            .lock()
            .unwrap()
            .push(conversation.conversation_id.clone());
        Ok(MessageRef::new(conversation.clone(), "reply"))
    }
    async fn update(
        &self,
        _: &ConversationRef,
        _: &MessageRef,
        _: &OutboundView,
    ) -> std::result::Result<(), ChannelError> {
        Ok(())
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    config: Config,
    traffic: Arc<Traffic>,
    sender: mpsc::Sender<InboundEnvelope>,
    source: Arc<tokio::sync::Mutex<mpsc::Receiver<InboundEnvelope>>>,
    agent: Arc<crate::tests::LifecycleAgent>,
    failure: Arc<AtomicBool>,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, format!("[channel]\nkind='telegram'\n[channel.telegram]\ntoken='mock'\nowner_user_ids=[42]\n[agent]\nkind='codex'\n[storage]\npath='{}'\n", directory.path().join("state.sqlite3").display())).unwrap();
        let (sender, source) = mpsc::channel(256);
        Self {
            config: Config::load(&path).unwrap(),
            directory,
            traffic: Arc::default(),
            sender,
            source: Arc::new(tokio::sync::Mutex::new(source)),
            agent: Arc::new(crate::tests::LifecycleAgent::new()),
            failure: Arc::default(),
        }
    }
    async fn prepare(&self, fail: bool) -> PreparedService {
        self.failure.store(fail, Ordering::SeqCst);
        let channel = Arc::new(ReceiverChannel {
            traffic: self.traffic.clone(),
            source: self.source.clone(),
            fail: self.failure.clone(),
        });
        PreparedService::new(
            self.config.clone(),
            self.agent.clone(),
            None,
            vec![channel],
            None,
            vec![],
        )
        .await
        .unwrap()
    }
    async fn start(&self) -> RunningService {
        let running = RunningService::start(
            Arc::new(self.prepare(false).await),
            None,
            Arc::new(ClaimRegistry::default()),
            self.directory.path().join("config.toml"),
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.traffic.active.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        running
    }
    async fn send(&self, id: usize) {
        self.sender
            .send(InboundEnvelope::text(
                format!("message-{id}"),
                ConversationRef::new(ChannelKind::Telegram, format!("chat-{id}")),
                "42",
                "/help",
            ))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn reload_handoff_under_traffic_delivers_each_message_once_without_overlapping_receivers() {
    let fixture = Fixture::new();
    let mut running = fixture.start().await;
    for round in 0..16 {
        for id in round * 8..(round + 1) * 8 {
            fixture.send(id).await;
        }
        let next = prepare_reload(fixture.prepare(false).await, running.prepared.clone())
            .await
            .unwrap();
        running.replace(next, Duration::from_secs(1)).await;
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while fixture.traffic.delivered.lock().unwrap().len() < 128 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    running.stop(Duration::from_secs(1)).await;
    let mut delivered = fixture.traffic.delivered.lock().unwrap().clone();
    delivered.sort();
    let mut expected = (0..128).map(|id| format!("chat-{id}")).collect::<Vec<_>>();
    expected.sort();
    assert_eq!(delivered, expected);
    assert_eq!(fixture.traffic.peak.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.traffic.active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn reload_connection_failure_after_preflight_keeps_control_alive_and_allows_repair() {
    let fixture = Fixture::new();
    let mut running = fixture.start().await;
    let next = prepare_reload(fixture.prepare(true).await, running.prepared.clone())
        .await
        .unwrap();
    running.replace(next, Duration::from_secs(1)).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while !running.channels[0].task.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!running.engine.is_finished());
    assert!(!running.handler.is_finished());
    fixture.failure.store(false, Ordering::SeqCst);
    // build_service reuses the adapter when the connection parameters are unchanged.
    let prepared = PreparedService::new(
        fixture.config.clone(),
        fixture.agent.clone(),
        None,
        running.prepared.channels.clone(),
        None,
        vec![],
    )
    .await
    .unwrap();
    let next = prepare_reload(prepared, running.prepared.clone())
        .await
        .unwrap();
    running.replace(next, Duration::from_secs(1)).await;
    fixture.send(0).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.traffic.delivered.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    running.stop(Duration::from_secs(1)).await;
    assert_eq!(*fixture.traffic.delivered.lock().unwrap(), ["chat-0"]);
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_signal_child() {
    let Some(directory) = std::env::var_os("AGENTIX_SHUTDOWN_TEST_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    let mut fixture = Fixture::new();
    fixture.config.server.endpoint = format!("unix://{}", directory.join("control.sock").display());
    let proxy = agentix_codex::CodexProxy::bind(
        &format!(
            "unix://{}",
            directory.join("app-server-control.sock").display()
        ),
        &format!("unix://{}", directory.join("upstream.sock").display()),
    )
    .await
    .unwrap();
    run(
        fixture.prepare(false).await,
        fixture.directory.path().join("config.toml"),
        ProxyOptions::default(),
        None,
        Arc::new(ClaimRegistry::default()),
        |_, _| async { anyhow::bail!("reload is not used by this fixture") },
        crate::shutdown_signal(),
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    drop(proxy);
}

#[cfg(unix)]
async fn assert_signal_cleans_sockets(signal: &str) {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let control = directory.path().join("control.sock");
    let proxy = directory.path().join("app-server-control.sock");
    // Two runs on the same paths verify restart without manual cleanup.
    for _ in 0..2 {
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "reload::tests::shutdown_signal_child",
                "--nocapture",
            ])
            .env("AGENTIX_SHUTDOWN_TEST_DIR", directory.path())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "service exited before readiness"
                );
                if proxy.exists()
                    && control::request(
                        &format!("unix://{}", control.display()),
                        &control::ControlRequest::Sessions {
                            cursor: None,
                            limit: 1,
                        },
                    )
                    .await
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("service must become ready");
        assert!(
            tokio::process::Command::new("kill")
                .args([signal, &child.id().unwrap().to_string()])
                .status()
                .await
                .unwrap()
                .success()
        );
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .expect("service must stop promptly")
            .unwrap();
        assert!(
            !control.exists(),
            "control.sock remains after {signal}: {status}"
        );
        assert!(
            !proxy.exists(),
            "app-server-control.sock remains after {signal}: {status}"
        );
        assert!(
            status.success(),
            "service must exit normally after {signal}: {status}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn sigterm_removes_control_sockets_and_allows_restart() {
    assert_signal_cleans_sockets("-TERM").await;
}

#[cfg(unix)]
#[tokio::test]
async fn sigint_removes_control_sockets_and_allows_restart() {
    assert_signal_cleans_sockets("-INT").await;
}

#[cfg(unix)]
#[tokio::test]
async fn sighup_removes_control_sockets_and_allows_restart() {
    assert_signal_cleans_sockets("-HUP").await;
}

#[cfg(unix)]
#[tokio::test]
async fn sigquit_removes_control_sockets_and_allows_restart() {
    assert_signal_cleans_sockets("-QUIT").await;
}

#[cfg(unix)]
#[tokio::test]
async fn sigusr1_removes_control_sockets_and_allows_restart() {
    assert_signal_cleans_sockets("-USR1").await;
}

#[cfg(unix)]
#[tokio::test]
async fn sigusr2_removes_control_sockets_and_allows_restart() {
    assert_signal_cleans_sockets("-USR2").await;
}

#[cfg(unix)]
#[tokio::test]
async fn sigalrm_removes_control_sockets_and_allows_restart() {
    assert_signal_cleans_sockets("-ALRM").await;
}

#[test]
fn multiplexer_changes_require_restart() {
    let fixture = Fixture::new();
    let path = fixture.directory.path().join("config.toml");
    let original = std::fs::read_to_string(&path).unwrap();
    assert!(load_candidate(&path, &fixture.config, &ProxyOptions::default()).is_ok());
    for settings in ["kind='tmux'", "working_dir='/another-workspace'"] {
        std::fs::write(&path, format!("{original}\n[multiplexer]\n{settings}\n")).unwrap();
        let error = load_candidate(&path, &fixture.config, &ProxyOptions::default()).unwrap_err();
        assert!(
            error.to_string().contains("changing multiplexer"),
            "{error}"
        );
    }
}
