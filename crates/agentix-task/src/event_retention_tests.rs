use super::*;
use crate::WriteOptions;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

async fn fixture() -> (tempfile::TempDir, Store, Arc<AtomicI64>) {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(AtomicI64::new(100 * 86400));
    let store = reopen(&dir, &clock).await;
    (dir, store, clock)
}
async fn reopen(dir: &tempfile::TempDir, clock: &Arc<AtomicI64>) -> Store {
    let clock = clock.clone();
    Store::open_with_clock(
        &dir.path().join("tasks.sqlite3"),
        Arc::new(move || clock.load(Ordering::SeqCst)),
    )
    .await
    .unwrap()
}
async fn seed(store: &Store, count: i64, timestamp: i64) {
    let mut tx = store.pool.begin().await.unwrap();
    for id in 0..count {
        let event = json!({"event_id":format!("old_{id}"),"project_id":"p","occurred_at":timestamp,"payload":{"title":"Notice","conversation":["x".repeat(8192)]}});
        sqlx::query("INSERT INTO task_events(event_id,data) VALUES (?,?)")
            .bind(format!("old_{id}"))
            .bind(event.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE event_retention SET compact_through=(SELECT MAX(sequence) FROM task_events) WHERE id=1").execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn automatic_batches_are_bounded_durable_and_serialized_across_stores() {
    let (dir, store, clock) = fixture().await;
    seed(&store, 150, 1).await;
    let other = reopen(&dir, &clock).await;
    let (a, b) = tokio::join!(store.auto_maintain_events(), other.auto_maintain_events());
    let reports: Vec<_> = [a.unwrap(), b.unwrap()].into_iter().flatten().collect();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["deleted_events"], 64);
    assert_eq!(
        store.event_policy(None, None, None).await.unwrap()["runs"],
        1
    );
    assert!(store.auto_maintain_events().await.unwrap().is_none());
    clock.fetch_add(5, Ordering::SeqCst);
    let restarted = reopen(&dir, &clock).await;
    assert_eq!(
        restarted.auto_maintain_events().await.unwrap().unwrap()["deleted_events"],
        64
    );
    clock.fetch_add(5, Ordering::SeqCst);
    assert_eq!(
        restarted.auto_maintain_events().await.unwrap().unwrap()["deleted_events"],
        22
    );
    let state = restarted.event_policy(None, None, None).await.unwrap();
    assert_eq!(state["next_run_at"], clock.load(Ordering::SeqCst) + 86400);
    assert_eq!(state["pruned_through"], 150);
    assert_eq!(restarted.latest_sequence().await.unwrap(), 150);
}

#[tokio::test]
async fn automatic_compaction_only_walks_legacy_records_once() {
    let (_dir, store, clock) = fixture().await;
    seed(&store, 150, clock.load(Ordering::SeqCst)).await;
    let mut compacted = 0;
    for _ in 0..150 {
        let report = store.auto_maintain_events().await.unwrap().unwrap();
        let count = report["compacted_events"].as_i64().unwrap();
        assert!((1..=64).contains(&count));
        compacted += count;
        if report["more"] == false {
            break;
        }
        clock.fetch_add(5, Ordering::SeqCst);
    }
    assert_eq!(compacted, 150);
    clock.fetch_add(86400, Ordering::SeqCst);
    assert_eq!(
        store.auto_maintain_events().await.unwrap().unwrap()["compacted_events"],
        0
    );
}

#[tokio::test]
async fn disabled_and_not_due_maintenance_does_not_touch_events() {
    let (_dir, store, _) = fixture().await;
    seed(&store, 1, 1).await;
    store
        .event_policy(Some(false), Some(7), Some(3600))
        .await
        .unwrap();
    assert!(store.auto_maintain_events().await.unwrap().is_none());
    assert_eq!(
        store.event_policy(None, None, None).await.unwrap()["runs"],
        0
    );
    assert!(store.event_policy(None, Some(0), None).await.is_err());
    assert!(store.event_policy(None, None, Some(1)).await.is_err());
    store.event_policy(Some(true), None, None).await.unwrap();
    assert_eq!(
        store.auto_maintain_events().await.unwrap().unwrap()["deleted_events"],
        1
    );
}

#[tokio::test]
async fn failed_maintenance_rolls_back_and_does_not_fail_committed_user_write() {
    let (dir, store, _) = fixture().await;
    store.set_background_maintenance(false);
    seed(&store, 3, 1).await;
    sqlx::query("CREATE TRIGGER fail_cleanup BEFORE DELETE ON task_events WHEN OLD.sequence=2 BEGIN SELECT RAISE(ABORT,'cleanup failed'); END").execute(&store.pool).await.unwrap();
    let outcome = store
        .execute(
            json!({"command":"project.register","root":dir.path(),"name":"Committed"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(outcome.result["id"].is_string());
    assert!(store.auto_maintain_events().await.is_err());
    assert_eq!(
        store.event_policy(None, None, None).await.unwrap()["runs"],
        0
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_events WHERE sequence<=3")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(count, 3);
    sqlx::query("DROP TRIGGER fail_cleanup")
        .execute(&store.pool)
        .await
        .unwrap();
    assert_eq!(
        store.auto_maintain_events().await.unwrap().unwrap()["deleted_events"],
        3
    );
}

#[tokio::test]
async fn retention_uses_age_index_without_a_full_event_scan() {
    let (_dir, store, _) = fixture().await;
    let rows = sqlx::query(&format!("EXPLAIN QUERY PLAN {EXPIRED}"))
        .bind(100)
        .fetch_all(&store.pool)
        .await
        .unwrap();
    let details: Vec<String> = rows.iter().map(|r| r.get("detail")).collect();
    assert!(
        details
            .iter()
            .any(|s| s.contains("SEARCH") && s.contains("events_by_age")),
        "{details:?}"
    );
    assert!(
        details.iter().all(|s| !s.contains("TEMP B-TREE")),
        "{details:?}"
    );
}

#[tokio::test]
#[ignore = "manual local performance measurement"]
async fn retention_check_and_batch_latency() {
    let (_dir, store, clock) = fixture().await;
    seed(&store, 2000, 1).await;
    let begin = Instant::now();
    let report = store.auto_maintain_events().await.unwrap().unwrap();
    let batch = begin.elapsed();
    let begin = Instant::now();
    for _ in 0..500 {
        assert!(store.auto_maintain_events().await.unwrap().is_none());
    }
    eprintln!(
        "retention batch: {batch:?}, report: {report}; not-due check mean: {:?}",
        begin.elapsed() / 500
    );
    clock.fetch_add(5, Ordering::SeqCst);
}

#[tokio::test]
async fn expired_backlog_is_deleted_before_compacting_surviving_history() {
    let (_dir, store, _) = fixture().await;
    seed(&store, 150, 1).await;
    let report = store.auto_maintain_events().await.unwrap().unwrap();
    assert_eq!(report["deleted_events"], 64);
    assert_eq!(
        report["compacted_events"], 0,
        "do not rewrite snapshots already queued for expiry deletion"
    );
}

#[tokio::test]
async fn automatic_batch_limits_legacy_bytes_and_reclaims_pages() {
    let (_dir, store, clock) = fixture().await;
    let mode: i64 = sqlx::query_scalar("PRAGMA auto_vacuum")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(
        mode, 2,
        "databases must support incremental space reclamation"
    );
    seed(&store, 20, clock.load(Ordering::SeqCst)).await;
    sqlx::query("UPDATE task_events SET data=json_set(data,'$.payload.conversation',?)")
        .bind("x".repeat(1024 * 1024))
        .execute(&store.pool)
        .await
        .unwrap();
    let report = store.auto_maintain_events().await.unwrap().unwrap();
    assert!(
        report["compacted_events"].as_i64().unwrap() <= 2,
        "legacy batches must bound bytes, not only row count"
    );
}

#[tokio::test]
async fn maintenance_does_not_wait_for_a_business_write_lock() {
    let (_dir, store, _) = fixture().await;
    seed(&store, 1, 1).await;
    let tx = store.pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let result =
        tokio::time::timeout(Duration::from_millis(200), store.auto_maintain_events()).await;
    assert!(
        result.is_ok(),
        "maintenance must yield immediately to foreground writers"
    );
    assert!(result.unwrap().unwrap().is_none());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn worker_reclaims_space_and_preserves_current_entities() {
    let (_dir, store, _) = fixture().await;
    seed(&store, 300, 1).await;
    let before: i64 = sqlx::query_scalar("PRAGMA page_count")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    let current = store.snapshot().await.unwrap();
    store.run_event_maintenance().await.unwrap();
    let after: i64 = sqlx::query_scalar("PRAGMA page_count")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert!(
        after < before,
        "incremental maintenance must shrink the database"
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_events")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        serde_json::to_value(current).unwrap(),
        serde_json::to_value(store.snapshot().await.unwrap()).unwrap()
    );
}

#[test]
fn summaries_are_versioned_bounded_and_idempotent() {
    let payload = json!({"id":"task_test","title":"界".repeat(10000),"reason":"reason","conversation":["secret"],"future_business_field":"not an event field"});
    let summary = compact_payload(&payload);
    assert_eq!(summary["schema_version"], 1);
    assert_eq!(summary["entity_type"], "task");
    assert!(summary["title"].as_str().unwrap().len() <= 4096);
    assert_eq!(summary["truncated_fields"], json!(["title"]));
    assert!(summary.get("conversation").is_none());
    assert_eq!(compact_payload(&summary), summary);
}

#[tokio::test]
async fn committed_write_does_not_execute_cleanup_inline() {
    let (dir, store, _) = fixture().await;
    seed(&store, 3, 1).await;
    let lock = store.try_maintenance_lock().unwrap().unwrap();
    let result = store
        .execute(
            json!({"command":"project.register","root":dir.path(),"name":"Foreground"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.result["id"].is_string());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_events WHERE sequence<=3")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(
        count, 3,
        "a committed write must delegate to the worker lock rather than run inline maintenance"
    );
    drop(lock);
    store.run_event_maintenance().await.unwrap();
}
