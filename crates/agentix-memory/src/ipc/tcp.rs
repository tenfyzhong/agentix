use anyhow::{Context, Result, ensure};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    net::SocketAddr,
    path::{Path, PathBuf},
};
use tokio::net::{TcpListener, TcpStream};

pub(super) struct Listener {
    inner: TcpListener,
    path: PathBuf,
    // Keep the stable lock file locked until the endpoint has been removed.
    _lock: File,
}
impl Listener {
    pub(super) fn bind(database: &Path) -> Result<Self> {
        let path = socket_path(database)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))?;
        lock.try_lock()
            .context("another memory service may already be running")?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let inner = TcpListener::from_std(listener)?;
        std::fs::write(&path, format!("tcp://{address}"))?;
        Ok(Self {
            inner,
            path,
            _lock: lock,
        })
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(super) async fn accept(&self) -> Result<TcpStream> {
        Ok(self.inner.accept().await?.0)
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
pub(super) fn socket_path(database: &Path) -> Result<PathBuf> {
    let parent = database
        .parent()
        .context("missing database parent")?
        .canonicalize()?;
    let mut name = database
        .file_name()
        .context("missing database filename")?
        .to_os_string();
    name.push(".tcp");
    Ok(parent.join(name))
}
pub(super) async fn connect(path: &Path) -> Result<TcpStream> {
    let mut endpoint = String::new();
    File::open(path)
        .context("memory service unavailable")?
        .take(129)
        .read_to_string(&mut endpoint)?;
    ensure!(endpoint.len() <= 128, "invalid memory TCP endpoint");
    let address: SocketAddr = endpoint
        .strip_prefix("tcp://")
        .context("memory endpoint must use tcp://")?
        .parse()?;
    ensure!(
        address.ip().is_loopback() && address.port() != 0,
        "memory TCP endpoint must use a loopback address and nonzero port"
    );
    TcpStream::connect(address)
        .await
        .context("memory service unavailable")
}
