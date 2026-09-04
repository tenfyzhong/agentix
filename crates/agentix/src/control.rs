use std::net::SocketAddr;
#[cfg(unix)]
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

const MAX_CONTROL_MESSAGE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum ControlRequest {
    Sessions {
        cursor: Option<String>,
        limit: u32,
    },
    Call {
        method: String,
        params: Value,
    },
    Claim {
        #[serde(rename = "ttlMinutes")]
        ttl_minutes: u64,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct ControlResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub struct ControlCall {
    pub request: ControlRequest,
    response: oneshot::Sender<std::result::Result<Value, String>>,
}

impl ControlCall {
    pub fn respond(self, response: std::result::Result<Value, String>) {
        let _ = self.response.send(response);
    }
}

#[derive(Debug, Clone)]
enum ControlEndpoint {
    #[cfg(unix)]
    Unix(PathBuf),
    Tcp(SocketAddr),
}

impl ControlEndpoint {
    fn parse(input: &str) -> Result<Self> {
        #[cfg(unix)]
        if let Some(path) = input.strip_prefix("unix://") {
            if path.is_empty() {
                bail!("Agentix control Unix socket path must not be empty");
            }
            return Ok(Self::Unix(PathBuf::from(path)));
        }

        let Some(address) = input.strip_prefix("tcp://") else {
            bail!("Agentix control endpoint must use unix:// or tcp://");
        };
        let address = address
            .parse::<SocketAddr>()
            .with_context(|| format!("invalid Agentix control TCP address: {address}"))?;
        if !address.ip().is_loopback() {
            bail!("Agentix control TCP endpoint must use a loopback address");
        }
        Ok(Self::Tcp(address))
    }
}

pub async fn request(endpoint: &str, request: &ControlRequest) -> Result<Value> {
    match ControlEndpoint::parse(endpoint)? {
        #[cfg(unix)]
        ControlEndpoint::Unix(path) => {
            let stream = UnixStream::connect(&path).await.with_context(|| {
                format!(
                    "failed to connect to Agentix control socket {}",
                    path.display()
                )
            })?;
            exchange(stream, request).await
        }
        ControlEndpoint::Tcp(address) => {
            let stream = TcpStream::connect(address).await.with_context(|| {
                format!("failed to connect to Agentix control server {address}")
            })?;
            exchange(stream, request).await
        }
    }
}

pub async fn serve(
    endpoint: &str,
    calls: mpsc::Sender<ControlCall>,
    shutdown: CancellationToken,
) -> Result<()> {
    match ControlEndpoint::parse(endpoint)? {
        #[cfg(unix)]
        ControlEndpoint::Unix(path) => serve_unix(&path, calls, shutdown).await,
        ControlEndpoint::Tcp(address) => serve_tcp(address, calls, shutdown).await,
    }
}

async fn exchange<S>(stream: S, request: &ControlRequest) -> Result<Value>
where
    S: tokio::io::AsyncRead + AsyncWrite + Unpin,
{
    let mut stream = BufReader::new(stream);
    let mut encoded = serde_json::to_vec(request)?;
    encoded.push(b'\n');
    stream.get_mut().write_all(&encoded).await?;
    let mut response = String::new();
    stream
        .take(MAX_CONTROL_MESSAGE_BYTES)
        .read_line(&mut response)
        .await
        .context("failed to read Agentix control response")?;
    if response.is_empty() {
        bail!("Agentix control server closed the connection without a response");
    }
    let response: ControlResponse =
        serde_json::from_str(&response).context("Agentix control response is invalid")?;
    if response.ok {
        response
            .result
            .context("Agentix control response did not contain a result")
    } else {
        bail!(
            "{}",
            response
                .error
                .unwrap_or_else(|| "Agentix control request failed".into())
        )
    }
}

#[cfg(unix)]
async fn serve_unix(
    path: &Path,
    calls: mpsc::Sender<ControlCall>,
    shutdown: CancellationToken,
) -> Result<()> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.with_context(|| {
            format!(
                "failed to create Agentix control socket directory {}",
                parent.display()
            )
        })?;
    }
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if !metadata.file_type().is_socket() {
            bail!(
                "Agentix control endpoint exists and is not a socket: {}",
                path.display()
            );
        }
        if UnixStream::connect(path).await.is_ok() {
            bail!(
                "Agentix control socket is already in use: {}",
                path.display()
            );
        }
        std::fs::remove_file(path)
            .with_context(|| format!("failed to remove stale socket {}", path.display()))?;
    }
    let listener = UnixListener::bind(path)
        .with_context(|| format!("failed to bind Agentix control socket {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let result = accept_unix(&listener, calls, shutdown).await;
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, socket = %path.display(), "failed to remove Agentix control socket");
    }
    result
}

#[cfg(unix)]
async fn accept_unix(
    listener: &UnixListener,
    calls: mpsc::Sender<ControlCall>,
    shutdown: CancellationToken,
) -> Result<()> {
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("Agentix control socket accept failed")?;
                spawn_connection(stream, calls.clone(), shutdown.clone());
            }
        }
    }
}

async fn serve_tcp(
    address: SocketAddr,
    calls: mpsc::Sender<ControlCall>,
    shutdown: CancellationToken,
) -> Result<()> {
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to bind Agentix control server {address}"))?;
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("Agentix control TCP accept failed")?;
                spawn_connection(stream, calls.clone(), shutdown.clone());
            }
        }
    }
}

fn spawn_connection<S>(stream: S, calls: mpsc::Sender<ControlCall>, shutdown: CancellationToken)
where
    S: tokio::io::AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        if let Err(error) = serve_connection(stream, calls, shutdown).await {
            tracing::debug!(%error, "Agentix control connection failed");
        }
    });
}

async fn serve_connection<S>(
    stream: S,
    calls: mpsc::Sender<ControlCall>,
    shutdown: CancellationToken,
) -> Result<()>
where
    S: tokio::io::AsyncRead + AsyncWrite + Unpin,
{
    let mut stream = BufReader::new(stream);
    let mut request = String::new();
    {
        let mut limited = (&mut stream).take(MAX_CONTROL_MESSAGE_BYTES);
        tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            result = limited.read_line(&mut request) => {
                result?;
            }
        }
    }
    let response = if request.len() as u64 >= MAX_CONTROL_MESSAGE_BYTES && !request.ends_with('\n')
    {
        ControlResponse {
            ok: false,
            result: None,
            error: Some(format!(
                "Agentix control request exceeds the {MAX_CONTROL_MESSAGE_BYTES}-byte limit"
            )),
        }
    } else {
        match serde_json::from_str::<ControlRequest>(&request) {
            Ok(request) => {
                let (response_tx, response_rx) = oneshot::channel();
                calls
                    .send(ControlCall {
                        request,
                        response: response_tx,
                    })
                    .await
                    .context("Agentix control request handler stopped")?;
                let response = tokio::select! {
                    () = shutdown.cancelled() => return Ok(()),
                    response = response_rx => response,
                };
                match response {
                    Ok(Ok(result)) => ControlResponse {
                        ok: true,
                        result: Some(result),
                        error: None,
                    },
                    Ok(Err(error)) => ControlResponse {
                        ok: false,
                        result: None,
                        error: Some(error),
                    },
                    Err(_) => ControlResponse {
                        ok: false,
                        result: None,
                        error: Some("Agentix control request handler stopped".into()),
                    },
                }
            }
            Err(error) => ControlResponse {
                ok: false,
                result: None,
                error: Some(format!("invalid Agentix control request: {error}")),
            },
        }
    };
    let mut encoded = serde_json::to_vec(&response)?;
    encoded.push(b'\n');
    stream.get_mut().write_all(&encoded).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use serde_json::json;

    use super::*;

    #[test]
    fn tcp_control_endpoints_are_restricted_to_loopback_addresses() {
        assert!(ControlEndpoint::parse("tcp://127.0.0.1:32198").is_ok());
        assert!(ControlEndpoint::parse("tcp://[::1]:32198").is_ok());
        assert!(ControlEndpoint::parse("tcp://0.0.0.0:32198").is_err());
        assert!(ControlEndpoint::parse("tcp://localhost:32198").is_err());
    }

    #[tokio::test]
    async fn tcp_control_server_round_trips_requests_and_stops_cleanly() {
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = probe.local_addr().unwrap();
        drop(probe);
        let endpoint = format!("tcp://{address}");
        let shutdown = CancellationToken::new();
        let (call_tx, mut call_rx) = mpsc::channel(1);
        let task = tokio::spawn({
            let endpoint = endpoint.clone();
            let shutdown = shutdown.clone();
            async move { serve(&endpoint, call_tx, shutdown).await }
        });
        wait_for_tcp_server(address).await;

        let handler = tokio::spawn(async move {
            let call = call_rx.recv().await.unwrap();
            assert_eq!(
                call.request,
                ControlRequest::Sessions {
                    cursor: Some("next".into()),
                    limit: 5,
                }
            );
            call.respond(Ok(json!({"sessions": [{"id": "thr_tcp"}]})));
        });
        let response = request(
            &endpoint,
            &ControlRequest::Sessions {
                cursor: Some("next".into()),
                limit: 5,
            },
        )
        .await
        .unwrap();
        assert_eq!(response["sessions"][0]["id"], "thr_tcp");
        handler.await.unwrap();

        shutdown.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn tcp_control_server_rejects_malformed_requests_without_reaching_the_handler() {
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = probe.local_addr().unwrap();
        drop(probe);
        let endpoint = format!("tcp://{address}");
        let shutdown = CancellationToken::new();
        let (call_tx, mut call_rx) = mpsc::channel(1);
        let task = tokio::spawn({
            let endpoint = endpoint.clone();
            let shutdown = shutdown.clone();
            async move { serve(&endpoint, call_tx, shutdown).await }
        });
        wait_for_tcp_server(address).await;

        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(b"not-json\n").await.unwrap();
        let mut response = String::new();
        BufReader::new(stream)
            .read_line(&mut response)
            .await
            .unwrap();
        let response: ControlResponse = serde_json::from_str(&response).unwrap();
        assert!(!response.ok);
        assert!(
            response
                .error
                .as_deref()
                .unwrap()
                .contains("invalid Agentix control request")
        );
        assert!(call_rx.try_recv().is_err());

        shutdown.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn tcp_control_server_rejects_requests_at_the_message_size_limit() {
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = probe.local_addr().unwrap();
        drop(probe);
        let endpoint = format!("tcp://{address}");
        let shutdown = CancellationToken::new();
        let (call_tx, mut call_rx) = mpsc::channel(1);
        let task = tokio::spawn({
            let endpoint = endpoint.clone();
            let shutdown = shutdown.clone();
            async move { serve(&endpoint, call_tx, shutdown).await }
        });
        wait_for_tcp_server(address).await;

        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(&vec![
                b' ';
                usize::try_from(MAX_CONTROL_MESSAGE_BYTES).unwrap()
            ])
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
        let mut response = String::new();
        BufReader::new(stream)
            .read_line(&mut response)
            .await
            .unwrap();
        let response: ControlResponse = serde_json::from_str(&response).unwrap();
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("Agentix control request exceeds the 16777216-byte limit")
        );
        assert!(call_rx.try_recv().is_err());

        shutdown.cancel();
        task.await.unwrap().unwrap();
    }

    async fn wait_for_tcp_server(address: SocketAddr) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if TcpStream::connect(address).await.is_ok() {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_control_server_round_trips_requests_and_removes_its_socket() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let endpoint = format!("unix://{}", socket.display());
        let shutdown = CancellationToken::new();
        let (call_tx, mut call_rx) = mpsc::channel(1);
        let task = tokio::spawn({
            let endpoint = endpoint.clone();
            let shutdown = shutdown.clone();
            async move { serve(&endpoint, call_tx, shutdown).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !socket.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let handler = tokio::spawn(async move {
            let call = call_rx.recv().await.unwrap();
            assert_eq!(
                call.request,
                ControlRequest::Sessions {
                    cursor: None,
                    limit: 3,
                }
            );
            call.respond(Ok(json!({"sessions": [], "nextCursor": null})));
        });
        let response = request(
            &endpoint,
            &ControlRequest::Sessions {
                cursor: None,
                limit: 3,
            },
        )
        .await
        .unwrap();
        assert_eq!(response, json!({"sessions": [], "nextCursor": null}));
        handler.await.unwrap();

        shutdown.cancel();
        task.await.unwrap().unwrap();
        assert!(!socket.exists());
    }
}
