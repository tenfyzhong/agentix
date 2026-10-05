use agentix_memory::{Actor, MemoryInput, MemoryStore, Source, Status};
use serde_json::json;

fn source(project: &str, receipt: &str) -> Source {
    serde_json::from_value(json!({
        "instance_id":"task-source", "receipt_id":receipt, "sequence":1,
        "project_id":project,"session_id":format!("session-{project}"),"turn_id":"turn","revision":1,
        "job_id":null,"recorded_at":100,
        "messages":[{"id":"decision","role":"user","text":"离线使用是约束，因此使用本地备份；拒绝依赖远程服务。"}]
    })).unwrap()
}

fn input(receipt: &str) -> MemoryInput {
    serde_json::from_value(json!({
        "title":"离线备份决策","conclusion":"为满足离线使用，备份必须可在本地完成。",
        "rationale":"拒绝依赖远程服务，以满足离线约束。","scope":"project",
        "tags":["backup_policy"],"kind":"user_decision",
        "evidence":[{"receipt_id":receipt,"message_id":"decision","quote":"离线使用是约束"}]
    }))
    .unwrap()
}

#[tokio::test]
async fn evidence_ingestion_is_idempotent_and_cannot_be_rewritten() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.sqlite3"))
        .await
        .unwrap();
    let original = source("p1", "r1");
    assert!(store.ingest(&original).await.unwrap());
    assert!(!store.ingest(&original).await.unwrap());
    let mut forged = original.clone();
    forged.project_id = "p2".into();
    assert!(store.ingest(&forged).await.is_err());
    assert_eq!(store.source("p1", "r1").await.unwrap(), original);
    assert!(store.source("p2", "r1").await.is_err());
}

#[tokio::test]
async fn chinese_search_is_scoped_and_versions_are_immutable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.sqlite3");
    let store = MemoryStore::open(&path).await.unwrap();
    store.ingest(&source("p1", "r1")).await.unwrap();
    let memory = store.create("p1", input("r1"), Actor::Agent).await.unwrap();
    assert_eq!(
        store.search("p1", "备份", 10).await.unwrap()[0].id,
        memory.id
    );
    assert_eq!(
        store.search("p1", "backup_policy", 10).await.unwrap()[0].id,
        memory.id
    );
    assert!(store.search("p2", "备份", 10).await.unwrap().is_empty());
    assert!(store.show("p2", &memory.id, None).await.is_err());
    let mut revised = input("r1");
    revised.rationale = "人类补充：只适用于离线部署。".into();
    let updated = store
        .update("p1", &memory.id, 1, revised, Actor::Human)
        .await
        .unwrap();
    assert_eq!(updated.revision, 2);
    assert_eq!(updated.actor, Actor::Human);
    assert!(
        store
            .update("p1", &memory.id, 1, input("r1"), Actor::Agent)
            .await
            .is_err()
    );
    assert_eq!(store.show("p1", &memory.id, Some(1)).await.unwrap(), memory);
    drop(store);
    let store = MemoryStore::open(&path).await.unwrap();
    assert_eq!(store.show("p1", &memory.id, None).await.unwrap(), updated);
}

#[tokio::test]
async fn foreign_or_fabricated_evidence_cannot_enter_memory() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.sqlite3"))
        .await
        .unwrap();
    store.ingest(&source("p1", "r1")).await.unwrap();
    assert!(store.create("p2", input("r1"), Actor::Agent).await.is_err());
    let mut forged = input("r1");
    forged.evidence[0].quote = "Unsupported claim".into();
    assert!(store.create("p1", forged, Actor::Agent).await.is_err());
    assert!(store.search("p1", "备份", 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn forgetting_suppresses_replay_even_with_a_new_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.sqlite3"))
        .await
        .unwrap();
    store.ingest(&source("p1", "r1")).await.unwrap();
    let memory = store.create("p1", input("r1"), Actor::Agent).await.unwrap();
    let forgotten = store
        .set_status(
            "p1",
            &memory.id,
            1,
            Status::Forgotten,
            "User requested forgetting",
            Actor::Human,
        )
        .await
        .unwrap();
    assert_eq!(forgotten.status, Status::Forgotten);
    assert!(store.search("p1", "备份", 10).await.unwrap().is_empty());
    store.ingest(&source("p1", "r2")).await.unwrap();
    assert!(store.create("p1", input("r2"), Actor::Agent).await.is_err());
    assert_eq!(
        store
            .show("p1", &memory.id, Some(1))
            .await
            .unwrap()
            .revision,
        1
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn vector_recall_is_independent_and_fenced_by_profile_and_content() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.sqlite3"))
        .await
        .unwrap();
    store.ingest(&source("p1", "r1")).await.unwrap();
    let memory = store.create("p1", input("r1"), Actor::Agent).await.unwrap();
    let generation = store
        .configure_embedding("p1", "profile-a", 2)
        .await
        .unwrap();
    assert_eq!(
        store
            .embedding_pending("p1", generation, "", 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .put_embedding("p1", &memory.id, 1, generation, &[1.0, 0.0])
            .await
            .unwrap()
    );
    assert!(
        store
            .search("p1", "disconnected laptop", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .hybrid_search("p1", "disconnected laptop", generation, &[1.0, 0.0], 10)
            .await
            .unwrap()[0]
            .id,
        memory.id
    );
    assert!(
        store
            .embedding_pending("p1", generation, "", 10)
            .await
            .unwrap()
            .is_empty()
    );
    let updated = store
        .update("p1", &memory.id, 1, input("r1"), Actor::Human)
        .await
        .unwrap();
    assert!(
        !store
            .put_embedding("p1", &memory.id, 1, generation, &[1.0, 0.0])
            .await
            .unwrap()
    );
    assert!(
        store
            .hybrid_search("p1", "disconnected laptop", generation, &[1.0, 0.0], 10)
            .await
            .unwrap()
            .is_empty()
    );
    let next = store
        .configure_embedding("p1", "profile-b", 3)
        .await
        .unwrap();
    assert!(next > generation);
    assert!(
        !store
            .put_embedding("p1", &memory.id, updated.revision, generation, &[1.0, 0.0])
            .await
            .unwrap()
    );
    assert!(
        store
            .put_embedding("p1", &memory.id, updated.revision, next, &[1.0, 0.0])
            .await
            .is_err()
    );
    assert!(
        store
            .put_embedding(
                "p1",
                &memory.id,
                updated.revision,
                next,
                &[1.0, f32::NAN, 0.0]
            )
            .await
            .is_err()
    );
    assert!(
        store
            .put_embedding("p1", &memory.id, updated.revision, next, &[0.0, 0.0, 0.0])
            .await
            .is_err()
    );
    assert!(
        store
            .put_embedding("p1", &memory.id, updated.revision, next, &[1.0, 0.0, 0.0])
            .await
            .unwrap()
    );
    store
        .set_status(
            "p1",
            &memory.id,
            updated.revision,
            Status::Forgotten,
            "Forget",
            Actor::Human,
        )
        .await
        .unwrap();
    assert!(
        store
            .hybrid_search("p1", "disconnected laptop", next, &[1.0, 0.0, 0.0], 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn offline_reader_never_creates_or_writes_a_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.sqlite3");
    assert!(MemoryStore::open_read_only(&path).await.is_err());
    assert!(!path.exists());
    let writer = MemoryStore::open(&path).await.unwrap();
    writer.ingest(&source("p1", "r1")).await.unwrap();
    let memory = writer
        .create("p1", input("r1"), Actor::Agent)
        .await
        .unwrap();
    let reader = MemoryStore::open_read_only(&path).await.unwrap();
    assert_eq!(
        reader.search("p1", "备份", 10).await.unwrap()[0].id,
        memory.id
    );
    assert!(
        reader
            .create("p1", input("r1"), Actor::Agent)
            .await
            .is_err()
    );
    assert_eq!(reader.list("p1", "", 10, false).await.unwrap().len(), 1);
}

#[tokio::test]
async fn concurrent_edit_keeps_one_winner_and_historical_states_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.sqlite3"))
        .await
        .unwrap();
    store.ingest(&source("p1", "r1")).await.unwrap();
    let memory = store.create("p1", input("r1"), Actor::Agent).await.unwrap();
    let (a, b) = tokio::join!(
        store.update("p1", &memory.id, 1, input("r1"), Actor::Agent),
        store.update("p1", &memory.id, 1, input("r1"), Actor::Human)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let conflicted = store
        .set_status(
            "p1",
            &memory.id,
            2,
            Status::Conflicted,
            "Conflicting scope",
            Actor::Agent,
        )
        .await
        .unwrap();
    assert_eq!(
        store.conflicts("p1", "", 10).await.unwrap()[0].status,
        Status::Conflicted
    );
    store
        .set_status(
            "p1",
            &memory.id,
            conflicted.revision,
            Status::Archived,
            "Now documented",
            Actor::Agent,
        )
        .await
        .unwrap();
    assert!(store.search("p1", "备份", 10).await.unwrap().is_empty());
    assert!(store.list("p1", "", 10, false).await.unwrap().is_empty());
    assert_eq!(store.list("p1", "", 10, true).await.unwrap().len(), 1);
    assert!(
        store
            .list("p1", &memory.id, 10, true)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn assistant_recommendations_cannot_be_promoted_to_user_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.sqlite3"))
        .await
        .unwrap();
    let mut transcript = source("p1", "r1");
    transcript.messages[0].role = "assistant".into();
    store.ingest(&transcript).await.unwrap();
    assert!(store.create("p1", input("r1"), Actor::Agent).await.is_err());
    let mut inference = input("r1");
    inference.kind = agentix_memory::Kind::Inference;
    store.create("p1", inference, Actor::Agent).await.unwrap();
    assert!(
        store
            .search("p1", "\" OR nonexistent_column:*", 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn supersession_is_atomic_and_links_both_versions() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.sqlite3"))
        .await
        .unwrap();
    store.ingest(&source("p1", "r1")).await.unwrap();
    let old = store.create("p1", input("r1"), Actor::Agent).await.unwrap();
    let new = store
        .supersede(
            "p1",
            &old.id,
            1,
            input("r1"),
            "User changed the constraint",
            Actor::Human,
        )
        .await
        .unwrap();
    assert_eq!(new.supersedes.as_deref(), Some(old.id.as_str()));
    let prior = store.show("p1", &old.id, None).await.unwrap();
    assert_eq!(prior.status, Status::Superseded);
    assert_eq!(prior.superseded_by.as_deref(), Some(new.id.as_str()));
    assert_eq!(store.search("p1", "备份", 10).await.unwrap().len(), 1);
    assert_eq!(store.show("p1", &old.id, Some(1)).await.unwrap(), old);
    assert!(
        store
            .supersede(
                "p1",
                &old.id,
                1,
                input("r1"),
                "Stale proposal",
                Actor::Agent
            )
            .await
            .is_err()
    );
    assert_eq!(store.list("p1", "", 10, true).await.unwrap().len(), 2);
}

#[tokio::test]
async fn rebuilding_fts_is_paged_and_preserves_versions_and_vectors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.sqlite3");
    let store = MemoryStore::open(&path).await.unwrap();
    store.ingest(&source("p1", "r1")).await.unwrap();
    let memory = store.create("p1", input("r1"), Actor::Agent).await.unwrap();
    let generation = store.configure_embedding("p1", "profile", 2).await.unwrap();
    store
        .put_embedding("p1", &memory.id, 1, generation, &[1.0, 0.0])
        .await
        .unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", path.display()))
        .await
        .unwrap();
    sqlx::query("DELETE FROM memory_fts")
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.search("p1", "备份", 10).await.unwrap().is_empty());
    let page = store.reindex_fts("p1", "", 10).await.unwrap();
    assert_eq!(page.indexed, 1);
    assert!(page.complete);
    assert_eq!(store.search("p1", "备份", 10).await.unwrap()[0], memory);
    assert!(
        store
            .embedding_pending("p1", generation, "", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.reindex_fts("p2", "", 10).await.unwrap().complete);
}

#[tokio::test]
async fn expired_facts_are_historical_until_explicitly_revalidated() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.sqlite3"))
        .await
        .unwrap();
    store.ingest(&source("p1", "r1")).await.unwrap();
    let mut fact = input("r1");
    fact.valid_until = Some(1);
    fact.conditions = vec!["Offline deployments".into()];
    let memory = store.create("p1", fact, Actor::Agent).await.unwrap();
    assert!(store.search("p1", "备份", 10).await.unwrap().is_empty());
    assert!(store.list("p1", "", 10, false).await.unwrap().is_empty());
    assert_eq!(store.list("p1", "", 10, true).await.unwrap().len(), 1);
    let generation = store.configure_embedding("p1", "profile", 2).await.unwrap();
    assert!(
        store
            .embedding_pending("p1", generation, "", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !store
            .put_embedding("p1", &memory.id, 1, generation, &[1.0, 0.0])
            .await
            .unwrap()
    );
    store
        .update("p1", &memory.id, 1, input("r1"), Actor::Human)
        .await
        .unwrap();
    assert_eq!(store.search("p1", "备份", 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn vector_recall_crosses_pages_without_reading_other_projects() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.sqlite3"))
        .await
        .unwrap();
    store.ingest(&source("p1", "r1")).await.unwrap();
    store.ingest(&source("p2", "r2")).await.unwrap();
    let generation = store.configure_embedding("p1", "profile", 2).await.unwrap();
    let other = store.configure_embedding("p2", "profile", 2).await.unwrap();
    let foreign = store.create("p2", input("r2"), Actor::Agent).await.unwrap();
    store
        .put_embedding("p2", &foreign.id, 1, other, &[1.0, 0.0])
        .await
        .unwrap();
    for _ in 0..260 {
        let memory = store.create("p1", input("r1"), Actor::Agent).await.unwrap();
        store
            .put_embedding("p1", &memory.id, 1, generation, &[0.0, 1.0])
            .await
            .unwrap();
    }
    let mut content = input("r1");
    content.conditions = vec!["airgapped deployments".into()];
    let target = store.create("p1", content, Actor::Agent).await.unwrap();
    store
        .put_embedding("p1", &target.id, 1, generation, &[1.0, 0.0])
        .await
        .unwrap();
    let hits = store
        .hybrid_search("p1", "unmatchedphrase", generation, &[1.0, 0.0], 10)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, target.id);
    assert_eq!(
        store.search("p1", "airgapped", 10).await.unwrap()[0].id,
        target.id
    );
}

#[tokio::test]
async fn recovery_binding_and_cursor_survive_restart_and_fence_foreign_sources() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.sqlite3");
    let store = MemoryStore::open(&path).await.unwrap();
    assert_eq!(store.bind_source("task-source").await.unwrap(), 0);
    assert!(store.bind_source("foreign").await.is_err());
    store.ingest(&source("p1", "r1")).await.unwrap();
    let mut other = source("p2", "r2");
    other.instance_id = "foreign".into();
    assert!(store.ingest(&other).await.is_err());
    drop(store);
    let store = MemoryStore::open(&path).await.unwrap();
    assert_eq!(store.bind_source("task-source").await.unwrap(), 0);
    store
        .checkpoint_replay(0, &source("p1", "r1"))
        .await
        .unwrap();
    assert_eq!(store.bind_source("task-source").await.unwrap(), 1);
    assert_eq!(
        store.recovery_sources("", 100).await.unwrap(),
        vec![source("p1", "r1")]
    );
}

#[tokio::test]
async fn replay_checkpoint_requires_persisted_source_and_survives_out_of_order_intake() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.sqlite3");
    let store = MemoryStore::open(&path).await.unwrap();
    let first = source("p1", "r1");
    let mut later = source("p2", "r3");
    later.sequence = 3;
    // Simulate an older database with persisted receipts beyond an unfilled hole.
    store.ingest(&later).await.unwrap();
    assert_eq!(store.bind_source("task-source").await.unwrap(), 0);
    assert!(store.checkpoint_replay(0, &first).await.is_err());
    store.ingest(&first).await.unwrap();
    let mut forged = first.clone();
    forged.messages[0].text = "Changed receipt".into();
    assert!(store.checkpoint_replay(0, &forged).await.is_err());
    store.checkpoint_replay(0, &first).await.unwrap();
    assert!(store.checkpoint_replay(0, &later).await.is_err());
    assert!(store.checkpoint_replay(1, &first).await.is_err());
    drop(store);
    let store = MemoryStore::open(&path).await.unwrap();
    assert_eq!(store.bind_source("task-source").await.unwrap(), 1);
    // Numeric sequence gaps are valid; only the task database defines next-in-order.
    store.checkpoint_replay(1, &later).await.unwrap();
    assert_eq!(store.bind_source("task-source").await.unwrap(), 3);
}

#[tokio::test]
async fn atomic_memory_document_has_a_serialized_size_budget() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let mut content = input("r");
    content.evidence.clear();
    content.conditions = vec!["\0".repeat(1024); 16];
    assert!(store.create("p", content, Actor::Human).await.is_err());
}

#[tokio::test]
async fn user_choice_can_cite_the_proposal_it_explicitly_accepts() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let source: Source = serde_json::from_value(json!({"instance_id":"db","receipt_id":"receipt","sequence":1,"project_id":"p","session_id":"s","turn_id":"t","revision":1,"job_id":null,"recorded_at":1,"messages":[{"id":"proposal","role":"assistant","text":"Option B keeps all customer data offline."},{"id":"choice","role":"user","text":"Choose option B."}]})).unwrap();
    store.ingest(&source).await.unwrap();
    let mut decision = input("receipt");
    decision.evidence = vec![
        agentix_memory::Evidence {
            receipt_id: "receipt".into(),
            message_id: "choice".into(),
            quote: "Choose option B.".into(),
        },
        agentix_memory::Evidence {
            receipt_id: "receipt".into(),
            message_id: "proposal".into(),
            quote: "Option B keeps all customer data offline.".into(),
        },
    ];
    let result = store.create("p", decision, Actor::Agent).await;
    assert!(
        result.is_ok(),
        "explicit user selection plus its proposal must be valid evidence: {result:?}"
    );
}
