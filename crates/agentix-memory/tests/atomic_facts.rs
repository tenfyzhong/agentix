use agentix_memory::*;
use serde_json::json;

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn input(receipt: &str, attribute: &str, value: &str) -> MemoryInput {
    serde_json::from_value(json!({
        "title":format!("Sing-box {attribute}"),"conclusion":value,
        "rationale":"Explicit external configuration decision","scope":"dogyun sing-box",
        "conditions":[],"tags":["sing-box"],"kind":"user_decision",
        "fact":{"entity":"dogyun/sing-box","attribute":attribute,"qualifiers":[],"value":value},
        "evidence":[{"receipt_id":receipt,"message_id":"m","quote":value}]
    }))
    .expect("an atomic memory needs a structured fact identity and value")
}

async fn receipt(store: &MemoryStore, id: &str, text: &str, at: i64) {
    let source: Source = serde_json::from_value(json!({"instance_id":"db","receipt_id":id,"sequence":at,"project_id":"p","session_id":"s","turn_id":id,"revision":1,"job_id":null,"recorded_at":at,"messages":[{"id":"m","role":"user","text":text}]})).unwrap();
    store.ingest(&source).await.unwrap();
}

async fn write(
    store: &MemoryStore,
    content: &MemoryInput,
    target: Option<&Memory>,
    action: &str,
) -> anyhow::Result<Vec<Memory>> {
    let extract = store
        .claim_work("test", &AgentConfig::default(), now())
        .await?
        .unwrap();
    store
        .complete_extraction(&extract, vec![content.clone()], now())
        .await?;
    let consolidate = store
        .claim_work("test", &AgentConfig::default(), now())
        .await?
        .unwrap();
    let decisions = serde_json::from_value(
        json!([{"candidate":0,"action":action,"target":target.map(|m| &m.id),"expected_revision":target.map(|m| m.revision),"content":content,"reason":"Confirmed atomic fact","related":[]}]),
    )?;
    store
        .complete_consolidation(&consolidate, decisions, now())
        .await
}

#[tokio::test]
async fn fact_identity_and_value_round_trip_independently_of_title() {
    let content = input("r", "reality.domain", "old.example");
    let value = serde_json::to_value(content).unwrap();
    assert_eq!(value["fact"]["attribute"], "reality.domain");
    assert_eq!(value["fact"]["value"], "old.example");
}

#[tokio::test]
async fn write_time_replacement_preserves_other_facts_and_does_not_schedule_compaction() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let mut originals = Vec::new();
    for (id, attribute, value) in [
        ("path", "deployment.path", "/root/deploy/sing-box"),
        ("boot", "autostart", "enabled"),
        ("domain", "reality.domain", "old.example"),
    ] {
        receipt(&store, id, value, now() - 100).await;
        let content = input(id, attribute, value);
        originals.push(
            write(&store, &content, None, "create")
                .await
                .unwrap()
                .remove(0),
        );
    }
    let old = originals.last().unwrap();
    receipt(&store, "replacement", "new.example", now()).await;
    let current = write(
        &store,
        &input("replacement", "reality.domain", "new.example"),
        Some(old),
        "supersede",
    )
    .await
    .unwrap();
    let active = store.list("p", "", 100, false).await.unwrap();
    assert_eq!(active.len(), 3);
    assert!(active.iter().any(|m| m.content.conclusion == "new.example"));
    assert_eq!(
        store.show("p", &old.id, None).await.unwrap().status,
        Status::Superseded
    );
    assert!(
        current
            .iter()
            .any(|m| m.supersedes.as_deref() == Some(old.id.as_str()))
    );
    for original in originals.iter().take(2) {
        assert_eq!(
            store.show("p", &original.id, None).await.unwrap(),
            *original
        );
    }
    assert_eq!(
        store.show("p", &old.id, Some(1)).await.unwrap().content,
        old.content
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
async fn duplicate_and_changed_values_cannot_create_a_second_active_fact() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    receipt(&store, "original", "old.example", now() - 100).await;
    let original = write(
        &store,
        &input("original", "reality.domain", "old.example"),
        None,
        "create",
    )
    .await
    .unwrap()
    .remove(0);
    receipt(&store, "duplicate", "old.example", now()).await;
    let updated = write(
        &store,
        &input("duplicate", "reality.domain", "old.example"),
        Some(&original),
        "merge",
    )
    .await
    .unwrap()
    .remove(0);
    assert_eq!(updated.id, original.id);
    assert_eq!(updated.content.evidence.len(), 2);
    receipt(&store, "collision", "new.example", now() + 1).await;
    assert!(
        write(
            &store,
            &input("collision", "reality.domain", "new.example"),
            None,
            "create"
        )
        .await
        .is_err()
    );
    assert_eq!(store.list("p", "", 100, false).await.unwrap().len(), 1);
}

#[tokio::test]
async fn older_evidence_cannot_replace_a_newer_fact_and_rolls_back_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    receipt(&store, "current", "new.example", now()).await;
    let current = write(
        &store,
        &input("current", "reality.domain", "new.example"),
        None,
        "create",
    )
    .await
    .unwrap()
    .remove(0);
    receipt(&store, "backfill", "old.example", now() - 1000).await;
    let error = write(
        &store,
        &input("backfill", "reality.domain", "old.example"),
        Some(&current),
        "supersede",
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("stale fact evidence"));
    assert_eq!(store.show("p", &current.id, None).await.unwrap(), current);
    assert_eq!(store.list("p", "", 100, true).await.unwrap().len(), 1);
}

#[tokio::test]
async fn value_edits_and_cross_fact_merges_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    receipt(&store, "domain", "old.example", now()).await;
    let domain = write(
        &store,
        &input("domain", "reality.domain", "old.example"),
        None,
        "create",
    )
    .await
    .unwrap()
    .remove(0);
    let mut content = domain.content.clone();
    content.fact.as_mut().unwrap().value = "new.example".into();
    assert!(
        store
            .update("p", &domain.id, domain.revision, content, Actor::Agent)
            .await
            .is_err()
    );
    receipt(&store, "path", "/srv/proxy", now()).await;
    assert!(
        write(
            &store,
            &input("path", "deployment.path", "/srv/proxy"),
            Some(&domain),
            "merge"
        )
        .await
        .is_err()
    );
    assert_eq!(store.show("p", &domain.id, None).await.unwrap(), domain);
}

#[tokio::test]
async fn unresolved_conflicts_leave_default_retrieval_but_remain_inspectable() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    receipt(&store, "old", "old.example", now()).await;
    let old = write(
        &store,
        &input("old", "reality.domain", "old.example"),
        None,
        "create",
    )
    .await
    .unwrap()
    .remove(0);
    receipt(&store, "uncertain", "other.example", now()).await;
    write(
        &store,
        &input("uncertain", "reality.domain", "other.example"),
        Some(&old),
        "conflict",
    )
    .await
    .unwrap();
    assert!(store.list("p", "", 100, false).await.unwrap().is_empty());
    assert!(store.search("p", "Sing-box", 10).await.unwrap().is_empty());
    assert_eq!(store.list("p", "", 100, true).await.unwrap().len(), 2);
    assert_eq!(
        store.show("p", &old.id, None).await.unwrap().status,
        Status::Conflicted
    );
}

struct Repository(std::path::PathBuf);
#[async_trait::async_trait]
impl ProjectRepository for Repository {
    async fn root(&self, _: &str) -> anyhow::Result<Option<std::path::PathBuf>> {
        Ok(Some(self.0.clone()))
    }
}
struct SplitModel(Vec<MemoryInput>);
#[async_trait::async_trait]
impl Model for SplitModel {
    async fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelReply> {
        assert!(
            request
                .tools
                .iter()
                .all(|tool| tool.name == "submit_fact_compaction"),
            "complete preloaded snapshots must go directly to a split proposal, without repeated evidence reads"
        );
        Ok(ModelReply {
            continuation: json!([]),
            calls: vec![ToolCall {
                id: "split".into(),
                name: "submit_fact_compaction".into(),
                arguments: json!({"parts":self.0.iter().map(|content| json!({"content":content,"action":"create","target":null,"expected_revision":null,"reason":"Independent configuration fact"})).collect::<Vec<_>>(),"related":[],"reason":"Split the mixed deployment record into independently replaceable facts"}),
            }],
            text: String::new(),
            usage: TokenUsage::default(),
        })
    }
}

#[tokio::test]
async fn legacy_compaction_splits_path_autostart_and_domain_without_recursive_work() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let text = "Deploy at /srv/proxy, enable autostart, and use old.example.";
    receipt(&store, "legacy", text, now()).await;
    let extract = store
        .claim_work("setup", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    store
        .complete_extraction(&extract, vec![], now())
        .await
        .unwrap();
    let mut legacy = serde_json::to_value(input("legacy", "deployment.path", text)).unwrap();
    legacy.as_object_mut().unwrap().remove("fact");
    let old = store
        .create("p", serde_json::from_value(legacy).unwrap(), Actor::Agent)
        .await
        .unwrap();
    let parts = [
        ("deployment.path", "/srv/proxy"),
        ("autostart", "enabled"),
        ("reality.domain", "old.example"),
    ]
    .into_iter()
    .map(|(attribute, value)| {
        let mut content = input("legacy", attribute, value);
        content.evidence[0].quote = text.into();
        content
    })
    .collect();
    store
        .schedule_compaction("p", "", 10, true, 0, now())
        .await
        .unwrap();
    let worker = MemoryWorker::new(
        store.clone(),
        std::sync::Arc::new(SplitModel(parts)),
        AgentConfig::default(),
        std::sync::Arc::new(Repository(temp.path().into())),
    );
    worker
        .run_once("compact")
        .await
        .expect("compact must split one mixed seed into independent fact memories");
    let active = store.list("p", "", 100, false).await.unwrap();
    assert_eq!(active.len(), 3);
    assert!(active.iter().all(|m| m.content.fact.is_some()));
    assert!(active.iter().all(|m| {
        serde_json::to_value(m).unwrap()["derived_from"]
            .as_array()
            .is_some_and(|origins| origins.contains(&json!(old.id)))
    }));

    assert_eq!(
        store.show("p", &old.id, None).await.unwrap().status,
        Status::Superseded
    );
    assert_eq!(
        store.show("p", &old.id, Some(1)).await.unwrap().content,
        old.content
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
async fn simultaneous_writes_can_commit_only_one_active_version_of_a_fact() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    for id in ["left", "right"] {
        receipt(&store, id, "old.example", now()).await;
        let lease = store
            .claim_work("setup", &AgentConfig::default(), now())
            .await
            .unwrap()
            .unwrap();
        store
            .complete_extraction(&lease, vec![], now())
            .await
            .unwrap();
    }
    let (left, right) = tokio::join!(
        store.create(
            "p",
            input("left", "reality.domain", "old.example"),
            Actor::Agent
        ),
        store.create(
            "p",
            input("right", "reality.domain", "old.example"),
            Actor::Agent
        )
    );
    assert_ne!(left.is_ok(), right.is_ok());
    assert_eq!(store.list("p", "", 100, true).await.unwrap().len(), 1);
}

#[tokio::test]
async fn different_qualifiers_keep_independent_facts_and_qualifier_order_is_not_identity() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    receipt(&store, "ports", "old.example", now()).await;
    let lease = store
        .claim_work("setup", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    store
        .complete_extraction(&lease, vec![], now())
        .await
        .unwrap();
    let mut a = input("ports", "reality.domain", "old.example");
    a.fact.as_mut().unwrap().qualifiers = vec![
        FactQualifier {
            name: "port".into(),
            value: "443".into(),
        },
        FactQualifier {
            name: "transport".into(),
            value: "tcp".into(),
        },
    ];
    store.create("p", a.clone(), Actor::Agent).await.unwrap();
    a.fact.as_mut().unwrap().qualifiers.reverse();
    assert!(store.create("p", a.clone(), Actor::Agent).await.is_err());
    a.fact.as_mut().unwrap().qualifiers[1].value = "10443".into();
    store.create("p", a, Actor::Agent).await.unwrap();
    assert_eq!(store.list("p", "", 100, false).await.unwrap().len(), 2);
}

#[tokio::test]
async fn stale_revisions_and_human_facts_cannot_be_overwritten() {
    for actor in [Actor::Agent, Actor::Human] {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&temp.path().join("memory.db"))
            .await
            .unwrap();
        receipt(&store, "original", "old.example", now() - 100).await;
        let old = write(
            &store,
            &input("original", "reality.domain", "old.example"),
            None,
            "create",
        )
        .await
        .unwrap()
        .remove(0);
        let updated = store
            .update("p", &old.id, old.revision, old.content.clone(), actor)
            .await
            .unwrap();
        receipt(&store, "replacement", "new.example", now()).await;
        let target = if actor == Actor::Human {
            &updated
        } else {
            &old
        };
        assert!(
            write(
                &store,
                &input("replacement", "reality.domain", "new.example"),
                Some(target),
                "supersede"
            )
            .await
            .is_err()
        );
        assert_eq!(store.show("p", &old.id, None).await.unwrap(), updated);
    }
}

#[tokio::test]
async fn legacy_split_evidence_failure_rolls_back_all_parts_and_seed_retirement() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let text = "Deploy at /srv/proxy and enable autostart.";
    receipt(&store, "legacy", text, now()).await;
    let lease = store
        .claim_work("setup", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    store
        .complete_extraction(&lease, vec![], now())
        .await
        .unwrap();
    let mut legacy = serde_json::to_value(input("legacy", "deployment.path", text)).unwrap();
    legacy.as_object_mut().unwrap().remove("fact");
    let old = store
        .create("p", serde_json::from_value(legacy).unwrap(), Actor::Agent)
        .await
        .unwrap();
    store
        .schedule_compaction("p", "", 10, true, 0, now())
        .await
        .unwrap();
    let lease = store
        .claim_work("compact", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    let mut path = input("legacy", "deployment.path", "/srv/proxy");
    path.evidence[0].quote = text.into();
    let mut boot = input("legacy", "autostart", "enabled");
    boot.evidence[0].quote = "Invented source quotation".into();
    let parts = vec![path, boot]
        .into_iter()
        .map(|content| FactPart {
            content,
            action: DecisionAction::Create,
            target: None,
            expected_revision: None,
            reason: "Split independent fact".into(),
        })
        .collect();
    let proposal = FactCompaction {
        parts,
        related: vec![],
        reason: "Split legacy record".into(),
    };
    assert!(
        store
            .complete_fact_compaction(&lease, proposal, now())
            .await
            .is_err()
    );
    assert_eq!(store.show("p", &old.id, None).await.unwrap(), old);
    assert_eq!(store.list("p", "", 100, true).await.unwrap().len(), 1);
}

#[tokio::test]
async fn conflict_diagnostics_default_to_a_bounded_first_page() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let tools = ProjectTools::new(store, "p".into(), Some(temp.path().into())).unwrap();
    let result = tools
        .execute("memory_conflicts", json!({}))
        .await
        .expect("an omitted cursor means the first bounded conflict page");
    assert_eq!(result, json!([]));
}

struct AtomicExtractionModel {
    calls: std::sync::atomic::AtomicUsize,
    candidates: Vec<MemoryInput>,
}
#[async_trait::async_trait]
impl Model for AtomicExtractionModel {
    async fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelReply> {
        let step = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (name, arguments) = match step {
            0 => ("repo_search", json!({"query":"sing-box"})),
            1 => {
                let mut mixed = self.candidates[0].clone();
                mixed.fact = None;
                ("submit_candidates", json!({"candidates":[mixed]}))
            }
            2 => {
                assert!(request.history.iter().any(|m| matches!(m,Message::Tool {output,..} if output.contains("structured atomic fact"))));
                ("submit_candidates", json!({"candidates":self.candidates}))
            }
            _ => (
                "submit_decisions",
                json!({"decisions":(0..self.candidates.len()).map(|candidate| json!({"candidate":candidate,"action":"create","target":null,"expected_revision":null,"content":null,"reason":"Independent confirmed fact","related":[]})).collect::<Vec<_>>()}),
            ),
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
async fn worker_rejects_mixed_extraction_then_writes_all_three_atomic_facts_as_settled() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    receipt(
        &store,
        "source",
        "Deploy at /srv/proxy; autostart enabled; domain old.example.",
        now(),
    )
    .await;
    let candidates = [
        ("deployment.path", "/srv/proxy"),
        ("autostart", "enabled"),
        ("reality.domain", "old.example"),
    ]
    .into_iter()
    .map(|(attribute, value)| input("source", attribute, value))
    .collect();
    let model = std::sync::Arc::new(AtomicExtractionModel {
        calls: std::sync::atomic::AtomicUsize::new(0),
        candidates,
    });
    let worker = MemoryWorker::new(
        store.clone(),
        model.clone(),
        AgentConfig::default(),
        std::sync::Arc::new(Repository(temp.path().into())),
    );
    assert!(worker.run_once("extract").await.unwrap());
    assert!(worker.run_once("consolidate").await.unwrap());
    assert_eq!(store.list("p", "", 100, false).await.unwrap().len(), 3);
    assert_eq!(model.calls.load(std::sync::atomic::Ordering::SeqCst), 4);
    assert_eq!(
        store
            .schedule_background_compaction("p", &AgentConfig::default(), now() + 60)
            .await
            .unwrap(),
        0
    );
}

struct QuoteCorrectionModel {
    calls: std::sync::atomic::AtomicUsize,
    content: MemoryInput,
}
#[async_trait::async_trait]
impl Model for QuoteCorrectionModel {
    async fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelReply> {
        let step = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut content = self.content.clone();
        if step == 0 {
            content.evidence[0].quote = "Invented quotation".into();
        } else {
            assert!(request.history.iter().any(|m| matches!(m, Message::Tool {output,..} if output.contains("literal source quotation"))));
        }
        Ok(ModelReply {
            continuation: json!([]),
            calls: vec![ToolCall {
                id: format!("quote{step}"),
                name: "submit_fact_compaction".into(),
                arguments: json!({"parts":[{"content":content,"action":"create","target":null,"expected_revision":null,"reason":"Independent deployment fact"}],"related":[],"reason":"Split legacy record"}),
            }],
            text: String::new(),
            usage: TokenUsage::default(),
        })
    }
}

#[tokio::test]
async fn legacy_split_returns_literal_quote_errors_to_model_before_writing() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let text = "Deploy at /srv/proxy.";
    receipt(&store, "legacy", text, now()).await;
    let lease = store
        .claim_work("setup", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    store
        .complete_extraction(&lease, vec![], now())
        .await
        .unwrap();
    let mut part = input("legacy", "deployment.path", "/srv/proxy");
    part.evidence[0].quote = text.into();
    let mut legacy = part.clone();
    legacy.fact = None;
    let old = store.create("p", legacy, Actor::Agent).await.unwrap();
    store
        .schedule_compaction("p", "", 10, true, 0, now())
        .await
        .unwrap();
    let model = std::sync::Arc::new(QuoteCorrectionModel {
        calls: std::sync::atomic::AtomicUsize::new(0),
        content: part,
    });
    let worker = MemoryWorker::new(
        store.clone(),
        model.clone(),
        AgentConfig::default(),
        std::sync::Arc::new(Repository(temp.path().into())),
    );
    worker
        .run_once("compact")
        .await
        .expect("model must correct altered quotation before transaction");
    assert_eq!(model.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(
        store.show("p", &old.id, None).await.unwrap().status,
        Status::Superseded
    );
    assert_eq!(store.list("p", "", 100, false).await.unwrap().len(), 1);
}

#[tokio::test]
async fn schema_upgrade_reopens_settled_legacy_records_once_including_null_fact() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("memory.db");
    let store = MemoryStore::open(&path).await.unwrap();
    receipt(&store, "legacy", "/srv/proxy", now()).await;
    let lease = store
        .claim_work("setup", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    store
        .complete_extraction(&lease, vec![], now())
        .await
        .unwrap();
    let mut legacy = input("legacy", "deployment.path", "/srv/proxy");
    legacy.fact = None;
    let old = store.create("p", legacy, Actor::Agent).await.unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query("UPDATE memories SET data=json_set(data,'$.content.fact',json('null')) WHERE id=?")
        .bind(&old.id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE memory_compactions SET dirty=0,checked_at=?,suspended=1")
        .bind(now())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("PRAGMA user_version=2")
        .execute(&pool)
        .await
        .unwrap();
    let upgraded = MemoryStore::open(&path).await.unwrap();
    assert_eq!(upgraded.schema_version().await.unwrap(), 3);
    assert_eq!(
        upgraded
            .schedule_background_compaction("p", &AgentConfig::default(), now() + 60)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store.show("p", &old.id, Some(1)).await.unwrap().content,
        old.content
    );
    sqlx::query("UPDATE memory_compactions SET dirty=0,suspended=1")
        .execute(&pool)
        .await
        .unwrap();
    let reopened = MemoryStore::open(&path).await.unwrap();
    assert_eq!(
        reopened
            .schedule_background_compaction("p", &AgentConfig::default(), now() + 120)
            .await
            .unwrap(),
        0
    );
}

struct BoundedSplitModel {
    calls: std::sync::atomic::AtomicUsize,
    content: MemoryInput,
}
#[async_trait::async_trait]
impl Model for BoundedSplitModel {
    async fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelReply> {
        let step = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let final_step = request.history.iter().any(|message| {
            matches!(message, Message::User(text) if text.contains("This is the final step"))
        });
        let mut content = self.content.clone();
        if !final_step {
            content.evidence[0].quote = "Needs a corrected literal quotation".into();
        }
        let name = "submit_fact_compaction";
        let arguments = json!({"parts":[{"content":content,"action":"create","target":null,"expected_revision":null,"reason":"Independent deployment fact"}],"related":[],"reason":"Use supplied complete evidence"});
        Ok(ModelReply {
            continuation: json!([]),
            calls: vec![ToolCall {
                id: format!("bounded{step}"),
                name: name.into(),
                arguments,
            }],
            text: String::new(),
            usage: TokenUsage::default(),
        })
    }
}

#[tokio::test]
async fn legacy_split_bounds_proposal_corrections_and_preserves_a_lower_user_budget() {
    for requested_steps in [12, 2] {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&temp.path().join("memory.db"))
            .await
            .unwrap();
        receipt(&store, "legacy", "/srv/proxy", now()).await;
        let lease = store
            .claim_work("setup", &AgentConfig::default(), now())
            .await
            .unwrap()
            .unwrap();
        store
            .complete_extraction(&lease, vec![], now())
            .await
            .unwrap();
        let part = input("legacy", "deployment.path", "/srv/proxy");
        let mut legacy = part.clone();
        legacy.fact = None;
        let old = store.create("p", legacy, Actor::Agent).await.unwrap();
        store
            .schedule_compaction("p", "", 10, true, 0, now())
            .await
            .unwrap();
        let model = std::sync::Arc::new(BoundedSplitModel {
            calls: std::sync::atomic::AtomicUsize::new(0),
            content: part,
        });
        let config = AgentConfig {
            max_steps: requested_steps,
            ..AgentConfig::default()
        };
        let worker = MemoryWorker::new(
            store.clone(),
            model.clone(),
            config,
            std::sync::Arc::new(Repository(temp.path().into())),
        );
        worker.run_once("compact").await.unwrap();
        assert!(
            model.calls.load(std::sync::atomic::Ordering::SeqCst) <= requested_steps.min(4),
            "legacy splitting must submit promptly from preloaded evidence and honor a lower user budget"
        );
        assert_eq!(
            store.show("p", &old.id, None).await.unwrap().status,
            Status::Superseded
        );
        assert_eq!(store.list("p", "", 100, false).await.unwrap().len(), 1);
    }
}

struct ReferencedSplitModel {
    calls: std::sync::atomic::AtomicUsize,
    content: MemoryInput,
    seed: Memory,
}
#[async_trait::async_trait]
impl Model for ReferencedSplitModel {
    async fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelReply> {
        let step = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let preload: serde_json::Value = serde_json::from_str(
            request
                .history
                .iter()
                .find_map(|m| {
                    if let Message::User(text) = m {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            preload["migration_scope"], self.seed.content.conclusion,
            "migration scope must describe the current retained conclusion, not all example attributes"
        );
        assert!(
            preload.get("candidates").is_none(),
            "legacy seed quotes must not be duplicated in an extra candidate payload"
        );
        let schema = &request.tools[0].parameters["properties"]["parts"]["items"]["properties"]["content"]
            ["properties"]["evidence"]["items"]["anyOf"][0]["properties"];
        assert_eq!(schema["memory_id"]["enum"], json!([self.seed.id]));
        assert_eq!(schema["quote_index"]["enum"], json!([0]));
        assert!(request.tools[0].parameters["properties"]["parts"]["items"]["properties"]["content"]["properties"]["fact"]["properties"].is_object(), "a submitted atomic part cannot advertise a nullable fact");
        if step > 0 {
            let expected = match step {
                1 => "unknown preloaded quote source",
                2 => "valid indices are 0..1",
                _ => "invalid type: null",
            };
            assert!(
                request
                    .history
                    .iter()
                    .any(|m| matches!(m, Message::Tool {output,..} if output.contains(expected)))
            );
        }
        let mut content = serde_json::to_value(&self.content)?;
        let id = if step == 0 {
            "not-preloaded"
        } else {
            &self.seed.id
        };
        let index = if step == 1 { 999 } else { 0 };
        content["evidence"] = json!([{"memory_id":id,"quote_index":index}]);
        if step == 2 {
            content["title"] = serde_json::Value::Null;
        }
        Ok(ModelReply {
            continuation: json!([]),
            calls: vec![ToolCall {
                id: "referenced".into(),
                name: "submit_fact_compaction".into(),
                arguments: json!({"parts":[{"content":content,"action":"create","target":null,"expected_revision":null,"reason":"Independent supported fact"}],"related":[],"reason":"Reuse exact preloaded evidence"}),
            }],
            text: String::new(),
            usage: TokenUsage::default(),
        })
    }
}

#[tokio::test]
async fn legacy_split_resolves_quote_references_without_model_repeating_source_text() {
    scoped_reference_split(false).await;
}

#[tokio::test]
async fn legacy_split_scopes_a_retained_record_to_its_current_conclusion() {
    scoped_reference_split(true).await;
}

async fn scoped_reference_split(stale_title: bool) {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let text = "Deploy at /srv/proxy. Preserve every original character!";
    receipt(&store, "legacy", text, now()).await;
    let lease = store
        .claim_work("setup", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    store
        .complete_extraction(&lease, vec![], now())
        .await
        .unwrap();
    let mut part = input("legacy", "deployment.path", "/srv/proxy");
    part.evidence[0].quote = text.into();
    let mut legacy = part.clone();
    legacy.fact = None;
    if stale_title {
        legacy.title = "Former combined SSH, camouflage domain and autostart configuration".into();
    }
    let old = store.create("p", legacy, Actor::Agent).await.unwrap();
    store
        .schedule_compaction("p", "", 10, true, 0, now())
        .await
        .unwrap();
    let worker = MemoryWorker::new(
        store.clone(),
        std::sync::Arc::new(ReferencedSplitModel {
            calls: std::sync::atomic::AtomicUsize::new(0),
            content: part,
            seed: old.clone(),
        }),
        AgentConfig::default(),
        std::sync::Arc::new(Repository(temp.path().into())),
    );
    worker
        .run_once("compact")
        .await
        .expect("resolve only supplied quote references before fenced validation");
    let current = store.list("p", "", 100, false).await.unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].content.evidence, old.content.evidence);
    assert_eq!(current[0].derived_from, vec![old.id.clone()]);
    assert_eq!(
        store.show("p", &old.id, Some(1)).await.unwrap().content,
        old.content
    );
    assert_eq!(
        store.show("p", &old.id, None).await.unwrap().status,
        Status::Superseded
    );
}

struct OversizedReferencedSplitModel {
    content: MemoryInput,
    seed: Memory,
}
#[async_trait::async_trait]
impl Model for OversizedReferencedSplitModel {
    async fn complete(&self, _: &ModelRequest) -> anyhow::Result<ModelReply> {
        let parts: Vec<_> = (0..16).map(|index| {
            let mut content = serde_json::to_value(&self.content).unwrap();
            content["fact"]["attribute"] = json!(format!("independent{index}"));
            content["evidence"] = json!([{"memory_id":self.seed.id,"quote_index":0}]);
            json!({"content":content,"action":"create","target":null,"expected_revision":null,"reason":"Independent supported fact"})
        }).collect();
        Ok(ModelReply {
            continuation: json!([]),
            calls: vec![ToolCall {
                id: "oversized".into(),
                name: "submit_fact_compaction".into(),
                arguments: json!({"parts":parts,"related":[],"reason":"Reuse exact evidence"}),
            }],
            text: String::new(),
            usage: TokenUsage::default(),
        })
    }
}

#[tokio::test]
async fn quote_references_cannot_bypass_the_expanded_submission_byte_budget() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let text = "x".repeat(2048);
    receipt(&store, "legacy", &text, now()).await;
    let lease = store
        .claim_work("setup", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    store
        .complete_extraction(&lease, vec![], now())
        .await
        .unwrap();
    let mut part = input("legacy", "deployment.path", "/srv/proxy");
    part.evidence[0].quote = text;
    let mut legacy = part.clone();
    legacy.fact = None;
    let old = store.create("p", legacy, Actor::Agent).await.unwrap();
    store
        .schedule_compaction("p", "", 10, true, 0, now())
        .await
        .unwrap();
    let worker = MemoryWorker::new(
        store.clone(),
        std::sync::Arc::new(OversizedReferencedSplitModel {
            content: part,
            seed: old.clone(),
        }),
        AgentConfig {
            max_steps: 1,
            max_context_bytes: 32768,
            ..AgentConfig::default()
        },
        std::sync::Arc::new(Repository(temp.path().into())),
    );
    assert!(
        worker.run_once("compact").await.is_err(),
        "small references must not authorize an oversized expanded proposal"
    );
    assert_eq!(store.list("p", "", 100, true).await.unwrap(), vec![old]);
}

struct AttributionCorrectionModel {
    calls: std::sync::atomic::AtomicUsize,
    content: MemoryInput,
}
#[async_trait::async_trait]
impl Model for AttributionCorrectionModel {
    async fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelReply> {
        let step = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut content = self.content.clone();
        if step == 0 {
            content.kind = Kind::UserDecision;
        } else {
            assert!(request.history.iter().any(|m| matches!(m, Message::Tool {output,..} if output.contains("assistant text alone cannot establish a user decision"))));
            let Message::User(input) = &request.history[0] else {
                panic!("missing preloaded snapshots")
            };
            let input: serde_json::Value = serde_json::from_str(input)?;
            assert_eq!(input["evidence_dates"][0]["role"], "assistant");
        }
        Ok(ModelReply {
            continuation: json!([]),
            calls: vec![ToolCall {
                id: format!("attribution{step}"),
                name: "submit_fact_compaction".into(),
                arguments: json!({"parts":[{"content":content,"action":"create","target":null,"expected_revision":null,"reason":"Preserve reported observation"}],"related":[],"reason":"Split without promoting assistant text to a user decision"}),
            }],
            text: String::new(),
            usage: TokenUsage::default(),
        })
    }
}

#[tokio::test]
async fn legacy_split_corrects_assistant_only_user_decisions_before_transaction() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let text = "Deployment observed at /srv/proxy.";
    let source: Source = serde_json::from_value(json!({"instance_id":"db","receipt_id":"legacy","sequence":1,"project_id":"p","session_id":"s","turn_id":"legacy","revision":1,"job_id":null,"recorded_at":now(),"messages":[{"id":"m","role":"assistant","text":text}]})).unwrap();
    store.ingest(&source).await.unwrap();
    let lease = store
        .claim_work("setup", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    store
        .complete_extraction(&lease, vec![], now())
        .await
        .unwrap();
    let mut part = input("legacy", "deployment.path", "/srv/proxy");
    part.kind = Kind::Observation;
    part.evidence[0].quote = text.into();
    let mut legacy = part.clone();
    legacy.fact = None;
    let old = store.create("p", legacy, Actor::Agent).await.unwrap();
    store
        .schedule_compaction("p", "", 10, true, 0, now())
        .await
        .unwrap();
    let model = std::sync::Arc::new(AttributionCorrectionModel {
        calls: std::sync::atomic::AtomicUsize::new(0),
        content: part,
    });
    let worker = MemoryWorker::new(
        store.clone(),
        model.clone(),
        AgentConfig::default(),
        std::sync::Arc::new(Repository(temp.path().into())),
    );
    worker
        .run_once("compact")
        .await
        .expect("correct attribution in the original loop instead of failing the whole work");
    assert_eq!(model.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    let current = store.list("p", "", 100, false).await.unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].content.kind, Kind::Observation);
    assert_eq!(current[0].content.evidence, old.content.evidence);
    assert_eq!(
        store.show("p", &old.id, None).await.unwrap().status,
        Status::Superseded
    );
}

struct MissingQuoteModel {
    calls: std::sync::atomic::AtomicUsize,
    seed: Memory,
    parts: Vec<MemoryInput>,
}
#[async_trait::async_trait]
impl Model for MissingQuoteModel {
    async fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelReply> {
        let step = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if step > 0 {
            assert!(request.history.iter().any(|m| matches!(m, Message::Tool {output,..} if output.contains("missing quote references") && output.contains(&self.seed.id) && output.contains("quote_index"))), "correction must identify missing original quotation references");
        }
        let count = if step == 0 { 1 } else { self.parts.len() };
        let parts: Vec<_> = self.parts.iter().take(count).enumerate().map(|(index, part)| {
            let mut content = serde_json::to_value(part).unwrap();
            content["evidence"] = json!([{"memory_id":self.seed.id,"quote_index":index}]);
            json!({"content":content,"action":"create","target":null,"expected_revision":null,"reason":"Independent supported fact"})
        }).collect();
        Ok(ModelReply {
            continuation: json!([]),
            calls: vec![ToolCall {
                id: format!("missing{step}"),
                name: "submit_fact_compaction".into(),
                arguments: json!({"parts":parts,"related":[],"reason":"Retain complete original evidence"}),
            }],
            text: String::new(),
            usage: TokenUsage::default(),
        })
    }
}

#[tokio::test]
async fn legacy_split_identifies_missing_quote_references_for_correction() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    receipt(
        &store,
        "legacy",
        "Deploy at /srv/proxy. Enable autostart.",
        now(),
    )
    .await;
    let lease = store
        .claim_work("setup", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    store
        .complete_extraction(&lease, vec![], now())
        .await
        .unwrap();
    let mut path = input("legacy", "deployment.path", "/srv/proxy");
    path.evidence[0].quote = "Deploy at /srv/proxy.".into();
    let mut boot = input("legacy", "autostart", "enabled");
    boot.evidence[0].quote = "Enable autostart.".into();
    let mut legacy = path.clone();
    legacy.fact = None;
    legacy.evidence.extend(boot.evidence.clone());
    let old = store.create("p", legacy, Actor::Agent).await.unwrap();
    store
        .schedule_compaction("p", "", 10, true, 0, now())
        .await
        .unwrap();
    let model = std::sync::Arc::new(MissingQuoteModel {
        calls: std::sync::atomic::AtomicUsize::new(0),
        seed: old.clone(),
        parts: vec![path, boot],
    });
    let worker = MemoryWorker::new(
        store.clone(),
        model.clone(),
        AgentConfig::default(),
        std::sync::Arc::new(Repository(temp.path().into())),
    );
    worker
        .run_once("compact")
        .await
        .expect("identify missing refs, then split with exact evidence");
    assert_eq!(model.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(
        store.show("p", &old.id, None).await.unwrap().status,
        Status::Superseded
    );
    let current = store.list("p", "", 100, false).await.unwrap();
    assert_eq!(current.len(), 2);
    assert!(
        old.content
            .evidence
            .iter()
            .all(|quote| current.iter().any(|m| m.content.evidence.contains(quote)))
    );
}

struct ExistingFactCorrectionModel {
    calls: std::sync::atomic::AtomicUsize,
    existing: Memory,
    parts: Vec<MemoryInput>,
    wrong_revision: bool,
}
#[async_trait::async_trait]
impl Model for ExistingFactCorrectionModel {
    async fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelReply> {
        let step = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if !self.wrong_revision {
            let bound = &request.tools[0].parameters["properties"]["related"]["items"]["anyOf"][0]
                ["properties"];
            assert_eq!(bound["id"]["enum"], json!([self.existing.id]));
            assert_eq!(bound["revision"]["enum"], json!([self.existing.revision]));
            assert_eq!(bound["action"]["enum"], json!(["keep"]));
            assert_eq!(bound["retained"]["type"], "null");
        }
        if step > 0 {
            assert!(request.history.iter().any(|m| matches!(m,Message::Tool {output,..} if output.contains(if self.wrong_revision {"related assessment"}else{"active fact"}) && output.contains(&self.existing.id))), "duplicate creation must be corrected before the transaction");
        }
        let parts: Vec<_> = self.parts.iter().enumerate().map(|(index,content)| {
            let merge = (step > 0 || self.wrong_revision) && index == 1;
            json!({"content":content,"action":if merge {"merge"} else {"create"},"target":if merge {Some(&self.existing.id)}else{None},"expected_revision":if merge {Some(self.existing.revision)}else{None},"reason":"Independent fact with original evidence"})
        }).collect();
        let related = vec![
            json!({"id":self.existing.id,"revision":self.existing.revision + i64::from(self.wrong_revision && step == 0),"action":"keep","reason":"Existing atomic fact","retained":null,"forget_request":null}),
        ];
        Ok(ModelReply {
            continuation: json!([]),
            calls: vec![ToolCall {
                id: format!("existing{step}"),
                name: "submit_fact_compaction".into(),
                arguments: json!({"parts":parts,"related":related,"reason":"Split mixed deployment and SSH facts"}),
            }],
            text: String::new(),
            usage: TokenUsage::default(),
        })
    }
}

#[tokio::test]
async fn legacy_split_corrects_existing_active_fact_creation_before_transaction() {
    existing_fact_correction(false).await;
}

#[tokio::test]
async fn legacy_split_corrects_wrong_related_revision_before_transaction() {
    existing_fact_correction(true).await;
}

async fn existing_fact_correction(wrong_revision: bool) {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let text = "Deploy at /srv/proxy and use SSH port 27254.";
    receipt(&store, "legacy", text, now()).await;
    let lease = store
        .claim_work("setup", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    store
        .complete_extraction(&lease, vec![], now())
        .await
        .unwrap();
    let mut path = input("legacy", "deployment.path", "/srv/proxy");
    path.evidence[0].quote = text.into();
    let mut port = input("legacy", "ssh.port", "27254");
    port.evidence[0].quote = text.into();
    let existing = store.create("p", port.clone(), Actor::Agent).await.unwrap();
    let mut legacy = path.clone();
    legacy.fact = None;
    let old = store.create("p", legacy, Actor::Agent).await.unwrap();
    store
        .schedule_compaction("p", &existing.id, 10, true, 0, now())
        .await
        .unwrap();
    let model = std::sync::Arc::new(ExistingFactCorrectionModel {
        calls: std::sync::atomic::AtomicUsize::new(0),
        existing: existing.clone(),
        parts: vec![path, port],
        wrong_revision,
    });
    let worker = MemoryWorker::new(
        store.clone(),
        model.clone(),
        AgentConfig::default(),
        std::sync::Arc::new(Repository(temp.path().into())),
    );
    worker
        .run_once("compact")
        .await
        .expect("correct duplicate creation to merge before committing");
    assert_eq!(model.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(store.list("p", "", 100, false).await.unwrap().len(), 2);
    assert_eq!(
        store.show("p", &old.id, None).await.unwrap().status,
        Status::Superseded
    );
    let updated = store.show("p", &existing.id, None).await.unwrap();
    assert_eq!(updated.content.fact, existing.content.fact);
    assert!(updated.derived_from.contains(&old.id));
}

struct IndexedOnlySplitModel {
    calls: std::sync::atomic::AtomicUsize,
    existing: Vec<Memory>,
    parts: Vec<MemoryInput>,
    seed: Memory,
}
#[async_trait::async_trait]
impl Model for IndexedOnlySplitModel {
    async fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelReply> {
        let step = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let preload: serde_json::Value = serde_json::from_str(
            request
                .history
                .iter()
                .find_map(|m| match m {
                    Message::User(text) => Some(text),
                    _ => None,
                })
                .unwrap(),
        )?;
        assert!(
            preload["related_memories"][0]
                .as_array()
                .unwrap()
                .iter()
                .all(|snapshot| self.existing.iter().all(|m| snapshot["id"] != m.id))
        );
        if step > 0 {
            let feedback = request
                .history
                .iter()
                .find_map(|m| match m {
                    Message::Tool { output, .. } => Some(output),
                    _ => None,
                })
                .unwrap();
            assert!(feedback.len() <= 2048);
            for memory in &self.existing {
                assert!(
                    feedback.contains(&memory.id),
                    "all indexed collisions should be corrected together"
                );
            }
            assert!(
                !feedback.contains("receipt_id"),
                "indexed correction must not reload full quotation bodies"
            );
        }
        let parts: Vec<_> = self.parts.iter().enumerate().map(|(index, part)| {
            let mut content = serde_json::to_value(part).unwrap();
            content["evidence"] = json!([{"memory_id":self.seed.id,"quote_index":0}]);
            json!({"content":content,"action":if step == 0 {"create"}else{"merge"},"target":if step == 0 {None}else{Some(&self.existing[index].id)},"expected_revision":if step == 0 {None}else{Some(self.existing[index].revision)},"reason":"Preserve an indexed current fact and add legacy evidence"})
        }).collect();
        Ok(ModelReply {
            continuation: json!([]),
            calls: vec![ToolCall {
                id: format!("indexed{step}"),
                name: "submit_fact_compaction".into(),
                arguments: json!({"parts":parts,"related":[],"reason":"Migrate the current scoped SSH facts"}),
            }],
            text: String::new(),
            usage: TokenUsage::default(),
        })
    }
}

#[tokio::test]
async fn legacy_split_corrects_all_active_fact_collisions_outside_lexical_snapshots() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let text = "Use SSH port 27254 and disable password authentication.";
    receipt(&store, "existing", text, now() - 10).await;
    receipt(&store, "legacy", text, now()).await;
    while let Some(lease) = store
        .claim_work("setup", &AgentConfig::default(), now())
        .await
        .unwrap()
    {
        store
            .complete_extraction(&lease, vec![], now())
            .await
            .unwrap();
    }
    let mut existing = Vec::new();
    let mut parts = Vec::new();
    for (attribute, value) in [
        ("ssh.port", "27254"),
        ("ssh.password_authentication", "disabled"),
    ] {
        let mut current = input("existing", attribute, value);
        current.title = "Stored access fact".into();
        current.scope = "accesssettings".into();
        current.tags.clear();
        current.evidence[0].quote = text.into();
        existing.push(
            store
                .create("p", current.clone(), Actor::Agent)
                .await
                .unwrap(),
        );
        current.evidence[0].receipt_id = "legacy".into();
        parts.push(current);
    }
    let mut legacy = parts[0].clone();
    legacy.fact = None;
    legacy.title = "Migrationseed".into();
    legacy.scope = "migrationcontext".into();
    legacy.tags.clear();
    legacy.conclusion = text.into();
    let seed = store.create("p", legacy, Actor::Agent).await.unwrap();
    store
        .schedule_compaction("p", &existing[1].id, 10, true, 0, now())
        .await
        .unwrap();
    let model = std::sync::Arc::new(IndexedOnlySplitModel {
        calls: std::sync::atomic::AtomicUsize::new(0),
        existing: existing.clone(),
        parts,
        seed: seed.clone(),
    });
    let worker = MemoryWorker::new(
        store.clone(),
        model.clone(),
        AgentConfig::default(),
        std::sync::Arc::new(Repository(temp.path().into())),
    );
    worker
        .run_once("compact")
        .await
        .expect("indexed collisions must be model-correctable before the fenced write");
    assert_eq!(model.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(
        store.show("p", &seed.id, None).await.unwrap().status,
        Status::Superseded
    );
    let current = store.list("p", "", 100, false).await.unwrap();
    assert_eq!(current.len(), 2);
    for original in &existing {
        let merged = current.iter().find(|m| m.id == original.id).unwrap();
        assert_eq!(merged.content.fact, original.content.fact);
        assert_eq!(merged.content.evidence.len(), 2);
        assert!(merged.derived_from.contains(&seed.id));
    }
}
