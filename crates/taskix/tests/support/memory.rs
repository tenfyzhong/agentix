use super::*;
use std::{
    process::Stdio,
    time::{Duration, Instant},
};

struct Daemon(std::process::Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[allow(clippy::too_many_lines)]
#[test]
fn memory_daemon_cli_and_offline_fallback_do_not_require_a_vault_or_model_credentials() {
    let cli = Cli::new();
    let help = cli.run(&["memory", "--help"]);
    assert!(
        help.status.success(),
        "{}",
        String::from_utf8_lossy(&help.stderr)
    );
    let project = cli.ok(&[
        "project",
        "register",
        "--root",
        cli.dir.path().to_str().unwrap(),
    ]);
    let project = project["id"].as_str().unwrap();
    let path = cli.dir.path().join("config.toml");
    let mut config = std::fs::read_to_string(&path).unwrap();
    config.push_str("\n[memory]\nenabled = true\n[memory.providers.openai]\nbase_url = 'http://127.0.0.1:9/v1'\n[memory.agent]\nmodel = 'gpt-6-astra'\nrequest_timeout_seconds = 1\n[memory.service]\npoll_interval_ms = 50\n");
    std::fs::write(&path, &config).unwrap();
    let capture = cli.dir.path().join("source.json");
    std::fs::write(&capture,json!({"turn_id":"turn","messages":[{"id":"message","role":"user","text":"External project constraint"}]}).to_string()).unwrap();
    cli.ok(&[
        "hook",
        "record",
        "--project",
        project,
        "--session",
        "discussion",
        "--file",
        capture.to_str().unwrap(),
    ]);
    std::fs::remove_dir_all(cli.dir.path().join("vault")).unwrap();
    let log = std::fs::File::create(cli.dir.path().join("daemon.log")).unwrap();
    let mut daemon = Daemon(
        cli.command(&["memory", "serve"])
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let output = cli.run(&["memory", "status", "--project", project]);
        if output.status.success() {
            let status: Value = serde_json::from_slice(&output.stdout).unwrap();
            if status["result"]["online"] == true
                && status["result"]["sources"].as_i64().unwrap_or(0) > 0
            {
                break;
            }
        }
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "{}",
            std::fs::read_to_string(cli.dir.path().join("daemon.log")).unwrap()
        );
        assert!(
            Instant::now() < deadline,
            "memory daemon failed to become ready"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let input = cli.dir.path().join("memory.json");
    std::fs::write(&input,json!({"title":"离线约束","conclusion":"离线可用","rationale":"用户要求","scope":"project","tags":[],"kind":"user_decision","evidence":[]}).to_string()).unwrap();
    let created = cli.ok(&[
        "memory",
        "create",
        "--project",
        project,
        "--file",
        input.to_str().unwrap(),
    ]);
    let found = cli.ok(&["memory", "search", "离线", "--project", project]);
    assert_eq!(found["memories"][0]["id"], created["id"]);
    let context = cli.ok(&[
        "memory",
        "context",
        "离线",
        "--project",
        project,
        "--session",
        "host",
        "--turn",
        "t1",
    ]);
    assert_eq!(context["items"].as_array().unwrap().len(), 1);
    let context = cli.ok(&[
        "memory",
        "context",
        "离线",
        "--project",
        project,
        "--session",
        "host",
        "--turn",
        "t2",
    ]);
    assert_eq!(
        context["items"].as_array().unwrap().len(),
        1,
        "CLI reads do not imply host delivery"
    );
    daemon.0.kill().unwrap();
    daemon.0.wait().unwrap();
    std::fs::write(
        &path,
        config.replace("model = 'gpt-6-astra'", "model = false"),
    )
    .unwrap();
    let offline = cli.ok(&["memory", "search", "离线", "--project", project]);
    assert_eq!(offline["mode"], "offline_fts");
    assert_eq!(offline["memories"][0]["id"], created["id"]);
    assert!(
        !cli.run(&[
            "memory",
            "create",
            "--project",
            project,
            "--file",
            input.to_str().unwrap()
        ])
        .status
        .success()
    );
}

#[test]
fn memory_reload_rejects_ipc_limits_without_changing_the_running_configuration() {
    let cli = Cli::new();
    cli.ok(&[
        "project",
        "register",
        "--root",
        cli.dir.path().to_str().unwrap(),
    ]);
    let path = cli.dir.path().join("config.toml");
    let base = std::fs::read_to_string(&path).unwrap();
    let config = format!(
        "{base}\n[memory]\nenabled=true\n[memory.providers.openai]\nbase_url='http://127.0.0.1:9/v1'\n[memory.agent]\nmodel='gpt-6-astra'\n"
    );
    std::fs::write(&path, &config).unwrap();
    let log_path = cli.dir.path().join("reload-service.log");
    let mut daemon = Daemon(
        cli.command(&["memory", "serve"])
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log_path).unwrap())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if cli.ok(&["memory", "status"])["online"] == true {
            break;
        }
        assert!(
            daemon.0.try_wait().unwrap().is_none() && Instant::now() < deadline,
            "{}",
            std::fs::read_to_string(&log_path).unwrap()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    std::fs::write(
        &path,
        format!("{config}\n[memory.service]\nmax_request_bytes=524288\n"),
    )
    .unwrap();
    let result = cli.run(&["memory", "reload"]);
    assert!(!result.status.success(), "IPC bounds require a restart");
    assert_eq!(cli.ok(&["memory", "status"])["model"], "gpt-6-astra");
    std::fs::write(&path, config.replace("gpt-6-astra", "gpt-6-sol")).unwrap();
    assert_eq!(cli.ok(&["memory", "reload"])["reloaded"], true);
    assert_eq!(cli.ok(&["memory", "status"])["model"], "gpt-6-sol");
    std::fs::write(&path, format!("{config}reasoning_effort='low'\n")).unwrap();
    assert_eq!(cli.ok(&["memory", "reload"])["reloaded"], true);
    assert_eq!(cli.ok(&["memory", "status"])["reasoning_effort"], "low");
    assert_eq!(
        cli.ok(&["memory", "doctor"])["configuration"]["reasoning_effort"],
        "low"
    );
    std::fs::write(&path, format!("{config}reasoning_effort='loow'\n")).unwrap();
    assert!(!cli.run(&["memory", "reload"]).status.success());
    assert_eq!(cli.ok(&["memory", "status"])["reasoning_effort"], "low");
}

#[allow(clippy::too_many_lines)]
#[test]
fn memory_service_restores_read_only_notes_through_the_real_cli() {
    let cli = Cli::new();
    let project = cli.ok(&[
        "project",
        "register",
        "--root",
        cli.dir.path().to_str().unwrap(),
    ]);
    let id = project["id"].as_str().unwrap();
    let config_path = cli.dir.path().join("config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(&config_path, format!("{config}\n[memory]\nenabled=true\n[memory.providers.openai]\nbase_url='http://127.0.0.1:9/v1'\n")).unwrap();
    let daemon = Daemon(
        cli.command(&["memory", "serve"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while cli.ok(&["memory", "status"])["online"] != true {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let input = cli.dir.path().join("memory-input.json");
    std::fs::write(&input, json!({"title":"Regional endpoint","conclusion":"Use regional service","rationale":"Residency policy","scope":"Production","conditions":[],"tags":[],"kind":"user_assertion","evidence":[]}).to_string()).unwrap();
    let memory = cli.ok(&[
        "memory",
        "create",
        "--project",
        id,
        "--file",
        input.to_str().unwrap(),
    ]);
    let memory_id = memory["id"].as_str().unwrap();
    cli.ok(&["memory", "sync", "--project", id]);
    let path = cli
        .dir
        .path()
        .join("vault")
        .join("Tasks ☃")
        .join("Projects")
        .join(project["key"].as_str().unwrap())
        .join("Memory")
        .join(format!("{memory_id}.md"));
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.replace("Use regional service", "Use European service"),
    )
    .unwrap();
    cli.ok(&["memory", "sync", "--project", id]);
    assert_eq!(
        cli.ok(&["memory", "show", memory_id, "--project", id])["content"]["conclusion"],
        "Use regional service"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    assert_eq!(
        cli.ok(&["memory", "projection-status", "--project", id])["pending"],
        0
    );
    drop(daemon);
    let relative = path
        .strip_prefix(cli.dir.path().join("vault"))
        .unwrap()
        .to_str()
        .unwrap();
    let document = cli.ok(&["memory", "document", relative]);
    assert_eq!(document["text"], text);
    assert_eq!(document["path"], relative);
    let foreign = relative.replace(project["key"].as_str().unwrap(), "foreign-project");
    assert!(!cli.run(&["memory", "document", &foreign]).status.success());

    assert!(
        !cli.run(&["memory", "document", &format!("../{relative}")])
            .status
            .success()
    );
}

fn start_memory(cli: &Cli, expected_sources: i64) -> Daemon {
    let log = std::fs::File::create(cli.dir.path().join("recovery.log")).unwrap();
    let mut daemon = Daemon(
        cli.command(&["memory", "serve"])
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = cli.run(&["memory", "status"]);
        if status.status.success() {
            let value: Value = serde_json::from_slice(&status.stdout).unwrap();
            if value["result"]["online"] == true && value["result"]["sources"] == expected_sources {
                return daemon;
            }
        }
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "{}",
            std::fs::read_to_string(cli.dir.path().join("recovery.log")).unwrap()
        );
        assert!(Instant::now() < deadline, "replay did not complete");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn recovery_capture(cli: &Cli, turn: &str) {
    let path = cli.dir.path().join("capture.json");
    std::fs::write(&path, json!({"turn_id":turn,"messages":[{"id":turn,"role":"user","text":"Offline recovery is required"}]}).to_string()).unwrap();
    cli.ok(&[
        "hook",
        "record",
        "--session",
        "restore-session",
        "--file",
        path.to_str().unwrap(),
    ]);
}

#[test]
fn memory_replay_retries_a_failed_acknowledged_receipt_after_later_receipts_succeed() {
    let cli = Cli::new();
    cli.ok(&[
        "project",
        "register",
        "--root",
        cli.dir.path().to_str().unwrap(),
    ]);
    let path = cli.dir.path().join("config.toml");
    let config = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("{config}\n[memory]\nenabled=true\n[memory.providers.openai]\nbase_url='http://127.0.0.1:9/v1'\n[memory.service]\npoll_interval_ms=20\n")).unwrap();
    for turn in ["first", "second", "third"] {
        recovery_capture(&cli, turn);
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let memory_path = cli.dir.path().join("memory.sqlite3");
    runtime.block_on(async {
        let tasks = agentix_task::Store::open(&cli.dir.path().join("state.sqlite3")).await.unwrap();
        // A restored older memory database must replay receipts already acknowledged
        // by the newer task database. Fail only the second replay insertion.
        for source in tasks.memory_sources(0, 100).await.unwrap() {
            tasks.acknowledge_memory_source(&source.instance_id, &source.receipt_id).await.unwrap();
        }
        let _memory = agentix_memory::MemoryStore::open(&memory_path).await.unwrap();
        let pool = sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&memory_path)).await.unwrap();
        sqlx::query("CREATE TRIGGER fail_replay BEFORE INSERT ON sources WHEN json_extract(NEW.data,'$.sequence')=2 BEGIN SELECT RAISE(FAIL,'injected transient replay failure'); END").execute(&pool).await.unwrap();
        pool.close().await;
    });
    let daemon = Daemon(
        cli.command(&["memory", "serve"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = cli.ok(&["memory", "status"]);
        if status["background_errors"]
            .to_string()
            .contains("injected transient replay failure")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "injected replay failure was not observed"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // Keep the failure installed until the daemon stops, so recovery must cross restart.
    drop(daemon);
    runtime.block_on(async {
        let tasks = agentix_task::Store::open(&cli.dir.path().join("state.sqlite3"))
            .await
            .unwrap();
        let later = tasks.replay_memory_sources(2, 1).await.unwrap().remove(0);
        let memory = agentix_memory::MemoryStore::open(&memory_path)
            .await
            .unwrap();
        // Independent intake may have persisted a later source beyond the checkpoint.
        memory
            .ingest(&serde_json::from_value(serde_json::to_value(later).unwrap()).unwrap())
            .await
            .unwrap();
        let pool = sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(&memory_path),
        )
        .await
        .unwrap();
        sqlx::query("DROP TRIGGER fail_replay")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    });
    // Restart also must not treat MAX(sequence)=3 as proof that receipt 2 exists.
    let _daemon = Daemon(
        cli.command(&["memory", "serve"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut count;
    loop {
        count = cli.ok(&["memory", "status"])["sources"]
            .as_i64()
            .unwrap_or(0);
        if count == 3 || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        count, 3,
        "a failed replay receipt must remain recoverable after later successes and restart"
    );
}

fn database_snapshot(source: &std::path::Path, destination: &std::path::Path) {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let pool = sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(source),
        )
        .await
        .unwrap();
        sqlx::query("VACUUM INTO ?")
            .bind(destination.to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    });
}

#[test]
#[allow(clippy::too_many_lines)]
fn memory_restore_replays_acknowledged_sources_and_rejects_a_forked_task_history() {
    let cli = Cli::new();
    cli.ok(&[
        "project",
        "register",
        "--root",
        cli.dir.path().to_str().unwrap(),
    ]);
    let config_path = cli.dir.path().join("config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(&config_path, format!("{config}\n[memory]\nenabled=true\n[memory.providers.openai]\nbase_url='http://127.0.0.1:9/v1'\n[memory.service]\npoll_interval_ms=50\n")).unwrap();
    recovery_capture(&cli, "first");
    let daemon = start_memory(&cli, 1);
    let memory_path = cli.dir.path().join("memory.sqlite3");
    let old_memory = cli.dir.path().join("old-memory.sqlite3");
    database_snapshot(&memory_path, &old_memory);
    drop(daemon);
    recovery_capture(&cli, "second");
    let daemon = start_memory(&cli, 2);
    // Confirm the outbox no longer exposes either receipt as pending.
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let tasks = agentix_task::Store::open(&cli.dir.path().join("state.sqlite3"))
            .await
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !tasks.memory_sources(0, 100).await.unwrap().is_empty() {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    drop(daemon);
    // An offline snapshot safely folds a possible crash WAL before replacement.
    let discarded = cli.dir.path().join("discarded.sqlite3");
    database_snapshot(&memory_path, &discarded);
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", memory_path.display()));
    }
    std::fs::copy(old_memory, &memory_path).unwrap();
    let daemon = start_memory(&cli, 2);
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/taskix-backup.py");
    let archives = cli.dir.path().join("backups");
    let backup = Command::new("python3")
        .arg(&script)
        .arg("--config")
        .arg(&config_path)
        .arg("--output-dir")
        .arg(&archives)
        .args(["--remote", "mock:archive", "--rclone", "/usr/bin/true"])
        .output()
        .unwrap();
    assert!(
        backup.status.success(),
        "{}",
        String::from_utf8_lossy(&backup.stderr)
    );
    let archive = std::fs::read_dir(&archives)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with(".tar.gz"))
        .unwrap();
    let restored = cli.dir.path().join("restored");
    let restore = Command::new("python3")
        .arg(script)
        .arg("--restore")
        .arg(archive)
        .arg("--restore-dir")
        .arg(&restored)
        .output()
        .unwrap();
    assert!(
        restore.status.success(),
        "{}",
        String::from_utf8_lossy(&restore.stderr)
    );
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let memory = agentix_memory::MemoryStore::open_read_only(&restored.join("memory.sqlite3"))
            .await
            .unwrap();
        let tasks = agentix_task::Store::open(&restored.join("tasks.sqlite3"))
            .await
            .unwrap();
        let sources = memory.recovery_sources("", 100).await.unwrap();
        assert_eq!(sources.len(), 2);
        tasks
            .verify_memory_sources(
                &serde_json::from_value::<Vec<agentix_task::MemorySource>>(
                    serde_json::to_value(sources).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
    });
    drop(daemon);
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let pool = sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(cli.dir.path().join("state.sqlite3")),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE memory_source_outbox SET receipt_id='fork' WHERE sequence=1")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    });
    let rejected = cli.run(&["memory", "serve"]);
    assert!(!rejected.status.success());
    assert!(
        format!(
            "{}{}",
            String::from_utf8_lossy(&rejected.stdout),
            String::from_utf8_lossy(&rejected.stderr)
        )
        .contains("histories differ")
    );
}

#[test]
fn memory_doctor_reports_invalid_model_configuration_without_a_provider_request() {
    let cli = Cli::new();
    let path = cli.dir.path().join("config.toml");
    let config = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        format!("{config}\n[memory]\nenabled=true\n[memory.agent]\nmodel=false\n"),
    )
    .unwrap();
    let result = cli.ok(&["memory", "doctor"]);
    assert_eq!(result["configuration"]["valid"], false);
    assert_eq!(result["database"]["exists"], false);
    assert!(!cli.dir.path().join("memory.sqlite3").exists());
}

#[path = "../../../agentix-memory/tests/support/http.rs"]
mod provider_http;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_visible_turn_to_mock_model_to_real_host_hook_end_to_end() {
    fn response(name: &str, args: &Value) -> (u16, Value) {
        (
            200,
            json!({"status":"completed","output":[{"type":"function_call","call_id":name,"name":name,"arguments":args.to_string()}]}),
        )
    }
    let cli = Cli::new();
    cli.ok(&[
        "project",
        "register",
        "--root",
        cli.dir.path().to_str().unwrap(),
    ]);
    recovery_capture(&cli, "first");
    let tasks = agentix_task::Store::open(&cli.dir.path().join("state.sqlite3"))
        .await
        .unwrap();
    let source = tasks.memory_sources(0, 1).await.unwrap().remove(0);
    let candidate = json!({"title":"Offline recovery","conclusion":"Offline recovery is required","rationale":"User constraint","scope":"project","conditions":[],"valid_until":null,"tags":[],"kind":"user_decision","evidence":[{"receipt_id":source.receipt_id,"message_id":"first","quote":"Offline recovery is required"}]});
    let server = provider_http::MockHttp::start(vec![
        response("repo_read", &json!({"path":"README.md","offset":0})),
        response("submit_candidates", &json!({"candidates":[candidate]})),
        response("memory_search", &json!({"query":"Offline recovery"})),
        response("submit_decisions", &json!({"decisions":[{"candidate":0,"action":"create","target":null,"expected_revision":null,"content":null,"reason":"New external constraint"}]})),
    ]).await;
    std::fs::write(cli.dir.path().join("README.md"), "Repository overview").unwrap();
    let config_path = cli.dir.path().join("config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(&config_path, format!("{config}\n[memory]\nenabled=true\n[memory.providers.mock]\nbase_url={}\n[memory.agent]\nprovider='mock'\nrepository_review_interval_seconds=0\n[memory.service]\npoll_interval_ms=50\n", json!(server.url))).unwrap();
    let _daemon = start_memory(&cli, 1);
    let receipt = cli.ok(&[
        "memory",
        "receipt",
        &source.receipt_id,
        "--wait-seconds",
        "10",
    ]);
    assert_eq!(receipt["complete"], true, "{receipt}");
    let memories = cli.ok(&["memory", "search", "Offline recovery"]);
    assert_eq!(
        memories["memories"][0]["content"]["evidence"][0]["receipt_id"],
        source.receipt_id
    );
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let binary_dir = std::path::Path::new(env!("CARGO_BIN_EXE_taskix"))
        .parent()
        .unwrap();
    let mut paths = vec![binary_dir.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let host = Command::new("node")
        .arg("--test")
        .arg(root.join("plugins/taskix-manager/tests/support/memory-service.mjs"))
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("TASKIX_CONFIG", config_path)
        .current_dir(cli.dir.path())
        .output()
        .unwrap();
    assert!(
        host.status.success(),
        "{}{}",
        String::from_utf8_lossy(&host.stdout),
        String::from_utf8_lossy(&host.stderr)
    );
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert_eq!(
        requests[2].1["input"].as_array().unwrap().len(),
        1,
        "consolidation must start with fresh context"
    );
}

#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_jev_triage_uses_existing_environment_and_reports_skip_extract_and_fallback() {
    for (enabled, choice, confidence, status, expected_calls, action) in [
        ("true", "skip", 0.99, 200, 0, "skip"),
        ("false", "skip", 0.99, 200, 1, "disabled"),
        ("1", "extract", 0.99, 200, 1, "extract"),
        ("true", "skip", 0.5, 200, 1, "agent"),
        ("true", "uncertain", 0.99, 200, 1, "agent"),
        ("true", "extract", 0.5, 200, 1, "agent"),
        ("true", "invalid", 0.99, 200, 1, "agent"),
        ("true", "skip", 0.99, 500, 1, "agent"),
    ] {
        let cli = Cli::new();
        cli.ok(&[
            "project",
            "register",
            "--root",
            cli.dir.path().to_str().unwrap(),
        ]);
        recovery_capture(&cli, "first");
        let tasks = agentix_task::Store::open(&cli.dir.path().join("state.sqlite3"))
            .await
            .unwrap();
        let source = tasks.memory_sources(0, 1).await.unwrap().remove(0);
        let mut probabilities = json!({"skip":0.005,"extract":0.005,"uncertain":0.005});
        probabilities[choice] = json!(0.99);
        let jev = provider_http::MockHttp::start(vec![(status, json!({"answers":{"memory_triage":{"type":"choice","choice":choice,"confidence":confidence,"probabilities":probabilities}}}))]).await;
        let model = provider_http::MockHttp::start(vec![(200, json!({"status":"completed","output":[{"type":"function_call","call_id":"finish","name":"submit_candidates","arguments":"{\"candidates\":[]}"}]}))]).await;
        let path = cli.dir.path().join("config.toml");
        let config = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("{config}\n[memory]\nenabled=true\n[memory.providers.mock]\nbase_url='{}'\n[memory.agent]\nprovider='mock'\n[memory.service]\npoll_interval_ms=20\n",model.url)).unwrap();
        let metrics_path = cli.dir.path().join("jev-metrics.db");
        let _daemon = Daemon(
            cli.command(&["memory", "serve"])
                .env("TASKIX_JEV_ENABLED", enabled)
                .env("TASKIX_JEV_URL", format!("{}/evaluate", jev.url))
                .env("TASKIX_JEV_API_KEY", "mock-key")
                .env("TASKIX_JEV_MODEL", "jev-fixture")
                .env("TASKIX_JEV_MIN_CONFIDENCE", "NaN")
                .env("TASKIX_MEMORY_JEV_MIN_CONFIDENCE", "0.8")
                .env("TASKIX_JEV_METRICS_ENABLED", "true")
                .env("TASKIX_JEV_METRICS_DB", &metrics_path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = cli.ok(&["memory", "status"]);
            if status["online"] == true && status["sources"] == 1 {
                break;
            }
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let receipt = cli.ok(&[
            "memory",
            "receipt",
            &source.receipt_id,
            "--wait-seconds",
            "10",
        ]);
        assert_eq!(receipt["complete"], true, "{action}: {receipt}");
        assert_eq!(
            model.requests.lock().unwrap().len(),
            expected_calls,
            "{action}"
        );
        if enabled == "false" {
            assert!(jev.requests.lock().unwrap().is_empty());
            assert!(!metrics_path.exists());
        } else {
            let requests = jev.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].1["model"], "jev-fixture");
            assert_eq!(
                requests[0].1["state"]["messages"][0]["text"],
                "Offline recovery is required"
            );
            drop(requests);
            let output = cli
                .command(&["routing", "metrics", "report"])
                .env("TASKIX_JEV_METRICS_DB", &metrics_path)
                .output()
                .unwrap();
            let report: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(report["result"]["by_kind"][0]["kind"], "memory_triage");
            assert_eq!(report["result"]["memory_triage"]["requests"], 1);
            let field = match action {
                "skip" => "skipped",
                "extract" => "extract",
                _ => "fallback",
            };
            assert_eq!(report["result"]["memory_triage"][field], 1);
        }
    }
}
