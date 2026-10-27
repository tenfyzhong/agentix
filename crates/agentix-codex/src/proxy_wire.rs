//! Transparent wire forwarding with a disconnected, read-only message observer.
use super::proxy::IoStream;
use anyhow::{Context, Result};
use std::io;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_tungstenite::tungstenite::protocol::Role;

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

mod json;
const SMALL_MESSAGE: usize = 64 * 1024;

#[derive(Default)]
struct TextMessage {
    small: Vec<u8>,
    projection: Option<json::Projection>,
}
impl TextMessage {
    fn feed(&mut self, bytes: &[u8]) {
        if let Some(projection) = &mut self.projection {
            projection.feed(bytes);
        } else if self.small.len() + bytes.len() <= SMALL_MESSAGE {
            let capacity = (self.small.len() + bytes.len()).next_power_of_two();
            if self.small.capacity() < capacity {
                self.small.reserve_exact(capacity - self.small.len());
            }
            self.small.extend_from_slice(bytes);
        } else {
            let mut projection = json::Projection::default();
            projection.feed(&self.small);
            self.small.clear();
            projection.feed(bytes);
            self.projection = Some(projection);
        }
    }
    fn finish(&mut self, text: &mut impl FnMut(&str, bool)) {
        if let Some(projection) = self.projection.take() {
            if let Some(value) = projection.finish() {
                text(&value, true);
            }
        } else if let Ok(value) = std::str::from_utf8(&self.small) {
            text(value, false);
        }
        self.small.clear();
    }
}

struct Frame {
    remaining: u64,
    offset: usize,
    mask: Option<[u8; 4]>,
    opcode: u8,
    fin: bool,
}
pub(super) struct Observer {
    role: Role,
    header: [u8; 14],
    header_len: usize,
    frame: Option<Frame>,
    message_opcode: Option<u8>,
    message: TextMessage,
    disabled: bool,
    closed: bool,
}
impl Observer {
    pub fn new(role: Role) -> Self {
        Self {
            role,
            header: [0; 14],
            header_len: 0,
            frame: None,
            message_opcode: None,
            message: TextMessage::default(),
            disabled: false,
            closed: false,
        }
    }
    /// Observe bytes without changing forwarding. JSON/UTF-8 failures are local
    /// to a message; untrustworthy framing disables this connection's registry.
    #[cfg(test)]
    pub fn feed(&mut self, bytes: &[u8], mut text: impl FnMut(&str)) -> Result<bool> {
        self.feed_with(bytes, |value, _| text(value))
    }
    pub fn feed_with(&mut self, bytes: &[u8], mut text: impl FnMut(&str, bool)) -> Result<bool> {
        if self.disabled || self.closed {
            return Ok(!self.closed);
        }
        if let Err(error) = self.decode(bytes, &mut text) {
            self.disabled = true;
            self.message = TextMessage::default();
            self.frame = None;
            tracing::warn!(%error, "Codex proxy framing lost; session registration requires reconnect");
            return Err(error);
        }
        Ok(!self.closed)
    }
    fn decode(&mut self, mut bytes: &[u8], text: &mut impl FnMut(&str, bool)) -> Result<()> {
        while !bytes.is_empty() || self.frame.as_ref().is_some_and(|f| f.remaining == 0) {
            if self.frame.is_none() {
                loop {
                    let needed = if self.header_len < 2 {
                        2
                    } else {
                        2 + match self.header[1] & 127 {
                            126 => 2,
                            127 => 8,
                            _ => 0,
                        } + if self.header[1] & 128 != 0 { 4 } else { 0 }
                    };
                    let n = (needed - self.header_len).min(bytes.len());
                    self.header[self.header_len..self.header_len + n].copy_from_slice(&bytes[..n]);
                    self.header_len += n;
                    bytes = &bytes[n..];
                    if self.header_len < needed {
                        return Ok(());
                    }
                    if self.header_len == 2
                        && needed == 2
                        && (self.header[1] & 127 >= 126 || self.header[1] & 128 != 0)
                    {
                        continue;
                    }
                    break;
                }
                self.begin_frame()?;
            }
            let frame = self.frame.as_mut().unwrap();
            let n = usize::try_from(frame.remaining.min(bytes.len() as u64)).unwrap();
            if self.message_opcode == Some(1) && frame.opcode < 8 {
                if let Some(mask) = frame.mask {
                    // Fixed scratch space; never allocate according to wire length.
                    let mut scratch = [0u8; 8192];
                    for chunk in bytes[..n].chunks(scratch.len()) {
                        for (i, b) in chunk.iter().enumerate() {
                            scratch[i] = b ^ mask[(frame.offset + i) & 3];
                        }
                        self.message.feed(&scratch[..chunk.len()]);
                        frame.offset = (frame.offset + chunk.len()) & 3;
                    }
                } else {
                    self.message.feed(&bytes[..n]);
                }
            }
            frame.remaining -= n as u64;
            bytes = &bytes[n..];
            if frame.remaining != 0 {
                return Ok(());
            }
            let frame = self.frame.take().unwrap();
            if frame.opcode == 8 {
                self.closed = true;
                return Ok(());
            }
            if frame.opcode < 8 && frame.fin {
                if self.message_opcode == Some(1) {
                    self.message.finish(text);
                }
                self.message_opcode = None;
            }
        }
        Ok(())
    }
    fn begin_frame(&mut self) -> Result<()> {
        use anyhow::ensure;
        let first = self.header[0];
        let opcode = first & 15;
        let fin = first & 128 != 0;
        let masked = self.header[1] & 128 != 0;
        ensure!(
            first & 0x70 == 0 && matches!(opcode, 0 | 1 | 2 | 8 | 9 | 10),
            "invalid WebSocket flags/opcode"
        );
        ensure!(
            masked == matches!(self.role, Role::Server),
            "invalid WebSocket masking"
        );
        let (length, end) = match self.header[1] & 127 {
            126 => (
                u64::from(u16::from_be_bytes(self.header[2..4].try_into().unwrap())),
                4,
            ),
            127 => (
                u64::from_be_bytes(self.header[2..10].try_into().unwrap()),
                10,
            ),
            n => (u64::from(n), 2),
        };
        ensure!(i64::try_from(length).is_ok(), "invalid WebSocket length");
        if opcode >= 8 {
            ensure!(
                fin && length <= 125 && !(opcode == 8 && length == 1),
                "invalid control frame"
            );
        } else if opcode == 0 {
            ensure!(self.message_opcode.is_some(), "unexpected continuation");
        } else {
            ensure!(
                self.message_opcode.is_none(),
                "unfinished fragmented message"
            );
            self.message_opcode = Some(opcode);
        }
        self.frame = Some(Frame {
            remaining: length,
            offset: 0,
            mask: masked.then(|| self.header[end..end + 4].try_into().unwrap()),
            opcode,
            fin,
        });
        self.header_len = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn oversized_lifecycle_response_is_projected_and_tracking_continues() {
        let registry = crate::ClientRegistry::default();
        let connection = registry.connect(None);
        registry.client_frame(connection, r#"{"id":1,"method":"thread/start"}"#);
        let mut observer = Observer::new(Role::Client);
        let prefix = br#"{"result":{"thread":{"turns":[{"text":""#;
        let suffix = br#""}],"id":"new","source":"cli"}},"id":1}"#;
        let length = 65 * 1024 * 1024;
        let mut header = vec![0x81, 127];
        header.extend_from_slice(&((prefix.len() + length + suffix.len()) as u64).to_be_bytes());
        let mut observe = |text: &str| registry.server_frame(connection, text);
        observer.feed(&header, &mut observe).unwrap();
        observer.feed(prefix, &mut observe).unwrap();
        let block = vec![b'x'; 16 * 1024];
        for _ in 0..length / block.len() {
            observer.feed(&block, &mut observe).unwrap();
            assert!(registry.snapshot()[0].sessions.is_empty());
        }
        observer.feed(suffix, &mut observe).unwrap();
        assert_eq!(registry.snapshot()[0].sessions, ["new"]);
        assert!(!observer.feed(&[0x88, 0], |_| {}).unwrap());
    }

    #[test]
    fn invalid_text_message_does_not_disable_the_next_message() {
        let mut observer = Observer::new(Role::Client);
        let mut seen = Vec::new();
        let _ = observer.feed(&[0x81, 1, 0xff], |_| panic!("invalid UTF-8"));
        observer
            .feed(&[0x81, 2, b'{', b'}'], |s| seen.push(s.to_owned()))
            .unwrap();
        assert_eq!(seen, ["{}"]);
    }

    #[test]
    fn large_frame_preserves_native_new_session_tracking_without_attachment() {
        let registry = crate::ClientRegistry::default();
        let connection = registry.connect(None);
        let mut observer = Observer::new(Role::Client);
        let mut observe = |text: &str| registry.server_frame(connection, text);
        let feed = |observer: &mut Observer, text: &str, observe: &mut dyn FnMut(&str)| {
            let mut frame = vec![0x81, 127];
            frame.extend_from_slice(&(text.len() as u64).to_be_bytes());
            frame.extend_from_slice(text.as_bytes());
            for chunk in frame.chunks(16 * 1024) {
                assert!(observer.feed(chunk, &mut *observe).unwrap());
            }
        };
        registry.client_frame(connection, r#"{"id":1,"method":"thread/start"}"#);
        feed(
            &mut observer,
            r#"{"id":1,"result":{"thread":{"id":"old"}}}"#,
            &mut observe,
        );
        assert_eq!(registry.snapshot()[0].sessions, ["old"]);

        // A large history/tool response must not disable subsequent lifecycle observation.
        let large =
            serde_json::json!({"id":99,"result":{"data":"x".repeat(17 * 1024 * 1024)}}).to_string();
        feed(&mut observer, &large, &mut observe);
        registry.client_frame(
            connection,
            r#"{"id":2,"method":"thread/unsubscribe","params":{"threadId":"old"}}"#,
        );
        feed(
            &mut observer,
            r#"{"id":2,"result":{"status":"unsubscribed"}}"#,
            &mut observe,
        );
        registry.client_frame(connection, r#"{"id":3,"method":"thread/start"}"#);
        feed(
            &mut observer,
            r#"{"id":3,"result":{"thread":{"id":"new"}}}"#,
            &mut observe,
        );
        assert_eq!(registry.snapshot()[0].sessions, ["new"]);
        assert!(
            registry
                .lifecycle_since(0)
                .iter()
                .any(|(_, event)| matches!(event,
            agentix_domain::AgentEvent::SessionReplaced { session_id, replacement_session_id, .. }
            if session_id == "old" && replacement_session_id == "new"))
        );
    }

    #[test]
    fn framing_error_disables_decoding_and_releases_buffers() {
        let mut observer = Observer::new(Role::Client);
        assert!(observer.feed(&[0x83, 0], |_| {}).is_err());
        assert!(observer.disabled);
        assert!(observer.message.small.is_empty());
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

#[cfg(test)]
mod benchmarks;
#[cfg(test)]
mod legacy_bench;

#[cfg(test)]
mod streaming_tests {
    use super::*;

    #[test]
    fn projected_answer_receipts_resolve_existing_questions() {
        for from_client in [false, true] {
            let registry = crate::ClientRegistry::default();
            let connection = registry.connect(None);
            if from_client {
                registry.server_frame(connection, &serde_json::json!({"id":42,"method":"item/tool/requestUserInput","params":{"threadId":"t","turnId":"turn","itemId":"item","questions":[]}}).to_string());
            } else {
                registry.server_frame(connection, &serde_json::json!({"method":"item/completed","params":{"threadId":"t","turnId":"turn","item":{"type":"agentMessage","id":"message","text":"","questions":[{"title":"Which approach?","options":["Fast","Careful"]}]}}}).to_string());
                registry.client_frame(connection, &serde_json::json!({"id":42,"method":"turn/start","params":{"threadId":"t","input":[{"type":"text","text":"Which approach?\nFast"}]}}).to_string());
            }
            let response = serde_json::json!({"id":42,"result":{"data":"x".repeat(SMALL_MESSAGE)}})
                .to_string();
            let mut observer = Observer::new(if from_client {
                Role::Server
            } else {
                Role::Client
            });
            observer
                .feed_with(
                    &frame(response.as_bytes(), 1, true, from_client),
                    |s, projected| {
                        assert!(projected);
                        registry.projected_frame(connection, s, from_client);
                    },
                )
                .unwrap();
            assert_eq!(
                registry
                    .lifecycle_since(0)
                    .iter()
                    .filter(|(_, event)| matches!(
                        event,
                        agentix_domain::AgentEvent::InteractionResolved { .. }
                    ))
                    .count(),
                1
            );
        }
    }

    #[test]
    fn projected_completion_wakes_content_without_replaying_partial_interactions() {
        let registry = crate::ClientRegistry::default();
        let connection = registry.connect(None);
        registry.client_frame(connection, r#"{"id":1,"method":"thread/start"}"#);
        registry.server_frame(connection, r#"{"id":1,"result":{"thread":{"id":"live"}}}"#);
        let mut observer = Observer::new(Role::Client);
        let notification = serde_json::json!({"method":"turn/completed","params":{"threadId":"live","turn":{"output":"x".repeat(SMALL_MESSAGE)}}}).to_string();
        observer
            .feed_with(
                &frame(notification.as_bytes(), 1, true, false),
                |s, projected| {
                    assert!(projected);
                    registry.projected_frame(connection, s, false);
                },
            )
            .unwrap();
        assert!(registry.content_versions()["live"].1 > 0);
        let request = serde_json::json!({"id":99,"method":"item/tool/requestUserInput","params":{"threadId":"live","questions":[{"id":"q","question":"x".repeat(SMALL_MESSAGE)}]}}).to_string();
        observer
            .feed_with(
                &frame(request.as_bytes(), 1, true, false),
                |s, projected| {
                    assert!(projected);
                    registry.projected_frame(connection, s, false);
                },
            )
            .unwrap();
        assert!(!registry.question_is_pending(&serde_json::json!(99)));
        assert!(registry.lifecycle_since(0).is_empty());
    }
    fn frame(payload: &[u8], opcode: u8, fin: bool, masked: bool) -> Vec<u8> {
        let mut bytes = vec![
            opcode | if fin { 128 } else { 0 },
            127 | if masked { 128 } else { 0 },
        ];
        bytes.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        if masked {
            bytes.extend_from_slice(&[1, 2, 3, 4]);
        }
        bytes.extend(
            payload
                .iter()
                .enumerate()
                .map(|(i, b)| if masked { b ^ [1, 2, 3, 4][i & 3] } else { *b }),
        );
        bytes
    }
    #[test]
    fn fragmented_masked_projection_handles_control_frames_unicode_and_reordered_fields() {
        let payload = format!(
            "{{\"params\":{{\"ignored\":\"{}\",\"threadId\":\"雪🙂\"}},\"method\":\"thread/resume\",\"id\":42}}",
            "a\\\"\\\\雪🙂".repeat(10000)
        );
        for masked in [false, true] {
            for split in [1, 7, 16381] {
                let mut wire = frame(&payload.as_bytes()[..split], 1, false, masked);
                wire.extend(frame(b"ping", 9, true, masked));
                wire.extend(frame(&payload.as_bytes()[split..], 0, true, masked));
                let mut observer = Observer::new(if masked { Role::Server } else { Role::Client });
                let mut seen = Vec::new();
                for bytes in wire.chunks(13) {
                    observer
                        .feed(bytes, |s| {
                            seen.push(serde_json::from_str::<serde_json::Value>(s).unwrap());
                        })
                        .unwrap();
                }
                assert_eq!(
                    seen,
                    [
                        serde_json::json!({"id":42,"method":"thread/resume","params":{"threadId":"雪🙂"}})
                    ]
                );
                assert!(observer.message.small.capacity() <= SMALL_MESSAGE);
                assert!(observer.message.projection.is_none());
            }
        }
    }
    #[test]
    fn malformed_large_json_recovers_at_next_message_without_partial_registration() {
        let mut observer = Observer::new(Role::Client);
        let payload = format!(
            "{{\"id\":1,\"result\":{{\"thread\":{{\"id\":\"wrong\",\"turns\":\"{}\"}}}},\"bad\":[1,]}}",
            "x".repeat(SMALL_MESSAGE)
        );
        let mut seen = Vec::new();
        observer
            .feed(&frame(payload.as_bytes(), 1, true, false), |s| {
                seen.push(s.to_owned());
            })
            .unwrap();
        assert!(seen.is_empty());
        observer
            .feed(&frame(b"{}", 1, true, false), |s| seen.push(s.to_owned()))
            .unwrap();
        assert_eq!(seen, ["{}"]);
    }
}
