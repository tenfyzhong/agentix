use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Weak};

use tokio::sync::{Mutex, mpsc};

use crate::{ConversationRef, InboundEnvelope};

/// Conversation-scoped outbound FIFOs shared by an adapter and its clones.
///
/// Operations for one conversation enter in first-poll order and include pacing
/// and retries. Different conversations can perform I/O concurrently. Unscoped
/// operations share their own queue. Inbound delivery has an independent FIFO.
///
/// Pending work stays in caller futures; dropping one removes it from its queue.
/// Weak queue entries do not retain inactive conversations. Transport cooldown
/// state must outlive caller futures so cancellation cannot bypass rate limits.
#[derive(Clone, Default)]
pub struct MessageCenter {
    queues: Arc<Queues>,
}

#[derive(Default)]
struct Queues {
    inbound: Mutex<()>,
    outbound: Mutex<HashMap<Option<ConversationRef>, Weak<Mutex<()>>>>,
}

impl MessageCenter {
    /// Execute a complete operation at this conversation's FIFO head.
    /// The operation must not recursively enter the same outbound queue.
    pub async fn outbound<T>(
        &self,
        conversation: Option<&ConversationRef>,
        operation: impl Future<Output = T>,
    ) -> T {
        let queue = {
            let mut queues = self.queues.outbound.lock().await;
            queues.retain(|_, queue| queue.strong_count() != 0);
            let key = conversation.cloned();
            if let Some(queue) = queues.get(&key).and_then(Weak::upgrade) {
                queue
            } else {
                let queue = Arc::new(Mutex::new(()));
                queues.insert(key, Arc::downgrade(&queue));
                queue
            }
        };
        let _head = queue.lock().await;
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
