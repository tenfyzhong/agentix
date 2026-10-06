use agentix_memory::*;
use serde_json::json;

fn source(receipt: &str, job: Option<&str>, revision: i64) -> Source {
    serde_json::from_value(json!({"instance_id":"db","receipt_id":receipt,"sequence":revision,"project_id":"p","session_id":"s","turn_id":"t","revision":revision,"job_id":job,"recorded_at":revision,"messages":[{"id":"m","role":"user","text":"Use offline backups"}]})).unwrap()
}
fn input(receipt: &str) -> MemoryInput {
    serde_json::from_value(json!({"title":"Backups","conclusion":"Use offline backups","rationale":"Policy","scope":"project","tags":[],"kind":"user_decision","evidence":[{"receipt_id":receipt,"message_id":"m","quote":"Use offline backups"}]})).unwrap()
}

#[tokio::test]
async fn cancellation_fences_workers_preserves_history_and_survives_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    store.ingest(&source("a", Some("j"), 1)).await.unwrap();
    let memory = store.create("p", input("a"), Actor::Agent).await.unwrap();
    let lease = store
        .claim_work("w", &AgentConfig::default(), 10)
        .await
        .unwrap()
        .unwrap();
    let packet = store
        .context("p", "consumer", "turn", vec![memory.clone()], 8192)
        .await
        .unwrap();
    assert_eq!(packet.items.len(), 1);
    store.invalidate_job("p", "j", 2, 11).await.unwrap();
    assert!(
        store
            .cached_context("p", "consumer", "turn", 8192)
            .await
            .unwrap()
            .unwrap()
            .items
            .is_empty()
    );
    assert!(store.search("p", "backups", 10).await.unwrap().is_empty());
    let invalid = store.show("p", &memory.id, None).await.unwrap();
    assert_eq!(invalid.status, Status::Invalidated);
    assert_eq!(
        store.show("p", &memory.id, Some(1)).await.unwrap().status,
        Status::Active
    );
    assert!(
        store
            .complete_extraction(&lease, vec![input("a")], 12)
            .await
            .is_err()
    );
    assert!(
        store
            .set_status(
                "p",
                &memory.id,
                invalid.revision,
                Status::Active,
                "reopen",
                Actor::Human
            )
            .await
            .is_err()
    );
    store.invalidate_job("p", "j", 2, 11).await.unwrap();
    assert_eq!(
        store.show("p", &memory.id, None).await.unwrap().revision,
        invalid.revision
    );
    drop(store);
    let store = MemoryStore::open(&path).await.unwrap();
    store.ingest(&source("b", Some("j"), 2)).await.unwrap();
    assert!(store.create("p", input("b"), Actor::Agent).await.is_err());
    assert!(
        store
            .claim_work("w", &AgentConfig::default(), 20)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn late_job_attachment_invalidates_previously_unbound_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    store.ingest(&source("a", None, 1)).await.unwrap();
    let memory = store.create("p", input("a"), Actor::Agent).await.unwrap();
    store.invalidate_job("p", "j", 2, 11).await.unwrap();
    store.ingest(&source("b", Some("j"), 2)).await.unwrap();
    assert_eq!(
        store.show("p", &memory.id, None).await.unwrap().status,
        Status::Invalidated
    );
}

#[tokio::test]
async fn mixed_evidence_is_invalidated_without_reviving_superseded_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let mut a = source("a", Some("old"), 1);
    a.turn_id = "a".into();
    store.ingest(&a).await.unwrap();
    let prior = store.create("p", input("a"), Actor::Agent).await.unwrap();
    let mut b = source("b", Some("new"), 2);
    b.turn_id = "b".into();
    store.ingest(&b).await.unwrap();
    let replacement = store
        .supersede("p", &prior.id, 1, input("b"), "New decision", Actor::Agent)
        .await
        .unwrap();
    let mut mixed = input("b");
    mixed.evidence.extend(input("a").evidence);
    let merged = store.create("p", mixed, Actor::Agent).await.unwrap();
    store.invalidate_job("p", "new", 2, 10).await.unwrap();
    assert_eq!(
        store.show("p", &replacement.id, None).await.unwrap().status,
        Status::Invalidated
    );
    assert_eq!(
        store.show("p", &merged.id, None).await.unwrap().status,
        Status::Invalidated
    );
    assert_eq!(
        store.show("p", &prior.id, None).await.unwrap().status,
        Status::Superseded
    );
    assert!(store.search("p", "backups", 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_binding_precedes_replay_of_unbound_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    store
        .bind_cancelled_source(&source("bound", Some("j"), 2))
        .await
        .unwrap();
    store.invalidate_job("p", "j", 2, 11).await.unwrap();
    store.ingest(&source("unbound", None, 1)).await.unwrap();
    assert!(
        store
            .create("p", input("unbound"), Actor::Agent)
            .await
            .is_err(),
        "cancelled ownership must fence replay even before the bound revision arrives"
    );
}

#[tokio::test]
async fn compaction_children_inherit_revocation_and_running_split_is_fenced() {
    for cancel_before_commit in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&dir.path().join("memory.db"))
            .await
            .unwrap();
        store.ingest(&source("a", Some("j"), 1)).await.unwrap();
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let extract = store
            .claim_work("w", &AgentConfig::default(), now)
            .await
            .unwrap()
            .unwrap();
        store
            .complete_extraction(&extract, vec![], now)
            .await
            .unwrap();
        let seed = store.create("p", input("a"), Actor::Agent).await.unwrap();
        store
            .schedule_compaction("p", "", 10, true, 0, now)
            .await
            .unwrap();
        let lease = store
            .claim_work("w", &AgentConfig::default(), now)
            .await
            .unwrap()
            .unwrap();
        let mut content = input("a");
        content.fact = Some(Fact {
            entity: "backups".into(),
            attribute: "mode".into(),
            qualifiers: vec![],
            value: "offline".into(),
        });
        let proposal = FactCompaction {
            parts: vec![FactPart {
                content,
                action: DecisionAction::Create,
                target: None,
                expected_revision: None,
                reason: "Atomic policy".into(),
            }],
            related: vec![],
            reason: "Split legacy policy".into(),
        };
        if cancel_before_commit {
            assert!(store.work_is_current(&lease, now).await.unwrap());
            store.invalidate_job("p", "j", 2, now).await.unwrap();
            assert!(!store.work_is_current(&lease, now).await.unwrap());
            assert!(
                store
                    .complete_fact_compaction(&lease, proposal, now)
                    .await
                    .is_err()
            );
            assert_eq!(store.list("p", "", 10, true).await.unwrap().len(), 1);
            continue;
        }
        let changed = store
            .complete_fact_compaction(&lease, proposal.clone(), now)
            .await
            .unwrap();
        let child = changed.iter().find(|m| m.id != seed.id).unwrap();
        assert!(child.derived_from.contains(&seed.id));
        store.invalidate_job("p", "j", 2, now).await.unwrap();
        assert_eq!(
            store.show("p", &child.id, None).await.unwrap().status,
            Status::Invalidated
        );
        assert!(
            store
                .complete_fact_compaction(&lease, proposal, now)
                .await
                .is_err()
        );
        assert_eq!(
            store
                .schedule_compaction("p", "", 10, true, 0, now)
                .await
                .unwrap()
                .scheduled,
            0
        );
    }
}

#[tokio::test]
async fn cancellation_rejects_inflight_consolidation() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    store.ingest(&source("a", Some("j"), 1)).await.unwrap();
    let lease = store
        .claim_work("w", &AgentConfig::default(), 10)
        .await
        .unwrap()
        .unwrap();
    store
        .complete_extraction(&lease, vec![input("a")], 11)
        .await
        .unwrap();
    let consolidate = store
        .claim_work("w", &AgentConfig::default(), 12)
        .await
        .unwrap()
        .unwrap();
    store.invalidate_job("p", "j", 2, 13).await.unwrap();
    assert!(
        store
            .complete_consolidation(
                &consolidate,
                vec![ConsolidationDecision {
                    candidate: 0,
                    action: DecisionAction::Create,
                    target: None,
                    expected_revision: None,
                    content: Some(input("a")),
                    reason: "Late reply".into(),
                    related: vec![],
                }],
                14
            )
            .await
            .is_err()
    );
    assert!(store.list("p", "", 10, true).await.unwrap().is_empty());
}

#[tokio::test]
async fn invalidated_memory_cannot_be_laundered_through_archived_status() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    store.ingest(&source("a", Some("j"), 1)).await.unwrap();
    let memory = store.create("p", input("a"), Actor::Agent).await.unwrap();
    store.invalidate_job("p", "j", 2, 11).await.unwrap();
    let current = store.show("p", &memory.id, None).await.unwrap();
    assert!(
        store
            .set_status(
                "p",
                &memory.id,
                current.revision,
                Status::Archived,
                "Archive",
                Actor::Human
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn schema_three_backfills_source_indexes_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    store.ingest(&source("a", Some("j"), 1)).await.unwrap();
    let memory = store.create("p", input("a"), Actor::Agent).await.unwrap();
    drop(store);
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    sqlx::raw_sql("DELETE FROM memory_evidence; DELETE FROM source_jobs; DELETE FROM source_job_turns; PRAGMA user_version=3;").execute(&pool).await.unwrap();
    pool.close().await;
    let store = MemoryStore::open(&path).await.unwrap();
    store.invalidate_job("p", "j", 2, 11).await.unwrap();
    assert_eq!(
        store.show("p", &memory.id, None).await.unwrap().status,
        Status::Invalidated
    );
}
