use agentix_memory::{Actor, MemoryInput, MemoryStore, Source};
use serde_json::json;

#[tokio::test]
async fn derived_cleanup_retains_sources_versions_current_vectors_and_audited_work() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let input:MemoryInput=serde_json::from_value(json!({"title":"offline","conclusion":"constraint","rationale":"user","scope":"project","tags":[],"kind":"user_decision","evidence":[]})).unwrap();
    let memory = store.create("p", input, Actor::Human).await.unwrap();
    for generation in 1..=2 {
        assert_eq!(
            store
                .configure_embedding("p", &format!("model{generation}"), 2)
                .await
                .unwrap(),
            generation
        );
        store
            .put_embedding("p", &memory.id, 1, generation, &[1.0, 0.0])
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO embedding_failures VALUES(?,1,1,1,0,'old generation')")
        .bind(&memory.id)
        .execute(&pool)
        .await
        .unwrap();
    for i in 0..3 {
        let source:Source=serde_json::from_value(json!({"instance_id":"db","receipt_id":format!("r{i}"),"sequence":i+1,"project_id":"p","session_id":"s","turn_id":format!("t{i}"),"revision":1,"job_id":null,"recorded_at":1,"messages":[{"id":"m","role":"user","text":"External constraint"}]})).unwrap();
        store.ingest(&source).await.unwrap();
    }
    sqlx::query("UPDATE work_items SET state='cancelled'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO work_audits SELECT min(id),0,'{}' FROM work_items")
        .execute(&pool)
        .await
        .unwrap();
    store.cleanup_derived().await.unwrap();
    assert_eq!(
        store.work_counts().await.unwrap().cancelled,
        3,
        "first observation starts retention"
    );
    let vectors: i64 = sqlx::query_scalar("SELECT count(*) FROM memory_vectors WHERE generation=2")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(vectors, 1);
    let obsolete:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM memory_vectors WHERE generation=1)+(SELECT count(*) FROM embedding_failures)").fetch_one(&pool).await.unwrap();
    assert_eq!(obsolete, 0);
    sqlx::query("UPDATE work_retention SET observed_at=0")
        .execute(&pool)
        .await
        .unwrap();
    drop(store);
    let store = MemoryStore::open(&path).await.unwrap();
    store.cleanup_derived().await.unwrap();
    assert_eq!(
        store.work_counts().await.unwrap().cancelled,
        1,
        "audited cancellation must survive"
    );
    let sources: i64 = sqlx::query_scalar("SELECT count(*) FROM sources")
        .fetch_one(&pool)
        .await
        .unwrap();
    let versions: i64 = sqlx::query_scalar("SELECT count(*) FROM memory_versions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((sources, versions), (3, 1));
    assert_eq!(store.show("p", &memory.id, None).await.unwrap().revision, 1);
}

#[tokio::test]
async fn derived_cleanup_limits_each_pass_and_continues_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let input:MemoryInput=serde_json::from_value(json!({"title":"offline","conclusion":"constraint","rationale":"user","scope":"project","tags":[],"kind":"user_decision","evidence":[]})).unwrap();
    let memory = store.create("p", input, Actor::Human).await.unwrap();
    store.configure_embedding("p", "model", 2).await.unwrap();
    sqlx::query("UPDATE embedding_profiles SET generation=1003")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<1003) INSERT INTO memory_vectors SELECT ?,'p',x,1,zeroblob(16) FROM n").bind(&memory.id).execute(&pool).await.unwrap();
    store.cleanup_derived().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM memory_vectors")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 3);
    drop(store);
    let store = MemoryStore::open(&path).await.unwrap();
    store.cleanup_derived().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM memory_vectors")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn retention_reference_checks_use_a_work_id_index() {
    use sqlx::Row;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let _store = MemoryStore::open(&path).await.unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let rows = sqlx::query("EXPLAIN QUERY PLAN SELECT 1 FROM memory_reviews WHERE work_id=?")
        .bind(1_i64)
        .fetch_all(&pool)
        .await
        .unwrap();
    let details = rows
        .iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        details.contains("SEARCH") && details.contains("work_id=?"),
        "{details}"
    );
}
