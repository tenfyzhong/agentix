use crate::CodexEndpoint;
use anyhow::{Context, Result, bail};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Owns launched children until shutdown; existing upstreams are never terminated.
pub struct UpstreamServer {
    stop: CancellationToken,
    reaper: Mutex<Option<tokio::task::JoinHandle<Result<()>>>>,
}

impl Drop for UpstreamServer {
    fn drop(&mut self) {
        self.stop.cancel();
    }
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
    /// Terminate and reap our child, leaving adopted upstreams untouched.
    /// Concurrent callers share one shutdown; cancellation does not lose the reaper.
    pub async fn shutdown(&self) -> Result<()> {
        self.stop.cancel();
        let mut reaper = self.reaper.lock().await;
        if let Some(task) = reaper.as_mut() {
            let result = task.await;
            reaper.take();
            result.context("join Codex upstream shutdown")??;
        }
        Ok(())
    }

    pub async fn ensure(endpoint: &CodexEndpoint, command: &Path) -> Result<Self> {
        anyhow::ensure!(
            !endpoint.is_stdio(),
            "stdio:// is a client-facing proxy transport; shared upstream must use unix:// or ws://"
        );
        let address = endpoint.address();
        match tokio::time::timeout(Duration::from_secs(3), crate::proxy::open_socket(&address))
            .await
        {
            Ok(Ok(_)) => {
                return Ok(Self {
                    stop: CancellationToken::new(),
                    reaper: Mutex::new(None),
                });
            }
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
            .kill_on_drop(true)
            .spawn()
            .context("start Codex upstream app-server")?;
        let mut startup = StartingChild(Some(child));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = startup.0.as_mut().unwrap().try_wait()? {
                bail!("Codex upstream exited during startup: {status}");
            }
            if let Ok(Ok(socket)) = tokio::time::timeout(
                Duration::from_secs(1),
                ready_socket(endpoint, startup.0.as_ref().unwrap().id()),
            )
            .await
            {
                let mut child = startup.0.take().unwrap();
                let stop = CancellationToken::new();
                let token = stop.clone();
                let reaper = tokio::spawn(async move {
                    // Keep the guard in the future even if runtime teardown cancels it.
                    // kill_on_drop handles the child when this future is cancelled.
                    let socket = socket;
                    let result = tokio::select! {
                        status = child.wait() => status.map(|_| ()).context("reap Codex upstream"),
                        () = token.cancelled() => terminate_child(&mut child).await,
                    };
                    drop(socket);
                    if let Err(error) = &result {
                        tracing::warn!(%error, "failed to stop owned Codex upstream");
                    }
                    result
                });
                return Ok(Self {
                    stop,
                    reaper: Mutex::new(Some(reaper)),
                });
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("Codex upstream did not become ready: {address}");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

async fn ready_socket(
    endpoint: &CodexEndpoint,
    child_pid: Option<u32>,
) -> Result<Option<OwnedSocket>> {
    if endpoint.is_websocket() {
        crate::proxy::open_socket(&endpoint.address()).await?;
        return Ok(None);
    }
    let path = endpoint.socket_path();
    let before = std::fs::symlink_metadata(path)?;
    let stream = tokio::net::UnixStream::connect(path).await?;
    let peer_pid = crate::proxy::unix_pid(&stream);
    tokio_tungstenite::client_async("ws://localhost/", stream).await?;
    let after = std::fs::symlink_metadata(path)?;
    // Readiness can come from a competing server. Only claim the child's own inode.
    Ok((peer_pid.is_some()
        && peer_pid == child_pid
        && (before.dev(), before.ino()) == (after.dev(), after.ino()))
        .then(|| OwnedSocket {
            path: path.to_owned(),
            identity: (after.dev(), after.ino()),
        }))
}

struct OwnedSocket {
    path: std::path::PathBuf,
    identity: (u64, u64),
}

impl Drop for OwnedSocket {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path)
            .is_ok_and(|meta| (meta.dev(), meta.ino()) == self.identity)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

async fn terminate_child(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    let pid = child.id().context("missing owned Codex PID")?;
    match nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(pid)?),
        nix::sys::signal::Signal::SIGTERM,
    ) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
        Err(error) => return Err(error.into()),
    }
    if let Ok(status) = tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
        status?;
    } else {
        tracing::warn!(
            pid,
            "Codex upstream did not stop within five seconds; killing owned child"
        );
        child.kill().await?;
    }
    Ok(())
}
