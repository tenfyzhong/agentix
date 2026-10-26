use agentix_domain::{ChannelAdapter, ChannelKind, ConversationRef, MessageRef, OutboundView};
use agentix_telegram::TelegramAdapter;
use std::task::{Context, Poll, Waker};

#[tokio::test]
async fn disabled_instrumentation_adds_no_poll_time_allocation() {
    let adapter = TelegramAdapter::new("unused", [1]);
    let conversation = ConversationRef::new(ChannelKind::Feishu, "unused");
    let view = OutboundView::text("title", "body");
    let message = MessageRef::new(conversation.clone(), "unused");
    // The existing async-trait allocation is outside the measured poll. Only the
    // invalid-channel error string should allocate; no network request is made.
    let mut send = adapter.send(&conversation, &view);
    let mut update = adapter.update(&conversation, &message, &view);
    let mut context = Context::from_waker(Waker::noop());
    let allocation = allocation_counter::measure(|| {
        assert!(matches!(
            send.as_mut().poll(&mut context),
            Poll::Ready(Err(_))
        ));
        assert!(matches!(
            update.as_mut().poll(&mut context),
            Poll::Ready(Err(_))
        ));
    });
    assert_eq!(
        allocation.count_total, 2,
        "only two error strings may allocate"
    );
}
