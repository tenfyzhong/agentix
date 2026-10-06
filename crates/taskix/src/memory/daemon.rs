use agentix_memory::{
    DeepQuery, EmbeddingIndex, HttpEmbedding, HttpModel, HttpProvider, IpcServer, MemoryApi,
    MemoryConfig, MemoryLocation, MemoryProjection, MemoryStore, MemoryWorker, ProjectRepository,
    ProviderLimits, RequestHandler, ServiceConfig, Source,
};
use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{Mutex, RwLock, Semaphore, watch},
    task::JoinSet,
};

struct Repositories(agentix_task::Store);
#[async_trait]
impl ProjectRepository for Repositories {
    async fn root(&self, project: &str) -> Result<Option<PathBuf>> {
        let project = self.0.project_result(project).await?;
        if project.archived_at.is_some() {
            return Ok(None);
        }
        let root = PathBuf::from(project.root);
        let metadata = match tokio::fs::metadata(&root).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("cannot inspect Project root {}", root.display()));
            }
        };
        ensure!(
            metadata.is_dir(),
            "Project root must be a directory: {}",
            root.display()
        );
        Ok(Some(root))
    }
}
struct Runtime {
    api: Arc<MemoryApi>,
    worker: Option<Arc<MemoryWorker>>,
    embedding: Option<Arc<EmbeddingIndex>>,
    config: MemoryConfig,
    errors: Vec<String>,
    limits: Arc<RuntimeLimits>,
}
struct RuntimeLimits {
    queries: Semaphore,
    deep: Semaphore,
    providers: std::sync::Mutex<BTreeMap<String, Arc<ProviderLimits>>>,
}
impl RuntimeLimits {
    fn new(service: ServiceConfig) -> Self {
        Self {
            queries: Semaphore::new(service.max_query_concurrency),
            deep: Semaphore::new(service.max_deep_queries),
            providers: std::sync::Mutex::new(BTreeMap::new()),
        }
    }

    fn provider(&self, name: &str, capacity: usize) -> Result<Arc<ProviderLimits>> {
        let mut providers = self
            .providers
            .lock()
            .expect("provider admission state poisoned");
        let limits = if let Some(limits) = providers.get(name) {
            limits.clone()
        } else {
            let limits = Arc::new(ProviderLimits::new(capacity)?);
            providers.insert(name.into(), limits.clone());
            limits
        };
        limits.set_limit(capacity)?;
        Ok(limits)
    }
}
impl Runtime {
    fn build(config: MemoryConfig, store: &MemoryStore, repositories: Arc<Repositories>) -> Self {
        let limits = Arc::new(RuntimeLimits::new(config.service));
        Self::with_limits(config, store, repositories, limits)
    }

    fn with_limits(
        config: MemoryConfig,
        store: &MemoryStore,
        repositories: Arc<Repositories>,
        limits: Arc<RuntimeLimits>,
    ) -> Self {
        let mut providers = BTreeMap::new();
        let mut errors = Vec::new();
        let mut api = MemoryApi::new(store.clone(), config.retrieval.clone(), config.service);
        let model = (|| {
            let connection = config
                .providers
                .get(&config.agent.provider)
                .context("missing Agent provider")?;
            let provider = Arc::new(HttpProvider::with_limits(
                connection.clone(),
                limits.provider(&config.agent.provider, connection.max_in_flight)?,
            )?);
            providers.insert(config.agent.provider.clone(), provider.clone());
            Ok::<_, anyhow::Error>(Arc::new(HttpModel::new(provider, config.agent.clone())?))
        })();
        let worker = match model {
            Ok(model) => {
                api = api.with_deep_query(DeepQuery::new(
                    store.clone(),
                    model.clone(),
                    config.agent.clone(),
                    repositories.clone(),
                    config.service.max_deep_queries,
                ));
                let worker =
                    MemoryWorker::new(store.clone(), model, config.agent.clone(), repositories);
                let worker = if let Some(gate) = super::triage::JevTriage::from_env() {
                    worker.with_extraction_gate(gate)
                } else {
                    worker
                };
                Some(Arc::new(worker))
            }
            Err(error) => {
                errors.push(format!("Agent unavailable: {error}"));
                None
            }
        };
        let embedding = if config.embedding.enabled {
            let result = (|| {
                let provider = if let Some(provider) = providers.get(&config.embedding.provider) {
                    provider.clone()
                } else {
                    let connection = config
                        .providers
                        .get(&config.embedding.provider)
                        .context("missing embedding provider")?
                        .clone();
                    Arc::new(HttpProvider::with_limits(
                        connection.clone(),
                        limits.provider(&config.embedding.provider, connection.max_in_flight)?,
                    )?)
                };
                Ok::<_, anyhow::Error>(Arc::new(HttpEmbedding::new(
                    provider,
                    config.embedding.clone(),
                )?))
            })();
            match result {
                Ok(embedding) => {
                    api = api.with_embedding(embedding.clone());
                    Some(Arc::new(EmbeddingIndex::new(
                        store.clone(),
                        embedding,
                        config.embedding.clone(),
                    )))
                }
                Err(error) => {
                    errors.push(format!("Embedding unavailable: {error}"));
                    None
                }
            }
        } else {
            None
        };
        Self {
            api: Arc::new(api),
            worker,
            embedding,
            config,
            errors,
            limits,
        }
    }
}
struct Application {
    path: PathBuf,
    location: MemoryLocation,
    store: MemoryStore,
    tasks: agentix_task::Store,
    repositories: Arc<Repositories>,
    runtime: RwLock<Arc<Runtime>>,
    errors: Mutex<BTreeMap<String, String>>,
    projection: Mutex<()>,
}
impl Application {
    async fn report(&self, component: &str, error: Option<String>) {
        let mut errors = self.errors.lock().await;
        if let Some(mut error) = error {
            if error.len() > 2048 {
                let mut end = 2048;
                while !error.is_char_boundary(end) {
                    end -= 1;
                }
                error.truncate(end);
            }
            errors.insert(component.into(), error);
            while errors.len() > 16 {
                errors.pop_first();
            }
        } else {
            errors.remove(component);
        }
    }
    async fn reload(&self) -> Result<Value> {
        // Serialize configuration snapshots and admission-limit updates.
        let mut installed = self.runtime.write().await;
        let location = MemoryLocation::load(&self.path)?;
        ensure!(
            location.enabled
                && location.path == self.location.path
                && location.task_path == self.location.task_path,
            "storage paths or enabled state changed; restart the memory service"
        );
        let config = MemoryConfig::load(&self.path)?;
        ensure!(
            config.service.max_query_concurrency == self.location.service.max_query_concurrency
                && config.service.max_deep_queries == self.location.service.max_deep_queries
                && config.service.max_request_bytes == self.location.service.max_request_bytes
                && config.service.max_response_bytes == self.location.service.max_response_bytes,
            "IPC limits changed; restart the memory service"
        );
        let runtime = Runtime::with_limits(
            config,
            &self.store,
            self.repositories.clone(),
            installed.limits.clone(),
        );
        let result = json!({"reloaded":true,"errors":runtime.errors});
        *installed = Arc::new(runtime);
        self.store.wake_compaction();
        Ok(result)
    }
}
#[async_trait]
impl RequestHandler for Application {
    async fn handle(&self, mut request: Value) -> Result<Value> {
        if request["op"] == "reload" {
            return self.reload().await;
        }
        if let Some(project) = request["project"].as_str() {
            request["project"] = json!(self.tasks.project_result(project).await?.id);
        }
        if request["op"] == "sync" {
            let project = request["project"].as_str().context("missing Project")?;
            return self
                .sync_projection(
                    project,
                    request["after"].as_str().unwrap_or(""),
                    request["limit"].as_i64().unwrap_or(20),
                )
                .await;
        }
        if request["op"] == "backfill" {
            let project = request["project"].as_str().context("missing Project")?;
            let job = request["job"].as_str().context("missing Job")?;
            let result = self
                .tasks
                .backfill_memory_job(
                    project,
                    job,
                    request["offset"].as_i64().unwrap_or(0),
                    request["limit"].as_i64().unwrap_or(20),
                )
                .await?;
            return Ok(serde_json::to_value(result)?);
        }
        let runtime = self.runtime.read().await.clone();
        let _permit = if request["op"] == "ask" {
            runtime
                .limits
                .deep
                .try_acquire()
                .context("busy: deep memory query limit reached")?
        } else {
            runtime
                .limits
                .queries
                .try_acquire()
                .context("busy: memory query limit reached")?
        };
        let status = request["op"] == "status";
        let mut result = runtime.api.handle(request).await?;
        if status {
            result["online"] = json!(true);
            result["provider_errors"] = json!(runtime.errors);
            result["background_errors"] = json!(*self.errors.lock().await);
            result["model"] = json!(runtime.config.agent.model);
            result["reasoning_effort"] = json!(runtime.config.agent.reasoning_effort);
            result["embedding_enabled"] = json!(runtime.config.embedding.enabled);
        }
        Ok(result)
    }
}

pub async fn serve(path: &Path, location: MemoryLocation) -> Result<Value> {
    ensure!(
        location.task_path.exists(),
        "task database must be initialized before memory serve"
    );
    std::fs::create_dir_all(
        location
            .path
            .parent()
            .context("missing memory database parent")?,
    )?;
    let config = MemoryConfig::load(path)?;
    let server = IpcServer::bind(&location.path, config.service)?;
    let store = MemoryStore::open(&location.path).await?;
    let tasks = agentix_task::Store::open(&location.task_path).await?;
    let replay_cursor = store
        .bind_source(&tasks.memory_source_instance().await?)
        .await?;
    let mut after = String::new();
    loop {
        let page = store.recovery_sources(&after, 100).await?;
        if page.is_empty() {
            break;
        }
        after.clone_from(&page.last().expect("nonempty recovery page").receipt_id);
        let receipts: Vec<agentix_task::MemorySource> =
            serde_json::from_value(serde_json::to_value(&page)?)?;
        tasks.verify_memory_sources(&receipts).await?;
    }
    let repositories = Arc::new(Repositories(tasks.clone()));
    let runtime = Arc::new(Runtime::build(config, &store, repositories.clone()));
    let app = Arc::new(Application {
        path: path.into(),
        location,
        store,
        tasks,
        repositories,
        runtime: RwLock::new(runtime),
        errors: Mutex::new(BTreeMap::new()),
        projection: Mutex::new(()),
    });
    let mut background = JoinSet::new();
    let sources = app.clone();
    background.spawn(async move { poll_sources(sources, replay_cursor).await });
    let compactions = app.clone();
    background.spawn(async move { run_compactions(compactions).await });
    let reviews = app.clone();
    background.spawn(async move { run_reviews(reviews).await });
    let workers = app.clone();
    background.spawn(async move { run_workers(workers).await });
    let embeddings = app.clone();
    background.spawn(async move { run_embeddings(embeddings).await });
    let projection = app.clone();
    background.spawn(async move { run_projection(projection).await });
    let maintenance = app.store.clone();
    background.spawn(async move {
        loop {
            maintenance.cleanup_context().await?;
            maintenance.cleanup_derived().await?;
            tokio::time::sleep(Duration::from_mins(1)).await;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    });
    let (stop, rx) = watch::channel(false);
    eprintln!("memory service listening at {}", server.path().display());
    let mut serving = tokio::spawn(server.serve(app, rx));
    let shutdown = shutdown_signal();
    let result = tokio::select! {
        result=&mut serving=>Some(result),
        result=shutdown=>{result?;None},
        result=background.join_next()=>{if let Some(result)=result {result??;}None},
    };
    let _ = stop.send(true);
    background.abort_all();
    while background.join_next().await.is_some() {}
    if let Some(result) = result {
        result??;
    } else {
        serving.await??;
    }
    Ok(json!({"stopped":true}))
}

async fn poll_sources(app: Arc<Application>, mut replay_cursor: i64) -> Result<()> {
    let mut cursor = 0;
    loop {
        let runtime = app.runtime.read().await.clone();
        let pending = app.tasks.memory_sources(cursor, 100).await;
        match pending {
            Ok(sources) => {
                cursor = if sources.len() < 100 {
                    0
                } else {
                    sources.last().expect("full source page").sequence
                };
                for source in sources {
                    let result = ingest_source(&app, &source).await;
                    app.report(
                        &format!("source:{}", source.receipt_id),
                        result.err().map(|e| e.to_string()),
                    )
                    .await;
                }
            }
            Err(error) => {
                app.report("source_poll", Some(error.to_string())).await;
                cursor = 0;
            }
        }
        // Replay includes already acknowledged inputs after restore. Unlike pending
        // intake, it must stop at a failed source and persist only ordered progress.
        match app.tasks.replay_memory_sources(replay_cursor, 100).await {
            Ok(sources) => {
                for source in sources {
                    let result = async {
                        let input = ingest_source(&app, &source).await?;
                        app.store.checkpoint_replay(replay_cursor, &input).await?;
                        Ok::<_, anyhow::Error>(())
                    }
                    .await;
                    let failed = result.is_err();
                    app.report(
                        &format!("replay:{}", source.receipt_id),
                        result.err().map(|e| e.to_string()),
                    )
                    .await;
                    if failed {
                        break;
                    }
                    replay_cursor = source.sequence;
                }
                app.report("replay_poll", None).await;
            }
            Err(error) => app.report("replay_poll", Some(error.to_string())).await,
        }
        tokio::time::sleep(Duration::from_millis(
            runtime.config.service.poll_interval_ms,
        ))
        .await;
    }
}

async fn ingest_source(app: &Application, source: &agentix_task::MemorySource) -> Result<Source> {
    let input: Source = serde_json::from_value(serde_json::to_value(source)?)?;
    app.store.ingest(&input).await?;
    app.tasks
        .acknowledge_memory_source(&source.instance_id, &source.receipt_id)
        .await?;
    Ok(input)
}
struct WorkerIdle {
    next: tokio::time::Instant,
    misses: u32,
}
impl WorkerIdle {
    fn new(now: tokio::time::Instant) -> Self {
        Self {
            next: now,
            misses: 0,
        }
    }
    fn ready(&self, now: tokio::time::Instant) -> bool {
        now >= self.next
    }
    fn wake(&mut self, now: tokio::time::Instant) {
        self.next = now;
        self.misses = 0;
    }
    fn empty(&mut self, now: tokio::time::Instant, interval: Duration) {
        self.misses = (self.misses + 1).min(6);
        self.next = now
            + interval
                .saturating_mul(1 << self.misses)
                .min(Duration::from_secs(5));
    }
}
async fn run_workers(app: Arc<Application>) -> Result<()> {
    let mut workers = JoinSet::new();
    let mut sequence = 0_u64;
    let mut work = app.store.subscribe_work();
    let mut idle = WorkerIdle::new(tokio::time::Instant::now());
    let mut installed = app.runtime.read().await.clone();
    loop {
        let runtime = app.runtime.read().await.clone();
        if !Arc::ptr_eq(&installed, &runtime) {
            idle.wake(tokio::time::Instant::now());
            installed = runtime.clone();
        }
        let interval = Duration::from_millis(runtime.config.service.poll_interval_ms);
        if let Some(worker) = &runtime.worker
            && idle.ready(tokio::time::Instant::now())
        {
            while workers.len() < runtime.config.agent.max_concurrent_loops {
                sequence += 1;
                let owner = format!("{}:{sequence}", std::process::id());
                let worker = worker.clone();
                let epoch = *work.borrow();
                workers.spawn(async move { (epoch, worker.run_once(&owner).await) });
            }
        }
        tokio::select! {
            Some(result)=workers.join_next(),if !workers.is_empty()=>{
                let (epoch, result) = result?;
                if matches!(result, Ok(true)) || epoch != *work.borrow() {
                    idle.wake(tokio::time::Instant::now());
                } else {
                    idle.empty(tokio::time::Instant::now(), interval);
                }
                match result {
                    Ok(true) => app.report("worker", None).await,
                    Ok(false) => {},
                    Err(error) => app.report("worker", Some(error.to_string())).await,
                }
            },
            changed=work.changed()=>{
                if changed.is_err() { return Ok(()); }
                idle.wake(tokio::time::Instant::now());
            },
            ()=tokio::time::sleep(interval.min(Duration::from_secs(5)))=>{},
        }
    }
}
#[derive(Default)]
struct EmbeddingSchedule {
    due: BTreeMap<String, tokio::time::Instant>,
    idle: BTreeMap<String, u32>,
}
impl EmbeddingSchedule {
    fn ready(&self, project: &str, now: tokio::time::Instant) -> bool {
        self.due.get(project).is_none_or(|due| *due <= now)
    }
    fn wake(&mut self, project: &str) {
        self.due.remove(project);
        self.idle.remove(project);
    }
    fn complete(
        &mut self,
        project: &str,
        empty: bool,
        now: tokio::time::Instant,
        interval: Duration,
    ) {
        let delay = if empty {
            let count = self.idle.entry(project.into()).or_default();
            *count = (*count + 1).min(5);
            interval
                .saturating_mul(1 << *count)
                .min(Duration::from_secs(30))
        } else {
            self.idle.remove(project);
            interval
        };
        self.due.insert(project.into(), now + delay);
    }
}

async fn embedding_project_ids(tasks: &agentix_task::Store) -> Result<Vec<String>> {
    let mut ids: Vec<_> = tasks
        .projects()
        .await?
        .into_iter()
        .filter(|project| project.archived_at.is_none())
        .map(|project| project.id)
        .collect();
    ids.sort();
    Ok(ids)
}

async fn run_embeddings(app: Arc<Application>) -> Result<()> {
    let mut workers = JoinSet::<(String, Result<usize>)>::new();
    let mut active = BTreeSet::new();
    let mut schedule = EmbeddingSchedule::default();
    let mut changes = app.store.subscribe_changes();
    let mut dirty = BTreeSet::new();
    let mut project_ids = Vec::<String>::new();
    let mut refresh_at = tokio::time::Instant::now();
    let mut last = String::new();
    let mut installed = app.runtime.read().await.clone();
    loop {
        let runtime = app.runtime.read().await.clone();
        if !Arc::ptr_eq(&installed, &runtime) {
            workers.abort_all();
            while workers.join_next().await.is_some() {}
            active.clear();
            schedule = EmbeddingSchedule::default();
            dirty.clear();
            refresh_at = tokio::time::Instant::now();
            installed = runtime.clone();
        }
        let interval = Duration::from_millis(runtime.config.service.poll_interval_ms.max(1000));
        if let Some(index) = &runtime.embedding {
            if tokio::time::Instant::now() >= refresh_at {
                project_ids = embedding_project_ids(&app.tasks).await?;
                schedule
                    .due
                    .retain(|id, _| project_ids.binary_search(id).is_ok());
                schedule
                    .idle
                    .retain(|id, _| project_ids.binary_search(id).is_ok());
                refresh_at = tokio::time::Instant::now() + Duration::from_secs(5);
            }
            let mut projects = project_ids.clone();
            // Rotate admission independently of completion order. A slow Project
            // occupies one slot, not an entire scheduling round.
            let pivot = projects.partition_point(|id| id <= &last);
            projects.rotate_left(pivot);
            for project in projects {
                if workers.len() >= runtime.config.embedding.max_concurrent_projects {
                    break;
                }
                if active.contains(&project)
                    || !schedule.ready(&project, tokio::time::Instant::now())
                {
                    continue;
                }
                active.insert(project.clone());
                last.clone_from(&project);
                let index = index.clone();
                let app = app.clone();
                let runtime = runtime.clone();
                workers.spawn(async move {
                    let result = async {
                        let current = app.runtime.read().await;
                        if !Arc::ptr_eq(&current, &runtime) {
                            return Ok(0);
                        }
                        let generation = index.activate(&project).await?;
                        drop(current);
                        index.step(&project, generation).await
                    }
                    .await;
                    (project, result)
                });
            }
        }
        tokio::select! {
            Some(result) = workers.join_next(), if !workers.is_empty() => {
                let (project, result) = result?;
                active.remove(&project);
                if dirty.remove(&project) {
                    schedule.wake(&project);
                } else {
                    schedule.complete(&project, matches!(&result, Ok(0)), tokio::time::Instant::now(), interval);
                }
                app.report(&format!("embedding:{project}"),result.err().map(|e|e.to_string())).await;
            },
            change = changes.recv() => {
                match change {
                    Ok(project) => {
                        schedule.wake(&project);
                        if active.contains(&project) { dirty.insert(project.clone()); }
                        if project_ids.binary_search(&project).is_err() { refresh_at = tokio::time::Instant::now(); }
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        schedule = EmbeddingSchedule::default();
                        dirty.extend(active.iter().cloned());
                        refresh_at = tokio::time::Instant::now();
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
                }
            },
            () = tokio::time::sleep(Duration::from_millis(runtime.config.service.poll_interval_ms)) => {},
        }
    }
}

impl Application {
    async fn sync_projection(&self, project: &str, after: &str, limit: i64) -> Result<Value> {
        self.sync_projection_page(project, after, limit, false)
            .await
    }

    async fn sync_projection_page(
        &self,
        project: &str,
        after: &str,
        limit: i64,
        pending: bool,
    ) -> Result<Value> {
        let _guard = self.projection.lock().await;
        ensure!(
            self.runtime.read().await.config.projection.enabled,
            "memory projection is disabled"
        );
        let config = agentix_task::Config::load(&self.path)?;
        let project = self.tasks.project_result(project).await?;
        let directory = config
            .vault_relative_path(std::path::Path::new(&project.document_directory()))
            .join("Memory")
            .components()
            .collect::<PathBuf>();
        let projection =
            MemoryProjection::new(self.store.clone(), &config.documents.root, &directory)?;
        Ok(serde_json::to_value(if pending {
            projection.sync_pending(&project.id, after, limit).await?
        } else {
            projection.sync(&project.id, after, limit).await?
        })?)
    }
}
async fn run_projection(app: Arc<Application>) -> Result<()> {
    let mut cursors = BTreeMap::<String, String>::new();
    let mut pending_cursors = BTreeMap::<String, String>::new();
    let mut reconciled = BTreeMap::<String, tokio::time::Instant>::new();
    loop {
        let config = app.runtime.read().await.config.projection;
        if config.enabled {
            let projects = app.tasks.projects().await?;
            cursors.retain(|id, _| projects.iter().any(|p| &p.id == id));
            pending_cursors.retain(|id, _| projects.iter().any(|p| &p.id == id));
            reconciled.retain(|id, _| projects.iter().any(|p| &p.id == id));
            for project in projects {
                if project.archived_at.is_some() {
                    continue;
                }
                let pending_cursor = pending_cursors.entry(project.id.clone()).or_default();
                match app
                    .sync_projection_page(&project.id, pending_cursor, config.batch_size, true)
                    .await
                {
                    Ok(page) => {
                        *pending_cursor = if page["complete"] == true {
                            String::new()
                        } else {
                            page["next_cursor"].as_str().unwrap_or("").into()
                        };
                        app.report(&format!("projection_pending:{}", project.id), None)
                            .await;
                    }
                    Err(error) => {
                        app.report(
                            &format!("projection_pending:{}", project.id),
                            Some(error.to_string()),
                        )
                        .await;
                    }
                }
                let cursor = cursors.entry(project.id.clone()).or_default();
                if cursor.is_empty()
                    && reconciled.get(&project.id).is_some_and(|last| {
                        last.elapsed() < Duration::from_secs(config.reconcile_interval_seconds)
                    })
                {
                    continue;
                }
                match app
                    .sync_projection(&project.id, cursor, config.batch_size)
                    .await
                {
                    Ok(page) => {
                        if page["complete"] == true {
                            reconciled.insert(project.id.clone(), tokio::time::Instant::now());
                        }
                        *cursor = if page["complete"] == true {
                            String::new()
                        } else {
                            page["next_cursor"].as_str().unwrap_or("").into()
                        };
                        app.report(&format!("projection:{}", project.id), None)
                            .await;
                    }
                    Err(error) => {
                        app.report(
                            &format!("projection:{}", project.id),
                            Some(error.to_string()),
                        )
                        .await;
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(config.poll_interval_ms)).await;
    }
}

/// Only writes, startup recovery and configuration reload wake semantic maintenance.
async fn run_compactions(app: Arc<Application>) -> Result<()> {
    let mut changes = app.store.subscribe_compaction();
    let mut due = BTreeMap::<String, tokio::time::Instant>::new();
    let mut recover = true;
    loop {
        let runtime = app.runtime.read().await.clone();
        if runtime.worker.is_some() && runtime.config.agent.compaction_enabled {
            if recover {
                for project in app.tasks.projects().await? {
                    if project.archived_at.is_none() {
                        due.insert(project.id, tokio::time::Instant::now());
                    }
                }
                recover = false;
            }
            let ready: Vec<_> = due
                .iter()
                .filter(|(_, at)| **at <= tokio::time::Instant::now())
                .map(|(project, _)| project.clone())
                .collect();
            for project in ready {
                due.remove(&project);
                let result = async {
                    if app
                        .tasks
                        .project_result(&project)
                        .await?
                        .archived_at
                        .is_some()
                    {
                        return Ok(None);
                    }
                    let now = time::OffsetDateTime::now_utc().unix_timestamp();
                    app.store
                        .schedule_background_compaction(&project, &runtime.config.agent, now)
                        .await?;
                    app.store
                        .next_compaction_at(&project, &runtime.config.agent, now)
                        .await
                }
                .await;
                match result {
                    Ok(next) => {
                        if let Some(next) = next {
                            let now = time::OffsetDateTime::now_utc().unix_timestamp();
                            let delay = u64::try_from(next.saturating_sub(now)).unwrap_or(0);
                            due.insert(
                                project.clone(),
                                tokio::time::Instant::now() + Duration::from_secs(delay),
                            );
                        }
                        app.report(&format!("compact:{project}"), None).await;
                    }
                    Err(error) => {
                        // A failed database check has outstanding recovery work, unlike idle.
                        due.insert(
                            project.clone(),
                            tokio::time::Instant::now() + Duration::from_mins(1),
                        );
                        app.report(&format!("compact:{project}"), Some(error.to_string()))
                            .await;
                    }
                }
            }
        } else {
            due.clear();
            recover = true;
        }
        let next = due.values().min().copied();
        tokio::select! {
            event = changes.recv() => {
                match event {
                    Ok(Some(project)) => { due.insert(project, tokio::time::Instant::now()); },
                    Ok(None) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => { recover = true; },
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
                }
            },
            () = async {
                if let Some(at) = next {
                    tokio::time::sleep_until(at).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {},
        }
    }
}

async fn run_reviews(app: Arc<Application>) -> Result<()> {
    loop {
        let runtime = app.runtime.read().await.clone();
        let interval = runtime.config.agent.repository_review_interval_seconds;
        if runtime.worker.is_some() && interval > 0 {
            for project in app.tasks.projects().await? {
                if project.archived_at.is_some() {
                    continue;
                }
                let result = async {
                    let Some(root) = app.repositories.root(&project.id).await? else {
                        // Removed workspaces remain registered for historical reads.
                        // Recheck next round so restoring the directory resumes reviews.
                        return Ok::<_, anyhow::Error>(());
                    };
                    let tools = agentix_memory::ProjectTools::new(
                        app.store.clone(),
                        project.id.clone(),
                        Some(root),
                    )?;
                    // Daily renewal also covers dirty/unversioned repositories. A commit
                    // change triggers review earlier, without polling file contents.
                    let day = time::OffsetDateTime::now_utc().unix_timestamp()
                        / i64::try_from(interval).unwrap_or(i64::MAX);
                    let fingerprint = format!(
                        "{}:{day}",
                        tools.repository_head().await?.unwrap_or_default()
                    );
                    app.store
                        .schedule_reviews(&project.id, &fingerprint, 10)
                        .await?;
                    Ok::<_, anyhow::Error>(())
                }
                .await;
                app.report(
                    &format!("review:{}", project.id),
                    result.err().map(|error| error.to_string()),
                )
                .await;
            }
        }
        tokio::time::sleep(Duration::from_mins(1)).await;
    }
}

#[cfg(test)]
#[path = "daemon_tests.rs"]
mod tests;

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    #[cfg(windows)]
    let mut terminate = tokio::signal::windows::ctrl_break()?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result?,
        _ = terminate.recv() => {},
    }
    Ok(())
}
