//! Authenticated upgrades: confirm the CLI only after the upstream succeeds.
use crate::proxy::{IoStream, open_stream_checked};
use crate::proxy_auth::AuthPolicy;
use anyhow::{Context, Result};
use std::{sync::Arc, time::Duration};

type RawPeer = (Box<dyn IoStream>, Vec<u8>);

#[allow(clippy::result_large_err)] // tungstenite callbacks return HTTP responses on rejection.
pub(super) async fn establish(
    stream: Box<dyn IoStream>,
    upstream: &str,
    auth: Arc<AuthPolicy>,
    local: Option<std::net::SocketAddr>,
) -> Result<(RawPeer, RawPeer)> {
    use crate::proxy_http::{forward_rejection, write_response};
    use crate::proxy_wire::HandshakeStream;
    use tokio_tungstenite::tungstenite::Error;
    let mut handshake = HandshakeStream::deferred(stream);
    let mut request = None;
    let accepted = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::accept_hdr_async(
            &mut handshake,
            |incoming: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                let response = auth.upgrade(incoming, response)?;
                request = Some(incoming.clone());
                Ok(response)
            },
        ),
    )
    .await?;
    match accepted {
        Ok(ws) => drop(ws),
        Err(error) => {
            handshake.send_rejection().await?;
            return Err(error.into());
        }
    }
    let request = request.context("missing authenticated upgrade request")?;
    let (mut client, client_pending) = handshake.finish()?;
    let attempt = tokio::time::timeout(Duration::from_secs(10), async {
        let (stream, url) = open_stream_checked(upstream, local).await?;
        let upstream_request = upstream_request(&url, &request)?;
        let mut stream = HandshakeStream::new(stream);
        let result = tokio_tungstenite::client_async(upstream_request, &mut stream)
            .await
            .map(|(ws, response)| {
                drop(ws);
                response
            });
        Ok::<_, anyhow::Error>((stream, result))
    })
    .await;
    let (server, result) = match attempt {
        Ok(Ok(value)) => value,
        failure => {
            let status = if failure.is_err() {
                "504 Gateway Timeout"
            } else {
                "502 Bad Gateway"
            };
            write_response(
                &mut client,
                format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await?;
            return Err(match failure {
                Err(e) => e.into(),
                Ok(Err(e)) => e,
                _ => unreachable!(),
            });
        }
    };
    match result {
        Ok(_) => write_response(&mut client, server.header()).await?,
        Err(Error::Http(response)) => {
            forward_rejection(&mut client, server, response.status(), response.headers()).await?;
            return Err(Error::Http(response).into());
        }
        Err(error) => {
            write_response(
                &mut client,
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await?;
            return Err(error.into());
        }
    }
    let server = server.finish()?;
    Ok(((client, client_pending), server))
}

fn upstream_request(
    url: &str,
    incoming: &tokio_tungstenite::tungstenite::handshake::server::Request,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = url.into_client_request()?;
    let host = request.headers()["host"].clone();
    request.headers_mut().clear();
    request.headers_mut().insert("host", host);
    // Keep the validated client key and negotiation headers. URI/Host target the
    // configured upstream, while proxy Authorization must never cross this boundary.
    for (name, value) in incoming.headers() {
        if !matches!(name.as_str(), "host" | "authorization" | "origin") {
            request.headers_mut().append(name, value.clone());
        }
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn stalled_success_response_releases_connections() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("up.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let (mut peer, stream) = tokio::io::duplex(64);
        let task = tokio::spawn(async move {
            establish(
                Box::new(stream),
                &format!("unix://{}", path.display()),
                Arc::new(AuthPolicy::None),
                None,
            )
            .await
        });
        peer.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").await.unwrap();
        let mut upstream = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
            .await
            .unwrap()
            .into_inner();
        let result = tokio::time::timeout(Duration::from_secs(12), task)
            .await
            .expect("response write must have a deadline")
            .unwrap();
        assert!(result.is_err());
        assert_eq!(upstream.read(&mut [0]).await.unwrap(), 0);
    }
}
