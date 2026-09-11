use crate::proxy_auth::AuthPolicy;
use crate::{ClientRegistry, CodexEndpoint, ProxyOptions};
use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::task::{JoinHandle, JoinSet};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};
use tokio_util::sync::CancellationToken;

pub(crate) trait IoStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> IoStream for T {}
pub(crate) type Socket = WebSocketStream<Box<dyn IoStream>>;

pub(crate) async fn open_socket(endpoint: &str) -> Result<Socket> {
    let (stream, url) = open_stream(endpoint).await?;
    Ok(tokio_tungstenite::client_async(url, stream).await?.0)
}

pub(super) async fn open_stream(endpoint: &str) -> Result<(Box<dyn IoStream>, String)> {
    open_stream_checked(endpoint, None).await
}

pub(super) async fn open_stream_checked(
    endpoint: &str,
    listener: Option<std::net::SocketAddr>,
) -> Result<(Box<dyn IoStream>, String)> {
    let result: (Box<dyn IoStream>, String) = if endpoint.starts_with("unix://") {
        let e = CodexEndpoint::parse(endpoint)?;
        (
            Box::new(UnixStream::connect(e.socket_path()).await?),
            "ws://localhost/".into(),
        )
    } else {
        let url = url::Url::parse(endpoint)?;
        if url.scheme() != "ws" {
            bail!("expected unix:// or ws:// endpoint");
        }
        let host = socket_host(&url)?;
        let port = url.port_or_known_default().context("missing port")?;
        let stream = TcpStream::connect((host.as_str(), port)).await?;
        if let Some(listener) = listener {
            crate::proxy_address::reject_self(listener, stream.peer_addr()?)?;
        }
        (Box::new(stream), endpoint.to_owned())
    };
    Ok(result)
}

pub(crate) fn socket_host(url: &url::Url) -> Result<String> {
    Ok(match url.host().context("missing WebSocket host")? {
        url::Host::Domain(name) => name.to_owned(),
        url::Host::Ipv4(ip) => ip.to_string(),
        url::Host::Ipv6(ip) => ip.to_string(),
    })
}

/// Resolve existing prefixes while allowing a socket or parent directories to
/// be created later. Resolve symlinks before processing subsequent "..".
fn socket_identity(path: &std::path::Path) -> std::io::Result<PathBuf> {
    use std::path::Component;
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => continue,
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other.as_os_str()),
        }
        match std::fs::canonicalize(&resolved) {
            Ok(path) => resolved = path,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(resolved)
}

fn ensure_distinct_endpoints(listen: &str, upstream: &str) -> Result<()> {
    let listen = CodexEndpoint::parse(listen)?;
    let upstream = CodexEndpoint::parse(upstream)?;
    let same = listen.address() == upstream.address()
        || (!listen.is_websocket()
            && !listen.is_stdio()
            && !upstream.is_websocket()
            && !upstream.is_stdio()
            && socket_identity(listen.socket_path())? == socket_identity(upstream.socket_path())?);
    if same {
        return Err(
            crate::ProxyConfigError("proxy and upstream endpoints must differ".into()).into(),
        );
    }
    Ok(())
}

enum Listener {
    Unix(UnixListener),
    Tcp(TcpListener),
}
enum Accepted {
    Unix(UnixStream),
    Tcp(TcpStream),
}
impl Accepted {
    fn identify(self) -> (Box<dyn IoStream>, Option<u32>) {
        match self {
            Self::Unix(stream) => {
                let pid = unix_pid(&stream);
                (Box::new(stream), pid)
            }
            Self::Tcp(stream) => (Box::new(stream), None),
        }
    }
}
impl Listener {
    async fn accept(&self) -> Result<Accepted> {
        #[cfg(test)]
        if let Self::Tcp(listener) = self
            && let Some(error) = tests::ACCEPT_ERROR
                .lock()
                .unwrap()
                .remove(&listener.local_addr()?)
        {
            return Err(error.into());
        }
        match self {
            Self::Unix(listener) => Ok(Accepted::Unix(listener.accept().await?.0)),
            Self::Tcp(listener) => Ok(Accepted::Tcp(listener.accept().await?.0)),
        }
    }
}

fn unix_pid(stream: &UnixStream) -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::LocalPeerPid)
            .ok()
            .and_then(|p| u32::try_from(p).ok())
    }
    #[cfg(not(target_os = "macos"))]
    {
        stream
            .peer_cred()
            .ok()?
            .pid()
            .and_then(|p| u32::try_from(p).ok())
    }
}

struct Registration {
    registry: ClientRegistry,
    id: u64,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.registry.disconnect(self.id);
    }
}

struct SocketFile {
    path: PathBuf,
    dev: u64,
    ino: u64,
}
impl Drop for SocketFile {
    fn drop(&mut self) {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = std::fs::symlink_metadata(&self.path)
            && meta.dev() == self.dev
            && meta.ino() == self.ino
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Transparent per-client proxy. Dropping the owner closes clients and registrations.
pub struct CodexProxy {
    endpoint: String,
    registry: ClientRegistry,
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}
impl CodexProxy {
    pub async fn bind(listen: &str, upstream: &str) -> Result<Self> {
        Self::bind_with_options(listen, upstream, &ProxyOptions::default()).await
    }
    pub async fn bind_with_options(
        listen: &str,
        upstream: &str,
        options: &ProxyOptions,
    ) -> Result<Self> {
        let invalid = |error: anyhow::Error| crate::ProxyConfigError(error.to_string());
        let auth = Arc::new(AuthPolicy::load(options).map_err(invalid)?);
        if !listen.starts_with("ws://") && auth.enabled() {
            return Err(crate::ProxyConfigError(
                "WebSocket authentication requires a ws:// proxy listener".into(),
            )
            .into());
        }
        ensure_distinct_endpoints(listen, upstream)?;
        if listen == "stdio://" {
            return Self::bind_stdio(listen, upstream);
        }
        let (listener, endpoint, file) = if listen.starts_with("unix://") {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let e = CodexEndpoint::parse(listen)?;
            let path = e.socket_path();
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            // Never unlink an occupied path: it may belong to a live daemon.
            let listener = UnixListener::bind(path).map_err(|source| crate::ProxyBindError {
                endpoint: listen.to_owned(),
                source,
            })?;
            let meta = std::fs::symlink_metadata(path)?;
            let file = SocketFile {
                path: path.to_owned(),
                dev: meta.dev(),
                ino: meta.ino(),
            };
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            // Resolve again after bind: a previously dangling upstream symlink
            // can now refer to our socket. The guard removes only our own inode.
            ensure_distinct_endpoints(listen, upstream)?;
            (
                Listener::Unix(listener),
                format!("unix://{}", path.display()),
                Some(file),
            )
        } else {
            let url = url::Url::parse(listen)?;
            if url.scheme() != "ws" || url.path() != "/" || url.query().is_some() {
                bail!("proxy listener must use unix://PATH or ws://HOST:PORT");
            }
            let listener = TcpListener::bind((
                socket_host(&url)?.as_str(),
                url.port_or_known_default().context("missing port")?,
            ))
            .await
            .map_err(|source| crate::ProxyBindError {
                endpoint: listen.to_owned(),
                source,
            })?;
            if !listener.local_addr()?.ip().is_loopback() && !auth.enabled() {
                return Err(crate::ProxyConfigError(
                    "non-loopback Codex proxy requires WebSocket authentication".into(),
                )
                .into());
            }
            crate::proxy_address::validate(listener.local_addr()?, upstream).await?;
            let endpoint = format!("ws://{}", listener.local_addr()?);
            (Listener::Tcp(listener), endpoint, None)
        };
        let registry = ClientRegistry::default();
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let registrations = registry.clone();
        let upstream = upstream.to_owned();
        let task = tokio::spawn(async move {
            let _file = file;
            run_listener(listener, stop, registrations, upstream, auth).await;
        });
        Ok(Self {
            endpoint,
            registry,
            cancel,
            task: Some(task),
        })
    }
    fn bind_stdio(listen: &str, upstream: &str) -> Result<Self> {
        use std::os::fd::AsFd;
        let (input, output) = crate::proxy_stdio_io::Stdio::pair(
            std::io::stdin().as_fd(),
            std::io::stdout().as_fd(),
        )?;
        let registry = ClientRegistry::default();
        let cancel = CancellationToken::new();
        let r = registry.clone();
        let stop = cancel.clone();
        let upstream = upstream.to_owned();
        let task = tokio::spawn(async move {
            tokio::select! {
                () = stop.cancelled() => {},
                result = proxy_stdio(input,output,&upstream,r,None) => {
                    if let Err(error) = result { tracing::warn!(%error,"Codex stdio proxy closed"); }
                }
            }
        });
        Ok(Self {
            endpoint: listen.into(),
            registry,
            cancel,
            task: Some(task),
        })
    }
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    #[must_use]
    pub fn registry(&self) -> ClientRegistry {
        self.registry.clone()
    }
    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for CodexProxy {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn run_listener(
    listener: Listener,
    stop: CancellationToken,
    registrations: ClientRegistry,
    upstream: String,
    auth: Arc<AuthPolicy>,
) {
    let local = match &listener {
        Listener::Tcp(listener) => listener.local_addr().ok(),
        Listener::Unix(_) => None,
    };
    let mut clients = JoinSet::new();
    let mut retry_at = tokio::time::Instant::now();
    let mut retry_delay = Duration::from_millis(25);
    loop {
        tokio::select! {
            () = stop.cancelled() => break,
            accepted = async {
                if retry_at > tokio::time::Instant::now() {
                    tokio::time::sleep_until(retry_at).await;
                }
                listener.accept().await
            } => {
                let accepted = match accepted {
                    Ok(accepted) => {
                        retry_delay = Duration::from_millis(25);
                        retry_at = tokio::time::Instant::now();
                        accepted
                    }
                    Err(error) => {
                        // An accept failure does not invalidate already accepted streams.
                        // Retrying also keeps shutdown and completed-client reaping responsive.
                        tracing::error!(%error, ?retry_delay, "Codex proxy accept failed; retrying");
                        retry_at = tokio::time::Instant::now() + retry_delay;
                        retry_delay = (retry_delay * 2).min(Duration::from_secs(1));
                        continue;
                    }
                };
                let registry = registrations.clone();
                let upstream = upstream.clone();
                let auth = auth.clone();
                clients.spawn(async move {
                    let (stream, pid) = accepted.identify();
                    let result = relay(stream,pid,&upstream,registry,auth,local).await;
                    if let Err(error) = result { tracing::debug!(%error,"Codex proxy connection closed"); }
                });
            }
            _ = clients.join_next(), if !clients.is_empty() => {}
        }
    }
    clients.abort_all();
    while clients.join_next().await.is_some() {}
}

#[allow(clippy::result_large_err)] // tungstenite requires an HTTP response as its callback error.
async fn relay(
    stream: Box<dyn IoStream>,
    pid: Option<u32>,
    upstream: &str,
    registry: ClientRegistry,
    auth: Arc<AuthPolicy>,
    local: Option<std::net::SocketAddr>,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let ((mut client, client_pending), (mut server, server_pending)) =
        crate::proxy_handshake::establish(stream, upstream, auth, local).await?;
    let registration = Registration {
        id: registry.connect(pid),
        registry,
    };
    {
        let (client_read, client_write) = tokio::io::split(&mut client);
        let (server_read, server_write) = tokio::io::split(&mut server);
        let client_closed = CancellationToken::new();
        let server_closed = CancellationToken::new();
        // Each direction owns a bounded buffer; blocked writes never stall the other pump.
        tokio::select! {
            result = forward_direction(client_read, server_write, client_pending, true, &registration, &client_closed) => { result?; }
            result = forward_direction(server_read, client_write, server_pending, false, &registration, &server_closed) => { result?; }
            () = async { tokio::join!(client_closed.cancelled(), server_closed.cancelled()); } => {}
        }
    }
    // Both Close frames have been written (or a peer reached EOF). Shut down
    // the write halves, then dropping the streams releases both socket handles.
    // Registration also drops on errors and task cancellation.
    let (client_end, server_end) = tokio::join!(client.shutdown(), server.shutdown());
    client_end?;
    server_end?;
    Ok(())
}

/// Report Close only after writing it, and keep reading to detect subsequent EOF.
async fn forward_direction<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut source: R,
    mut destination: W,
    pending: Vec<u8>,
    from_client: bool,
    registration: &Registration,
    closed: &CancellationToken,
) -> Result<()> {
    use crate::proxy_wire::Observer;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_tungstenite::tungstenite::protocol::Role;
    let mut observer = Observer::new(if from_client {
        Role::Server
    } else {
        Role::Client
    });
    let text = |text: &str| {
        if from_client {
            registration.registry.client_frame(registration.id, text);
        } else {
            registration.registry.server_frame(registration.id, text);
        }
    };
    let open = observer.feed(&pending, text).unwrap_or(true);
    destination.write_all(&pending).await?;
    if !open {
        closed.cancel();
    }
    let mut buffer = vec![0; 16 * 1024];
    loop {
        let count = source.read(&mut buffer).await?;
        if count == 0 {
            return Ok(());
        }
        let bytes = &buffer[..count];
        let open = observer.feed(bytes, text).unwrap_or(true);
        destination.write_all(bytes).await?;
        if !open {
            closed.cancel();
        }
        // Large streams amortize syscalls without preallocating a large buffer
        // for every idle CLI connection. Never wait to fill the buffer.
        if count == buffer.len() && buffer.len() < 128 * 1024 {
            buffer.resize(buffer.len() * 2, 0);
        }
    }
}

/// Bridge a single JSONL client to a shared WebSocket upstream.
/// `pid` must come from the caller's verified process identity, not JSON input.
pub async fn proxy_stdio<R, W>(
    input: R,
    mut output: W,
    upstream: &str,
    registry: ClientRegistry,
    pid: Option<u32>,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::{mpsc, oneshot};
    let mut lines = BufReader::new(input).lines();
    let (output_tx, mut output_rx) = mpsc::channel::<tokio_tungstenite::tungstenite::Utf8Bytes>(8);
    let network = async {
        let server = tokio::time::timeout(Duration::from_secs(10), open_socket(upstream)).await??;
        let registration = Registration {
            id: registry.connect(pid),
            registry,
        };
        let (mut sink, mut stream) = server.split();
        let (flush_tx, mut flush_rx) = mpsc::channel::<Option<oneshot::Sender<()>>>(8);
        let send = async {
            loop {
                tokio::select! {
                    line = lines.next_line() => {
                        let Some(line) = line? else { return Ok::<_, anyhow::Error>(false); };
                        let value: serde_json::Value = serde_json::from_str(&line)?;
                        registration.registry.client_message(registration.id, &value);
                        sink.send(Message::text(line)).await?;
                    }
                    Some(ack) = flush_rx.recv() => {
                        sink.flush().await?;
                        if let Some(ack) = ack { let _ = ack.send(()); }
                    }
                }
            }
        };
        let receive = async {
            while let Some(frame) = stream.next().await {
                match frame? {
                    Message::Text(text) => {
                        let value: serde_json::Value = serde_json::from_str(&text)?;
                        registration
                            .registry
                            .server_message(registration.id, &value);
                        let line = if text.contains('\n') || text.contains('\r') {
                            // Pretty-printed WS JSON must become one physical JSONL line.
                            value.to_string().into()
                        } else {
                            text
                        };
                        output_tx.send(line).await?;
                    }
                    Message::Ping(_) => flush_tx.send(None).await?,
                    Message::Close(_) => {
                        let (ack, done) = oneshot::channel();
                        flush_tx.send(Some(ack)).await?;
                        done.await?;
                        break;
                    }
                    _ => {}
                }
            }
            // Deliver already received responses before normal completion. Input EOF or
            // an error can still cancel a blocked stdout writer through the outer select.
            Ok::<_, anyhow::Error>(true)
        };
        tokio::select! {
            result = send => result,
            result = receive => result,
        }
    };
    let write = async {
        while let Some(line) = output_rx.recv().await {
            output.write_all(line.as_bytes()).await?;
            output.write_all(b"\n").await?;
            output.flush().await?;
        }
        Ok::<_, anyhow::Error>(())
    };
    tokio::pin!(write);
    // The network future owns the socket and registration.
    // Drop it before draining stdout, so backpressure cannot retain a live claim.
    let drain = {
        let network = network;
        tokio::pin!(network);
        tokio::select! {
            result = &mut network => result?,
            result = &mut write => return result,
        }
    };
    drop(output_tx);
    if !drain {
        return Ok(());
    }
    tokio::select! {
        result = &mut write => result,
        line = lines.next_line() => {
            if line?.is_some() { bail!("Codex upstream is closed"); }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) static ACCEPT_ERROR: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<std::net::SocketAddr, std::io::Error>>,
    > = std::sync::LazyLock::new(Default::default);

    #[tokio::test]
    async fn accept_error_preserves_clients_and_listener() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("up.sock");
        let upstream = UnixListener::bind(&path).unwrap();
        let proxy = CodexProxy::bind("ws://127.0.0.1:0", &format!("unix://{}", path.display()))
            .await
            .unwrap();
        let connect = tokio_tungstenite::connect_async(proxy.endpoint());
        let accept = async {
            tokio_tungstenite::accept_async(upstream.accept().await.unwrap().0)
                .await
                .unwrap()
                .into_inner()
        };
        let (cli, mut server) = tokio::join!(connect, accept);
        let mut cli = cli.unwrap().0;
        let addr = proxy
            .endpoint()
            .trim_start_matches("ws://")
            .parse()
            .unwrap();
        ACCEPT_ERROR
            .lock()
            .unwrap()
            .insert(addr, std::io::Error::from_raw_os_error(nix::libc::EMFILE));
        // Wake an already pending accept; the next iteration receives the injected error.
        let _wake = TcpStream::connect(addr).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        cli.send(Message::Ping(vec![7].into())).await.unwrap();
        let mut ping = [0; 7];
        tokio::time::timeout(Duration::from_secs(2), server.read_exact(&mut ping))
            .await
            .unwrap()
            .expect("accept failure must preserve existing clients");
        assert_eq!(ping[0], 0x89);
        let connect = tokio_tungstenite::connect_async(proxy.endpoint());
        let accept = async {
            tokio_tungstenite::accept_async(upstream.accept().await.unwrap().0)
                .await
                .unwrap()
        };
        let (second, _) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(connect, accept)
        })
        .await
        .unwrap();
        assert!(second.is_ok(), "listener must recover");
        server.shutdown().await.unwrap();
        proxy.shutdown().await;
    }

    #[tokio::test]
    async fn backpressure_does_not_block_reverse_frames_or_eof() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("up.sock");
        let listener = UnixListener::bind(&path).unwrap();
        // A 64-byte receive buffer makes forward backpressure deterministic.
        let (cli, stream) = tokio::io::duplex(64);
        let registry = ClientRegistry::default();
        let r = registry.clone();
        let task = tokio::spawn(async move {
            relay(
                Box::new(stream),
                None,
                &format!("unix://{}", path.display()),
                r,
                Arc::new(AuthPolicy::None),
                None,
            )
            .await
        });
        let connect = tokio_tungstenite::client_async("ws://localhost/", cli);
        let accept = async {
            tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
                .await
                .unwrap()
                .into_inner()
        };
        let (cli, mut server) = tokio::join!(connect, accept);
        let mut cli = cli.unwrap().0.into_inner();
        // Binary data fills the client receive buffer; the client deliberately does not read.
        let payload = [&[0x82, 126, 4, 0][..], &[0; 1024]].concat();
        server.write_all(&payload).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        let ping = [0x89, 0x80, 1, 2, 3, 4];
        cli.write_all(&ping).await.unwrap();
        let mut forwarded = [0; 6];
        tokio::time::timeout(Duration::from_secs(1), server.read_exact(&mut forwarded))
            .await
            .expect("reverse direction blocked by forward backpressure")
            .unwrap();
        assert_eq!(forwarded, ping);
        cli.shutdown().await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("EOF must cancel a blocked write")
            .unwrap()
            .unwrap();
        assert!(registry.snapshot().is_empty());
    }

    #[tokio::test]
    async fn tcp_accept_does_not_wait_for_peer_identification() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let listener = Listener::Tcp(listener);
        let _first = TcpStream::connect(address).await.unwrap();
        let Accepted::Tcp(first) = listener.accept().await.unwrap() else {
            panic!("expected TCP")
        };
        // Hold the first connection without starting its PID lookup.
        let _second = TcpStream::connect(address).await.unwrap();
        let second = tokio::time::timeout(Duration::from_secs(1), listener.accept())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(second, Accepted::Tcp(_)));
        drop(first);
    }
}
