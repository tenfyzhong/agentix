// Unix-only memory service integration tests.
#![cfg(unix)]
use agentix_memory::{IpcClient, IpcServer, RequestHandler, ServiceConfig};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::watch;

struct Echo;

#[tokio::test]
async fn timed_out_context_does_not_suppress_undelivered_memory_on_next_turn() {
    stalled_embedding_context(std::time::Duration::from_millis(20), false).await;
}

#[tokio::test]
async fn slow_embedding_context_falls_back_within_the_host_deadline() {
    stalled_embedding_context(std::time::Duration::from_millis(1500), true).await;
}

async fn stalled_embedding_context(deadline: std::time::Duration, expect_delivery: bool) {
    use agentix_memory::{
        Actor, EmbeddingConfig, EmbeddingIndex, HttpEmbedding, HttpProvider, MemoryApi,
        MemoryStore, ProviderConfig, ProviderProtocol, RetrievalConfig,
    };
    use std::time::Duration;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    let memory = store.create("project", serde_json::from_value(json!({"title":"Offline policy","conclusion":"Offline recovery is required","rationale":"Customer requirement","scope":"project","kind":"user_decision","tags":[],"evidence":[]})).unwrap(), Actor::Human).await.unwrap();
    // Accept TCP connections but never reply: exercise the real embedding deadline.
    let stalled = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = EmbeddingConfig {
        enabled: true,
        dimensions: Some(2),
        ..EmbeddingConfig::default()
    };
    let provider = Arc::new(
        HttpProvider::new(ProviderConfig {
            base_url: format!("http://{}", stalled.local_addr().unwrap()),
            api_key_env: None,
            protocol: ProviderProtocol::Openai,
            max_in_flight: 4,
        })
        .unwrap(),
    );
    let embedding = Arc::new(HttpEmbedding::new(provider, config.clone()).unwrap());
    EmbeddingIndex::new(store.clone(), embedding.clone(), config)
        .activate("project")
        .await
        .unwrap();
    let service = ServiceConfig::default();
    let retrieval = RetrievalConfig::default();
    let handler =
        MemoryApi::new(store.clone(), retrieval.clone(), service).with_embedding(embedding);
    let server = IpcServer::bind(&path, service).unwrap();
    let (stop, rx) = watch::channel(false);
    let serving = tokio::spawn(server.serve(Arc::new(handler), rx));
    let client = IpcClient::new(&path, service).unwrap();
    let first = client
        .call_with_timeout(
            json!({"op":"context","project":"project","session":"s","turn":"t1","query":"offline"}),
            deadline,
        )
        .await;
    assert_eq!(first.is_ok(), expect_delivery, "{first:?}");
    if let Ok(delivered) = first {
        stop.send(true).unwrap();
        serving.await.unwrap().unwrap();
        assert_eq!(delivered["items"][0]["id"], memory.id);
        return;
    }
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let count: i64 =
                sqlx::query_scalar("SELECT count(*) FROM context_receipts WHERE turn_id='t1'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            if count == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let second = client
        .call(
            json!({"op":"context","project":"project","session":"s","turn":"t2","query":"offline"}),
        )
        .await
        .unwrap();
    stop.send(true).unwrap();
    serving.await.unwrap().unwrap();
    assert_eq!(
        second["items"][0]["id"], memory.id,
        "the first packet was never delivered, so a later turn must still receive it"
    );
}
#[async_trait]
impl RequestHandler for Echo {
    async fn handle(&self, request: Value) -> Result<Value> {
        Ok(request)
    }
}
#[allow(clippy::items_after_statements)]
#[tokio::test]
async fn local_ipc_is_single_instance_bounded_and_recovers_after_shutdown() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("memory.db");
    let config = ServiceConfig {
        max_request_bytes: 1024,
        ..ServiceConfig::default()
    };
    let server = IpcServer::bind(&database, config).unwrap();
    assert!(IpcServer::bind(&database, config).is_err());
    let path = server.path().to_owned();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let (stop, rx) = watch::channel(false);
    let serving = tokio::spawn(server.serve(Arc::new(Echo), rx));
    let client = IpcClient::new(&database, config).unwrap();
    let (a, b) = tokio::join!(
        client.call(json!({"query":"a"})),
        client.call(json!({"query":"b"}))
    );
    assert_eq!(a.unwrap()["query"], "a");
    assert_eq!(b.unwrap()["query"], "b");
    assert!(
        client
            .call(json!({"large":"x".repeat(2048)}))
            .await
            .is_err()
    );
    stop.send(true).unwrap();
    serving.await.unwrap().unwrap();
    assert!(!path.exists());
    assert!(IpcServer::bind(&database, config).is_ok());
}

#[tokio::test]
async fn ipc_accepts_the_maximum_configured_request_budget() {
    let dir = tempfile::tempdir().unwrap();
    let config = ServiceConfig {
        max_request_bytes: 4 * 1024 * 1024,
        ..ServiceConfig::default()
    };
    assert!(IpcServer::bind(&dir.path().join("memory.db"), config).is_ok());
}
