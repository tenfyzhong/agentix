// Local memory service integration tests.
#![cfg(any(unix, windows))]
use agentix_memory::{MemoryApi, MemoryStore, RequestHandler, RetrievalConfig, ServiceConfig};
use serde_json::json;

#[tokio::test]
async fn offline_status_accepts_legacy_database_without_embedding_failures() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    drop(store);
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    sqlx::query("DROP TABLE embedding_failures")
        .execute(&pool)
        .await
        .unwrap();
    let old = MemoryStore::open_read_only(&path).await.unwrap();
    assert!(old.memory_status(None).await.is_ok());
}
#[tokio::test]
async fn api_supports_scoped_reads_and_revision_guarded_writes_without_model_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let api = MemoryApi::new(store, RetrievalConfig::default(), ServiceConfig::default());
    let content = json!({"title":"外部约束","conclusion":"离线可用","rationale":"用户要求","scope":"project","tags":[],"kind":"user_decision","evidence":[]});
    let memory = api
        .handle(json!({"op":"create","project":"a","content":content,"actor":"human"}))
        .await
        .unwrap();
    let id = memory["id"].as_str().unwrap();
    let found = api
        .handle(json!({"op":"search","project":"a","query":"离线","limit":5}))
        .await
        .unwrap();
    assert_eq!(found["memories"][0]["id"], id);
    assert!(
        api.handle(json!({"op":"show","project":"b","id":id}))
            .await
            .is_err()
    );
    assert!(api.handle(json!({"op":"update","project":"a","id":id,"revision":0,"content":content,"actor":"human"})).await.is_err());
    api.handle(json!({"op":"set_status","project":"a","id":id,"revision":1,"status":"forgotten","reason":"Explicit user request","actor":"human"})).await.unwrap();
    let found = api
        .handle(json!({"op":"search","project":"a","query":"离线","limit":5}))
        .await
        .unwrap();
    assert!(found["memories"].as_array().unwrap().is_empty());
    assert!(
        api.handle(json!({"op":"ask","project":"a","query":"why?"}))
            .await
            .is_err()
    );
}

struct NoRepository;
#[async_trait::async_trait]
impl agentix_memory::ProjectRepository for NoRepository {
    async fn root(&self, _project: &str) -> anyhow::Result<Option<std::path::PathBuf>> {
        Ok(None)
    }
}
struct AnswerModel(String);
#[async_trait::async_trait]
impl agentix_memory::Model for AnswerModel {
    async fn complete(
        &self,
        request: &agentix_memory::ModelRequest,
    ) -> anyhow::Result<agentix_memory::ModelReply> {
        let call = if request.history.len() == 1 {
            agentix_memory::ToolCall {
                id: "search".into(),
                name: "memory_search".into(),
                arguments: json!({"query":"离线"}),
            }
        } else {
            agentix_memory::ToolCall {
                id: "finish".into(),
                name: "submit_answer".into(),
                arguments: json!({"answer":"用户要求离线可用","insufficient_evidence":false,"memories":[{"id":self.0,"revision":1}],"sources":[]}),
            }
        };
        Ok(agentix_memory::ModelReply {
            continuation: json!([]),
            calls: vec![call],
            text: String::new(),
            usage: agentix_memory::TokenUsage::default(),
        })
    }
}
#[tokio::test]
async fn deep_queries_are_read_only_and_reject_fabricated_citations() {
    use std::sync::Arc;
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let content=serde_json::from_value(json!({"title":"离线约束","conclusion":"离线可用","rationale":"用户要求","scope":"project","tags":[],"kind":"user_decision","evidence":[]})).unwrap();
    let memory = store
        .create("a", content, agentix_memory::Actor::Human)
        .await
        .unwrap();
    for (id, valid) in [(memory.id.clone(), true), ("fabricated".into(), false)] {
        let deep = agentix_memory::DeepQuery::new(
            store.clone(),
            Arc::new(AnswerModel(id)),
            agentix_memory::AgentConfig::default(),
            Arc::new(NoRepository),
            1,
        );
        let api = MemoryApi::new(
            store.clone(),
            RetrievalConfig::default(),
            ServiceConfig::default(),
        )
        .with_deep_query(deep);
        let answer = api
            .handle(json!({"op":"ask","project":"a","query":"为什么离线？"}))
            .await;
        assert_eq!(answer.is_ok(), valid);
    }
    assert_eq!(store.show("a", &memory.id, None).await.unwrap(), memory);
    assert_eq!(store.work_counts().await.unwrap().pending, 0);
}

#[tokio::test]
async fn context_retry_skips_retrieval_and_revalidates_even_empty_receipts() {
    use agentix_memory::{Actor, MemoryInput};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    let input: MemoryInput = serde_json::from_value(json!({"title":"constraint","conclusion":"offline","rationale":"user","scope":"project","tags":[],"kind":"user_decision","evidence":[]})).unwrap();
    let memory = store.create("p", input, Actor::Human).await.unwrap();
    let api = MemoryApi::new(
        store.clone(),
        RetrievalConfig::default(),
        ServiceConfig::default(),
    );
    let request = json!({"op":"context","project":"p","session":"s","turn":"t","query":"offline"});
    let first = api.handle(request.clone()).await.unwrap();
    assert_eq!(first["items"][0]["id"], memory.id);
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    // A retry must not touch lexical retrieval, even after the embedding cache expires.
    sqlx::query("DROP TABLE memory_fts")
        .execute(&pool)
        .await
        .unwrap();
    let repeat = api.handle(request.clone()).await.unwrap();
    assert_eq!(repeat["text"], first["text"]);
    // Expiry can change visibility without needing an FTS write.
    sqlx::query("UPDATE memories SET data=json_set(data,'$.content.valid_until',0) WHERE id=?")
        .bind(&memory.id)
        .execute(&pool)
        .await
        .unwrap();
    let expired = api.handle(request.clone()).await.unwrap();
    assert!(expired["items"].as_array().unwrap().is_empty());
    assert!(
        api.handle(request).await.unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        api.handle(
            json!({"op":"context","project":"other","session":"s","turn":"t","query":"offline"})
        )
        .await
        .is_err()
    );
}
