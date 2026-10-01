// Unix-only memory service regression tests.
#![cfg(any(unix, windows))]
#[path = "support/http.rs"]
mod http;
use agentix_memory::*;
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[tokio::test]
async fn identical_queries_share_work_and_short_waiter_does_not_cancel_long_waiter() {
    let reply = json!({"data":[{"index":0,"embedding":[1.0,0.0]}]});
    let server = http::MockHttp::start_delayed(
        vec![(200, reply.clone()), (200, reply)],
        Duration::from_millis(150),
    )
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
    let index = EmbeddingIndex::new(store.clone(), embedding.clone(), config);
    index.activate("p").await.unwrap();
    let retrieval = SemanticRetrieval::new(store, Some(embedding), 1000);
    let (short, long) = tokio::join!(
        retrieval.search_with_timeout("p", "offline", 5, 40),
        retrieval.search_with_timeout("p", "offline", 5, 1000)
    );
    assert_eq!(short.unwrap().mode, "fts_fallback");
    assert_eq!(long.unwrap().mode, "hybrid");
    assert_eq!(
        server.requests.lock().unwrap().len(),
        1,
        "overlapping queries must share the provider call"
    );
    assert_eq!(
        retrieval.search("p", "offline", 5).await.unwrap().mode,
        "hybrid"
    );
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn failed_queries_retry_and_project_generation_keys_are_isolated() {
    let reply = json!({"data":[{"index":0,"embedding":[1.0,0.0]}]});
    let server = http::MockHttp::start(vec![
        (400, json!({"error":"bad"})),
        (200, reply.clone()),
        (200, reply.clone()),
        (200, reply),
    ])
    .await;
    let config = EmbeddingConfig {
        enabled: true,
        dimensions: Some(2),
        ..EmbeddingConfig::default()
    };
    let embedding = Arc::new(
        HttpEmbedding::new(
            Arc::new(
                HttpProvider::new(ProviderConfig {
                    base_url: server.url.clone(),
                    api_key_env: None,
                    protocol: ProviderProtocol::Openai,
                    max_in_flight: 4,
                })
                .unwrap(),
            ),
            config.clone(),
        )
        .unwrap(),
    );
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let index = EmbeddingIndex::new(store.clone(), embedding.clone(), config);
    index.activate("p").await.unwrap();
    index.activate("q").await.unwrap();
    let retrieval = SemanticRetrieval::new(store.clone(), Some(embedding), 1000);
    assert_eq!(
        retrieval.search("p", "offline", 5).await.unwrap().mode,
        "fts_fallback"
    );
    assert_eq!(
        retrieval.search("p", "offline", 5).await.unwrap().mode,
        "hybrid"
    );
    assert_eq!(
        retrieval.search("q", "offline", 5).await.unwrap().mode,
        "hybrid"
    );
    store
        .reserve_embedding("p", "changed-provider", Some(2))
        .await
        .unwrap();
    index.activate("p").await.unwrap();
    assert_eq!(
        retrieval.search("p", "offline", 5).await.unwrap().mode,
        "hybrid"
    );
    assert_eq!(server.requests.lock().unwrap().len(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "query concurrency benchmark"]
async fn concurrent_query_latency_and_provider_request_count() {
    let server = http::MockHttp::start_delayed(
        vec![(200, json!({"data":[{"index":0,"embedding":[1.0,0.0]}]}))],
        Duration::from_millis(100),
    )
    .await;
    let config = EmbeddingConfig {
        enabled: true,
        dimensions: Some(2),
        ..EmbeddingConfig::default()
    };
    let embedding = Arc::new(
        HttpEmbedding::new(
            Arc::new(
                HttpProvider::new(ProviderConfig {
                    base_url: server.url.clone(),
                    api_key_env: None,
                    protocol: ProviderProtocol::Openai,
                    max_in_flight: 4,
                })
                .unwrap(),
            ),
            config.clone(),
        )
        .unwrap(),
    );
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    EmbeddingIndex::new(store.clone(), embedding.clone(), config)
        .activate("p")
        .await
        .unwrap();
    let retrieval = Arc::new(SemanticRetrieval::new(store, Some(embedding), 2000));
    let mut jobs = tokio::task::JoinSet::new();
    let barrier = Arc::new(tokio::sync::Barrier::new(16));
    for _ in 0..16 {
        let retrieval = retrieval.clone();
        let barrier = barrier.clone();
        jobs.spawn(async move {
            barrier.wait().await;
            let start = std::time::Instant::now();
            assert_eq!(
                retrieval.search("p", "offline", 8).await.unwrap().mode,
                "hybrid"
            );
            start.elapsed().as_micros()
        });
    }
    let mut samples = Vec::new();
    while let Some(result) = jobs.join_next().await {
        samples.push(result.unwrap());
    }
    samples.sort_unstable();
    let count = server.requests.lock().unwrap().len();
    assert_eq!(count, 1);
    println!(
        "16 concurrent identical queries, mock HTTP delay=100ms: p50={}us p95={}us p99={}us provider_requests={count}",
        samples[7], samples[15], samples[15]
    );
}
