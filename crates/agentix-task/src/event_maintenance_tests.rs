use crate::{Store, WriteOptions};
use serde_json::json;
use std::sync::Arc;

async fn fixture() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_with_clock(&dir.path().join("tasks.sqlite3"), Arc::new(|| 100 * 86400))
        .await
        .unwrap();
    for (id, stamp) in [(1, 1), (2, 1), (3, 100 * 86400)] {
        let data = json!({"sequence":0,"event_id":format!("event_{id}"),"project_id":"p", "job_id":null,"task_id":null,"actor_ref":"user:test","session_ref":null,"delegated_by":null,"event_type":"job.pending_review","revision":id,"occurred_at":stamp,"payload":{"id":"j","title":"Notice","review_reason":"Keep reason","conversation":[{"text":"x".repeat(10000)}],"prompt":"sensitive prompt"}});
        sqlx::query("INSERT INTO task_events(event_id,data) VALUES (?,?)")
            .bind(format!("event_{id}"))
            .bind(data.to_string())
            .execute(&store.pool)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE event_retention SET compact_through=3 WHERE id=1")
        .execute(&store.pool)
        .await
        .unwrap();
    (dir, store)
}

#[tokio::test]
async fn maintenance_previews_without_writes_and_compacts_without_pruning() {
    let (_dir, store) = fixture().await;
    let before: Vec<String> = sqlx::query_scalar("SELECT data FROM task_events ORDER BY sequence")
        .fetch_all(&store.pool)
        .await
        .unwrap();
    let preview = store.maintain_events(30, false, false).await.unwrap();
    assert_eq!(preview["compacted_events"], 3);
    assert_eq!(preview["deleted_events"], 0);
    assert!(preview["reclaimed_payload_bytes"].as_u64().unwrap() > 30000);
    let after: Vec<String> = sqlx::query_scalar("SELECT data FROM task_events ORDER BY sequence")
        .fetch_all(&store.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    store.maintain_events(30, false, true).await.unwrap();
    let events = store.events(None, 0, 100).await.unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].payload["title"], "Notice");
    assert_eq!(events[0].payload["review_reason"], "Keep reason");
    assert!(events[0].payload.get("conversation").is_none());
    assert_eq!(
        store.maintain_events(30, false, true).await.unwrap()["compacted_events"],
        0
    );
}

#[tokio::test]
async fn migration_backfills_watermarks_without_rewriting_history() {
    let (dir, store) = fixture().await;
    sqlx::raw_sql(
        "DROP TRIGGER event_watermark_insert; DROP TABLE event_watermarks; PRAGMA user_version=14;",
    )
    .execute(&store.pool)
    .await
    .unwrap();
    store.pool.close().await;
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    assert_eq!(store.latest_sequence().await.unwrap(), 3);
    assert!(
        store.events(None, 0, 100).await.unwrap()[0]
            .payload
            .get("conversation")
            .is_some()
    );
    sqlx::query("DELETE FROM task_events")
        .execute(&store.pool)
        .await
        .unwrap();
    store.pool.close().await;
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    assert_eq!(store.latest_sequence().await.unwrap(), 3);
    let result = store
        .execute(
            json!({"command":"project.register","root":dir.path(),"name":"Later"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.sequence > 3);
}

#[tokio::test]
async fn pruning_keeps_the_retention_boundary_and_compaction_preserves_current_state() {
    let (dir, store) = fixture().await;
    store.event_policy(Some(false), None, None).await.unwrap();
    let created = store
        .execute(
            json!({"command":"project.register","root":dir.path(),"name":"Saved"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let before = store.snapshot().await.unwrap();
    sqlx::query("UPDATE task_events SET data=json_set(data,'$.occurred_at',?) WHERE sequence=2")
        .bind(70 * 86400)
        .execute(&store.pool)
        .await
        .unwrap();
    store.maintain_events(30, true, true).await.unwrap();
    assert!(
        store
            .events(None, 0, 100)
            .await
            .unwrap()
            .iter()
            .any(|event| event.sequence == 2)
    );
    assert_eq!(store.snapshot().await.unwrap(), before);
    store.vacuum_events().await.unwrap();
    assert_eq!(store.latest_sequence().await.unwrap(), created.sequence);
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
}

#[tokio::test]
async fn maintenance_rolls_back_partial_pruning_on_failure() {
    let (_dir, store) = fixture().await;
    sqlx::query("CREATE TRIGGER fail_pruning BEFORE DELETE ON task_events WHEN OLD.sequence=2 BEGIN SELECT RAISE(ABORT,'pruning failed'); END").execute(&store.pool).await.unwrap();
    assert!(store.maintain_events(30, true, true).await.is_err());
    let events = store.events(None, 0, 100).await.unwrap();
    assert_eq!(events.len(), 3);
    assert!(events[0].payload.get("conversation").is_some());
    assert_eq!(store.latest_sequence().await.unwrap(), 3);
}

#[tokio::test]
async fn normal_writes_prune_expired_events_without_acknowledgements() {
    let (dir, store) = fixture().await;
    store
        .execute(
            json!({"command":"project.register","root":dir.path(),"name":"Automatic"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while store.event_policy(None, None, None).await.unwrap()["runs"] == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let events = store.events(None, 0, 100).await.unwrap();
    assert!(
        events.iter().all(|event| event.sequence > 2),
        "expired history must not depend on an external consumer"
    );
    assert!(events.iter().any(|event| event.sequence == 3));
}

#[tokio::test]
async fn manual_pruning_does_not_require_a_consumer() {
    let (_dir, store) = fixture().await;
    assert_eq!(
        store.maintain_events(30, true, false).await.unwrap()["deleted_events"],
        2
    );
}

#[test]
fn non_object_payloads_are_explicitly_rejected_instead_of_retained_unbounded() {
    let result = super::compact_payload(&json!(["x".repeat(1024 * 1024)]));
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["unsupported_payload"], true);
    assert!(result.to_string().len() < 256);
}
