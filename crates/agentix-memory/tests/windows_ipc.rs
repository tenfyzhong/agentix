// Windows TCP memory service integration tests.
#![cfg(windows)]
use agentix_memory::{IpcClient, IpcServer, RequestHandler, ServiceConfig};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

struct Echo;
#[async_trait]
impl RequestHandler for Echo {
    async fn handle(&self, value: Value) -> Result<Value> {
        Ok(value)
    }
}

#[tokio::test]
async fn windows_tcp_is_exclusive_and_handles_concurrent_clients() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.sqlite3");
    let config = ServiceConfig::default();
    let server = IpcServer::bind(&path, config).unwrap();
    assert!(
        std::fs::read_to_string(server.path())
            .unwrap()
            .starts_with("tcp://127.0.0.1:")
    );
    assert!(IpcServer::bind(&path, config).is_err());
    let (stop, rx) = watch::channel(false);
    let serving = tokio::spawn(server.serve(Arc::new(Echo), rx));
    let mut clients = tokio::task::JoinSet::new();
    for i in 0..8 {
        let client = IpcClient::new(&path, config).unwrap();
        clients.spawn(async move {
            assert_eq!(client.call(json!({"i":i})).await.unwrap()["i"], i);
        });
    }
    while let Some(result) = clients.join_next().await {
        result.unwrap();
    }
    stop.send(true).unwrap();
    serving.await.unwrap().unwrap();
    assert!(IpcServer::bind(&path, config).is_ok());
}

#[tokio::test]
async fn windows_missing_service_returns_within_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let client = IpcClient::new(
        &dir.path().join("missing.sqlite3"),
        ServiceConfig::default(),
    )
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        client.call_with_timeout(json!({}), Duration::from_millis(100)),
    )
    .await
    .unwrap();
    assert!(result.is_err());
}

#[tokio::test]
async fn windows_stalled_reply_reader_does_not_block_shutdown() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
    };
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("memory.sqlite3");
    let config = ServiceConfig::default();
    let server = IpcServer::bind(&database, config).unwrap();
    let endpoint = std::fs::read_to_string(server.path()).unwrap();
    let mut client = TcpStream::connect(
        endpoint
            .lines()
            .next()
            .unwrap()
            .strip_prefix("tcp://")
            .unwrap(),
    )
    .await
    .unwrap();
    let (stop, rx) = watch::channel(false);
    let serving = tokio::spawn(server.serve(Arc::new(Echo), rx));
    let mut identity = [0; 16];
    client.read_exact(&mut identity).await.unwrap();
    assert_eq!(
        &identity,
        uuid::Uuid::parse_str(endpoint.lines().nth(1).unwrap())
            .unwrap()
            .as_bytes()
    );
    let request =
        serde_json::to_vec(&json!({"version":1,"request":{"body":"x".repeat(64*1024)}})).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        client
            .write_u32(u32::try_from(request.len()).unwrap())
            .await
            .unwrap();
        client.write_all(&request).await.unwrap();
        assert!(client.read_u32().await.unwrap() > 64 * 1024);
    })
    .await
    .unwrap();
    // Never consume the reply body.
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(2), serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(client);
    assert!(IpcServer::bind(&database, config).is_ok());
}
