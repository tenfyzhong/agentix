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
