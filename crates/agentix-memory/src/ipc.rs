use crate::ServiceConfig;
use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Semaphore, watch},
    task::JoinSet,
};

#[cfg(unix)]
#[path = "ipc/unix.rs"]
mod transport;
#[cfg(windows)]
#[path = "ipc/tcp.rs"]
mod transport;

#[async_trait]
pub trait RequestHandler: Send + Sync {
    async fn handle(&self, request: Value) -> Result<Value>;
}

pub struct IpcServer {
    listener: transport::Listener,
    config: ServiceConfig,
}
impl IpcServer {
    pub fn bind(database: &Path, config: ServiceConfig) -> Result<Self> {
        ensure!(
            (1..=256).contains(&config.max_query_concurrency)
                && (128..=4 * 1024 * 1024).contains(&config.max_request_bytes)
                && (128..=8 * 1024 * 1024).contains(&config.max_response_bytes),
            "invalid IPC budgets"
        );
        Ok(Self {
            listener: transport::Listener::bind(database)?,
            config,
        })
    }
    pub fn path(&self) -> &Path {
        self.listener.path()
    }
    pub async fn serve(
        self,
        handler: Arc<dyn RequestHandler>,
        mut stop: watch::Receiver<bool>,
    ) -> Result<()> {
        let permits = Arc::new(Semaphore::new(
            self.config.max_query_concurrency + self.config.max_deep_queries + 4,
        ));
        let mut tasks = JoinSet::new();
        loop {
            if *stop.borrow() {
                break;
            }
            tokio::select! {
                changed=stop.changed()=>{if changed.is_err() || *stop.borrow(){break;}},
                Some(_)=tasks.join_next(),if !tasks.is_empty()=>{},
                accepted=self.listener.accept()=>{
                    let stream=accepted?;
                    let Ok(permit)=permits.clone().try_acquire_owned() else {continue;};
                    let handler=handler.clone();let config=self.config;
                    tasks.spawn(async move {let _permit=permit;let _=handle_connection(stream,handler,config).await;});
                }
            }
        }
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        Ok(())
    }
}
pub struct IpcClient {
    path: PathBuf,
    config: ServiceConfig,
}
impl IpcClient {
    pub fn new(database: &Path, config: ServiceConfig) -> Result<Self> {
        Ok(Self {
            path: transport::socket_path(database)?,
            config,
        })
    }
    pub async fn call(&self, request: Value) -> Result<Value> {
        self.call_with_timeout(request, Duration::from_secs(10))
            .await
    }
    pub async fn call_with_timeout(&self, request: Value, timeout: Duration) -> Result<Value> {
        tokio::time::timeout(timeout, self.call_inner(request))
            .await
            .context("memory service request timeout")?
    }
    async fn call_inner(&self, request: Value) -> Result<Value> {
        let mut stream = transport::connect(&self.path).await?;
        write_frame(
            &mut stream,
            &json!({"version":1,"request":request}),
            self.config.max_request_bytes,
        )
        .await?;
        let reply = read_frame(&mut stream, self.config.max_response_bytes).await?;
        ensure!(reply["version"] == 1, "unsupported memory service protocol");
        ensure!(
            reply["ok"] == true,
            "{}",
            reply["error"]
                .as_str()
                .unwrap_or("memory service request failed")
        );
        Ok(reply["result"].clone())
    }
}
async fn handle_connection(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    handler: Arc<dyn RequestHandler>,
    config: ServiceConfig,
) -> Result<()> {
    let request = tokio::time::timeout(
        Duration::from_secs(5),
        read_frame(&mut stream, config.max_request_bytes),
    )
    .await??;
    let result = if request["version"] == 1 {
        handler.handle(request["request"].clone()).await
    } else {
        Err(anyhow::anyhow!("unsupported memory service protocol"))
    };
    let mut reply = match result {
        Ok(result) => json!({"version":1,"ok":true,"result":result}),
        Err(error) => json!({"version":1,"ok":false,"error":error.to_string()}),
    };
    if serde_json::to_vec(&reply)?.len() > config.max_response_bytes {
        reply = json!({"version":1,"ok":false,"error":"memory response exceeds budget; use a smaller page"});
    }
    tokio::time::timeout(
        Duration::from_secs(5),
        write_frame(&mut stream, &reply, config.max_response_bytes),
    )
    .await??;
    Ok(())
}
async fn read_frame(stream: &mut (impl AsyncRead + Unpin), limit: usize) -> Result<Value> {
    let length = usize::try_from(stream.read_u32().await?)?;
    ensure!(
        length > 0 && length <= limit,
        "memory request or response exceeds budget"
    );
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}
async fn write_frame(
    stream: &mut (impl AsyncWrite + Unpin),
    value: &Value,
    limit: usize,
) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(
        bytes.len() <= limit,
        "memory request or response exceeds budget"
    );
    stream.write_u32(u32::try_from(bytes.len())?).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Echo;
    #[async_trait]
    impl RequestHandler for Echo {
        async fn handle(&self, request: Value) -> Result<Value> {
            Ok(request)
        }
    }
    #[tokio::test]
    async fn framing_is_transport_independent_and_rejects_unknown_versions() {
        let (mut client, server) = tokio::io::duplex(1024);
        let serving = tokio::spawn(handle_connection(
            server,
            Arc::new(Echo),
            ServiceConfig::default(),
        ));
        write_frame(&mut client, &json!({"version":99,"request":{}}), 1024)
            .await
            .unwrap();
        let reply = read_frame(&mut client, 1024).await.unwrap();
        assert_eq!(reply["ok"], false);
        assert!(reply["error"].as_str().unwrap().contains("unsupported"));
        serving.await.unwrap().unwrap();
    }
}

#[cfg(all(test, not(windows)))]
#[path = "ipc/tcp.rs"]
mod tcp_test_transport;

#[cfg(test)]
mod tcp_tests {
    #[cfg(not(windows))]
    use super::tcp_test_transport as tcp;
    #[cfg(windows)]
    use super::transport as tcp;
    use super::*;
    #[tokio::test]
    async fn tcp_transport_is_exclusive_concurrent_and_recovers_stale_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("memory.sqlite3");
        let server = tcp::Listener::bind(&database).unwrap();
        let endpoint = server.path().to_path_buf();
        assert!(
            std::fs::read_to_string(&endpoint)
                .unwrap()
                .starts_with("tcp://127.0.0.1:")
        );
        assert!(tcp::Listener::bind(&database).is_err());
        let mut clients = JoinSet::new();
        for i in 0..8 {
            let endpoint = endpoint.clone();
            clients.spawn(async move {
                let mut client = tcp::connect(&endpoint).await.unwrap();
                write_frame(&mut client, &json!({"i":i}), 1024)
                    .await
                    .unwrap();
                assert_eq!(read_frame(&mut client, 1024).await.unwrap()["i"], i);
            });
        }
        for _ in 0..8 {
            let mut stream = server.accept().await.unwrap();
            let value = read_frame(&mut stream, 1024).await.unwrap();
            write_frame(&mut stream, &value, 1024).await.unwrap();
        }
        while let Some(result) = clients.join_next().await {
            result.unwrap();
        }
        drop(server);
        assert!(!endpoint.exists());
        std::fs::write(&endpoint, "tcp://127.0.0.1:1").unwrap();
        assert!(tcp::Listener::bind(&database).is_ok());
    }
    #[tokio::test]
    async fn tcp_transport_rejects_remote_invalid_and_missing_endpoints() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("endpoint");
        assert!(tcp::connect(&path).await.is_err());
        for value in [
            "tcp://192.0.2.1:1",
            "tcp://0.0.0.0:1",
            "tcp://127.0.0.1:0",
            "unix://x",
            "invalid",
        ] {
            std::fs::write(&path, value).unwrap();
            assert!(tcp::connect(&path).await.is_err());
        }
    }
}
