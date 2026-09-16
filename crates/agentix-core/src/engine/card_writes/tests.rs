use super::*;
use std::{hint::black_box, sync::atomic::AtomicUsize, time::Instant};

#[derive(Default)]
struct Observer(AtomicUsize);

#[async_trait]
impl ChannelAdapter for Observer {
    fn kind(&self) -> ChannelKind {
        ChannelKind::Telegram
    }

    async fn send(
        &self,
        conversation: &ConversationRef,
        _: &OutboundView,
    ) -> Result<MessageRef, ChannelError> {
        Ok(MessageRef::new(conversation.clone(), "replacement"))
    }

    async fn update(
        &self,
        _: &ConversationRef,
        _: &MessageRef,
        view: &OutboundView,
    ) -> Result<(), ChannelError> {
        self.0
            .store(std::ptr::from_ref(view) as usize, Ordering::Relaxed);
        Ok(())
    }
}

fn message(index: usize) -> MessageRef {
    MessageRef::new(
        ConversationRef::new(ChannelKind::Telegram, "perf"),
        index.to_string(),
    )
}

#[tokio::test]
async fn ordinary_update_borrows_original_view() {
    let writes = CardWrites::default();
    let message = message(0);
    let revision = writes.reserve(&message);
    let channel = Observer::default();
    let view = OutboundView::text("Card", "x".repeat(1_000_000));
    writes
        .update(&channel, &revision, &message.conversation, &view)
        .await
        .unwrap();
    assert_eq!(
        channel.0.load(Ordering::Relaxed),
        std::ptr::from_ref(&view) as usize
    );
}

#[tokio::test]
async fn stale_revision_does_not_wait_for_busy_writer() {
    let writes = CardWrites::default();
    let message = message(0);
    let old = writes.reserve(&message);
    let _latest = writes.reserve(&message);
    let _busy = old.card.state.lock().await;
    let view = OutboundView::text("Card", "old");
    let channel = Observer::default();
    assert!(
        !tokio::time::timeout(
            Duration::from_millis(50),
            writes.update(&channel, &old, &message.conversation, &view)
        )
        .await
        .unwrap()
        .unwrap()
    );
    assert_eq!(channel.0.load(Ordering::Relaxed), 0);
}

#[test]
fn one_lookup_does_not_sweep_all_idle_cards() {
    let writes = CardWrites::default();
    let revisions: Vec<_> = (0..1024).map(|i| writes.reserve(&message(i))).collect();
    let weak: Vec<_> = revisions.iter().map(|r| Arc::downgrade(&r.card)).collect();
    drop(revisions);
    let _new = writes.reserve(&message(1024));
    let removed = weak.iter().filter(|card| card.strong_count() == 0).count();
    assert!(removed > 0 && removed <= 8, "unbounded cleanup: {removed}");
}

#[test]
fn incremental_cleanup_preserves_live_revisions_and_retired_aliases() {
    let writes = CardWrites::default();
    let live = writes.reserve(&message(0));
    let retired = writes.reserve(&message(1));
    retired.card.retain.store(true, Ordering::SeqCst);
    let retired_weak = Arc::downgrade(&retired.card);
    writes
        .registry
        .lock()
        .unwrap()
        .cards
        .insert(message(2), retired.card.clone());
    drop(retired);
    for index in 3..4096 {
        writes.reserve(&message(index));
    }
    assert!(Arc::ptr_eq(&live.card, &writes.reserve(&message(0)).card));
    let original = writes.reserve(&message(1));
    let alias = writes.reserve(&message(2));
    assert!(Arc::ptr_eq(&original.card, &alias.card));
    assert!(Arc::ptr_eq(&retired_weak.upgrade().unwrap(), &alias.card));
    assert!(!original.current());
    assert!(alias.current());
}

#[test]
fn incremental_cleanup_revisits_temporarily_retained_cards() {
    let writes = CardWrites::default();
    let revision = writes.reserve(&message(0));
    revision.card.retain.store(true, Ordering::SeqCst);
    let weak = Arc::downgrade(&revision.card);
    for index in 1..1024 {
        writes.reserve(&message(index));
    }
    revision.card.retain.store(false, Ordering::SeqCst);
    drop(revision);
    for index in 1024..2048 {
        writes.reserve(&message(index));
    }
    assert_eq!(weak.strong_count(), 0);
    assert!(writes.registry.lock().unwrap().cards.len() <= 256);
}

#[test]
#[ignore = "manual repeatable microbenchmark; run with --release --ignored --nocapture"]
fn card_write_performance_reserve() {
    for count in [1, 256, 4096, 16384] {
        let writes = CardWrites::default();
        let revisions: Vec<_> = (0..count).map(|i| writes.reserve(&message(i))).collect();
        let hot = message(0);
        let start = Instant::now();
        for _ in 0..20_000 {
            black_box(writes.reserve(black_box(&hot)));
        }
        println!(
            "reserve retained={count}: {:.1} ns/op",
            start.elapsed().as_secs_f64() * 1e9 / 20_000.0
        );
        black_box(revisions);
    }
}

#[tokio::test]
#[ignore = "manual repeatable microbenchmark; run with --release --ignored --nocapture"]
async fn card_write_performance_update() {
    let writes = CardWrites::default();
    let message = message(0);
    let revision = writes.reserve(&message);
    let channel = Observer::default();
    for bytes in [1024, 1_000_000] {
        let view = OutboundView::text("Card", "x".repeat(bytes));
        let start = Instant::now();
        for _ in 0..5000 {
            black_box(
                writes
                    .update(&channel, &revision, &message.conversation, black_box(&view))
                    .await
                    .unwrap(),
            );
        }
        println!(
            "update body={bytes}: {:.1} ns/op",
            start.elapsed().as_secs_f64() * 1e9 / 5000.0
        );
    }
}
