use std::time::Duration;
#[tokio::test]
async fn metrics_only_service_does_not_initialize_memory_or_need_providers() {
    let dir = tempfile::tempdir().unwrap();
    let task_path = dir.path().join("tasks.db");
    agentix_task::Store::open(&task_path).await.unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let config = dir.path().join("config.toml");
    std::fs::write(&config,format!("schema_version=1\n[storage]\npath={task_path:?}\n[metrics]\nenabled=true\nlisten='{address}'\n")).unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_taskix"))
        .args(["--config", config.to_str().unwrap(), "serve"])
        .env("TASKIX_MEMORY_ENABLED", "false")
        .env("TASKIX_LOGIN_SHELL", "/nonexistent")
        .env("TASKIX_JEV_METRICS_DB", dir.path().join("absent-jev.db"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let client = reqwest::Client::new();
    let text = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "metrics-only service exited before exposing metrics"
            );
            if let Ok(response) = client.get(format!("http://{address}/metrics")).send().await {
                let text = response.text().await.unwrap();
                if text.contains("taskix_entities")
                    && text.contains("taskix_process_resident_memory_bytes ")
                {
                    break text;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    if let Ok(path) = std::env::var("METRICS_FIXTURE_FILE") {
        std::fs::write(path, &text).unwrap();
    }
    assert!(text.contains("taskix_build_info"));
    assert!(!text.contains("taskix_memory_work_items"));
    assert!(!dir.path().join("memory.sqlite3").exists());
    assert!(!dir.path().join("absent-jev.db").exists());
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}
