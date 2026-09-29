// Unix-only memory service integration tests.
#![cfg(unix)]
use agentix_memory::{IpcClient, IpcServer, RequestHandler, ServiceConfig};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::watch;

struct Echo;
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
