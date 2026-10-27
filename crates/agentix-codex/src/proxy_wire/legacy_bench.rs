//! Test-only pre-streaming 64 MiB observer used as a reproducible baseline.
use super::*;
use std::collections::VecDeque;
use std::io::{Read, Write};
use tokio_tungstenite::tungstenite::{Error, Message, WebSocket, protocol::WebSocketConfig};
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
        // Codex history/tool responses can exceed tungstenite's default 16 MiB
        // frame limit. Allow a single frame to use the existing bounded message
        // budget, just as a fragmented message can, or later /new events vanish.
        let config = WebSocketConfig::default();
        let config = config.max_frame_size(config.max_message_size);
        Self {
            socket: Some(WebSocket::from_raw_socket(
                ObservationBuffer::default(),
                role,
                Some(config),
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
