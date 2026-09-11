//! Transparent wire forwarding with a disconnected, read-only message observer.
use super::proxy::IoStream;
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_tungstenite::tungstenite::{Error, Message, WebSocket, protocol::Role};

/// Preserve frames read together with HTTP headers before discarding handshake state.
pub(super) struct HandshakeStream {
    stream: Box<dyn IoStream>,
    received: Vec<u8>,
    deferred: bool,
    response: Vec<u8>,
}
impl HandshakeStream {
    pub fn new(stream: Box<dyn IoStream>) -> Self {
        Self {
            stream,
            received: Vec::new(),
            deferred: false,
            response: Vec::new(),
        }
    }
    /// Validate/authenticate the upgrade without confirming it on the wire yet.
    pub fn deferred(stream: Box<dyn IoStream>) -> Self {
        Self {
            deferred: true,
            ..Self::new(stream)
        }
    }
    pub async fn send_rejection(&mut self) -> Result<()> {
        crate::proxy_http::write_response(&mut self.stream, &self.response).await
    }
    pub fn header(&self) -> &[u8] {
        let end = self
            .received
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map_or(self.received.len(), |p| p + 4);
        &self.received[..end]
    }
    pub fn into_received(self) -> (Box<dyn IoStream>, Vec<u8>) {
        (self.stream, self.received)
    }
    pub fn finish(self) -> Result<(Box<dyn IoStream>, Vec<u8>)> {
        let end = self
            .received
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .context("missing completed WebSocket handshake")?
            + 4;
        Ok((self.stream, self.received[end..].to_vec()))
    }
}
impl AsyncRead for HandshakeStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let start = buf.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(cx, buf);
        let previous = self.received.len();
        self.received.extend_from_slice(&buf.filled()[start..]);
        if let Some(end) = self.received.windows(4).position(|w| w == b"\r\n\r\n") {
            // Hand the HTTP decoder only headers; retain prefetched frames for the relay.
            let header_bytes = (end + 4).saturating_sub(previous);
            buf.set_filled(start + header_bytes.min(buf.filled().len() - start));
        }
        result
    }
}
impl AsyncWrite for HandshakeStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.deferred {
            self.response.extend_from_slice(buf);
            return Poll::Ready(Ok(buf.len()));
        }
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        if self.deferred {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

#[derive(Default)]
struct ObservationBuffer(VecDeque<u8>);
impl Read for ObservationBuffer {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.0.is_empty() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        self.0.read(buf)
    }
}
impl Write for ObservationBuffer {
    // Parser-generated Pong/Close frames never reach either transport.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(super) struct Observer {
    socket: Option<WebSocket<ObservationBuffer>>,
    closed: bool,
}
impl Observer {
    pub fn new(role: Role) -> Self {
        Self {
            socket: Some(WebSocket::from_raw_socket(
                ObservationBuffer::default(),
                role,
                None,
            )),
            closed: false,
        }
    }
    /// Decode partial/coalesced/fragmented frames; return false on a Close frame.
    pub fn feed(&mut self, bytes: &[u8], mut text: impl FnMut(&str)) -> Result<bool> {
        let Some(socket) = self.socket.as_mut() else {
            return Ok(!self.closed);
        };
        socket.get_mut().0.extend(bytes);
        loop {
            match socket.read() {
                Ok(Message::Text(value)) => text(&value),
                Ok(Message::Close(_)) => {
                    self.closed = true;
                    self.socket = None;
                    return Ok(false);
                }
                Ok(_) => {}
                Err(Error::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => return Ok(true),
                Err(e) => {
                    // Observation must never become a transport policy. Free buffered
                    // data and stop decoding this direction after any parser failure.
                    self.socket = None;
                    tracing::warn!(error = %e, "Codex proxy message observation disabled");
                    return Err(e.into());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn observer_limit_disables_decoding_and_releases_buffers() {
        let mut observer = Observer::new(Role::Client);
        observer.socket.as_mut().unwrap().set_config(|config| {
            config.max_message_size = Some(1);
        });
        assert!(observer.feed(&[0x81, 2, b'a', b'b'], |_| {}).is_err());
        assert!(observer.socket.is_none());
        assert!(
            observer
                .feed(&[0x81, 1, b'c'], |_| panic!("disabled observer"))
                .unwrap()
        );
    }

    #[tokio::test]
    async fn handshake_preserves_coalesced_frames_in_both_directions() {
        let (mut peer, stream) = tokio::io::duplex(4096);
        let request = b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
        let frame = [0x89, 0x80, 1, 2, 3, 4];
        peer.write_all(&[request.as_slice(), &frame].concat())
            .await
            .unwrap();
        let ws = tokio_tungstenite::accept_async(HandshakeStream::new(Box::new(stream)))
            .await
            .unwrap();
        assert_eq!(ws.into_inner().finish().unwrap().1, frame);

        let (mut peer, stream) = tokio::io::duplex(4096);
        let server = async {
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                header.push(peer.read_u8().await.unwrap());
            }
            let header = String::from_utf8(header).unwrap();
            let key = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("sec-websocket-key")
                        .then(|| value.trim())
                })
                .unwrap();
            let accept =
                tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
            let response = format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
            );
            peer.write_all(&[response.as_bytes(), &[0x8a, 0]].concat())
                .await
                .unwrap();
        };
        let client = tokio_tungstenite::client_async(
            "ws://localhost/",
            HandshakeStream::new(Box::new(stream)),
        );
        let ((), ws) = tokio::join!(server, client);
        assert_eq!(ws.unwrap().0.into_inner().finish().unwrap().1, [0x8a, 0]);
    }

    #[test]
    fn observer_handles_split_headers_fragmented_text_and_interleaved_control_frames() {
        let mut observer = Observer::new(Role::Client);
        let mut messages = Vec::new();
        // Text "hello" split across two frames, with a Ping between them.
        for byte in [1, 2, b'h', b'e', 0x89, 1, 42, 0x80, 3, b'l', b'l', b'o'] {
            assert!(
                observer
                    .feed(&[byte], |text| messages.push(text.to_owned()))
                    .unwrap()
            );
        }
        assert_eq!(messages, ["hello"]);
        assert!(
            !observer
                .feed(&[0x88, 0], |_| panic!("close is not text"))
                .unwrap()
        );
    }
}
