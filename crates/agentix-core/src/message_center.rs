use std::future::Future;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};

use crate::InboundEnvelope;

/// Duplex FIFO boundary shared by an IM adapter and all of its clones.
///
/// Each direction uses Tokio's fair mutex wait queue: operations enter in the
/// order their futures are first polled. Only the outbound head may perform I/O,
/// including pacing, token refresh, and rate-limit retries. Inbound delivery has
/// its own queue so an outbound cooldown cannot block incoming envelopes.
///
/// Pending operations stay in their calling futures; there are no detached
/// workers or extra unbounded buffers. Dropping a future intentionally abandons
/// that operation and removes it from the queue. Transport cooldown state must
/// outlive that future so cancellation cannot bypass an established rate limit.
#[derive(Clone, Default)]
pub struct MessageCenter {
    queues: Arc<Queues>,
}

#[derive(Default)]
struct Queues {
    inbound: Mutex<()>,
    outbound: Mutex<()>,
}

impl MessageCenter {
    /// Execute one complete outbound operation at the FIFO head.
    ///
    /// The supplied future must include all retries and must not recursively
    /// enter this center's outbound queue.
    pub async fn outbound<T>(&self, operation: impl Future<Output = T>) -> T {
        let _head = self.queues.outbound.lock().await;
        operation.await
    }

    /// Deliver a normalized envelope through the independent inbound FIFO.
    ///
    /// The bounded runtime sender provides backpressure. A closed receiver
    /// returns the original envelope to the caller.
    pub async fn inbound(
        &self,
        destination: &mpsc::Sender<InboundEnvelope>,
        envelope: InboundEnvelope,
    ) -> Result<(), Box<mpsc::error::SendError<InboundEnvelope>>> {
        let _head = self.queues.inbound.lock().await;
        destination.send(envelope).await.map_err(Box::new)
    }
}
