//! Bounded application and own-process instrumentation. No host or exporter metrics.
mod process;
mod resources;
mod server;
use prometheus_client::{
    encoding::{EncodeLabelSet, text::encode},
    metrics::{counter::Counter, family::Family, gauge::Gauge, histogram::Histogram},
    registry::Registry,
};
use serde::Deserialize;
pub use server::Exporter;
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub listen: SocketAddr,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: ([127, 0, 0, 1], 9464).into(),
        }
    }
}
impl Config {
    #[must_use]
    pub fn taskix() -> Self {
        Self {
            listen: ([127, 0, 0, 1], 9465).into(),
            ..Self::default()
        }
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.listen.ip().is_loopback(),
            "metrics.listen must use a loopback address"
        );
        Ok(())
    }
}
#[derive(Clone, Copy)]
pub enum Service {
    Agentix,
    Taskix,
}
impl Service {
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Agentix => "agentix",
            Self::Taskix => "taskix",
        }
    }
}

macro_rules! operations {
    ($($variant:ident => ($component:literal, $operation:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug)]
        #[repr(usize)]
        pub enum Operation { $($variant),+ }
        impl Operation {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
            fn labels(self) -> Labels { match self { $(Self::$variant => Labels { component:$component, operation:$operation }),+ } }
        }
    }
}
operations! {
    Engine=>("engine","dispatch"), Control=>("control","dispatch"),
    CodexRpc=>("codex","rpc"), PiRpc=>("pi","rpc"), OmpRpc=>("omp","rpc"), ClaudeRpc=>("claude","rpc"), OpencodeRpc=>("opencode","rpc"),
    TelegramSend=>("telegram","send"), TelegramUpdate=>("telegram","update"),
    FeishuSend=>("feishu","send"), FeishuUpdate=>("feishu","update"), SlackSend=>("slack","send"), SlackUpdate=>("slack","update"),
    CodexTurn=>("codex","turn"), PiTurn=>("pi","turn"), OmpTurn=>("omp","turn"), ClaudeTurn=>("claude","turn"), OpencodeTurn=>("opencode","turn"),
    CodexFirstOutput=>("codex","first_output"), PiFirstOutput=>("pi","first_output"), OmpFirstOutput=>("omp","first_output"), ClaudeFirstOutput=>("claude","first_output"), OpencodeFirstOutput=>("opencode","first_output"),
    MemoryApi=>("memory","api"), MemoryQuery=>("memory","query"), MemoryDeep=>("memory","deep_query"), MemoryExtract=>("memory","extract"), MemoryConsolidate=>("memory","consolidate"), MemoryEmbedding=>("memory","embedding"), MemoryProjection=>("memory","projection"),
    ModelRequest=>("model","request"), EmbeddingRequest=>("embedding","request"), ProviderWait=>("provider","wait"),
    JevRouting=>("jev","routing"), JevDiscussion=>("jev","discussion"), JevRecovery=>("jev","recovery"), JevOutcome=>("jev","outcome"), JevReview=>("jev","review_policy"), JevCompletion=>("jev","completion"), JevTriage=>("jev","memory_triage")
}
impl Operation {
    #[must_use]
    pub fn rpc(backend: &str) -> Self {
        match backend {
            "pi" => Self::PiRpc,
            "omp" | "oh-my-pi" => Self::OmpRpc,
            "claude" => Self::ClaudeRpc,
            "opencode" => Self::OpencodeRpc,
            _ => Self::CodexRpc,
        }
    }
    #[must_use]
    pub fn turn(backend: &str) -> Self {
        match backend {
            "pi" => Self::PiTurn,
            "omp" | "oh-my-pi" => Self::OmpTurn,
            "claude" => Self::ClaudeTurn,
            "opencode" => Self::OpencodeTurn,
            _ => Self::CodexTurn,
        }
    }
    #[must_use]
    pub fn first_output(backend: &str) -> Self {
        match backend {
            "pi" => Self::PiFirstOutput,
            "omp" | "oh-my-pi" => Self::OmpFirstOutput,
            "claude" => Self::ClaudeFirstOutput,
            "opencode" => Self::OpencodeFirstOutput,
            _ => Self::CodexFirstOutput,
        }
    }
    #[must_use]
    pub fn channel(channel: &str, update: bool) -> Self {
        match (channel, update) {
            ("telegram", false) => Self::TelegramSend,
            ("telegram", true) => Self::TelegramUpdate,
            ("feishu", false) => Self::FeishuSend,
            ("feishu", true) => Self::FeishuUpdate,
            (_, false) => Self::SlackSend,
            (_, true) => Self::SlackUpdate,
        }
    }
}
#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum Outcome {
    Success,
    Error,
    Cancelled,
    Timeout,
}
impl Outcome {
    const ALL: [Self; 4] = [Self::Success, Self::Error, Self::Cancelled, Self::Timeout];
    const fn name(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
        }
    }
}
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct Labels {
    component: &'static str,
    operation: &'static str,
}
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ResultLabels {
    component: &'static str,
    operation: &'static str,
    outcome: &'static str,
}
type HistogramFamily = Family<Labels, Histogram, fn() -> Histogram>;
fn histogram() -> Histogram {
    Histogram::new([
        0.001, 0.005, 0.025, 0.1, 0.5, 2.0, 10.0, 30.0, 120.0, 600.0, 3600.0,
    ])
}
struct OpData {
    counts: [Counter; 4],
    duration: Histogram,
    active: Gauge,
}
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Snapshot {
    Tasks,
    Memory,
    Jev,
    Process,
}
struct Inner {
    registry: Registry,
    service: Service,
    connections: Family<Vec<(&'static str, &'static str)>, Gauge>,
    counts: Family<ResultLabels, Counter>,
    durations: HistogramFamily,
    active: Family<Labels, Gauge>,
    operations: Vec<OnceLock<Arc<OpData>>>,
    // Lazy finite handles preserve absent-series semantics and avoid family locks on hot paths.
    event_handles: Vec<OnceLock<Counter>>,
    token_handles: Vec<OnceLock<Counter>>,
    events: Family<Vec<(&'static str, &'static str)>, Counter>,
    queues: Family<Vec<(&'static str, &'static str)>, Gauge>,
    queue_capacity: Family<Vec<(&'static str, &'static str)>, Gauge>,
    queue_oldest:
        Family<Vec<(&'static str, &'static str)>, Gauge<f64, std::sync::atomic::AtomicU64>>,
    tokens: Family<Vec<(&'static str, &'static str)>, Counter>,
    snapshots: Mutex<[Option<(Instant, String)>; 4]>,
}
/// Cloneable recorder; a disabled recorder performs no allocation or clock reads.
#[derive(Clone, Default)]
pub struct Metrics(Option<Arc<Inner>>);
static GLOBAL: OnceLock<Metrics> = OnceLock::new();
static DISABLED: Metrics = Metrics(None);
pub fn global() -> &'static Metrics {
    GLOBAL.get().unwrap_or(&DISABLED)
}
pub fn install(metrics: Metrics) -> anyhow::Result<()> {
    GLOBAL
        .set(metrics)
        .map_err(|_| anyhow::anyhow!("application metrics already initialized"))
}
impl Metrics {
    #[must_use]
    pub fn new(service: Service, version: &str) -> Self {
        let mut registry = Registry::with_prefix(service.prefix());
        let connections = Family::default();
        registry.register(
            "backend_connections",
            "Live backend transport connections",
            connections.clone(),
        );
        let counts = Family::default();
        let durations = Family::new_with_constructor(histogram as fn() -> Histogram);
        let active = Family::default();
        registry.register(
            "operations",
            "Completed application operations by outcome",
            counts.clone(),
        );
        registry.register(
            "operation_duration_seconds",
            "Completed application operation duration",
            durations.clone(),
        );
        registry.register(
            "operations_in_flight",
            "Application operations currently executing",
            active.clone(),
        );
        let events = Family::default();
        registry.register("events", "Application events", events.clone());
        let queues = Family::default();
        registry.register("queue_items", "Application queue occupancy", queues.clone());
        let queue_capacity = Family::default();
        registry.register(
            "queue_capacity",
            "Application queue admission capacity",
            queue_capacity.clone(),
        );
        let queue_oldest = Family::default();
        registry.register(
            "queue_oldest_age_seconds",
            "Age of the oldest pending application item",
            queue_oldest.clone(),
        );
        let tokens = Family::default();
        registry.register(
            "tokens",
            "Observed provider token usage; sources must not be summed with OTLP usage",
            tokens.clone(),
        );
        registry.register(
            "build",
            "Application build",
            prometheus_client::metrics::info::Info::new(vec![(
                "version".to_owned(),
                version.to_owned(),
            )]),
        );
        Self(Some(Arc::new(Inner {
            registry,
            service,
            connections,
            counts,
            durations,
            active,
            operations: (0..Operation::ALL.len()).map(|_| OnceLock::new()).collect(),
            event_handles: (0..Event::ALL.len()).map(|_| OnceLock::new()).collect(),
            token_handles: (0..TokenSource::ALL.len() * TokenKind::ALL.len())
                .map(|_| OnceLock::new())
                .collect(),
            events,
            queues,
            queue_capacity,
            queue_oldest,
            tokens,
            snapshots: Mutex::new([None, None, None, None]),
        })))
    }
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.0.is_some()
    }
    fn operation(&self, op: Operation) -> Option<&Arc<OpData>> {
        let inner = self.0.as_ref()?;
        Some(inner.operations[op as usize].get_or_init(|| {
            let labels = op.labels();
            Arc::new(OpData {
                counts: Outcome::ALL.map(|outcome| {
                    inner
                        .counts
                        .get_or_create(&ResultLabels {
                            component: labels.component,
                            operation: labels.operation,
                            outcome: outcome.name(),
                        })
                        .clone()
                }),
                duration: inner.durations.get_or_create(&labels).clone(),
                active: inner.active.get_or_create(&labels).clone(),
            })
        }))
    }
    #[must_use]
    pub fn start(&self, op: Operation) -> Guard {
        let data = self.operation(op).cloned();
        if let Some(data) = &data {
            data.active.inc();
        }
        Guard {
            started: data.as_ref().map(|_| std::time::Instant::now()),
            data,
            outcome: Outcome::Cancelled,
        }
    }
    pub fn observe(&self, op: Operation, outcome: Outcome, duration: Duration) {
        if let Some(data) = self.operation(op) {
            data.counts[outcome as usize].inc();
            data.duration.observe(duration.as_secs_f64());
        }
    }
    pub fn publish(&self, source: Snapshot, text: String) {
        if let Some(inner) = &self.0 {
            inner.snapshots.lock().unwrap()[source as usize] = Some((Instant::now(), text));
        }
    }
    pub fn clear(&self, source: Snapshot) {
        if let Some(inner) = &self.0 {
            inner.snapshots.lock().unwrap()[source as usize] = None;
        }
    }
    pub fn encode(&self) -> anyhow::Result<String> {
        let Some(inner) = &self.0 else {
            return Ok(String::new());
        };
        let mut text = String::new();
        encode(&mut text, &inner.registry)?;
        text.truncate(text.len().saturating_sub("# EOF\n".len()));
        for (at, snapshot) in inner.snapshots.lock().unwrap().iter().flatten() {
            if at.elapsed() <= Duration::from_mins(2) {
                text.push_str(snapshot);
            }
        }
        text.push_str("# EOF\n");
        Ok(text)
    }
    pub fn queue(
        &self,
        queue: Queue,
        pending: usize,
        active: usize,
        capacity: usize,
        oldest: Duration,
    ) {
        if let Some(inner) = &self.0 {
            let queue = queue.name();
            for (state, count) in [("pending", pending), ("active", active)] {
                inner
                    .queues
                    .get_or_create(&vec![("queue", queue), ("state", state)])
                    .set(i64::try_from(count).unwrap_or(i64::MAX));
            }
            inner
                .queue_capacity
                .get_or_create(&vec![("queue", queue)])
                .set(i64::try_from(capacity).unwrap_or(i64::MAX));
            inner
                .queue_oldest
                .get_or_create(&vec![("queue", queue)])
                .set(oldest.as_secs_f64());
        }
    }
    /// Track a live backend transport, including cancellation and concurrent peers.
    #[must_use]
    pub fn connected(&self, backend: &str) -> ConnectionGuard {
        let gauge = self.0.as_ref().map(|inner| {
            let name = Operation::rpc(backend).labels().component;
            let gauge = inner
                .connections
                .get_or_create(&vec![("backend", name)])
                .clone();
            gauge.inc();
            gauge
        });
        ConnectionGuard(gauge)
    }
    pub fn event(&self, event: Event, count: u64) {
        if let Some(inner) = &self.0 {
            inner.event_handles[event as usize]
                .get_or_init(|| {
                    let (component, event) = event.labels();
                    inner
                        .events
                        .get_or_create(&vec![("component", component), ("event", event)])
                        .clone()
                })
                .inc_by(count);
        }
    }
    pub fn tokens(&self, source: TokenSource, kind: TokenKind, count: u64) {
        if let Some(inner) = &self.0 {
            let index = source as usize * TokenKind::ALL.len() + kind as usize;
            inner.token_handles[index]
                .get_or_init(|| {
                    inner
                        .tokens
                        .get_or_create(&vec![("source", source.name()), ("kind", kind.name())])
                        .clone()
                })
                .inc_by(count);
        }
    }
}
pub struct Guard {
    data: Option<Arc<OpData>>,
    started: Option<std::time::Instant>,
    outcome: Outcome,
}
impl Guard {
    pub fn finish(mut self, outcome: Outcome) {
        self.outcome = outcome;
    }
    pub fn result<T, E>(self, result: &Result<T, E>) {
        self.finish(if result.is_ok() {
            Outcome::Success
        } else {
            Outcome::Error
        });
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(data) = &self.data {
            data.active.dec();
            data.counts[self.outcome as usize].inc();
            data.duration
                .observe(self.started.unwrap().elapsed().as_secs_f64());
        }
    }
}
#[derive(Clone, Copy)]
pub enum Queue {
    Engine,
    Control,
}
impl Queue {
    const fn name(self) -> &'static str {
        match self {
            Self::Engine => "engine",
            Self::Control => "control",
        }
    }
}
macro_rules! events {($($v:ident=>($c:literal,$e:literal)),+ $(,)?)=>{
    #[derive(Clone,Copy)] #[repr(usize)] pub enum Event {$($v),+}
    impl Event {const ALL: &[Self] = &[$(Self::$v),+]; fn labels(self)->(&'static str,&'static str){match self{$(Self::$v=>($c,$e)),+}}}
};}
events! {AgentLag=>("engine","event_lag"),EngineRejected=>("engine","rejected"),TelegramInbound=>("telegram","inbound"),FeishuInbound=>("feishu","inbound"),SlackInbound=>("slack","inbound"),TelegramRetry=>("telegram","retry"),FeishuRetry=>("feishu","retry"),SlackRetry=>("slack","retry"),MemoryFallback=>("memory","fallback"),MemoryResults=>("memory","results")}
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum TokenSource {
    Codex,
    Memory,
}
impl TokenSource {
    const ALL: [Self; 2] = [Self::Codex, Self::Memory];
    const fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex_app_server",
            Self::Memory => "memory_provider",
        }
    }
}
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum TokenKind {
    Input,
    Output,
    Cached,
}
impl TokenKind {
    const ALL: [Self; 3] = [Self::Input, Self::Output, Self::Cached];
    const fn name(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Cached => "cached_input",
        }
    }
}

impl Metrics {
    pub fn track<T, E>(
        &self,
        op: Operation,
        future: impl std::future::Future<Output = Result<T, E>>,
    ) -> impl std::future::Future<Output = Result<T, E>> {
        let guard = self.start(op);
        async move {
            let result = future.await;
            guard.result(&result);
            result
        }
    }
}

/// Runtime-owned bounded turn observations. Session identifiers never become labels.
pub struct Turns {
    metrics: Metrics,
    active: std::collections::HashMap<String, (String, Guard, std::time::Instant, bool)>,
    completed: std::collections::VecDeque<(String, String)>,
}
impl Turns {
    #[must_use]
    pub fn new(metrics: Metrics) -> Self {
        Self {
            metrics,
            active: std::collections::HashMap::default(),
            completed: std::collections::VecDeque::default(),
        }
    }
    pub fn started(&mut self, session: &str, turn: &str) {
        if !self.metrics.enabled()
            || self
                .completed
                .iter()
                .any(|(s, t)| s == session && t == turn)
            || self.active.get(session).is_some_and(|(t, ..)| t == turn)
        {
            return;
        }
        if self.active.len() >= 256 && !self.active.contains_key(session) {
            return;
        }
        let backend = session.split(':').next().unwrap_or("codex");
        self.active.insert(
            session.into(),
            (
                turn.into(),
                self.metrics.start(Operation::turn(backend)),
                std::time::Instant::now(),
                false,
            ),
        );
    }
    pub fn output(&mut self, session: &str, turn: &str) {
        if let Some((id, _, at, seen)) = self.active.get_mut(session)
            && id == turn
            && !*seen
        {
            *seen = true;
            self.metrics.observe(
                Operation::first_output(session.split(':').next().unwrap_or("codex")),
                Outcome::Success,
                at.elapsed(),
            );
        }
    }
    pub fn completed(&mut self, session: &str, turn: &str, outcome: Outcome) {
        if self.active.get(session).is_some_and(|(id, ..)| id == turn) {
            let (_, guard, ..) = self.active.remove(session).unwrap();
            guard.finish(outcome);
            if self.completed.len() == 256 {
                self.completed.pop_front();
            }
            self.completed.push_back((session.into(), turn.into()));
        }
    }
    pub fn disconnected(&mut self, backend: &str) {
        let component = Operation::rpc(backend).labels().component;
        self.active.retain(|session, _| {
            Operation::rpc(session.split(':').next().unwrap_or("codex"))
                .labels()
                .component
                != component
        });
    }
}

#[derive(Clone, Copy, Default)]
pub struct Usage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cached: Option<u64>,
}
#[derive(Default)]
pub struct TokenDeltas {
    baselines: std::collections::BTreeMap<String, Usage>,
}
impl TokenDeltas {
    pub fn clear(&mut self) {
        self.baselines.clear();
    }
    pub fn record(&mut self, metrics: &Metrics, session: &str, usage: Usage) {
        if !metrics.enabled() {
            return;
        }
        if self.baselines.len() >= 256 && !self.baselines.contains_key(session) {
            self.baselines.pop_first();
        }
        if let Some(baseline) = self.baselines.get_mut(session) {
            let old = std::mem::replace(baseline, usage);
            for (kind, old, new) in [
                (TokenKind::Input, old.input, usage.input),
                (TokenKind::Output, old.output, usage.output),
                (TokenKind::Cached, old.cached, usage.cached),
            ] {
                if let (Some(old), Some(new)) = (old, new)
                    && new >= old
                {
                    metrics.tokens(TokenSource::Codex, kind, new - old);
                }
            }
        } else {
            self.baselines.insert(session.into(), usage);
        }
    }
}

/// A live connection's ownership; dropping it decrements the backend total.
pub struct ConnectionGuard(Option<Gauge>);
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        if let Some(gauge) = &self.0 {
            gauge.dec();
        }
    }
}
