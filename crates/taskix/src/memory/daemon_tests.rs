use super::*;
use crate::memory_command::MemoryCommand;
use agentix_memory::{Actor, MemoryInput, ProviderConfig, ProviderProtocol};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Semaphore,
};

#[tokio::test]
async fn repository_roots_handle_missing_restored_and_invalid_directories() {
    let dir = tempfile::tempdir().unwrap();
    let tasks = agentix_task::Store::open(&dir.path().join("tasks.db"))
        .await
        .unwrap();
    let root = dir.path().join("repository");
    std::fs::create_dir(&root).unwrap();
    let project = tasks
        .execute(
            json!({"command":"project.register","name":"repository","root":root}),
            agentix_task::WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let repositories = Repositories(tasks);
    let registered = repositories.root(&project).await.unwrap().unwrap();
    std::fs::remove_dir(&root).unwrap();
    assert!(repositories.root(&project).await.unwrap().is_none());
    std::fs::create_dir(&root).unwrap();
    assert_eq!(repositories.root(&project).await.unwrap(), Some(registered));
    std::fs::remove_dir(&root).unwrap();
    std::fs::write(&root, "not a directory").unwrap();
    assert!(
        repositories
            .root(&project)
            .await
            .unwrap_err()
            .to_string()
            .contains("directory")
    );
}

#[tokio::test]
async fn repository_roots_missing_projects_are_skipped_by_reviews() {
    let (_dir, app, project) = repository_test_application().await;
    let root = app.repositories.root(&project).await.unwrap().unwrap();
    std::fs::remove_dir(&root).unwrap();
    let key = format!("review:{project}");
    app.report(&key, Some("old review failure".into())).await;
    let background = tokio::spawn(run_reviews(app.clone()));
    let cleared = tokio::time::timeout(Duration::from_secs(1), async {
        while app.errors.lock().await.contains_key(&key) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    background.abort();
    let _ = background.await;
    assert!(cleared.is_ok(), "missing roots must not keep review errors");
    assert_eq!(
        app.store.memory_status(Some(&project)).await.unwrap()["work"],
        json!({})
    );
}

async fn repository_test_application() -> (tempfile::TempDir, Arc<Application>, String) {
    let dir = tempfile::tempdir().unwrap();
    let task_path = dir.path().join("tasks.db");
    let memory_path = dir.path().join("memory.db");
    let tasks = agentix_task::Store::open(&task_path).await.unwrap();
    let root = dir.path().join("repository");
    std::fs::create_dir(&root).unwrap();
    let project = tasks
        .execute(
            json!({"command":"project.register","name":"repository","root":root}),
            agentix_task::WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let store = MemoryStore::open(&memory_path).await.unwrap();
    let mut config = MemoryConfig::default();
    config.providers.insert(
        "openai".into(),
        ProviderConfig {
            base_url: "http://127.0.0.1:1".into(),
            api_key_env: None,
            protocol: ProviderProtocol::Openai,
            max_in_flight: 1,
        },
    );
    let repositories = Arc::new(Repositories(tasks.clone()));
    let runtime = Arc::new(Runtime::build(config.clone(), &store, repositories.clone()));
    assert!(runtime.worker.is_some());
    let app = Arc::new(Application {
        logging: crate::service::logging::LoggingConfig::default(),
        path: dir.path().join("config.toml"),
        location: MemoryLocation {
            enabled: true,
            path: memory_path,
            task_path,
            service: config.service,
            retrieval: config.retrieval.clone(),
        },
        store,
        tasks,
        repositories,
        runtime: RwLock::new(runtime),
        errors: Mutex::new(BTreeMap::new()),
        projection: Mutex::new(()),
    });
    (dir, app, project)
}

struct EmptyExtractionModel;

#[async_trait]
impl agentix_memory::Model for EmptyExtractionModel {
    async fn complete(
        &self,
        _request: &agentix_memory::ModelRequest,
    ) -> Result<agentix_memory::ModelReply> {
        Ok(agentix_memory::ModelReply {
            continuation: json!([]),
            calls: vec![agentix_memory::ToolCall {
                id: "submit".into(),
                name: "submit_candidates".into(),
                arguments: json!({"candidates":[]}),
            }],
            text: String::new(),
            usage: agentix_memory::TokenUsage::default(),
        })
    }
}

#[tokio::test]
async fn worker_errors_clear_after_success_but_not_after_idle_probes() {
    let (_dir, app, project) = repository_test_application().await;
    let mut config = app.runtime.read().await.config.clone();
    config.agent.extraction_debounce_ms = 0;
    config.service.poll_interval_ms = 20;
    let mut runtime = Runtime::build(config.clone(), &app.store, app.repositories.clone());
    runtime.worker = Some(Arc::new(MemoryWorker::new(
        app.store.clone(),
        Arc::new(EmptyExtractionModel),
        config.agent,
        app.repositories.clone(),
    )));
    *app.runtime.write().await = Arc::new(runtime);
    app.report("worker", Some("Agent step budget exceeded".into()))
        .await;
    let background = tokio::spawn(run_workers(app.clone()));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(app.errors.lock().await.contains_key("worker"));
    let source = serde_json::from_value(json!({
        "instance_id":"db","receipt_id":"recovery","sequence":1,
        "project_id":project,"session_id":"session","turn_id":"turn",
        "revision":1,"job_id":null,"recorded_at":1,
        "messages":[{"id":"message","role":"user","text":"Routine progress"}]
    }))
    .unwrap();
    app.store.ingest(&source).await.unwrap();
    let cleared = tokio::time::timeout(Duration::from_secs(1), async {
        while app.errors.lock().await.contains_key("worker") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    background.abort();
    let _ = background.await;
    assert_eq!(app.store.work_counts().await.unwrap().done, 1);
    assert!(
        cleared.is_ok(),
        "successful work must clear the stale error"
    );
}

#[tokio::test]
async fn slow_project_embedding_does_not_block_another_project() {
    check_project_concurrency(2, true).await;
}

#[tokio::test]
async fn embedding_project_concurrency_limit_is_respected() {
    check_project_concurrency(1, false).await;
}

#[allow(clippy::too_many_lines)] // Keep the blocked-provider and reload scenario together.
async fn check_project_concurrency(limit: usize, parallel: bool) {
    let dir = tempfile::tempdir().unwrap();
    let tasks = agentix_task::Store::open(&dir.path().join("tasks.db"))
        .await
        .unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let mut ids = Vec::new();
    for name in ["a_slow", "b_fast", "c_fast"] {
        let root = dir.path().join(name);
        std::fs::create_dir(&root).unwrap();
        let id = tasks
            .execute(
                json!({"command":"project.register","name":name,"root":root}),
                agentix_task::WriteOptions::default(),
            )
            .await
            .unwrap()
            .result["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let input: MemoryInput=serde_json::from_value(json!({"title":name,"conclusion":"Offline constraint","rationale":"External decision","scope":"project","tags":[],"kind":"user_decision","evidence":[]})).unwrap();
        store.create(&id, input, Actor::Human).await.unwrap();
        ids.push(id);
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let blocked = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let entered = blocked.clone();
    let released = release.clone();
    let server = tokio::spawn(async move {
        let mut requests = JoinSet::new();
        loop {
            tokio::select! {
                result=listener.accept()=>{
                    let (mut socket,_)=result.unwrap(); let entered=entered.clone();let released=released.clone();
                    requests.spawn(async move {
                        let mut bytes=Vec::new();
                        loop {
                            let mut buffer=[0;4096]; let n=socket.read(&mut buffer).await.unwrap(); if n==0 {return;} bytes.extend_from_slice(&buffer[..n]);
                            if let Some(end)=bytes.windows(4).position(|w|w==b"\r\n\r\n") {
                                let head=String::from_utf8_lossy(&bytes[..end]);
                                let len:usize=head.lines().find_map(|l|l.to_ascii_lowercase().strip_prefix("content-length:").map(|v|v.trim().parse().unwrap())).unwrap();
                                if bytes.len()>=end+4+len {break;}
                            }
                        }
                        if String::from_utf8_lossy(&bytes).contains("a_slow") {
                            entered.add_permits(1); released.acquire().await.unwrap().forget();
                        }
                        let body=json!({"data":[{"index":0,"embedding":[1.0,0.0]}]}).to_string();
                        let reply=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                        let _=socket.write_all(reply.as_bytes()).await;
                    });
                },
                _=requests.join_next(),if !requests.is_empty()=>{},
            }
        }
    });
    let mut config = MemoryConfig::default();
    config.embedding.enabled = true;
    config.embedding.max_concurrent_projects = limit;
    config.service.poll_interval_ms = 20;
    config.providers.insert(
        "openai".into(),
        ProviderConfig {
            base_url: url,
            api_key_env: None,
            protocol: ProviderProtocol::Openai,
            max_in_flight: 3,
        },
    );
    let repositories = Arc::new(Repositories(tasks.clone()));
    let runtime = Arc::new(Runtime::build(config.clone(), &store, repositories.clone()));
    let app = Arc::new(Application {
        logging: crate::service::logging::LoggingConfig::default(),
        path: dir.path().join("config.toml"),
        location: MemoryLocation {
            enabled: true,
            path: dir.path().join("memory.db"),
            task_path: dir.path().join("tasks.db"),
            service: config.service,
            retrieval: config.retrieval.clone(),
        },
        store: store.clone(),
        tasks,
        repositories,
        runtime: RwLock::new(runtime),
        errors: Mutex::new(BTreeMap::new()),
        projection: Mutex::new(()),
    });
    let background = tokio::spawn(run_embeddings(app.clone()));
    tokio::time::timeout(Duration::from_secs(2), blocked.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let fast = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if store.memory_status(Some(&ids[1])).await.unwrap()["indexed"] == 1
                && store.memory_status(Some(&ids[2])).await.unwrap()["indexed"] == 1
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if parallel {
        // Let fast Projects become idle, then ensure a commit bypasses their backoff.
        tokio::time::sleep(Duration::from_millis(2200)).await;
        let memory = store.list(&ids[1], "", 1, false).await.unwrap().remove(0);
        store
            .update(
                &ids[1],
                &memory.id,
                memory.revision,
                memory.content,
                Actor::Human,
            )
            .await
            .unwrap();
        let pool = sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path().join("memory.db")),
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_millis(700), async {
            loop {
                let revision: i64 =
                    sqlx::query_scalar("SELECT revision FROM memory_vectors WHERE memory_id=?")
                        .bind(&memory.id)
                        .fetch_optional(&pool)
                        .await
                        .unwrap()
                        .unwrap_or(0);
                if revision == 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("new work must wake an idle Project");
        config.embedding.enabled = false;
        *app.runtime.write().await =
            Arc::new(Runtime::build(config, &store, app.repositories.clone()));
        tokio::time::sleep(Duration::from_millis(100)).await;
        release.add_permits(1);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            store.memory_status(Some(&ids[0])).await.unwrap()["indexed"],
            0,
            "reload must cancel old batches"
        );
    }
    background.abort();
    let _ = background.await;
    server.abort();
    let _ = server.await;
    assert_eq!(
        fast.is_ok(),
        parallel,
        "cross-Project progress must respect the configured bound"
    );
}

#[test]
fn embedding_idle_backoff_wakes_on_changes_and_recovers_periodically() {
    let mut schedule = EmbeddingSchedule::default();
    let now = tokio::time::Instant::now();
    let interval = Duration::from_secs(1);
    assert!(schedule.ready("p", now));
    schedule.complete("p", true, now, interval);
    assert!(!schedule.ready("p", now + interval));
    schedule.wake("p");
    assert!(schedule.ready("p", now));
    for _ in 0..20 {
        schedule.complete("p", true, now, interval);
    }
    assert!(!schedule.ready("p", now + Duration::from_secs(29)));
    assert!(schedule.ready("p", now + Duration::from_secs(30)));
    schedule.complete("p", false, now, interval);
    assert!(schedule.ready("p", now + interval));
}

#[test]
fn idle_workers_back_off_and_work_notification_resets_the_probe_deadline() {
    let now = tokio::time::Instant::now();
    let mut idle = WorkerIdle::new(now);
    let interval = Duration::from_millis(100);
    for _ in 0..20 {
        idle.empty(now, interval);
    }
    assert!(!idle.ready(now + Duration::from_secs(4)));
    assert!(idle.ready(now + Duration::from_secs(5)));
    idle.wake(now);
    assert!(idle.ready(now));
    idle.empty(now, interval);
    assert!(idle.ready(now + Duration::from_millis(200)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "idle daemon benchmark; run alone with --test-threads=1"]
async fn idle_daemon_cpu_with_many_projects() {
    let dir = tempfile::tempdir().unwrap();
    let tasks = agentix_task::Store::open(&dir.path().join("tasks.db"))
        .await
        .unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    for n in 0..200 {
        let root = dir.path().join(format!("p{n}"));
        std::fs::create_dir(&root).unwrap();
        tasks
            .execute(
                json!({"command":"project.register","name":format!("p{n}"),"root":root}),
                agentix_task::WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    let mut config = MemoryConfig::default();
    config.embedding.enabled = true;
    config.providers.insert(
        "openai".into(),
        ProviderConfig {
            base_url: "http://127.0.0.1:1".into(),
            api_key_env: None,
            protocol: ProviderProtocol::Openai,
            max_in_flight: 4,
        },
    );
    let repositories = Arc::new(Repositories(tasks.clone()));
    let runtime = Arc::new(Runtime::build(config.clone(), &store, repositories.clone()));
    assert!(runtime.worker.is_some());
    assert!(runtime.embedding.is_some());
    let app = Arc::new(Application {
        logging: crate::service::logging::LoggingConfig::default(),
        path: dir.path().join("config.toml"),
        location: MemoryLocation {
            enabled: true,
            path: dir.path().join("memory.db"),
            task_path: dir.path().join("tasks.db"),
            service: config.service,
            retrieval: config.retrieval.clone(),
        },
        store,
        tasks,
        repositories,
        runtime: RwLock::new(runtime),
        errors: Mutex::new(BTreeMap::new()),
        projection: Mutex::new(()),
    });
    let workers = tokio::spawn(run_workers(app.clone()));
    let embeddings = tokio::spawn(run_embeddings(app.clone()));
    tokio::time::sleep(Duration::from_secs(10)).await;
    let before = process_cpu_seconds();
    let start = std::time::Instant::now();
    tokio::time::sleep(Duration::from_secs(5)).await;
    let elapsed = start.elapsed().as_secs_f64();
    let after = process_cpu_seconds();
    workers.abort();
    embeddings.abort();
    let _ = workers.await;
    let _ = embeddings.await;
    assert!(app.errors.lock().await.is_empty());
    if let (Some(before), Some(after)) = (before, after) {
        println!(
            "200 idle Projects, extraction+embedding loops, 10s warmup: wall={elapsed:.3}s process_cpu={:.3}s one_core_utilization={:.2}%",
            after - before,
            100.0 * (after - before) / elapsed
        );
    } else {
        println!("CPU sampling unavailable: ps time field unsupported");
    }
}

fn process_cpu_seconds() -> Option<f64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    text.trim().split(':').try_fold(0.0, |seconds, part| {
        Some(seconds * 60.0 + part.parse::<f64>().ok()?)
    })
}

#[tokio::test]
async fn reload_preserves_admission_for_an_active_deep_query() {
    check_reload_admission(true).await;
}

#[tokio::test]
async fn reload_preserves_provider_admission_and_releases_cancelled_requests() {
    check_reload_admission(false).await;
}

#[allow(clippy::too_many_lines)] // Exercise reload while the original HTTP request is live.
async fn check_reload_admission(deep_limit: bool) {
    // Reload reads the process environment. Run each fixture in an isolated
    // process rather than mutating environment shared by parallel tests.
    const CHILD: &str = "TASKIX_MEMORY_RELOAD_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let name = if deep_limit {
            "memory::daemon::tests::reload_preserves_admission_for_an_active_deep_query"
        } else {
            "memory::daemon::tests::reload_preserves_provider_admission_and_releases_cancelled_requests"
        };
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env(CHILD, "1")
            .env("TASKIX_MEMORY_ENABLED", "true")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let task_path = dir.path().join("tasks.db");
    let memory_path = dir.path().join("memory.db");
    let tasks = agentix_task::Store::open(&task_path).await.unwrap();
    let project = tasks
        .execute(
            json!({"command":"project.register","name":"reload","root":dir.path()}),
            agentix_task::WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let store = MemoryStore::open(&memory_path).await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = MemoryConfig {
        enabled: true,
        ..MemoryConfig::default()
    };
    config.storage.path = Some(memory_path.clone());
    config.service.max_deep_queries = if deep_limit { 1 } else { 2 };
    config.agent.model = "gpt-6-astra".into();
    config.providers.insert(
        "openai".into(),
        ProviderConfig {
            base_url: format!("http://{}", listener.local_addr().unwrap()),
            api_key_env: None,
            protocol: ProviderProtocol::Openai,
            max_in_flight: if deep_limit { 2 } else { 1 },
        },
    );
    let path = dir.path().join("config.toml");
    let mut document = toml::Table::new();
    document.insert("schema_version".into(), toml::Value::Integer(1));
    document.insert(
        "storage".into(),
        toml::Value::Table(toml::Table::from_iter([(
            "path".into(),
            toml::Value::String(task_path.to_string_lossy().into_owned()),
        )])),
    );
    document.insert("memory".into(), toml::Value::try_from(&config).unwrap());
    std::fs::write(&path, toml::to_string(&document).unwrap()).unwrap();
    let repositories = Arc::new(Repositories(tasks.clone()));
    let runtime = Arc::new(Runtime::build(config.clone(), &store, repositories.clone()));
    let app = Arc::new(Application {
        logging: crate::service::logging::LoggingConfig::load(&path).unwrap(),
        path,
        location: MemoryLocation {
            enabled: true,
            path: memory_path,
            task_path,
            service: config.service,
            retrieval: config.retrieval.clone(),
        },
        store,
        tasks,
        repositories,
        runtime: RwLock::new(runtime),
        errors: Mutex::new(BTreeMap::new()),
        projection: Mutex::new(()),
    });
    let request = json!({"op":"ask","project":project,"query":"external decision"});
    let first_app = app.clone();
    let first_request = request.clone();
    let first = tokio::spawn(async move { first_app.handle(first_request).await });
    let (socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut compact_changes = app.store.subscribe_compaction();
    app.reload().await.unwrap();
    assert_eq!(
        compact_changes.try_recv().unwrap(),
        None,
        "reload must wake dirty recovery"
    );
    if !deep_limit {
        let second = tokio::spawn(async move { app.handle(request).await });
        let admitted = tokio::time::timeout(Duration::from_millis(100), listener.accept()).await;
        first.abort();
        let _ = first.await;
        assert!(
            admitted.is_err(),
            "reload admitted a second provider request"
        );
        // Cancellation must release the shared permit for the new generation.
        let resumed = tokio::time::timeout(Duration::from_secs(2), listener.accept()).await;
        second.abort();
        let _ = second.await;
        assert!(resumed.is_ok(), "provider permit leaked after cancellation");
        return;
    }
    let second = tokio::time::timeout(Duration::from_millis(200), app.handle(request)).await;
    first.abort();
    let _ = first.await;
    drop(socket);
    let error = second
        .expect("reload must reject excess queries without contacting the model")
        .unwrap_err();
    assert!(error.to_string().contains("busy:"), "{error}");
}

#[tokio::test]
async fn repository_review_loop_does_not_scan_semantic_compaction() {
    let (_dir, app, project) = repository_test_application().await;
    let key = format!("compact:{project}");
    app.report(
        &key,
        Some("Sentinel: only compact scheduling clears this".into()),
    )
    .await;
    let background = tokio::spawn(run_reviews(app.clone()));
    tokio::time::sleep(Duration::from_millis(100)).await;
    background.abort();
    let _ = background.await;
    assert!(
        app.errors.lock().await.contains_key(&key),
        "the periodic repository review must not perform a compact scan"
    );
}

async fn wait_compaction_pass(app: &Application, project: &str) {
    let key = format!("compact:{project}");
    tokio::time::timeout(Duration::from_secs(2), async {
        while app.errors.lock().await.contains_key(&key) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("compaction pass must finish");
}

#[tokio::test]
async fn compact_loop_waits_without_database_scans_when_idle() {
    let (_dir, app, project) = repository_test_application().await;
    let key = format!("compact:{project}");
    app.report(&key, Some("Await startup recovery".into()))
        .await;
    let background = tokio::spawn(run_compactions(app.clone()));
    wait_compaction_pass(&app, &project).await;
    // Any further compact SQL would fail, making a periodic scan observable.
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(&app.location.path),
    )
    .await
    .unwrap();
    sqlx::query("DROP TABLE memory_compactions")
        .execute(&pool)
        .await
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_hours(48)).await;
    tokio::time::resume();
    tokio::time::sleep(Duration::from_millis(100)).await;
    background.abort();
    let _ = background.await;
    assert!(
        !app.errors.lock().await.contains_key(&key),
        "two idle days must not trigger another compact database scan"
    );
    pool.close().await;
}

#[tokio::test]
async fn compact_loop_wakes_after_a_write_and_uses_the_debounce_deadline() {
    let (_dir, app, project) = repository_test_application().await;
    let mut config = app.runtime.read().await.config.clone();
    config.agent.compaction_debounce_seconds = 5;
    *app.runtime.write().await =
        Arc::new(Runtime::build(config, &app.store, app.repositories.clone()));
    let key = format!("compact:{project}");
    app.report(&key, Some("Await startup recovery".into()))
        .await;
    let background = tokio::spawn(run_compactions(app.clone()));
    wait_compaction_pass(&app, &project).await;
    // Leave room for SQLite setup and scheduling on loaded CI runners.
    create_compact_memory(&app, &project).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        app.store.work_counts().await.unwrap().pending,
        0,
        "do not compact before the debounce"
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while app.store.work_counts().await.unwrap().pending == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a write must wake compact without waiting for a periodic scan");
    background.abort();
    let _ = background.await;
    assert_eq!(app.store.work_counts().await.unwrap().pending, 1);
}

async fn create_compact_memory(app: &Application, project: &str) {
    let input: MemoryInput = serde_json::from_value(json!({
        "title":"External policy","conclusion":"Keep the external server policy",
        "rationale":"Confirmed constraint","scope":"server","tags":[],"kind":"user_decision",
        "evidence":[{"receipt_id":"write","message_id":"message","quote":"Keep the external server policy"}]
    })).unwrap();
    let source: Source = serde_json::from_value(json!({
        "instance_id":"db","receipt_id":"write","sequence":1,"project_id":project,
        "session_id":"session","turn_id":"turn","revision":1,"job_id":null,"recorded_at":1,
        "messages":[{"id":"message","role":"user","text":"Keep the external server policy"}]
    }))
    .unwrap();
    app.store.ingest(&source).await.unwrap();
    let lease = app
        .store
        .claim_work(
            "setup",
            &app.runtime.read().await.config.agent,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await
        .unwrap()
        .unwrap();
    app.store
        .complete_extraction(
            &lease,
            vec![],
            time::OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await
        .unwrap();
    app.store
        .create(project, input, Actor::Agent)
        .await
        .unwrap();
}

#[tokio::test]
async fn compact_loop_recovers_dirty_work_when_reenabled() {
    let (_dir, app, project) = repository_test_application().await;
    let mut config = app.runtime.read().await.config.clone();
    config.agent.compaction_enabled = false;
    config.agent.compaction_debounce_seconds = 0;
    *app.runtime.write().await = Arc::new(Runtime::build(
        config.clone(),
        &app.store,
        app.repositories.clone(),
    ));
    let background = tokio::spawn(run_compactions(app.clone()));
    create_compact_memory(&app, &project).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(app.store.work_counts().await.unwrap().pending, 0);
    config.agent.compaction_enabled = true;
    *app.runtime.write().await =
        Arc::new(Runtime::build(config, &app.store, app.repositories.clone()));
    app.store.wake_compaction();
    tokio::time::timeout(Duration::from_secs(2), async {
        while app.store.work_counts().await.unwrap().pending == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("reload must resume dirty work without waiting for a new write");
    background.abort();
    let _ = background.await;
    assert_eq!(app.store.work_counts().await.unwrap().pending, 1);
}

#[tokio::test]
async fn compact_loop_recovers_history_before_waiting_for_writes() {
    let (_dir, app, project) = repository_test_application().await;
    let mut config = app.runtime.read().await.config.clone();
    config.agent.compaction_debounce_seconds = 0;
    *app.runtime.write().await =
        Arc::new(Runtime::build(config, &app.store, app.repositories.clone()));
    create_compact_memory(&app, &project).await;
    // No receiver was subscribed when history was written; startup must recover it.
    let background = tokio::spawn(run_compactions(app.clone()));
    tokio::time::timeout(Duration::from_secs(2), async {
        while app.store.work_counts().await.unwrap().pending == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("startup must recover durable dirty history");
    background.abort();
    let _ = background.await;
    assert_eq!(app.store.work_counts().await.unwrap().pending, 1);
}

#[tokio::test]
async fn cancelled_job_is_excluded_before_background_poll() {
    let (_dir, app, project) = repository_test_application().await;
    let job = app
        .tasks
        .execute(
            json!({"command":"job.create","project":project,"title":"Backup policy"}),
            agentix_task::WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let source: Source = serde_json::from_value(json!({
        "instance_id":"test", "receipt_id":"cancel-source", "sequence":1,
        "project_id":project,"session_id":"s","turn_id":"t","revision":1,
        "job_id":job,"recorded_at":1,
        "messages":[{"id":"m","role":"user","text":"Use offline backups"}]
    }))
    .unwrap();
    app.store.ingest(&source).await.unwrap();
    let content: MemoryInput = serde_json::from_value(json!({
        "title":"Backup policy","conclusion":"Use offline backups","rationale":"Decision",
        "scope":"project","tags":[],"kind":"user_decision",
        "evidence":[{"receipt_id":"cancel-source","message_id":"m","quote":"Use offline backups"}]
    }))
    .unwrap();
    let memory = app
        .store
        .create(&project, content, Actor::Agent)
        .await
        .unwrap();
    app.tasks
        .execute(
            json!({"command":"job.cancel","job":job}),
            agentix_task::WriteOptions::default(),
        )
        .await
        .unwrap();
    app.handle(json!({"op":"search","project":project,"query":"backups"}))
        .await
        .unwrap();
    assert!(
        app.store
            .search(&project, "backups", 10)
            .await
            .unwrap()
            .is_empty(),
        "a query after cancellation must exclude the source without waiting for background polling"
    );
    let current = app.store.show(&project, &memory.id, None).await.unwrap();
    assert_eq!(serde_json::to_value(current.status).unwrap(), "invalidated");
    assert_eq!(
        app.store
            .show(&project, &memory.id, Some(1))
            .await
            .unwrap()
            .revision,
        1
    );
}

#[tokio::test]
async fn offline_search_filters_cancelled_sources_without_mutating_memory() {
    let (_dir, app, project) = repository_test_application().await;
    let job = app
        .tasks
        .execute(
            json!({"command":"job.create","project":project,"title":"Policy"}),
            agentix_task::WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let source: Source = serde_json::from_value(json!({"instance_id":"test","receipt_id":"offline","sequence":1,"project_id":project,"session_id":"s","turn_id":"t","revision":1,"job_id":job,"recorded_at":1,"messages":[{"id":"m","role":"user","text":"Offline backups"}]})).unwrap();
    app.store.ingest(&source).await.unwrap();
    let memory = app.store.create(&project, serde_json::from_value(json!({"title":"Backups","conclusion":"Offline backups","rationale":"Decision","scope":"project","tags":[],"kind":"user_decision","evidence":[{"receipt_id":"offline","message_id":"m","quote":"Offline backups"}]})).unwrap(), Actor::Agent).await.unwrap();
    app.tasks
        .execute(
            json!({"command":"job.cancel","job":job}),
            agentix_task::WriteOptions::default(),
        )
        .await
        .unwrap();
    let memories = super::super::filter_cancelled(&app.store, &app.location, vec![memory.clone()])
        .await
        .unwrap();
    assert!(memories.is_empty());
    for (action, request, field) in [
        (
            MemoryCommand::Search {
                query: "backups".into(),
                limit: 10,
            },
            json!({"op":"search","project":project,"query":"backups"}),
            "memories",
        ),
        (
            MemoryCommand::Context {
                query: "backups".into(),
                turn: "turn".into(),
                budget: None,
            },
            json!({"op":"context","project":project}),
            "items",
        ),
    ] {
        let result = super::super::offline(&app.location, request, &action, "service stopped")
            .await
            .unwrap();
        assert!(result[field].as_array().unwrap().is_empty());
    }
    let result = super::super::offline(
        &app.location,
        json!({"op":"list","project":project}),
        &MemoryCommand::List {
            after: String::new(),
            limit: 20,
            all: false,
        },
        "service stopped",
    )
    .await
    .unwrap();
    assert!(result.as_array().unwrap().is_empty());
    assert_eq!(
        app.store
            .show(&project, &memory.id, None)
            .await
            .unwrap()
            .status,
        agentix_memory::Status::Active
    );
}

#[tokio::test]
async fn cancellation_sync_rejects_a_task_database_older_than_its_checkpoint() {
    let (_dir, app, _project) = repository_test_application().await;
    app.store.checkpoint_cancellations(5).await.unwrap();
    assert!(sync_cancellations(&app.tasks, &app.store).await.is_err());
}
