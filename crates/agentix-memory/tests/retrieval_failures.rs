// Unix-only memory service regression tests.
#![cfg(any(unix, windows))]
#[path = "support/http.rs"]
mod http;

use agentix_memory::*;
use serde_json::json;
use std::sync::Arc;

fn content(title: &str) -> MemoryInput {
    serde_json::from_value(json!({"title":title,"conclusion":"External constraint","rationale":"Customer decision","scope":"project","tags":[],"kind":"user_decision","evidence":[]})).unwrap()
}

#[tokio::test]
async fn failed_batch_isolates_items_and_persists_bounded_retry_state() {
    let server = http::MockHttp::start(vec![
        (400, json!({"error":"bad batch"})),
        (400, json!({"error":"bad document"})),
        (200, json!({"data":[{"index":0,"embedding":[1.0,0.0]}]})),
        (400, json!({"error":"bad document"})),
        (400, json!({"error":"bad document"})),
    ])
    .await;
    let config = EmbeddingConfig {
        enabled: true,
        batch_size: 2,
        dimensions: Some(2),
        ..EmbeddingConfig::default()
    };
    let provider = Arc::new(
        HttpProvider::new(ProviderConfig {
            base_url: server.url.clone(),
            api_key_env: None,
            protocol: ProviderProtocol::Openai,
            max_in_flight: 4,
        })
        .unwrap(),
    );
    let embedding = Arc::new(HttpEmbedding::new(provider, config.clone()).unwrap());
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    let bad = store
        .create("p", content("Bad document"), Actor::Human)
        .await
        .unwrap();
    let good = store
        .create("p", content("Good document"), Actor::Human)
        .await
        .unwrap();
    let index = EmbeddingIndex::new(store.clone(), embedding.clone(), config.clone());
    let generation = index.activate("p").await.unwrap();
    assert!(index.step("p", generation).await.is_err());
    assert!(
        store
            .embedding_pending("p", generation, "", 10)
            .await
            .unwrap()
            .is_empty(),
        "failed item should back off; good item should already be indexed"
    );
    assert_eq!(server.requests.lock().unwrap().len(), 3);
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    // Keep the persisted deadline in the future even on a slow CI runner. Later
    // retry checks explicitly expire it, so this does not change their assertions.
    sqlx::query("UPDATE embedding_failures SET available_at=available_at+3600")
        .execute(&pool)
        .await
        .unwrap();
    drop(index);
    drop(store);
    let store = MemoryStore::open(&path).await.unwrap();
    let index = EmbeddingIndex::new(store.clone(), embedding, config);
    assert_eq!(
        index.step("p", generation).await.unwrap(),
        0,
        "backoff survives reopen"
    );
    for _ in 0..2 {
        sqlx::query("UPDATE embedding_failures SET available_at=0")
            .execute(&pool)
            .await
            .unwrap();
        assert!(index.step("p", generation).await.is_err());
    }
    sqlx::query("UPDATE embedding_failures SET available_at=0")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        index.step("p", generation).await.unwrap(),
        0,
        "third failure requires explicit retry"
    );
    assert_eq!(server.requests.lock().unwrap().len(), 5);
    let status = store.memory_status(Some("p")).await.unwrap();
    assert_eq!(status["embedding_failures"][0]["attempts"], 3);
    assert_eq!(status["indexed"], 1);
    store.reindex_fts("other", "", 100).await.unwrap();
    assert!(
        store
            .embedding_pending("p", generation, "", 10)
            .await
            .unwrap()
            .is_empty()
    );
    store.reindex_fts("p", "", 100).await.unwrap();
    let pending = store
        .embedding_pending("p", generation, "", 10)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, bad.id);
    assert_ne!(pending[0].id, good.id);
}

#[tokio::test]
async fn context_accepts_prompt_within_host_character_budget() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    store
        .create("p", content("t000"), Actor::Human)
        .await
        .unwrap();
    let api = MemoryApi::new(store, RetrievalConfig::default(), ServiceConfig::default());
    let query = (0..140)
        .map(|i| format!("t{i:03}"))
        .collect::<Vec<_>>()
        .join(" ");
    // The host passes the first 1000 characters unchanged.
    let query: String = query.chars().take(1000).collect();
    let result = api
        .handle(json!({"op":"context","project":"p","session":"s","turn":"t","query":query}))
        .await;
    assert!(
        result.is_ok(),
        "host-sized prompt must not disable memory: {result:?}"
    );
    assert_eq!(result.unwrap()["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn provider_outage_does_not_fan_out_and_new_revision_or_generation_can_retry() {
    let server = http::MockHttp::start(vec![(503, json!({"error":"unavailable"}))]).await;
    let config = EmbeddingConfig {
        enabled: true,
        batch_size: 2,
        dimensions: Some(2),
        ..EmbeddingConfig::default()
    };
    let provider = Arc::new(
        HttpProvider::new(ProviderConfig {
            base_url: server.url.clone(),
            api_key_env: None,
            protocol: ProviderProtocol::Openai,
            max_in_flight: 4,
        })
        .unwrap(),
    );
    let embedding = Arc::new(HttpEmbedding::new(provider, config.clone()).unwrap());
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let a = store
        .create("p", content("First"), Actor::Human)
        .await
        .unwrap();
    store
        .create("p", content("Second"), Actor::Human)
        .await
        .unwrap();
    let index = EmbeddingIndex::new(store.clone(), embedding, config);
    let generation = index.activate("p").await.unwrap();
    assert!(index.step("p", generation).await.is_err());
    assert_eq!(server.requests.lock().unwrap().len(), 1);
    assert!(
        store
            .embedding_pending("p", generation, "", 10)
            .await
            .unwrap()
            .is_empty()
    );
    store
        .update("p", &a.id, a.revision, content("Corrected"), Actor::Human)
        .await
        .unwrap();
    let pending = store
        .embedding_pending("p", generation, "", 10)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].revision, 2);
    let next = store
        .configure_embedding("p", "new model", 2)
        .await
        .unwrap();
    assert_eq!(
        store
            .embedding_pending("p", next, "", 10)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        store.memory_status(Some("p")).await.unwrap()["embedding_failures"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn permanent_embedding_failure_does_not_starve_later_memories() {
    let server = http::MockHttp::start(vec![
        (400, json!({"error":"input too long"})),
        (400, json!({"error":"input too long"})),
    ])
    .await;
    let config = EmbeddingConfig {
        enabled: true,
        batch_size: 1,
        dimensions: Some(2),
        ..EmbeddingConfig::default()
    };
    let provider = Arc::new(
        HttpProvider::new(ProviderConfig {
            base_url: server.url.clone(),
            api_key_env: None,
            protocol: ProviderProtocol::Openai,
            max_in_flight: 4,
        })
        .unwrap(),
    );
    let embedding = Arc::new(HttpEmbedding::new(provider, config.clone()).unwrap());
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    store
        .create("p", content("Rejected document"), Actor::Human)
        .await
        .unwrap();
    store
        .create("p", content("Valid later document"), Actor::Human)
        .await
        .unwrap();
    let index = EmbeddingIndex::new(store, embedding, config);
    let generation = index.activate("p").await.unwrap();
    assert!(index.step("p", generation).await.is_err());
    let _ = index.step("p", generation).await;
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_ne!(
        requests[0].1["input"], requests[1].1["input"],
        "a permanently rejected document must not block all later documents"
    );
}
