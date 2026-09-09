use agentix_core::{ChannelKind, ConversationRef, OutboundView, SqliteState};

fn conversation(id: &str) -> ConversationRef {
    ConversationRef {
        channel: ChannelKind::Telegram,
        conversation_id: id.into(),
    }
}

#[tokio::test]
async fn overload_rejections_are_deduplicated_and_coalesced_without_replaying_requests() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.sqlite3");
    let state = SqliteState::open(&path).await.unwrap();
    let chat = conversation("flood");
    for i in 0..100 {
        assert!(
            state
                .reject_overloaded("overload", &chat, &format!("event-{i}"), 16)
                .await
                .unwrap()
        );
    }
    drop(state);
    let state = SqliteState::open(&path).await.unwrap();
    assert!(
        !state
            .reject_overloaded("overload", &chat, "event-0", 16)
            .await
            .unwrap()
    );
    assert!(!state.claim_event(chat.channel, "event-0").await.unwrap());
    assert_eq!(state.rejected_event_count().await.unwrap(), 100);
    let batch = state
        .claim_notifications("overload", 100, 60, 32)
        .await
        .unwrap();
    assert_eq!(batch.len(), 1);
    assert!(batch[0].view.body.contains("100 requests"));
    state
        .reject_overloaded("overload", &chat, "later", 16)
        .await
        .unwrap();
    assert!(
        state
            .claim_notifications("overload", 100, 60, 32)
            .await
            .unwrap()
            .is_empty()
    );
    state
        .finish_notification(&batch[0], 100, None)
        .await
        .unwrap();
    let next = state
        .claim_notifications("overload", 100, 60, 32)
        .await
        .unwrap();
    assert_eq!(next.len(), 1);
    assert!(next[0].view.body.contains("1 requests"));
}

#[tokio::test]
async fn staging_requires_an_initialized_consumer_instead_of_silently_losing_delivery() {
    let state = SqliteState::in_memory().await.unwrap();
    let view = OutboundView::text("Task update", "body");
    assert!(
        state
            .stage_notification("missing", 1, Some((&conversation("a"), &view)))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn failed_notification_insert_rolls_back_the_consumer_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.sqlite3");
    let state = SqliteState::open(&path).await.unwrap();
    state.notification_cursor("tasks", 10).await.unwrap();
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_notification BEFORE INSERT ON notification_outbox BEGIN SELECT RAISE(ABORT, 'injected storage failure'); END")
        .execute(&pool).await.unwrap();
    let view = OutboundView::text("Task update", "body");
    assert!(
        state
            .stage_notification("tasks", 11, Some((&conversation("a"), &view)))
            .await
            .is_err()
    );
    assert_eq!(state.notification_cursor("tasks", 0).await.unwrap(), 10);
    sqlx::query("DROP TRIGGER fail_notification")
        .execute(&pool)
        .await
        .unwrap();
    state
        .stage_notification("tasks", 11, Some((&conversation("a"), &view)))
        .await
        .unwrap();
    assert_eq!(
        state
            .claim_notifications("tasks", 1, 30, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn notification_cursor_and_payload_survive_restart_without_reenqueuing_acknowledged_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.sqlite3");
    let state = SqliteState::open(&path).await.unwrap();
    assert_eq!(state.notification_cursor("tasks", 5).await.unwrap(), 5);
    let view = OutboundView::text("Task update", "waiting");
    state
        .stage_notification("tasks", 6, Some((&conversation("a"), &view)))
        .await
        .unwrap();
    assert_eq!(state.notification_cursor("tasks", 0).await.unwrap(), 6);
    drop(state);
    let state = SqliteState::open(&path).await.unwrap();
    let batch = state
        .claim_notifications("tasks", 100, 30, 10)
        .await
        .unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].view, view);
    assert!(
        state
            .finish_notification(&batch[0], 101, None)
            .await
            .unwrap()
    );
    state
        .stage_notification("tasks", 6, Some((&conversation("a"), &view)))
        .await
        .unwrap();
    assert!(
        state
            .claim_notifications("tasks", 200, 30, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(state.notification_cursor("tasks", 999).await.unwrap(), 6);
}

#[tokio::test]
async fn notifications_lease_one_head_per_conversation_and_retry_without_blocking_others() {
    let state = SqliteState::in_memory().await.unwrap();
    state.notification_cursor("tasks", 0).await.unwrap();
    let view = OutboundView::text("Task update", "body");
    for (sequence, destination) in [(1, "a"), (2, "a"), (3, "b")] {
        state
            .stage_notification("tasks", sequence, Some((&conversation(destination), &view)))
            .await
            .unwrap();
    }
    let first = state
        .claim_notifications("tasks", 100, 30, 10)
        .await
        .unwrap();
    assert_eq!(
        first.iter().map(|n| n.sequence).collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert!(
        state
            .claim_notifications("tasks", 100, 30, 10)
            .await
            .unwrap()
            .is_empty()
    );
    state
        .finish_notification(&first[0], 100, Some("offline"))
        .await
        .unwrap();
    state
        .finish_notification(&first[1], 100, None)
        .await
        .unwrap();
    assert!(
        state
            .claim_notifications("tasks", 100, 30, 10)
            .await
            .unwrap()
            .is_empty()
    );
    let retry = state
        .claim_notifications("tasks", 101, 30, 10)
        .await
        .unwrap();
    assert_eq!(retry.len(), 1);
    assert_eq!(retry[0].sequence, 1);
    assert_eq!(retry[0].attempts, 2);
    let recovered = state
        .claim_notifications("tasks", 132, 30, 10)
        .await
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert!(
        !state
            .finish_notification(&retry[0], 132, None)
            .await
            .unwrap()
    );
    assert!(
        state
            .finish_notification(&recovered[0], 132, None)
            .await
            .unwrap()
    );
    let next = state
        .claim_notifications("tasks", 132, 30, 10)
        .await
        .unwrap();
    assert_eq!(next[0].sequence, 2);
}
