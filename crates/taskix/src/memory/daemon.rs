use agentix_memory::{
    DeepQuery, EmbeddingIndex, HttpEmbedding, HttpModel, HttpProvider, IpcServer, MemoryApi,
    MemoryConfig, MemoryLocation, MemoryProjection, MemoryStore, MemoryWorker, ProjectRepository,
    RequestHandler, Source,
};
use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{Mutex, RwLock, watch},
    task::JoinSet,
};

struct Repositories(agentix_task::Store);
#[async_trait]
impl ProjectRepository for Repositories {
    async fn root(&self, project: &str) -> Result<Option<PathBuf>> {
        let project = self.0.project_result(project).await?;
        Ok(project
            .archived_at
            .is_none()
            .then(|| PathBuf::from(project.root)))
    }
}
struct Runtime {
    api: Arc<MemoryApi>,
    worker: Option<Arc<MemoryWorker>>,
    embedding: Option<Arc<EmbeddingIndex>>,
    config: MemoryConfig,
    errors: Vec<String>,
}
impl Runtime {
    fn build(config: MemoryConfig, store: &MemoryStore, repositories: Arc<Repositories>) -> Self {
        let mut providers = BTreeMap::new();
        let mut errors = Vec::new();
        let mut api = MemoryApi::new(store.clone(), config.retrieval.clone(), config.service);
        let model = (|| {
            let connection = config
                .providers
                .get(&config.agent.provider)
                .context("missing Agent provider")?;
            let provider = Arc::new(HttpProvider::new(connection.clone())?);
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
                    Arc::new(HttpProvider::new(
                        config
                            .providers
                            .get(&config.embedding.provider)
                            .context("missing embedding provider")?
                            .clone(),
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
        let runtime = Runtime::build(config, &self.store, self.repositories.clone());
        let result = json!({"reloaded":true,"errors":runtime.errors});
        *self.runtime.write().await = Arc::new(runtime);
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
        let status = request["op"] == "status";
        let mut result = runtime.api.handle(request).await?;
        if status {
            result["online"] = json!(true);
            result["provider_errors"] = json!(runtime.errors);
            result["background_errors"] = json!(*self.errors.lock().await);
            result["model"] = json!(runtime.config.agent.model);
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
    let reviews = app.clone();
    background.spawn(async move { run_reviews(reviews).await });
    let workers = app.clone();
    background.spawn(async move { run_workers(workers).await });
    let embeddings = app.clone();
    background.spawn(async move { run_embeddings(embeddings).await });
    let projection = app.clone();
    background.spawn(async move { run_projection(projection).await });
    let (stop, rx) = watch::channel(false);
    eprintln!("memory service listening at {}", server.path().display());
    let mut serving = tokio::spawn(server.serve(app, rx));
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let result = tokio::select! {
        result=&mut serving=>Some(result),
        result=tokio::signal::ctrl_c()=>{result?;None},
        _=terminate.recv()=>None,
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
        let replay = app.tasks.replay_memory_sources(replay_cursor, 100).await;
        match pending.and_then(|mut sources| {
            let replay = replay?;
            if let Some(last) = replay.last() {
                replay_cursor = last.sequence;
            }
            if sources.len() < 100 {
                cursor = 0;
            } else if let Some(last) = sources.last() {
                cursor = last.sequence;
            }
            sources.extend(replay);
            sources.sort_by_key(|source| source.sequence);
            sources.dedup_by_key(|source| source.sequence);
            Ok(sources)
        }) {
            Ok(sources) => {
                for source in sources {
                    let result = async {
                        let input: Source = serde_json::from_value(serde_json::to_value(&source)?)?;
                        app.store.ingest(&input).await?;
                        app.tasks
                            .acknowledge_memory_source(&source.instance_id, &source.receipt_id)
                            .await?;
                        Ok::<_, anyhow::Error>(())
                    }
                    .await;
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
        tokio::time::sleep(Duration::from_millis(
            runtime.config.service.poll_interval_ms,
        ))
        .await;
    }
}
async fn run_workers(app: Arc<Application>) -> Result<()> {
    let mut workers = JoinSet::new();
    let mut sequence = 0_u64;
    loop {
        let runtime = app.runtime.read().await.clone();
        if let Some(worker) = &runtime.worker {
            while workers.len() < runtime.config.agent.max_concurrent_loops {
                sequence += 1;
                let owner = format!("{}:{sequence}", std::process::id());
                let worker = worker.clone();
                workers.spawn(async move { worker.run_once(&owner).await });
            }
        }
        tokio::select! {
            Some(result)=workers.join_next(),if !workers.is_empty()=>{
                match result? {Ok(true)=>{},Ok(false)=>tokio::time::sleep(Duration::from_millis(runtime.config.service.poll_interval_ms)).await,Err(error)=>app.report("worker",Some(error.to_string())).await}
            },
            ()=tokio::time::sleep(Duration::from_millis(runtime.config.service.poll_interval_ms))=>{},
        }
    }
}
async fn run_embeddings(app: Arc<Application>) -> Result<()> {
    loop {
        let runtime = app.runtime.read().await.clone();
        if let Some(index) = &runtime.embedding {
            for project in app.tasks.projects().await? {
                if project.archived_at.is_some() {
                    continue;
                }
                let active = app.runtime.read().await;
                if !Arc::ptr_eq(&active, &runtime) {
                    break;
                }
                let generation = index.activate(&project.id).await?;
                drop(active);
                let result = index.step(&project.id, generation).await;
                app.report(
                    &format!("embedding:{}", project.id),
                    result.err().map(|e| e.to_string()),
                )
                .await;
            }
        }
        tokio::time::sleep(Duration::from_millis(
            runtime.config.service.poll_interval_ms.max(1000),
        ))
        .await;
    }
}

impl Application {
    async fn sync_projection(&self, project: &str, after: &str, limit: i64) -> Result<Value> {
        let _guard = self.projection.lock().await;
        ensure!(
            self.runtime.read().await.config.projection.enabled,
            "memory projection is disabled"
        );
        let config = agentix_task::Config::load(&self.path)?;
        let project = self.tasks.project_result(project).await?;
        let directory = config
            .documents
            .directory
            .join("Projects")
            .join(&project.key)
            .join("Memory");
        let projection =
            MemoryProjection::new(self.store.clone(), &config.documents.root, &directory)?;
        Ok(serde_json::to_value(
            projection.sync(&project.id, after, limit).await?,
        )?)
    }
}
async fn run_projection(app: Arc<Application>) -> Result<()> {
    let mut cursors = BTreeMap::<String, String>::new();
    loop {
        let config = app.runtime.read().await.config.projection;
        if config.enabled {
            let projects = app.tasks.projects().await?;
            cursors.retain(|id, _| projects.iter().any(|p| &p.id == id));
            for project in projects {
                if project.archived_at.is_some() {
                    continue;
                }
                let cursor = cursors.entry(project.id.clone()).or_default();
                match app
                    .sync_projection(&project.id, cursor, config.batch_size)
                    .await
                {
                    Ok(page) => {
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
                    let root = app
                        .repositories
                        .root(&project.id)
                        .await?
                        .context("Project repository unavailable")?;
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
