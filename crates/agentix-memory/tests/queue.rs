use agentix_memory::{AgentConfig, MemoryStore, Source, WorkKind};
use serde_json::json;

fn source(project: &str, receipt: &str, turn: &str, revision: i64) -> Source {
    serde_json::from_value(json!({"instance_id":"db","receipt_id":receipt,"sequence":1,"project_id":project,"session_id":"session","turn_id":turn,"revision":revision,"job_id":null,"recorded_at":1,"messages":[{"id":"message","role":"user","text":"Keep this external decision"}]})).unwrap()
}

#[tokio::test]
async fn targeted_claim_preserves_other_work_and_obeys_limits_and_source_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let config = AgentConfig {
        max_concurrent_loops: 1,
        ..AgentConfig::default()
    };
    store
        .ingest(&source("other", "other", "other", 1))
        .await
        .unwrap();
    store
        .ingest(&source("target", "target", "target", 1))
        .await
        .unwrap();
    let lease = store
        .claim_work_item("scoped", &config, 100, 2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.project_id, "target");
    assert_eq!(store.work_details(1).await.unwrap()["attempts"], 0);
    assert!(
        store
            .claim_work_item("blocked", &config, 100, 1)
            .await
            .unwrap()
            .is_none()
    );
    store
        .ingest(&source("target", "replacement", "target", 2))
        .await
        .unwrap();
    assert!(!store.work_is_current(&lease, 100).await.unwrap());
    assert!(
        store
            .claim_work_item("cancelled", &config, 100, 2)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(store.work_details(1).await.unwrap()["state"], "pending");
    assert_eq!(store.work_details(3).await.unwrap()["attempts"], 0);
    assert!(
        store
            .claim_work_item("missing", &config, 100, 999)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn ingest_and_queue_are_atomic_idempotent_and_fair_across_projects() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let config = AgentConfig {
        max_extraction_loops_per_project: 1,
        ..AgentConfig::default()
    };
    for i in 0..4 {
        store
            .ingest(&source("a", &format!("a{i}"), &format!("turn{i}"), 1))
            .await
            .unwrap();
    }
    let other = source("b", "b1", "other", 1);
    store.ingest(&other).await.unwrap();
    assert!(!store.ingest(&other).await.unwrap());
    let first = store
        .claim_work("worker1", &config, 100)
        .await
        .unwrap()
        .unwrap();
    let second = store
        .claim_work("worker2", &config, 100)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first.project_id, second.project_id);
    assert_eq!(first.kind, WorkKind::Extract);
    assert!(
        store
            .claim_work("worker3", &config, 100)
            .await
            .unwrap()
            .is_none()
    );
    store
        .complete_extraction(&first, Vec::new(), 101)
        .await
        .unwrap();
    store
        .complete_extraction(&second, Vec::new(), 101)
        .await
        .unwrap();
    assert_eq!(store.work_counts().await.unwrap().done, 2);
    assert_eq!(store.work_counts().await.unwrap().pending, 3);
}

#[tokio::test]
async fn expired_leases_recover_with_generation_fencing_and_durable_retry_limits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let config = AgentConfig {
        max_attempts: 2,
        lease_seconds: 10,
        ..AgentConfig::default()
    };
    let store = MemoryStore::open(&path).await.unwrap();
    store.ingest(&source("a", "r1", "t", 1)).await.unwrap();
    let old = store
        .claim_work("old", &config, 100)
        .await
        .unwrap()
        .unwrap();
    drop(store);
    let store = MemoryStore::open(&path).await.unwrap();
    let next = store
        .claim_work("new", &config, 111)
        .await
        .unwrap()
        .unwrap();
    assert!(next.generation > old.generation);
    assert!(
        store
            .complete_extraction(&old, Vec::new(), 112)
            .await
            .is_err()
    );
    store
        .fail_work(&next, "provider unavailable", 112)
        .await
        .unwrap();
    assert!(
        store
            .claim_work("third", &config, 1000)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(store.work_counts().await.unwrap().failed, 1);
}

#[tokio::test]
async fn newer_source_revision_cancels_old_inflight_extraction() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let config = AgentConfig::default();
    store.ingest(&source("a", "r1", "t", 1)).await.unwrap();
    let old = store
        .claim_work("old", &config, 100)
        .await
        .unwrap()
        .unwrap();
    store.ingest(&source("a", "r2", "t", 2)).await.unwrap();
    assert!(
        store
            .complete_extraction(&old, Vec::new(), 101)
            .await
            .is_err()
    );
    let next = store
        .claim_work("new", &config, 101)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.receipt_id, "r2");
    assert_eq!(store.work_counts().await.unwrap().cancelled, 1);
}

#[tokio::test]
async fn long_unicode_sources_are_fully_covered_by_bounded_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let mut input = source("a", "large", "t", 1);
    input.messages[0].text = "选择边界".repeat(10000);
    store.ingest(&input).await.unwrap();
    let mut covered = String::new();
    while let Some(work) = store
        .claim_work("worker", &AgentConfig::default(), 100)
        .await
        .unwrap()
    {
        let text = work.payload["text"].as_str().unwrap();
        assert!(text.len() <= 16384);
        covered.push_str(text);
        store
            .complete_extraction(&work, Vec::new(), 101)
            .await
            .unwrap();
    }
    assert_eq!(covered, input.messages[0].text);
}

fn candidate(receipt: &str) -> agentix_memory::MemoryInput {
    serde_json::from_value(json!({"title":"External decision","conclusion":"Keep this external decision","rationale":"User instruction","scope":"project","tags":[],"kind":"user_decision","evidence":[{"receipt_id":receipt,"message_id":"message","quote":"Keep this external decision"}]})).unwrap()
}

#[tokio::test]
async fn consolidation_is_serial_per_project_and_atomic_with_revision_guards() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let config = AgentConfig::default();
    store.ingest(&source("a", "r1", "t1", 1)).await.unwrap();
    store.ingest(&source("a", "r2", "t2", 1)).await.unwrap();
    let a = store.claim_work("a", &config, 100).await.unwrap().unwrap();
    let b = store.claim_work("b", &config, 100).await.unwrap().unwrap();
    store
        .complete_extraction(&a, vec![candidate("r1"), candidate("r1")], 101)
        .await
        .unwrap();
    store
        .complete_extraction(&b, vec![candidate("r2")], 101)
        .await
        .unwrap();
    let consolidation = store.claim_work("c", &config, 102).await.unwrap().unwrap();
    assert_eq!(consolidation.kind, WorkKind::Consolidate);
    assert!(store.claim_work("d", &config, 102).await.unwrap().is_none());
    let invalid = serde_json::from_value(json!([
        {"candidate":0,"action":"create","target":null,"expected_revision":null,"content":null,"reason":"new"},
        {"candidate":1,"action":"merge","target":"missing","expected_revision":1,"content":null,"reason":"merge"}
    ])).unwrap();
    assert!(
        store
            .complete_consolidation(&consolidation, invalid, 103)
            .await
            .is_err()
    );
    assert!(store.list("a", "", 10, true).await.unwrap().is_empty());
    assert_eq!(store.work_counts().await.unwrap().running, 1);
    let valid = serde_json::from_value(json!([
        {"candidate":0,"action":"create","target":null,"expected_revision":null,"content":null,"reason":"new"},
        {"candidate":1,"action":"discard","target":null,"expected_revision":null,"content":null,"reason":"duplicate"}
    ])).unwrap();
    let saved = store
        .complete_consolidation(&consolidation, valid, 103)
        .await
        .unwrap();
    assert_eq!(saved.len(), 1);
    let next = store.claim_work("d", &config, 104).await.unwrap().unwrap();
    let current = &saved[0];
    store
        .update(
            "a",
            &current.id,
            current.revision,
            current.content.clone(),
            agentix_memory::Actor::Human,
        )
        .await
        .unwrap();
    let stale = serde_json::from_value(json!([{"candidate":0,"action":"merge","target":current.id,"expected_revision":current.revision,"content":null,"reason":"merge"}])).unwrap();
    assert!(
        store
            .complete_consolidation(&next, stale, 105)
            .await
            .is_err()
    );
    assert_eq!(
        store.show("a", &current.id, None).await.unwrap().revision,
        2
    );
}

#[tokio::test]
async fn large_candidate_sets_are_split_into_bounded_consolidation_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let config = AgentConfig::default();
    store.ingest(&source("a", "r1", "t1", 1)).await.unwrap();
    let extraction = store
        .claim_work("extract", &config, 100)
        .await
        .unwrap()
        .unwrap();
    let mut large = candidate("r1");
    large.rationale = "r".repeat(8192);
    store
        .complete_extraction(&extraction, vec![large; 16], 101)
        .await
        .unwrap();
    let mut count = 0;
    while let Some(lease) = store.claim_work("merge", &config, 102).await.unwrap() {
        assert!(
            lease.payload.to_string().len() <= 65536,
            "consolidation input exceeds a bounded page"
        );
        let candidates = lease.payload.as_array().unwrap().len();
        let decisions = (0..candidates).map(|candidate| serde_json::from_value(json!({"candidate":candidate,"action":"discard","target":null,"expected_revision":null,"content":null,"reason":"duplicate"})).unwrap()).collect();
        store
            .complete_consolidation(&lease, decisions, 103)
            .await
            .unwrap();
        count += candidates;
    }
    assert_eq!(count, 16);
}

#[tokio::test]
async fn queue_notifications_follow_commits_and_ignore_duplicate_ingest() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let mut work = store.subscribe_work();
    let input = source("p", "receipt", "turn", 1);
    store.ingest(&input).await.unwrap();
    assert!(work.has_changed().unwrap());
    work.borrow_and_update();
    assert_eq!(store.work_counts().await.unwrap().pending, 1);
    store.ingest(&input).await.unwrap();
    assert!(!work.has_changed().unwrap());
}
