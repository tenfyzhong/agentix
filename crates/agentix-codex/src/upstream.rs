use crate::CodexEndpoint;
use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};

/// A shared server that survives Agentix shutdown. No socket ownership is taken.
// Dropping the reaper handle detaches it; it never terminates a ready server.
pub struct UpstreamServer {
    _reaper: Option<tokio::task::JoinHandle<()>>,
}
struct StartingChild(Option<Child>);
impl Drop for StartingChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.start_kill();
        }
    }
}

impl UpstreamServer {
    pub async fn ensure(endpoint: &CodexEndpoint, command: &Path) -> Result<Self> {
        anyhow::ensure!(
            !endpoint.is_stdio(),
            "stdio:// is a client-facing proxy transport; shared upstream must use unix:// or ws://"
        );
        let address = endpoint.address();
        match tokio::time::timeout(Duration::from_secs(3), crate::proxy::open_socket(&address))
            .await
        {
            Ok(Ok(_)) => return Ok(Self { _reaper: None }),
            Ok(Err(error)) => {
                let unavailable = error
                    .chain()
                    .filter_map(|e| e.downcast_ref::<std::io::Error>())
                    .any(|e| {
                        matches!(
                            e.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                        )
                    });
                if !unavailable {
                    return Err(error
                        .context("upstream endpoint is occupied or not a Codex WebSocket server"));
                }
            }
            Err(_) => bail!("upstream handshake timed out: {address}"),
        }
        if endpoint.is_websocket() {
            let url = url::Url::parse(&address)?;
            let local = match url.host() {
                Some(url::Host::Domain("localhost")) => true,
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                _ => false,
            };
            if !local {
                bail!(
                    "remote upstream is unavailable; refusing to start a local server for {address}"
                );
            }
        } else if let Some(parent) = endpoint.socket_path().parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut process = Command::new(command);
        crate::login_environment::apply(&mut process).await;
        let child = process
            .args(["app-server", "--listen", &address])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(false)
            .spawn()
            .context("start Codex upstream app-server")?;
        let mut startup = StartingChild(Some(child));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = startup.0.as_mut().unwrap().try_wait()? {
                bail!("Codex upstream exited during startup: {status}");
            }
            if tokio::time::timeout(Duration::from_secs(1), crate::proxy::open_socket(&address))
                .await
                .is_ok_and(|r| r.is_ok())
            {
                let mut child = startup.0.take().unwrap();
                let reaper = tokio::spawn(async move {
                    let status = child.wait().await;
                    tracing::debug!(?status, "Codex upstream exited");
                });
                return Ok(Self {
                    _reaper: Some(reaper),
                });
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("Codex upstream did not become ready: {address}");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
