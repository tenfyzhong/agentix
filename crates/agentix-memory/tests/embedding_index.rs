#[path = "support/http.rs"]
mod http;
use agentix_memory::{
    Actor, EmbeddingConfig, EmbeddingIndex, HttpEmbedding, HttpProvider, MemoryInput, MemoryStore,
    ProviderConfig, SemanticRetrieval,
};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn background_embedding_discovers_dimensions_and_query_failure_falls_back_to_fts() {
    let server = http::MockHttp::start(vec![
        (200, json!({"data":[{"index":0,"embedding":[1.0,0.0]}]})),
        (503, json!({"error":"unavailable"})),
    ])
    .await;
    let config = EmbeddingConfig {
        enabled: true,
        dimensions: None,
        ..EmbeddingConfig::default()
    };
    let provider = Arc::new(
        HttpProvider::new(ProviderConfig {
            base_url: server.url.clone(),
            api_key_env: None,
            protocol: agentix_memory::ProviderProtocol::Openai,
            max_in_flight: 4,
        })
        .unwrap(),
    );
    let embedding = Arc::new(HttpEmbedding::new(provider, config.clone()).unwrap());
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let input:MemoryInput=serde_json::from_value(json!({"title":"外部离线约束","conclusion":"需要离线可用","rationale":"用户要求","scope":"project","tags":[],"kind":"user_decision","evidence":[]})).unwrap();
    let memory = store.create("project", input, Actor::Human).await.unwrap();
    let index = EmbeddingIndex::new(store.clone(), embedding.clone(), config);
    let generation = index.activate("project").await.unwrap();
    assert_eq!(index.step("project", generation).await.unwrap(), 1);
    assert_eq!(index.step("project", generation).await.unwrap(), 0);
    assert!(
        store
            .embedding_pending("project", generation, "", 10)
            .await
            .unwrap()
            .is_empty()
    );
    let retrieval = SemanticRetrieval::new(store.clone(), Some(embedding), 1500);
    let result = retrieval.search("project", "离线", 5).await.unwrap();
    assert_eq!(result.mode, "fts_fallback");
    assert_eq!(result.memories[0].id, memory.id);
    assert_eq!(server.requests.lock().unwrap().len(), 2);
    let next = store
        .reserve_embedding("project", "different-model", None)
        .await
        .unwrap();
    assert!(next > generation);
    assert_eq!(index.step("project", generation).await.unwrap(), 0);
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn query_cache_reuses_vectors_but_not_memory_results_and_fences_profile_changes() {
    let server = http::MockHttp::start(vec![
        (200, json!({"data":[{"index":0,"embedding":[1.0,0.0]}]})),
        (200, json!({"data":[{"index":0,"embedding":[1.0,0.0]}]})),
        (200, json!({"data":[{"index":0,"embedding":[1.0,0.0]}]})),
    ])
    .await;
    let config = EmbeddingConfig {
        enabled: true,
        dimensions: Some(2),
        ..EmbeddingConfig::default()
    };
    let provider = Arc::new(
        HttpProvider::new(ProviderConfig {
            base_url: server.url.clone(),
            api_key_env: None,
            protocol: agentix_memory::ProviderProtocol::Openai,
            max_in_flight: 4,
        })
        .unwrap(),
    );
    let embedding = Arc::new(HttpEmbedding::new(provider, config.clone()).unwrap());
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let input: MemoryInput = serde_json::from_value(json!({"title":"External offline constraint","conclusion":"Offline required","rationale":"Customer decision","scope":"project","kind":"user_decision","tags":[],"evidence":[]})).unwrap();
    let memory = store.create("project", input, Actor::Human).await.unwrap();
    let index = EmbeddingIndex::new(store.clone(), embedding.clone(), config);
    let generation = index.activate("project").await.unwrap();
    index.step("project", generation).await.unwrap();
    let retrieval = SemanticRetrieval::new(store.clone(), Some(embedding), 1500);
    assert_eq!(
        retrieval
            .search("project", "offline", 5)
            .await
            .unwrap()
            .memories
            .len(),
        1
    );
    store
        .set_status(
            "project",
            &memory.id,
            memory.revision,
            agentix_memory::Status::Forgotten,
            "User request",
            Actor::Human,
        )
        .await
        .unwrap();
    assert!(
        retrieval
            .search("project", "offline", 5)
            .await
            .unwrap()
            .memories
            .is_empty()
    );
    assert_eq!(
        server.requests.lock().unwrap().len(),
        2,
        "repeated query should reuse its vector"
    );
    store
        .reserve_embedding("project", "different", Some(2))
        .await
        .unwrap();
    index.activate("project").await.unwrap();
    retrieval.search("project", "offline", 5).await.unwrap();
    assert_eq!(
        server.requests.lock().unwrap().len(),
        3,
        "generation change must invalidate cached vectors"
    );
}
