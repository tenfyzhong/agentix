//! Reproducible scale acceptance; run explicitly so ordinary unit feedback stays short.
use agentix_memory::{Actor, MemoryInput, MemoryStore, Source};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "scale acceptance: cargo test -p agentix-memory --test performance -- --ignored --nocapture"]
async fn scoped_retrieval_remains_responsive_during_source_ingestion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    let vector = vec![0.5; 384];
    for (project, count) in [("target", 2000), ("foreign", 8000)] {
        let generation = store
            .configure_embedding(project, "fixture", 384)
            .await
            .unwrap();
        for index in 0..count {
            let input: MemoryInput = serde_json::from_value(json!({"title":format!("离线决策 {index}"),"conclusion":format!("客户要求离线运行，区域约束 policy_{index}"),"rationale":"外部部署条件","scope":"project","tags":["offline_policy"],"kind":"user_decision","evidence":[]})).unwrap();
            let memory = store.create(project, input, Actor::Human).await.unwrap();
            store
                .put_embedding(project, &memory.id, 1, generation, &vector)
                .await
                .unwrap();
        }
    }
    let writer = store.clone();
    let maintenance = tokio::spawn(async move {
        for index in 0..100 {
            let source: Source = serde_json::from_value(json!({"instance_id":"fixture","receipt_id":format!("r{index}"),"sequence":index+1,"project_id":"target","session_id":"session","turn_id":format!("t{index}"),"revision":1,"job_id":null,"recorded_at":1,"messages":[{"id":"m","role":"user","text":"External decision"}]})).unwrap();
            writer.ingest(&source).await.unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    let mut lexical_ms = Vec::new();
    let mut hybrid_ms = Vec::new();
    for _ in 0..30 {
        let start = Instant::now();
        let lexical = store.search("target", "离线 policy", 8).await.unwrap();
        lexical_ms.push(start.elapsed().as_micros());
        assert_eq!(lexical.len(), 8);
        assert!(lexical.iter().all(|memory| memory.project_id == "target"));
        let start = Instant::now();
        let hybrid = tokio::time::timeout(
            Duration::from_secs(2),
            store.hybrid_search("target", "离线 policy", 1, &vector, 8),
        )
        .await
        .unwrap()
        .unwrap();
        hybrid_ms.push(start.elapsed().as_micros());
        assert_eq!(hybrid.len(), 8);
        assert!(hybrid.iter().all(|memory| memory.project_id == "target"));
    }
    maintenance.await.unwrap();
    lexical_ms.sort_unstable();
    hybrid_ms.sort_unstable();
    println!(
        "10000 memories, target=2000, foreign=8000, dimensions=384, 100 concurrent ingests; FTS p50={}us p95={}us; hybrid p50={}us p95={}us",
        lexical_ms[15], lexical_ms[28], hybrid_ms[15], hybrid_ms[28]
    );
    assert_eq!(store.work_counts().await.unwrap().pending, 100);
    assert_eq!(
        store.list("target", "", 100, false).await.unwrap().len(),
        100
    );
}
