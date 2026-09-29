use agentix_memory::{Actor, MemoryInput, MemoryStore};
use serde_json::json;
#[tokio::test]
async fn context_preparation_is_bounded_scoped_and_does_not_imply_delivery() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let input:MemoryInput=serde_json::from_value(json!({"title":"离线约束","conclusion":"必须离线可用","rationale":"外部要求","scope":"project","tags":[],"kind":"user_decision","evidence":[]})).unwrap();
    let a = store
        .create("a", input.clone(), Actor::Human)
        .await
        .unwrap();
    let b = store
        .create("b", input.clone(), Actor::Human)
        .await
        .unwrap();
    // Upgrade must also ignore old generation-time delivery markers.
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(temp.path().join("memory.db")),
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO context_deliveries(project_id,session_id,memory_id,revision,updated_at) VALUES('a','session',?,1,unixepoch())")
        .bind(&a.id).execute(&pool).await.unwrap();
    let first = store
        .context("a", "session", "turn1", vec![a.clone(), b], 1024)
        .await
        .unwrap();
    assert!(first.text.len() <= 1024);
    assert_eq!(first.items.len(), 1);
    let repeat = store
        .context("a", "session", "turn1", vec![a.clone()], 1024)
        .await
        .unwrap();
    assert_eq!(first.text, repeat.text);
    let next = store
        .context("a", "session", "turn2", vec![a.clone()], 1024)
        .await
        .unwrap();
    assert_eq!(
        next.items[0].id, a.id,
        "the host, not packet generation, confirms delivery"
    );
    let updated = store
        .update("a", &a.id, 1, input, Actor::Human)
        .await
        .unwrap();
    let changed = store
        .context("a", "session", "turn3", vec![updated], 1024)
        .await
        .unwrap();
    assert_eq!(changed.items[0].revision, 2);
    let other = store
        .context("a", "other", "turn1", vec![a], 1024)
        .await
        .unwrap();
    assert!(
        other.items.is_empty(),
        "stale retrieval revisions must be revalidated before injection"
    );
}

#[tokio::test]
async fn unchanged_context_does_not_rewrite_receipt_or_clean_history() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    store.context("p", "s", "t", vec![], 1024).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_receipt_update BEFORE UPDATE ON context_receipts BEGIN SELECT RAISE(ABORT,'unexpected receipt write'); END").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO context_receipts VALUES('old','s','t','{}',0)")
        .execute(&pool)
        .await
        .unwrap();
    store.context("p", "s", "t", vec![], 1024).await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM context_receipts WHERE project_id='old'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "cleanup belongs to background maintenance");
}

#[tokio::test]
async fn context_cleanup_is_bounded_and_preserves_recent_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    sqlx::query("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<1001) INSERT INTO context_receipts SELECT 'p','s',CAST(x AS TEXT),'{}',0 FROM n").execute(&pool).await.unwrap();
    store
        .context("p", "s", "recent", vec![], 1024)
        .await
        .unwrap();
    store.cleanup_context().await.unwrap();
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM context_receipts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 2);
    store.cleanup_context().await.unwrap();
    assert!(
        store
            .cached_context("p", "s", "recent", 1024)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .cached_context("p", "s", "missing", 1024)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn committed_memory_changes_notify_cloned_store_subscribers() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let mut changes = store.subscribe_changes();
    let input:MemoryInput=serde_json::from_value(json!({"title":"constraint","conclusion":"offline","rationale":"user","scope":"project","tags":[],"kind":"user_decision","evidence":[]})).unwrap();
    let memory = store
        .clone()
        .create("p", input.clone(), Actor::Human)
        .await
        .unwrap();
    assert_eq!(changes.try_recv().unwrap(), "p");
    assert!(store.show("p", &memory.id, None).await.is_ok());
    store
        .update("p", &memory.id, 1, input, Actor::Human)
        .await
        .unwrap();
    assert_eq!(changes.try_recv().unwrap(), "p");
}

#[tokio::test]
async fn cached_packets_preserve_order_and_revalidate_revisions_and_forgetting() {
    use agentix_memory::Status;
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let input:MemoryInput=serde_json::from_value(json!({"title":"constraint","conclusion":"offline","rationale":"user","scope":"project","tags":[],"kind":"user_decision","evidence":[]})).unwrap();
    let a = store
        .create("p", input.clone(), Actor::Human)
        .await
        .unwrap();
    let b = store
        .create("p", input.clone(), Actor::Human)
        .await
        .unwrap();
    let first = store
        .context("p", "s", "t", vec![b.clone(), a.clone()], 4096)
        .await
        .unwrap();
    assert_eq!(
        first.items.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        vec![b.id.clone(), a.id.clone()]
    );
    store
        .update("p", &a.id, 1, input, Actor::Human)
        .await
        .unwrap();
    let packet = store
        .cached_context("p", "s", "t", 4096)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(packet.items.len(), 1);
    assert_eq!(packet.items[0].id, b.id);
    store
        .set_status(
            "p",
            &b.id,
            1,
            Status::Forgotten,
            "user request",
            Actor::Human,
        )
        .await
        .unwrap();
    assert!(
        store
            .cached_context("p", "s", "t", 4096)
            .await
            .unwrap()
            .unwrap()
            .items
            .is_empty()
    );
    assert!(
        store
            .cached_context("q", "s", "t", 4096)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn context_cache_miss_does_not_wait_for_an_unrelated_writer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let writer = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        store.cached_context("p", "s", "missing", 1024),
    )
    .await;
    writer.rollback().await.unwrap();
    assert!(
        result
            .expect("a cache miss must use a read-only probe")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .cached_context("p", "", "missing", 1024)
            .await
            .is_err()
    );
    assert!(store.cached_context("p", "s", "missing", 1).await.is_err());
}
