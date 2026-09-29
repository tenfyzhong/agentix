use agentix_memory::{MemoryStore, ProjectTools, Source, ToolSet};
use serde_json::json;

#[tokio::test]
async fn conversation_index_migrates_and_retains_first_order_after_late_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    for (seq, turn, revision) in [(1, "a", 1), (2, "b", 1), (3, "a", 2)] {
        let source: Source = serde_json::from_value(json!({"instance_id":"db","receipt_id":format!("r{seq}"),"sequence":seq,"project_id":"p","session_id":"s","turn_id":turn,"revision":revision,"job_id":null,"recorded_at":seq,"messages":[{"id":"m","role":"user","text":"Decision"}]})).unwrap();
        store.ingest(&source).await.unwrap();
    }
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table'")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(
        columns.iter().any(|t| t == "source_turns"),
        "indexed turn metadata must be persisted"
    );
    sqlx::query("DROP TABLE source_turns")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM memory_metadata WHERE key='source_turn_index_version'")
        .execute(&pool)
        .await
        .unwrap();
    drop(store);
    let store = MemoryStore::open(&path).await.unwrap();
    let tools = ProjectTools::new(store, "p".into(), None).unwrap();
    let result = tools
        .execute("source_neighbors", json!({"receipt_id":"r2"}))
        .await
        .unwrap();
    assert_eq!(result["sources"][0]["receipt_id"], "r3");
    assert_eq!(result["sources"][0]["message_count"], 1);
    let first: i64 =
        sqlx::query_scalar("SELECT first_sequence FROM source_turns WHERE turn_id='a'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(first, 1);
    let plan: Vec<(i64,i64,i64,String)> = sqlx::query_as("EXPLAIN QUERY PLAN SELECT receipt_id FROM source_turns WHERE project_id='p' AND instance_id='db' AND session_id='s' AND first_sequence<2 ORDER BY first_sequence DESC LIMIT 8").fetch_all(&pool).await.unwrap();
    assert!(plan.iter().any(|r| r.3.contains("source_turns_by_order")));
    assert!(!plan.iter().any(|r| r.3.contains("TEMP B-TREE")));
}
