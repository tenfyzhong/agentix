use agentix_memory::*;
use serde_json::json;

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

async fn add(store: &MemoryStore, project: &str, receipt: &str, actor: Actor) -> Memory {
    let text = format!("Keep external server decision {receipt}");
    let source: Source = serde_json::from_value(json!({"instance_id":"db","receipt_id":receipt,"sequence":1,"project_id":project,"session_id":receipt,"turn_id":"turn","revision":1,"job_id":null,"recorded_at":now(),"messages":[{"id":"message","role":"user","text":text}]})).unwrap();
    store.ingest(&source).await.unwrap();
    let lease = store
        .claim_work("ingest", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    store
        .complete_extraction(&lease, vec![], now())
        .await
        .unwrap();
    let input = serde_json::from_value(json!({"title":"Server decision","conclusion":text,"rationale":"External policy","scope":"server","tags":["server"],"kind":"user_decision","evidence":[{"receipt_id":receipt,"message_id":"message","quote":text}]})).unwrap();
    store.create(project, input, actor).await.unwrap()
}

#[tokio::test]
async fn manual_compaction_is_paginated_idempotent_and_survives_restart() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    for i in 0..3 {
        add(&store, "p", &format!("r{i}"), Actor::Agent).await;
    }
    add(&store, "foreign", "foreign", Actor::Agent).await;
    let first = store
        .schedule_compaction("p", "", 2, true, 0, now())
        .await
        .unwrap();
    assert_eq!(first.scanned, 2);
    assert_eq!(first.scheduled, 2);
    assert!(!first.next_after.is_empty());
    assert_eq!(
        store
            .schedule_compaction("p", "", 2, true, 0, now())
            .await
            .unwrap()
            .scheduled,
        0
    );
    drop(store);
    let store = MemoryStore::open(&path).await.unwrap();
    let last = store
        .schedule_compaction("p", &first.next_after, 2, true, 0, now())
        .await
        .unwrap();
    assert_eq!(last.scanned, 1);
    assert_eq!(last.scheduled, 1);
    assert!(last.next_after.is_empty());
    assert_eq!(store.work_counts().await.unwrap().pending, 3);
    let lease = store
        .claim_work("compact", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.kind, WorkKind::Consolidate);
    assert!(lease.payload.get("compact").is_some());
}

#[tokio::test]
async fn background_compaction_debounces_changes_and_preserves_human_and_inactive_records() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let active = add(&store, "p", "active", Actor::Agent).await;
    add(&store, "p", "human", Actor::Human).await;
    let archived = add(&store, "p", "archived", Actor::Agent).await;
    store
        .set_status(
            "p",
            &archived.id,
            1,
            Status::Archived,
            "Documented",
            Actor::Agent,
        )
        .await
        .unwrap();
    let config = AgentConfig::default();
    assert_eq!(
        store
            .schedule_background_compaction("p", &config, now())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .schedule_background_compaction("p", &config, now() + 60)
            .await
            .unwrap(),
        1
    );
    let lease = store
        .claim_work("compact", &config, now() + 60)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.payload["compact"]["id"], active.id);
    assert_eq!(
        store
            .schedule_background_compaction("p", &config, now() + 120)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn compact_completion_does_not_create_duplicates_or_enqueue_itself_forever() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let seed = add(&store, "p", "seed", Actor::Agent).await;
    store
        .schedule_compaction("p", "", 10, true, 0, now())
        .await
        .unwrap();
    let lease = store
        .claim_work("compact", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    let decisions = serde_json::from_value(json!([{"candidate":0,"action":"merge","target":seed.id,"expected_revision":seed.revision,"content":seed.content,"reason":"Already concise","related":[]}])).unwrap();
    store
        .complete_consolidation(&lease, decisions, now())
        .await
        .unwrap();
    assert_eq!(store.list("p", "", 10, false).await.unwrap().len(), 1);
    assert_eq!(
        store
            .schedule_background_compaction("p", &AgentConfig::default(), now() + 60)
            .await
            .unwrap(),
        0
    );
    assert_eq!(store.work_details(lease.id).await.unwrap()["state"], "done");
}

#[tokio::test]
async fn stale_compaction_seed_cannot_overwrite_a_new_revision() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let seed = add(&store, "p", "seed", Actor::Agent).await;
    store
        .schedule_compaction("p", "", 10, true, 0, now())
        .await
        .unwrap();
    let lease = store
        .claim_work("compact", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    store
        .update("p", &seed.id, 1, seed.content.clone(), Actor::Human)
        .await
        .unwrap();
    let decisions = serde_json::from_value(json!([{"candidate":0,"action":"discard","target":null,"expected_revision":null,"content":null,"reason":"Keep","related":[]}])).unwrap();
    assert!(
        store
            .complete_consolidation(&lease, decisions, now())
            .await
            .is_err()
    );
    assert_eq!(
        store.show("p", &seed.id, None).await.unwrap().actor,
        Actor::Human
    );
}

struct NoCalls;
#[async_trait::async_trait]
impl Model for NoCalls {
    async fn complete(&self, _: &ModelRequest) -> anyhow::Result<ModelReply> {
        panic!("obsolete compaction must not call a provider")
    }
}
struct Repository(std::path::PathBuf);
#[async_trait::async_trait]
impl ProjectRepository for Repository {
    async fn root(&self, _: &str) -> anyhow::Result<Option<std::path::PathBuf>> {
        Ok(Some(self.0.clone()))
    }
}

#[tokio::test]
async fn worker_finishes_obsolete_compaction_without_model_calls_or_retries() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let seed = add(&store, "p", "seed", Actor::Agent).await;
    let page = store
        .schedule_compaction("p", "", 10, true, 0, now())
        .await
        .unwrap();
    store
        .update("p", &seed.id, 1, seed.content, Actor::Human)
        .await
        .unwrap();
    let worker = MemoryWorker::new(
        store.clone(),
        std::sync::Arc::new(NoCalls),
        AgentConfig::default(),
        std::sync::Arc::new(Repository(temp.path().into())),
    );
    assert!(worker.run_once("compact").await.unwrap());
    assert_eq!(
        store.work_details(page.work_ids[0]).await.unwrap()["state"],
        "done"
    );
    assert_eq!(store.work_counts().await.unwrap().pending, 0);
}

struct CurrentStateModel {
    seed: Memory,
    current: Memory,
    calls: std::sync::atomic::AtomicUsize,
}
#[async_trait::async_trait]
impl Model for CurrentStateModel {
    async fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelReply> {
        assert!(
            request
                .instructions
                .contains("remove obsolete current-state clauses"),
            "compaction must distinguish current claims from their historical quotations"
        );
        let step = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (name, arguments) = if request.tools.len() > 1 && step == 0 {
            ("memory_search", json!({"query":"Server decision"}))
        } else {
            let parts: Vec<_> = [("deployment.path","deployment"),("autostart","enabled"),("reality.domain","new.example")].into_iter().map(|(attribute,value)| {
                let mut content = self.seed.content.clone();
                content.conclusion = format!("{attribute}: {value}");
                content.fact = Some(Fact {entity:"server/sing-box".into(),attribute:attribute.into(),qualifiers:vec![],value:value.into()});
                content.evidence.extend(self.current.content.evidence.clone());
                json!({"content":content,"action":"create","target":null,"expected_revision":null,"reason":"Independent supported fact"})
            }).collect();
            (
                "submit_fact_compaction",
                json!({"parts":parts,"reason":"Split mixed configuration and remove obsolete current-state clauses","related":[{"id":self.current.id,"revision":self.current.revision,"action":"supersede","reason":"Current domain represented independently with all evidence","retained":null,"forget_request":null}]}),
            )
        };
        Ok(ModelReply {
            continuation: json!([]),
            calls: vec![ToolCall {
                id: format!("c{step}"),
                name: name.into(),
                arguments,
            }],
            text: String::new(),
            usage: TokenUsage::default(),
        })
    }
}

#[tokio::test]
async fn compact_worker_distinguishes_current_claims_from_preserved_historical_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let mut seed = add(&store, "p", "old", Actor::Agent).await;
    seed.content.conclusion =
        "Keep deployment and autostart; use old.example for camouflage.".into();
    seed = store
        .update("p", &seed.id, 1, seed.content.clone(), Actor::Agent)
        .await
        .unwrap();
    let mut current = add(&store, "p", "replacement", Actor::Agent).await;
    current.content.conclusion = "Use new.example after replacing old.example.".into();
    current = store
        .update("p", &current.id, 1, current.content.clone(), Actor::Agent)
        .await
        .unwrap();
    // The model sees the exact current revisions, including the setup edits.
    let seed_revision = seed.revision;
    let current_revision = current.revision;
    let model = std::sync::Arc::new(CurrentStateModel {
        seed: seed.clone(),
        current,
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    store
        .schedule_compaction("p", "", 1, true, 0, now())
        .await
        .unwrap();
    let worker = MemoryWorker::new(
        store.clone(),
        model,
        AgentConfig::default(),
        std::sync::Arc::new(Repository(temp.path().into())),
    );
    let result = worker.run_once("compact").await;
    assert!(
        result.is_ok(),
        "{result:?}; setup revisions {seed_revision}/{current_revision}"
    );
    assert_eq!(
        store.show("p", &seed.id, None).await.unwrap().status,
        Status::Superseded
    );
    let active = store.list("p", "", 100, false).await.unwrap();
    assert_eq!(active.len(), 3);
    assert!(
        active
            .iter()
            .any(|m| m.content.conclusion.contains("new.example"))
    );
    assert!(
        active
            .iter()
            .all(|m| !m.content.conclusion.contains("old.example"))
    );
    assert!(
        active
            .iter()
            .any(|m| m.content.fact.as_ref().unwrap().attribute == "deployment.path")
    );
    assert!(
        active
            .iter()
            .any(|m| m.content.fact.as_ref().unwrap().attribute == "autostart")
    );
    assert_eq!(
        store
            .show("p", &seed.id, Some(seed_revision))
            .await
            .unwrap()
            .content,
        seed.content
    );
}

#[tokio::test]
async fn terminal_compaction_failure_requires_manual_retry_or_a_changed_revision() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let seed = add(&store, "p", "seed", Actor::Agent).await;
    store
        .schedule_compaction("p", "", 10, true, 0, now())
        .await
        .unwrap();
    let config = AgentConfig {
        max_attempts: 1,
        ..AgentConfig::default()
    };
    let lease = store
        .claim_work("compact", &config, now())
        .await
        .unwrap()
        .unwrap();
    store
        .fail_work(&lease, "Permanent rejection", now())
        .await
        .unwrap();
    assert_eq!(
        store.work_details(lease.id).await.unwrap()["state"],
        "failed"
    );
    assert_eq!(
        store
            .schedule_background_compaction("p", &config, now() + 172_800)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .schedule_compaction("p", "", 10, true, 0, now() + 172_800)
            .await
            .unwrap()
            .scheduled,
        1
    );
    let manual = store
        .claim_work("compact", &config, now())
        .await
        .unwrap()
        .unwrap();
    store
        .fail_work(&manual, "Permanent rejection", now())
        .await
        .unwrap();
    store
        .update("p", &seed.id, seed.revision, seed.content, Actor::Agent)
        .await
        .unwrap();
    assert_eq!(
        store
            .schedule_background_compaction("p", &config, now() + 60)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn compact_rewrite_appends_seed_evidence_before_validating_the_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let seed = add(&store, "p", "seed", Actor::Agent).await;
    let replacement = add(&store, "p", "replacement", Actor::Agent).await;
    store
        .schedule_compaction("p", "", 1, true, 0, now())
        .await
        .unwrap();
    let lease = store
        .claim_work("compact", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    let decisions = serde_json::from_value(json!([{"candidate":0,"action":"merge","target":seed.id,"expected_revision":seed.revision,"content":replacement.content,"reason":"Supported current replacement","related":[{"id":replacement.id,"revision":replacement.revision,"action":"keep","reason":"Keep replacement source","retained":null,"forget_request":null}]}])).unwrap();
    store
        .complete_consolidation(&lease, decisions, now())
        .await
        .unwrap();
    let current = store.show("p", &seed.id, None).await.unwrap();
    assert_eq!(current.content.evidence.len(), 2);
    assert!(current.content.evidence.contains(&seed.content.evidence[0]));
    assert_eq!(
        store.show("p", &seed.id, Some(1)).await.unwrap().content,
        seed.content
    );
}

#[tokio::test]
async fn compact_worker_can_use_preloaded_current_memories_without_repeating_search() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let seed = add(&store, "p", "seed", Actor::Agent).await;
    let current = add(&store, "p", "replacement", Actor::Agent).await;
    let model = std::sync::Arc::new(CurrentStateModel {
        seed: seed.clone(),
        current,
        calls: std::sync::atomic::AtomicUsize::new(1),
    });
    store
        .schedule_compaction("p", "", 1, true, 0, now())
        .await
        .unwrap();
    let worker = MemoryWorker::new(
        store.clone(),
        model.clone(),
        AgentConfig::default(),
        std::sync::Arc::new(Repository(temp.path().into())),
    );
    worker.run_once("compact").await.unwrap();
    assert_eq!(model.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(
        store.show("p", &seed.id, None).await.unwrap().status,
        Status::Superseded
    );
    assert!(
        store
            .list("p", "", 100, false)
            .await
            .unwrap()
            .iter()
            .any(|m| m.content.conclusion.contains("new.example"))
    );
}

#[tokio::test]
async fn terminal_compaction_suspension_survives_work_retention_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    add(&store, "p", "seed", Actor::Agent).await;
    store
        .schedule_compaction("p", "", 1, true, 0, now())
        .await
        .unwrap();
    let config = AgentConfig {
        max_attempts: 1,
        ..AgentConfig::default()
    };
    let lease = store
        .claim_work("compact", &config, now())
        .await
        .unwrap()
        .unwrap();
    store
        .fail_work(&lease, "Permanent rejection", now())
        .await
        .unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query("DELETE FROM work_items WHERE id=?")
        .bind(lease.id)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    drop(store);
    let store = MemoryStore::open(&path).await.unwrap();
    assert_eq!(
        store
            .schedule_background_compaction("p", &config, now() + 172_800)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .schedule_compaction("p", "", 1, true, 0, now() + 172_800)
            .await
            .unwrap()
            .scheduled,
        1
    );
}

#[tokio::test]
async fn unchanged_compaction_is_not_repeated_after_a_day_or_restart() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    let seed = add(&store, "p", "seed", Actor::Agent).await;
    let config = AgentConfig::default();
    let checked = now() + 60;
    assert_eq!(
        store
            .schedule_background_compaction("p", &config, checked)
            .await
            .unwrap(),
        1
    );
    let lease = store
        .claim_work("compact", &config, checked)
        .await
        .unwrap()
        .unwrap();
    let decisions = serde_json::from_value(json!([{"candidate":0,"action":"discard","reason":"Current conclusion remains valid","related":[]}])).unwrap();
    store
        .complete_consolidation(&lease, decisions, checked)
        .await
        .unwrap();
    drop(store);
    let store = MemoryStore::open(&path).await.unwrap();
    assert_eq!(
        store
            .schedule_background_compaction("p", &config, checked + 172_800)
            .await
            .unwrap(),
        0,
        "a checked unchanged revision must not be re-enqueued on the next day"
    );
    store
        .update("p", &seed.id, seed.revision, seed.content, Actor::Agent)
        .await
        .unwrap();
    assert_eq!(
        store
            .schedule_background_compaction("p", &config, now() + 60)
            .await
            .unwrap(),
        1,
        "a new revision must still trigger compaction"
    );
}

#[tokio::test]
async fn historical_compaction_drains_in_batches_of_at_most_ten() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    for i in 0..23 {
        add(&store, "p", &format!("history{i}"), Actor::Agent).await;
    }
    let config = AgentConfig::default();
    assert_eq!(
        store
            .schedule_background_compaction("p", &config, now() + 60)
            .await
            .unwrap(),
        10,
        "one scheduling pass must not scan a second historical page"
    );
    drop(store);
    let store = MemoryStore::open(&path).await.unwrap();
    assert_eq!(
        store
            .schedule_background_compaction("p", &config, now() + 60)
            .await
            .unwrap(),
        10
    );
    assert_eq!(
        store
            .schedule_background_compaction("p", &config, now() + 60)
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        store
            .schedule_background_compaction("p", &config, now() + 60)
            .await
            .unwrap(),
        0
    );
    assert_eq!(store.work_counts().await.unwrap().pending, 23);
}

#[tokio::test]
async fn compaction_deadline_tracks_dirty_revisions_and_ignores_inflight_work() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let config = AgentConfig::default();
    assert_eq!(
        store.next_compaction_at("p", &config, now()).await.unwrap(),
        None
    );
    let seed = add(&store, "p", "seed", Actor::Agent).await;
    assert_eq!(
        store.next_compaction_at("p", &config, now()).await.unwrap(),
        Some(seed.updated_at + 30)
    );
    store
        .schedule_background_compaction("p", &config, now() + 60)
        .await
        .unwrap();
    let updated = store
        .update("p", &seed.id, 1, seed.content, Actor::Agent)
        .await
        .unwrap();
    assert_eq!(
        store.next_compaction_at("p", &config, now()).await.unwrap(),
        None,
        "a dirty revision with old inflight work must wait for its completion notification"
    );
    let mut changes = store.subscribe_compaction();
    let worker = MemoryWorker::new(
        store.clone(),
        std::sync::Arc::new(NoCalls),
        config.clone(),
        std::sync::Arc::new(Repository(temp.path().into())),
    );
    worker.run_once("obsolete").await.unwrap();
    assert_eq!(changes.try_recv().unwrap(), Some("p".to_owned()));
    assert_eq!(
        store.next_compaction_at("p", &config, now()).await.unwrap(),
        Some(updated.updated_at + 30)
    );
}

#[tokio::test]
async fn an_old_failed_compact_does_not_suspend_a_new_dirty_revision() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let seed = add(&store, "p", "seed", Actor::Agent).await;
    let config = AgentConfig {
        max_attempts: 1,
        ..AgentConfig::default()
    };
    store
        .schedule_background_compaction("p", &config, now() + 60)
        .await
        .unwrap();
    let lease = store
        .claim_work("compact", &config, now())
        .await
        .unwrap()
        .unwrap();
    store
        .update("p", &seed.id, 1, seed.content, Actor::Agent)
        .await
        .unwrap();
    store
        .fail_work(&lease, "old revision rejected", now())
        .await
        .unwrap();
    assert!(
        store
            .next_compaction_at("p", &config, now())
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        store
            .schedule_background_compaction("p", &config, now() + 60)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn an_expired_old_compact_wakes_the_new_revision() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let seed = add(&store, "p", "seed", Actor::Agent).await;
    let config = AgentConfig {
        max_attempts: 1,
        lease_seconds: 1,
        ..AgentConfig::default()
    };
    store
        .schedule_background_compaction("p", &config, now() + 60)
        .await
        .unwrap();
    let lease = store
        .claim_work("expired", &config, now())
        .await
        .unwrap()
        .unwrap();
    store
        .update("p", &seed.id, 1, seed.content, Actor::Agent)
        .await
        .unwrap();
    let mut changes = store.subscribe_compaction();
    assert!(
        store
            .claim_work("recover", &config, lease.lease_until)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        changes.try_recv().unwrap(),
        Some("p".to_owned()),
        "expiry of an old inflight seed must wake a waiting dirty revision"
    );
    assert!(
        store
            .next_compaction_at("p", &config, now())
            .await
            .unwrap()
            .is_some()
    );
}

async fn canonical_pair(store: &MemoryStore) -> (Memory, Memory) {
    let mut older = add(store, "p", "old_deployment", Actor::Agent).await;
    older.content.conclusion =
        "Use www.sakura.ad.jp for REALITY; deployment /root/deploy/sing-box with autostart.".into();
    older = store
        .update(
            "p",
            &older.id,
            older.revision,
            older.content.clone(),
            Actor::Agent,
        )
        .await
        .unwrap();
    let mut canonical = add(store, "p", "confirmed_replacement", Actor::Agent).await;
    canonical.content.conclusion = "Use www.sakura.ad.jp for REALITY; client SNI updated.".into();
    canonical = store
        .update(
            "p",
            &canonical.id,
            canonical.revision,
            canonical.content.clone(),
            Actor::Agent,
        )
        .await
        .unwrap();
    (older, canonical)
}

struct CanonicalModel {
    older: Memory,
    canonical: Memory,
}

#[async_trait::async_trait]
impl Model for CanonicalModel {
    async fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelReply> {
        assert!(
            request
                .instructions
                .contains("independently replaceable atomic facts")
        );
        let parts: Vec<_> = [("deployment.path","/root/deploy/sing-box"),("autostart","enabled"),("reality.domain","www.sakura.ad.jp"),("client.sni","www.sakura.ad.jp")].into_iter().map(|(attribute,value)| {
            let mut content = self.canonical.content.clone();
            content.conclusion = format!("{attribute}: {value}");
            content.fact = Some(Fact {entity:"server/sing-box".into(),attribute:attribute.into(),qualifiers:vec![],value:value.into()});
            content.evidence.extend(self.older.content.evidence.clone());
            json!({"content":content,"action":"create","target":null,"expected_revision":null,"reason":"Independent supported fact"})
        }).collect();
        Ok(ModelReply {
            continuation: json!([]),
            calls: vec![ToolCall {
                id: "split".into(),
                name: "submit_fact_compaction".into(),
                arguments: json!({"parts":parts,"reason":"Split duplicated legacy configurations into atomic facts","related":[{"id":self.canonical.id,"revision":self.canonical.revision,"action":"supersede","reason":"All current facts preserved independently","retained":null,"forget_request":null}]}),
            }],
            text: String::new(),
            usage: TokenUsage::default(),
        })
    }
}

#[tokio::test]
async fn compact_merges_duplicate_current_fact_into_existing_canonical_and_retires_old_seed() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let (older, canonical) = canonical_pair(&store).await;
    let page = store
        .schedule_compaction("p", "", 10, true, 0, now())
        .await
        .unwrap();
    let lease = store
        .claim_work("compact", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.payload["compact"]["id"], older.id);
    let mut content = canonical.content.clone();
    content.conclusion = "Use www.sakura.ad.jp for REALITY; deployment /root/deploy/sing-box with autostart and client SNI updated.".into();
    let decisions = serde_json::from_value(json!([{"candidate":0,"action":"merge","target":canonical.id,"expected_revision":canonical.revision,"content":content,"reason":"Use one canonical current configuration","related":[{"id":older.id,"revision":older.revision,"action":"supersede","reason":"Preserve all stable facts in the canonical configuration","retained":null,"forget_request":null}]}])).unwrap();
    store
        .complete_consolidation(&lease, decisions, now())
        .await
        .unwrap();
    let active = store.list("p", "", 100, false).await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, canonical.id);
    assert_eq!(active[0].revision, canonical.revision + 1);
    assert!(active[0].content.conclusion.contains("www.sakura.ad.jp"));
    assert!(
        active[0]
            .content
            .conclusion
            .contains("/root/deploy/sing-box with autostart")
    );
    assert!(active[0].content.conclusion.contains("client SNI"));
    for quote in older
        .content
        .evidence
        .iter()
        .chain(&canonical.content.evidence)
    {
        assert!(active[0].content.evidence.contains(quote));
    }
    let retired = store.show("p", &older.id, None).await.unwrap();
    assert_eq!(retired.status, Status::Superseded);
    assert_eq!(
        retired.superseded_by.as_deref(),
        Some(canonical.id.as_str())
    );
    assert_eq!(store.list("p", "", 100, true).await.unwrap().len(), 2);
    for original in [&older, &canonical] {
        assert_eq!(
            store
                .show("p", &original.id, Some(original.revision))
                .await
                .unwrap()
                .content,
            original.content
        );
    }
    let worker = MemoryWorker::new(
        store.clone(),
        std::sync::Arc::new(NoCalls),
        AgentConfig::default(),
        std::sync::Arc::new(Repository(temp.path().into())),
    );
    assert!(worker.run_once("compact").await.unwrap());
    assert_eq!(
        store.work_details(page.work_ids[1]).await.unwrap()["state"],
        "done"
    );
    assert_eq!(
        store
            .schedule_background_compaction("p", &AgentConfig::default(), now() + 60)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn compact_worker_splits_old_seed_and_related_record_into_independent_facts() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let (older, canonical) = canonical_pair(&store).await;
    store
        .schedule_compaction("p", "", 1, true, 0, now())
        .await
        .unwrap();
    let worker = MemoryWorker::new(
        store.clone(),
        std::sync::Arc::new(CanonicalModel {
            older: older.clone(),
            canonical: canonical.clone(),
        }),
        AgentConfig::default(),
        std::sync::Arc::new(Repository(temp.path().into())),
    );
    worker.run_once("compact").await.unwrap();
    assert_eq!(
        store.show("p", &older.id, None).await.unwrap().status,
        Status::Superseded
    );
    let active = store.list("p", "", 100, false).await.unwrap();
    assert_eq!(active.len(), 4);
    assert_eq!(
        store.show("p", &canonical.id, None).await.unwrap().status,
        Status::Superseded
    );
    assert!(active.iter().all(|m| m.content.fact.is_some()));
    assert_eq!(
        active
            .iter()
            .filter(|m| m.content.fact.as_ref().unwrap().attribute == "reality.domain")
            .count(),
        1
    );
}

#[tokio::test]
async fn compact_retarget_requires_exact_seed_retirement_without_a_retained_duplicate() {
    for (action, revision_delta, retain, include_seed) in [
        ("keep", 0, false, true),
        ("conflict", 0, false, true),
        ("supersede", 1, false, true),
        ("supersede", 0, true, true),
        ("supersede", 0, false, false),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&temp.path().join("memory.db"))
            .await
            .unwrap();
        let older = add(&store, "p", "older", Actor::Agent).await;
        let canonical = add(&store, "p", "canonical", Actor::Agent).await;
        store
            .schedule_compaction("p", "", 1, true, 0, now())
            .await
            .unwrap();
        let lease = store
            .claim_work("compact", &AgentConfig::default(), now())
            .await
            .unwrap()
            .unwrap();
        let related = if include_seed {
            json!([{"id":older.id,"revision":older.revision + revision_delta,"action":action,"reason":"Incomplete seed retirement","retained":if retain {Some(older.content.clone())} else {None},"forget_request":null}])
        } else {
            json!([])
        };
        let decisions = serde_json::from_value(json!([{"candidate":0,"action":"merge","target":canonical.id,"expected_revision":canonical.revision,"content":canonical.content,"reason":"Invalid retarget","related":related}])).unwrap();
        assert!(
            store
                .complete_consolidation(&lease, decisions, now())
                .await
                .is_err()
        );
        assert_eq!(
            store.show("p", &older.id, None).await.unwrap().revision,
            older.revision
        );
        assert_eq!(
            store.show("p", &canonical.id, None).await.unwrap().revision,
            canonical.revision
        );
        assert_eq!(
            store.work_details(lease.id).await.unwrap()["state"],
            "running"
        );
    }
}

#[tokio::test]
async fn compact_canonical_seed_retires_older_duplicate_without_creating_another_record() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let (older, canonical) = canonical_pair(&store).await;
    store
        .schedule_compaction("p", &older.id, 1, true, 0, now())
        .await
        .unwrap();
    let lease = store
        .claim_work("compact", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.payload["compact"]["id"], canonical.id);
    let mut content = canonical.content.clone();
    content.conclusion = "Use www.sakura.ad.jp for REALITY; deployment /root/deploy/sing-box with autostart and client SNI updated.".into();
    content.evidence.extend(older.content.evidence.clone());
    let decisions = serde_json::from_value(json!([{"candidate":0,"action":"merge","target":canonical.id,"expected_revision":canonical.revision,"content":content,"reason":"Retire the older overlapping record","related":[{"id":older.id,"revision":older.revision,"action":"supersede","reason":"Stable facts merged into canonical","retained":null,"forget_request":null}]}])).unwrap();
    store
        .complete_consolidation(&lease, decisions, now())
        .await
        .unwrap();
    assert_eq!(store.list("p", "", 100, false).await.unwrap().len(), 1);
    assert_eq!(
        store
            .show("p", &older.id, None)
            .await
            .unwrap()
            .superseded_by
            .as_deref(),
        Some(canonical.id.as_str())
    );
    assert_eq!(store.list("p", "", 100, true).await.unwrap().len(), 2);
}

#[tokio::test]
async fn compact_canonical_merge_rolls_back_for_stale_or_human_target() {
    for actor in [Actor::Agent, Actor::Human] {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&temp.path().join("memory.db"))
            .await
            .unwrap();
        let (older, canonical) = canonical_pair(&store).await;
        store
            .schedule_compaction("p", "", 1, true, 0, now())
            .await
            .unwrap();
        let lease = store
            .claim_work("compact", &AgentConfig::default(), now())
            .await
            .unwrap()
            .unwrap();
        let updated = store
            .update(
                "p",
                &canonical.id,
                canonical.revision,
                canonical.content.clone(),
                actor,
            )
            .await
            .unwrap();
        let expected_revision = if actor == Actor::Human {
            updated.revision
        } else {
            canonical.revision
        };
        let decisions = serde_json::from_value(json!([{"candidate":0,"action":"merge","target":canonical.id,"expected_revision":expected_revision,"content":canonical.content,"reason":"Merge duplicate with guarded target","related":[{"id":older.id,"revision":older.revision,"action":"supersede","reason":"Retire old duplicate","retained":null,"forget_request":null}]}])).unwrap();
        let error = store
            .complete_consolidation(&lease, decisions, now())
            .await
            .unwrap_err();
        assert!(error.to_string().contains(if actor == Actor::Human {
            "preserve human-authored"
        } else {
            "target changed"
        }));
        assert_eq!(
            store.show("p", &older.id, None).await.unwrap().revision,
            older.revision
        );
        assert_eq!(
            store.show("p", &older.id, None).await.unwrap().status,
            Status::Active
        );
        assert_eq!(
            store.show("p", &canonical.id, None).await.unwrap().content,
            updated.content
        );
        assert_eq!(
            store.work_details(lease.id).await.unwrap()["state"],
            "running"
        );
    }
}

#[tokio::test]
async fn ineligible_revisions_do_not_accumulate_in_the_dirty_index() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    add(&store, "p", "human", Actor::Human).await;
    let old = add(&store, "p", "archived", Actor::Agent).await;
    store
        .set_status(
            "p",
            &old.id,
            old.revision,
            Status::Archived,
            "Historical",
            Actor::Agent,
        )
        .await
        .unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let dirty: i64 = sqlx::query_scalar("SELECT count(*) FROM memory_compactions WHERE dirty=1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        dirty, 0,
        "human and retired revisions never need semantic maintenance"
    );
    assert!(
        store
            .next_compaction_at("p", &AgentConfig::default(), now())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn expiry_before_debounce_is_drained_without_model_work() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    for i in 0..13 {
        let seed = add(&store, "p", &format!("r{i}"), Actor::Agent).await;
        let mut content = seed.content.clone();
        content.valid_until = Some(now() + 10);
        store
            .update("p", &seed.id, seed.revision, content, Actor::Agent)
            .await
            .unwrap();
    }
    let future = now() + 60;
    let config = AgentConfig::default();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    assert_eq!(
        store
            .schedule_background_compaction("p", &config, future)
            .await
            .unwrap(),
        0
    );
    let dirty: i64 = sqlx::query_scalar("SELECT count(*) FROM memory_compactions WHERE dirty=1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        dirty, 3,
        "expired revisions must be retired from the dirty queue in bounded pages"
    );
    assert!(
        store
            .next_compaction_at("p", &config, future)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        store
            .schedule_background_compaction("p", &config, future)
            .await
            .unwrap(),
        0
    );
    assert!(
        store
            .next_compaction_at("p", &config, future)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(store.work_counts().await.unwrap().pending, 0);
}
