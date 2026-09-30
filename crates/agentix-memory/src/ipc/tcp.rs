use anyhow::{Context, Result, ensure};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read},
    net::SocketAddr,
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context as TaskContext, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
};

pub(super) struct Listener {
    inner: TcpListener,
    path: PathBuf,
    // Keep the stable lock file locked until the endpoint has been removed.
    _lock: File,
    identity: [u8; 16],
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
        let identity = uuid::Uuid::now_v7();
        std::fs::write(&path, format!("tcp://{address}\n{identity}"))?;
        Ok(Self {
            inner,
            path,
            _lock: lock,
            identity: *identity.as_bytes(),
        })
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(super) async fn accept(&self) -> Result<Stream> {
        Ok(Stream {
            inner: self.inner.accept().await?.0,
            identity: self.identity,
            sent: 0,
        })
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
    let mut lines = endpoint.lines();
    let address: SocketAddr = lines
        .next()
        .context("missing TCP address")?
        .strip_prefix("tcp://")
        .context("memory endpoint must use tcp://")?
        .parse()?;
    ensure!(
        address.ip().is_loopback() && address.port() != 0,
        "memory TCP endpoint must use a loopback address and nonzero port"
    );
    let expected = uuid::Uuid::parse_str(lines.next().context("missing service identity")?)?;
    ensure!(lines.next().is_none(), "invalid memory TCP endpoint");
    let mut stream = TcpStream::connect(address)
        .await
        .context("memory service unavailable")?;
    let mut identity = [0; 16];
    stream
        .read_exact(&mut identity)
        .await
        .context("memory service identity unavailable")?;
    ensure!(
        &identity == expected.as_bytes(),
        "memory service instance changed; retry with the current endpoint"
    );
    Ok(stream)
}

// Send the instance greeting inside each connection's existing read deadline,
// never in the accept loop. No client request bytes are sent before it matches.
pub(super) struct Stream {
    inner: TcpStream,
    identity: [u8; 16],
    sent: usize,
}
impl AsyncRead for Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        while this.sent < this.identity.len() {
            match Pin::new(&mut this.inner).poll_write(cx, &this.identity[this.sent..]) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(0)) => return Poll::Ready(Err(io::ErrorKind::WriteZero.into())),
                Poll::Ready(Ok(n)) => this.sent += n,
            }
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}
impl AsyncWrite for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
