//! Reproducible scale acceptance; run explicitly so ordinary unit feedback stays short.
use agentix_memory::{Actor, MemoryInput, MemoryStore, Source};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "scale acceptance: cargo test -p agentix-memory --test performance -- --ignored --nocapture"]
async fn scoped_retrieval_remains_responsive_during_source_ingestion() {
    retrieval_scale(2000, 8000).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "scale acceptance"]
async fn ten_thousand_project_vectors_remain_within_context_budget() {
    retrieval_scale(10000, 0).await;
}

async fn retrieval_scale(target: usize, foreign: usize) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    let vector = vec![0.5; 384];
    for (project, count) in [("target", target), ("foreign", foreign)] {
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
    drop(store);
    let cold = Instant::now();
    let store = MemoryStore::open(&path).await.unwrap();
    let cold_results = store.search("target", "离线 policy", 8).await.unwrap();
    println!(
        "target={target} connection-cold open+FTS={}us",
        cold.elapsed().as_micros()
    );
    let mut fresh = Vec::new();
    let mut cached = Vec::new();
    for turn in 0..30 {
        let turn = format!("t{turn}");
        let start = Instant::now();
        store
            .context("target", "bench", &turn, cold_results.clone(), 6144)
            .await
            .unwrap();
        fresh.push(start.elapsed().as_micros());
        let start = Instant::now();
        store
            .cached_context("target", "bench", &turn, 6144)
            .await
            .unwrap()
            .unwrap();
        cached.push(start.elapsed().as_micros());
    }
    report("context preparation", fresh);
    report("context receipt hit", cached);
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
    println!("target={target} foreign={foreign} dimensions=384 concurrent_ingests=100");
    report("FTS", lexical_ms);
    report("hybrid", hybrid_ms);
    assert_eq!(store.work_counts().await.unwrap().pending, 100);
    assert_eq!(
        store.list("target", "", 100, false).await.unwrap().len(),
        100
    );
}

fn report(label: &str, mut samples: Vec<u128>) {
    samples.sort_unstable();
    let percentile = |p: usize| samples[(samples.len() * p).div_ceil(100).saturating_sub(1)];
    println!(
        "{label}: n={} p50={}us p95={}us p99={}us",
        samples.len(),
        percentile(50),
        percentile(95),
        percentile(99)
    );
}

#[tokio::test]
#[ignore = "SQLite contention benchmark"]
async fn context_probe_latency_under_a_writer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let mut probes = Vec::new();
    let mut waits = Vec::new();
    for _ in 0..30 {
        let writer = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        let start = Instant::now();
        assert!(
            store
                .cached_context("p", "s", "missing", 1024)
                .await
                .unwrap()
                .is_none()
        );
        probes.push(start.elapsed().as_micros());
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            writer.rollback().await.unwrap();
        });
        let start = Instant::now();
        let tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        waits.push(start.elapsed().as_micros());
        tx.rollback().await.unwrap();
        release.await.unwrap();
    }
    report("cache miss under writer", probes);
    report("SQLite writer acquisition (20ms blocker)", waits);
}
