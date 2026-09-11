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
