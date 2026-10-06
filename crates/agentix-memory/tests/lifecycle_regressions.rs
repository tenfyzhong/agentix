// Unix-only memory service regression tests.
#![cfg(any(unix, windows))]
#[path = "support/http.rs"]
mod http;
use agentix_memory::*;
use serde_json::json;
use std::sync::Arc;

fn input(receipt: &str) -> MemoryInput {
    serde_json::from_value(json!({"title":"Offline backups","conclusion":"Backups must work offline","rationale":"External requirement","scope":"project","tags":[],"kind":"user_decision","evidence":[{"receipt_id":receipt,"message_id":receipt,"quote":"Backups must work offline"}]})).unwrap()
}
fn source(receipt: &str, sequence: i64) -> Source {
    serde_json::from_value(json!({"instance_id":"db","receipt_id":receipt,"sequence":sequence,"project_id":"p","session_id":"s","turn_id":receipt,"revision":1,"job_id":null,"recorded_at":sequence,"messages":[{"id":receipt,"role":"user","text":"Backups must work offline"}]})).unwrap()
}
#[tokio::test]
async fn forgetting_merged_memory_suppresses_original_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let config = AgentConfig::default();
    store.ingest(&source("a", 1)).await.unwrap();
    let prior = store.create("p", input("a"), Actor::Agent).await.unwrap();
    let old_work = store.claim_work("w", &config, 100).await.unwrap().unwrap();
    store
        .complete_extraction(&old_work, vec![], 101)
        .await
        .unwrap();
    store.ingest(&source("b", 2)).await.unwrap();
    let extract = store.claim_work("w", &config, 102).await.unwrap().unwrap();
    store
        .complete_extraction(&extract, vec![input("b")], 103)
        .await
        .unwrap();
    let merge = store.claim_work("w", &config, 104).await.unwrap().unwrap();
    let changed = store
        .complete_consolidation(
            &merge,
            vec![ConsolidationDecision {
                related: vec![],
                candidate: 0,
                action: DecisionAction::Merge,
                target: Some(prior.id.clone()),
                expected_revision: Some(prior.revision),
                content: Some(input("b")),
                reason: "Same external decision repeated".into(),
            }],
            105,
        )
        .await
        .unwrap();
    store
        .set_status(
            "p",
            &prior.id,
            changed[0].revision,
            Status::Forgotten,
            "Forget this decision",
            Actor::Human,
        )
        .await
        .unwrap();
    drop(store);
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    assert!(store.create("p", input("b"), Actor::Agent).await.is_err());
    assert!(
        store.create("p", input("a"), Actor::Agent).await.is_err(),
        "replaying the original source must not recreate a forgotten merged memory"
    );
}
#[tokio::test]
async fn changed_response_dimensions_do_not_starve_later_memories() {
    let server = http::MockHttp::start(vec![
        (200, json!({"data":[{"index":0,"embedding":[1.0,0.0]}]})),
        (200, json!({"data":[{"index":0,"embedding":[1.0,0.0,0.0]}]})),
        (200, json!({"data":[{"index":0,"embedding":[1.0,0.0]}]})),
    ])
    .await;
    let config = EmbeddingConfig {
        enabled: true,
        batch_size: 1,
        dimensions: None,
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
    for name in ["First", "Bad dimension", "Later valid"] {
        let mut content = input("unused");
        content.title = name.into();
        content.evidence.clear();
        store.create("p", content, Actor::Human).await.unwrap();
    }
    let index = EmbeddingIndex::new(store.clone(), embedding, config);
    let generation = index.activate("p").await.unwrap();
    assert_eq!(index.step("p", generation).await.unwrap(), 1);
    let error = index.step("p", generation).await.unwrap_err();
    assert!(error.to_string().contains("dimension"));
    let status = store.memory_status(Some("p")).await.unwrap();
    assert_eq!(status["embedding_failures"][0]["attempts"], 1);
    assert_eq!(index.step("p", generation).await.unwrap(), 1);
    assert_eq!(store.memory_status(Some("p")).await.unwrap()["indexed"], 2);
    let requests = server.requests.lock().unwrap();
    assert_ne!(
        requests[1].1["input"], requests[2].1["input"],
        "a dimension mismatch must back off rather than repeatedly occupying the head batch"
    );
}

#[tokio::test]
async fn individual_dimension_failure_does_not_abort_remaining_batch() {
    let server = http::MockHttp::start(vec![
        (200, json!({"data":[{"index":0,"embedding":[1.0,0.0]}]})),
        (400, json!({"error":"batch rejected"})),
        (200, json!({"data":[{"index":0,"embedding":[1.0,0.0,0.0]}]})),
        (200, json!({"data":[{"index":0,"embedding":[1.0,0.0]}]})),
    ])
    .await;
    let config = EmbeddingConfig {
        enabled: true,
        batch_size: 2,
        dimensions: None,
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
    let mut first = input("unused");
    first.title = "First".into();
    first.evidence.clear();
    store.create("p", first, Actor::Human).await.unwrap();
    let index = EmbeddingIndex::new(store.clone(), embedding, config);
    let generation = index.activate("p").await.unwrap();
    assert_eq!(index.step("p", generation).await.unwrap(), 1);
    for name in ["Bad dimension", "Later valid"] {
        let mut content = input("unused");
        content.title = name.into();
        content.evidence.clear();
        store.create("p", content, Actor::Human).await.unwrap();
    }
    let error = index.step("p", generation).await.unwrap_err();
    assert!(error.to_string().contains("400"));
    let status = store.memory_status(Some("p")).await.unwrap();
    assert_eq!(status["embedding_failures"][0]["attempts"], 1);
    assert_eq!(index.step("p", generation).await.unwrap(), 0);
    assert_eq!(store.memory_status(Some("p")).await.unwrap()["indexed"], 2);
    let requests = server.requests.lock().unwrap();
    assert_ne!(
        requests[2].1["input"], requests[3].1["input"],
        "a dimension mismatch must back off rather than repeatedly occupying the head batch"
    );
}

#[tokio::test]
async fn consolidation_accepts_covering_quotes_only_from_the_same_source() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let config = AgentConfig::default();
    let mut receipt = source("a", 1);
    receipt.messages[0].text = "Decision: Backups must work offline for remote sites.".into();
    store.ingest(&receipt).await.unwrap();
    let extract = store.claim_work("w", &config, 100).await.unwrap().unwrap();
    store
        .complete_extraction(&extract, vec![input("a")], 101)
        .await
        .unwrap();
    let lease = store.claim_work("w", &config, 102).await.unwrap().unwrap();
    let mut content = input("a");
    content.evidence[0]
        .quote
        .clone_from(&receipt.messages[0].text);
    let decision = ConsolidationDecision {
        related: vec![],
        candidate: 0,
        action: DecisionAction::Create,
        target: None,
        expected_revision: None,
        content: Some(content.clone()),
        reason: "Keep full source context".into(),
    };
    for invalid in ["receipt", "message", "shortened", "invented"] {
        let mut bad = decision.clone();
        let evidence = &mut bad.content.as_mut().unwrap().evidence[0];
        match invalid {
            "receipt" => evidence.receipt_id = "other".into(),
            "message" => evidence.message_id = "other".into(),
            "shortened" => evidence.quote = "Backups".into(),
            _ => evidence.quote.push_str(" Invented extension."),
        }
        assert!(
            store
                .complete_consolidation(&lease, vec![bad], 103)
                .await
                .is_err(),
            "{invalid}"
        );
    }
    let memories = store
        .complete_consolidation(&lease, vec![decision], 104)
        .await
        .unwrap();
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].content.evidence, content.evidence);
}

#[tokio::test]
async fn consolidation_cannot_archive_without_repository_review() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let config = AgentConfig::default();
    store.ingest(&source("a", 1)).await.unwrap();
    let prior = store.create("p", input("a"), Actor::Agent).await.unwrap();
    let extract = store.claim_work("w", &config, 100).await.unwrap().unwrap();
    store
        .complete_extraction(&extract, vec![input("a")], 101)
        .await
        .unwrap();
    let lease = store.claim_work("w", &config, 102).await.unwrap().unwrap();
    let result = store
        .complete_consolidation(
            &lease,
            vec![ConsolidationDecision {
                related: vec![],
                candidate: 0,
                action: DecisionAction::Archive,
                target: Some(prior.id.clone()),
                expected_revision: Some(prior.revision),
                content: None,
                reason: "Claimed documented without a repository citation".into(),
            }],
            103,
        )
        .await;
    assert!(
        result.is_err(),
        "only repository review may automatically archive memory"
    );
    assert_eq!(
        store.show("p", &prior.id, None).await.unwrap().status,
        Status::Active
    );
}
