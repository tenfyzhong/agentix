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
        .insert(Arc::new(message(2)), retired.card.clone());
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

#[test]
fn cleanup_queue_shares_message_storage_with_index() {
    let writes = CardWrites::default();
    let message = message(0);
    let _revision = writes.reserve(&message);
    let registry = writes.registry.lock().unwrap();
    let (key, _) = registry.cards.get_key_value(&message).unwrap();
    let queued = registry.sweep.front().unwrap();
    assert!(std::ptr::eq(
        std::borrow::Borrow::<MessageRef>::borrow(key),
        std::borrow::Borrow::<MessageRef>::borrow(queued),
    ));
}

#[tokio::test(start_paused = true)]
async fn idle_cleanup_releases_peak_capacity_with_retained_replacement() {
    let writes = CardWrites::default();
    let live = writes.reserve(&message(0));
    let retired = writes.reserve(&message(1));
    retired.card.state.lock().await.target = None;
    retired.card.retain.store(true, Ordering::SeqCst);
    writes
        .update(
            &Observer::default(),
            &retired,
            &message(1).conversation,
            &OutboundView::text("Card", "replacement"),
        )
        .await
        .unwrap();
    let retained = Arc::downgrade(&retired.card);
    drop(retired);
    let idle: Vec<_> = (2..4098).map(|i| writes.reserve(&message(i))).collect();
    let idle_refs: Vec<_> = idle.iter().map(|r| Arc::downgrade(&r.card)).collect();
    drop(idle);
    tokio::task::yield_now().await;
    for _ in 0..64 {
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
    }
    assert!(idle_refs.iter().all(|card| card.strong_count() == 0));
    assert!(Arc::ptr_eq(&live.card, &writes.reserve(&message(0)).card));
    let original = writes.reserve(&message(1));
    let alias = writes.reserve(&MessageRef::new(message(1).conversation, "replacement"));
    assert!(Arc::ptr_eq(&retained.upgrade().unwrap(), &alias.card));
    assert!(Arc::ptr_eq(&original.card, &alias.card));
    assert!(!original.current());
    let registry = writes.registry.lock().unwrap();
    let capacities = (registry.cards.capacity(), registry.sweep.capacity());
    drop(registry);
    assert!(
        capacities.0 <= 256,
        "map retained peak capacity: {}",
        capacities.0
    );
    assert!(
        capacities.1 <= 256,
        "queue retained peak capacity: {}",
        capacities.1
    );
}

#[tokio::test(start_paused = true)]
async fn idle_cards_are_reclaimed_without_further_access() {
    let writes = CardWrites::default();
    let revision = writes.reserve(&message(0));
    let weak = Arc::downgrade(&revision.card);
    drop(revision);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    assert_eq!(weak.strong_count(), 0);
    let registry = writes.registry.lock().unwrap();
    assert!(registry.cards.is_empty());
    assert!(registry.sweep.is_empty());
    assert_eq!(registry.cards.capacity(), 0);
    assert_eq!(registry.sweep.capacity(), 0);
}

#[tokio::test]
async fn compaction_bounds_batches_and_preserves_writes_across_both_tables() {
    let writes = CardWrites::default();
    let revisions: Vec<_> = (0..1024).map(|i| writes.reserve(&message(i))).collect();
    {
        let mut registry = writes.registry.lock().unwrap();
        registry.cards.reserve(8192);
        registry.maintain();
        assert_eq!(registry.compacting.as_ref().unwrap().cards.len(), 1024);
        registry.maintain();
        assert_eq!(registry.compacting.as_ref().unwrap().cards.len(), 768);
        assert_eq!(registry.cards.len(), 256);
    }
    // Find one migrated and one pending key without depending on sweep rotation.
    let (migrated, pending) = {
        let registry = writes.registry.lock().unwrap();
        (
            registry.sweep.front().unwrap().as_ref().clone(),
            registry
                .compacting
                .as_ref()
                .unwrap()
                .sweep
                .front()
                .unwrap()
                .as_ref()
                .clone(),
        )
    };
    for key in [&migrated, &pending] {
        let index: usize = key.message_id.parse().unwrap();
        let newer = writes.reserve(key);
        assert!(Arc::ptr_eq(&newer.card, &revisions[index].card));
        assert!(!revisions[index].current());
    }
    let replacement = writes.reserve(&pending);
    replacement.card.state.lock().await.target = None;
    replacement.card.retain.store(true, Ordering::SeqCst);
    writes
        .update(
            &Observer::default(),
            &replacement,
            &pending.conversation,
            &OutboundView::text("Card", "replacement"),
        )
        .await
        .unwrap();
    let fresh = writes.reserve(&message(9999));
    let idle: Vec<_> = revisions.iter().map(|r| Arc::downgrade(&r.card)).collect();
    drop(revisions);
    for _ in 0..16 {
        let mut registry = writes.registry.lock().unwrap();
        let before = registry
            .compacting
            .as_ref()
            .map_or(0, |old| old.cards.len());
        registry.maintain();
        let after = registry
            .compacting
            .as_ref()
            .map_or(0, |old| old.cards.len());
        assert!(before.saturating_sub(after) <= 256);
    }
    assert_eq!(
        idle.iter().filter(|card| card.strong_count() != 0).count(),
        1
    );
    assert!(Arc::ptr_eq(
        &fresh.card,
        &writes.reserve(&message(9999)).card
    ));
    let alias = writes.reserve(&MessageRef::new(pending.conversation, "replacement"));
    assert!(Arc::ptr_eq(&replacement.card, &alias.card));
    let registry = writes.registry.lock().unwrap();
    assert!(registry.compacting.is_none());
    assert_eq!(registry.cards.len(), 3);
    assert_eq!(registry.sweep.len(), 3);
}

#[test]
fn dropping_writer_during_compaction_releases_both_tables() {
    let writes = CardWrites::default();
    let revisions: Vec<_> = (0..1024).map(|i| writes.reserve(&message(i))).collect();
    let weak: Vec<_> = revisions.iter().map(|r| Arc::downgrade(&r.card)).collect();
    {
        let mut registry = writes.registry.lock().unwrap();
        registry.cards.reserve(8192);
        registry.maintain();
        registry.maintain();
        assert!(registry.compacting.is_some());
        assert!(!registry.cards.is_empty());
    }
    drop(revisions);
    drop(writes);
    assert!(weak.iter().all(|card| card.strong_count() == 0));
}

#[tokio::test(start_paused = true)]
async fn idle_cleanup_is_bounded_and_preserves_live_and_retired_cards() {
    let writes = CardWrites::default();
    let live = writes.reserve(&message(0));
    let retired = writes.reserve(&message(1));
    retired.card.retain.store(true, Ordering::SeqCst);
    writes
        .registry
        .lock()
        .unwrap()
        .insert(Arc::new(message(2)), retired.card.clone());
    let retired_weak = Arc::downgrade(&retired.card);
    drop(retired);
    let idle: Vec<_> = (3..1027).map(|i| writes.reserve(&message(i))).collect();
    let weak: Vec<_> = idle.iter().map(|r| Arc::downgrade(&r.card)).collect();
    drop(idle);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    let removed = weak.iter().filter(|w| w.strong_count() == 0).count();
    assert!(removed > 0 && removed <= 256);
    for _ in 0..5 {
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
    }
    assert!(weak.iter().all(|w| w.strong_count() == 0));
    assert!(Arc::ptr_eq(&live.card, &writes.reserve(&message(0)).card));
    assert!(Arc::ptr_eq(
        &retired_weak.upgrade().unwrap(),
        &writes.reserve(&message(2)).card
    ));
    assert_eq!(writes.registry.lock().unwrap().cards.len(), 3);
}

#[tokio::test]
async fn dropping_writer_stops_cleanup_and_releases_registry() {
    let writes = CardWrites::default();
    writes.reserve(&message(0));
    let registry = Arc::downgrade(&writes.registry);
    let reaper = writes
        .registry
        .lock()
        .unwrap()
        .reaper
        .as_ref()
        .unwrap()
        .clone();
    tokio::task::yield_now().await;
    drop(writes);
    tokio::task::yield_now().await;
    assert!(reaper.is_finished());
    assert!(registry.upgrade().is_none());
}

#[test]
fn cleanup_recovers_after_runtime_recreation() {
    let writes = CardWrites::default();
    let first = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    first.block_on(async {
        writes.reserve(&message(0));
        tokio::task::yield_now().await;
    });
    drop(first);
    assert!(
        writes
            .registry
            .lock()
            .unwrap()
            .reaper
            .as_ref()
            .unwrap()
            .is_finished()
    );
    let second = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap();
    second.block_on(async {
        let revision = writes.reserve(&message(1));
        let weak = Arc::downgrade(&revision.card);
        drop(revision);
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(2)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            weak.strong_count(),
            0,
            "idle cleanup must recover on the new runtime"
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_access_starts_only_one_reaper_and_restarts_after_exit() {
    let writes = Arc::new(CardWrites::default());
    let mut previous = None;
    for _ in 0..2 {
        let barrier = Arc::new(tokio::sync::Barrier::new(16));
        let mut tasks = Vec::new();
        for index in 0..16 {
            let writes = writes.clone();
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                let _revision = writes.reserve(&message(index));
                writes
                    .registry
                    .lock()
                    .unwrap()
                    .reaper
                    .as_ref()
                    .unwrap()
                    .id()
            }));
        }
        let reaper_id = tasks.pop().unwrap().await.unwrap();
        for task in tasks {
            assert_eq!(task.await.unwrap(), reaper_id);
        }
        assert_ne!(previous, Some(reaper_id));
        previous = Some(reaper_id);
        let reaper = writes
            .registry
            .lock()
            .unwrap()
            .reaper
            .as_ref()
            .unwrap()
            .clone();
        reaper.abort();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !reaper.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    // Restart once more, then verify the owner aborts the replacement task too.
    writes.reserve(&message(17));
    let reaper = writes
        .registry
        .lock()
        .unwrap()
        .reaper
        .as_ref()
        .unwrap()
        .clone();
    drop(writes);
    tokio::time::timeout(Duration::from_secs(1), async {
        while !reaper.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
