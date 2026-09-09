//! Minimal reusable control-listener fixture; production multiplexing is tested in agentix.
use agentix_bridge::BridgeHub;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::{AsyncBufReadExt, BufReader};
pub struct TestControl {
    pub hub: Arc<BridgeHub>,
    pub endpoint: String,
    path: PathBuf,
    task: tokio::task::JoinHandle<()>,
}
impl TestControl {
    pub fn bind(directory: &Path) -> std::io::Result<Self> {
        let path = directory.join("control.sock");
        let listener = tokio::net::UnixListener::bind(&path)?;
        let hub = Arc::new(BridgeHub::new());
        let owned = hub.clone();
        let task = tokio::spawn(async move {
            let mut peers = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        let hub = owned.clone();
                        peers.spawn(async move {
                            let mut stream = BufReader::new(stream);
                            let mut line = String::new();
                            if stream.read_line(&mut line).await.is_ok()
                                && let Ok(frame) = serde_json::from_str(&line) {
                                let _ = hub.accept(frame, stream).await;
                            }
                        });
                    },
                    _ = peers.join_next(), if !peers.is_empty() => {},
                }
            }
        });
        Ok(Self {
            hub,
            endpoint: format!("unix://{}", path.display()),
            path,
            task,
        })
    }
}
impl Drop for TestControl {
    fn drop(&mut self) {
        self.task.abort();
        let hub = self.hub.clone();
        tokio::spawn(async move {
            hub.shutdown().await;
        });
        let _ = std::fs::remove_file(&self.path);
    }
}
