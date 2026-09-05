use std::future::{Future, pending, poll_fn};
use std::pin::pin;
use std::sync::Mutex;
use std::task::Poll;
use std::time::Duration;

use agentix_core::{ChannelKind, ConversationRef, InboundEnvelope, MessageCenter};
use tokio::sync::{mpsc, oneshot};

async fn assert_pending(future: &mut (impl Future + Unpin)) {
    poll_fn(|cx| {
        assert!(std::pin::Pin::new(&mut *future).poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn outbound_queue_preserves_admission_order_across_clones() {
    let center = MessageCenter::default();
    let clone = center.clone();
    let completed = Mutex::new(Vec::new());
    let (release, ready) = oneshot::channel();
    let mut first = pin!(center.outbound(async {
        ready.await.unwrap();
        completed.lock().unwrap().push(1);
    }));
    let mut second = pin!(clone.outbound(async { completed.lock().unwrap().push(2) }));
    let mut third = pin!(center.outbound(async { completed.lock().unwrap().push(3) }));
    assert_pending(&mut first).await;
    assert_pending(&mut second).await;
    assert_pending(&mut third).await;
    assert!(completed.lock().unwrap().is_empty());
    release.send(()).unwrap();
    first.await;
    // Polling the later future first must not let it overtake the earlier waiter.
    assert_pending(&mut third).await;
    second.await;
    third.await;
    assert_eq!(*completed.lock().unwrap(), [1, 2, 3]);
}

#[tokio::test]
async fn inbound_progresses_in_fifo_order_while_outbound_is_blocked() {
    let center = MessageCenter::default();
    let mut blocked = pin!(center.outbound(pending::<()>()));
    assert_pending(&mut blocked).await;
    let (sender, mut receiver) = mpsc::channel(1);
    let conversation = ConversationRef::new(ChannelKind::Telegram, "42");
    let envelope = |id| InboundEnvelope::text(id, conversation.clone(), "42", id);
    center.inbound(&sender, envelope("first")).await.unwrap();
    let mut second = pin!(center.inbound(&sender, envelope("second")));
    let mut third = pin!(center.inbound(&sender, envelope("third")));
    assert_pending(&mut second).await;
    assert_pending(&mut third).await;
    assert_eq!(receiver.recv().await.unwrap().event_id, "first");
    assert_pending(&mut third).await;
    tokio::time::timeout(Duration::from_secs(1), second)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receiver.recv().await.unwrap().event_id, "second");
    third.await.unwrap();
    assert_eq!(receiver.recv().await.unwrap().event_id, "third");
}

#[tokio::test]
async fn cancelling_queued_or_active_outbound_work_releases_the_queue() {
    let center = MessageCenter::default();
    let mut head = Box::pin(center.outbound(pending::<()>()));
    assert_pending(&mut head).await;
    let mut cancelled = Box::pin(center.outbound(async { panic!("cancelled work ran") }));
    let mut next = pin!(center.outbound(async { 42 }));
    assert_pending(&mut cancelled).await;
    assert_pending(&mut next).await;
    drop(cancelled);
    drop(head);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), next)
            .await
            .unwrap(),
        42
    );
}

#[tokio::test]
async fn errors_release_the_head_and_closed_inbound_returns_the_envelope() {
    let center = MessageCenter::default();
    assert_eq!(
        center.outbound(async { Err::<(), _>("rejected") }).await,
        Err("rejected")
    );
    assert_eq!(center.outbound(async { "next" }).await, "next");
    let (sender, receiver) = mpsc::channel(1);
    drop(receiver);
    let envelope = InboundEnvelope::text(
        "event",
        ConversationRef::new(ChannelKind::Feishu, "chat"),
        "owner",
        "hello",
    );
    assert_eq!(
        center
            .inbound(&sender, envelope.clone())
            .await
            .unwrap_err()
            .0,
        envelope
    );
}
