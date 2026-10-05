use agentix_memory::{Actor, AgentConfig, MemoryInput, MemoryStore, Source, Status, WorkLease};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

struct Repository(std::path::PathBuf);
#[async_trait::async_trait]
impl agentix_memory::ProjectRepository for Repository {
    async fn root(&self, _: &str) -> anyhow::Result<Option<std::path::PathBuf>> {
        Ok(Some(self.0.clone()))
    }
}

struct AssessRelated {
    old: agentix_memory::Memory,
    calls: Mutex<usize>,
}
#[async_trait::async_trait]
impl agentix_memory::Model for AssessRelated {
    async fn complete(
        &self,
        request: &agentix_memory::ModelRequest,
    ) -> anyhow::Result<agentix_memory::ModelReply> {
        let mut step = self.calls.lock().unwrap();
        let (name, args) = if *step == 0 {
            ("memory_search", json!({"query":"Server"}))
        } else {
            let related = if *step == 1 {
                vec![]
            } else {
                assert!(request.history.iter().any(|m| matches!(m,agentix_memory::Message::Tool {output,..} if output.contains("assess every supplied related memory"))));
                vec![
                    json!({"id":self.old.id,"revision":1,"action":"keep","reason":"Retained as dated deployment history","retained":null,"forget_request":null}),
                ]
            };
            (
                "submit_decisions",
                json!({"decisions":[{"candidate":0,"action":"create","target":null,"expected_revision":null,"content":null,"reason":"New configuration","related":related}]}),
            )
        };
        *step += 1;
        Ok(agentix_memory::ModelReply {
            continuation: json!([]),
            calls: vec![agentix_memory::ToolCall {
                id: format!("c{step}"),
                name: name.into(),
                arguments: args,
            }],
            text: String::new(),
            usage: agentix_memory::TokenUsage::default(),
        })
    }
}

#[tokio::test]
async fn worker_requires_assessment_of_every_supplied_related_record_before_commit() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let old = prior(&store).await;
    let text = "Replace old.example with new.example for camouflage.";
    let extraction = source(&store, "new", text).await;
    store
        .complete_extraction(&extraction, vec![input("new", text)], now())
        .await
        .unwrap();
    let model = Arc::new(AssessRelated {
        old,
        calls: Mutex::new(0),
    });
    let worker = agentix_memory::MemoryWorker::new(
        store.clone(),
        model.clone(),
        AgentConfig::default(),
        Arc::new(Repository(dir.path().into())),
    );
    worker.run_once("worker").await.unwrap();
    assert_eq!(*model.calls.lock().unwrap(), 3);
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn input(receipt: &str, text: &str) -> MemoryInput {
    serde_json::from_value(json!({"title":"Server configuration","conclusion":text,"rationale":"Confirmed configuration","scope":"server","conditions":[],"tags":["server"],"kind":"user_decision","evidence":[{"receipt_id":receipt,"message_id":"message","quote":text}]})).unwrap()
}

async fn source(store: &MemoryStore, receipt: &str, text: &str) -> WorkLease {
    let source: Source = serde_json::from_value(json!({"instance_id":"db","receipt_id":receipt,"sequence":1,"project_id":"p","session_id":"session","turn_id":receipt,"revision":1,"job_id":null,"recorded_at":now(),"messages":[{"id":"message","role":"user","text":text}]})).unwrap();
    store.ingest(&source).await.unwrap();
    store
        .claim_work("worker", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap()
}

async fn prior(store: &MemoryStore) -> agentix_memory::Memory {
    let text = "Deploy at /srv/proxy, enable autostart, and use old.example for camouflage.";
    let lease = source(store, "old", text).await;
    store
        .complete_extraction(&lease, vec![], now())
        .await
        .unwrap();
    store
        .create("p", input("old", text), Actor::Agent)
        .await
        .unwrap()
}

async fn candidate(store: &MemoryStore) -> (WorkLease, MemoryInput) {
    let text = "Replace old.example with new.example for camouflage.";
    let lease = source(store, "new", text).await;
    let content = input("new", text);
    store
        .complete_extraction(&lease, vec![content.clone()], now())
        .await
        .unwrap();
    (
        store
            .claim_work("worker", &AgentConfig::default(), now())
            .await
            .unwrap()
            .unwrap(),
        content,
    )
}

#[tokio::test]
async fn replacement_preserves_unaffected_facts_and_history_and_removes_stale_search_result() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let old = prior(&store).await;
    let (lease, mut current) = candidate(&store).await;
    current.conclusion = "Use new.example for camouflage.".into();
    let mut retained = old.content.clone();
    retained.conclusion = "Deploy at /srv/proxy and enable autostart.".into();
    let decisions = serde_json::from_value(json!([{"candidate":0,"action":"create","target":null,"expected_revision":null,"content":current,"reason":"Explicit replacement","related":[{"id":old.id,"revision":old.revision,"action":"supersede","reason":"Retain deployment separately","retained":retained,"forget_request":null}]}])).unwrap();
    let changed = store
        .complete_consolidation(&lease, decisions, now())
        .await
        .unwrap();
    assert_eq!(
        store.show("p", &old.id, None).await.unwrap().status,
        Status::Superseded
    );
    assert!(
        store
            .show("p", &old.id, Some(1))
            .await
            .unwrap()
            .content
            .conclusion
            .contains("old.example")
    );
    let results = store.list("p", "", 20, false).await.unwrap();
    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .any(|m| m.content.conclusion.contains("new.example"))
    );
    assert!(
        results
            .iter()
            .any(|m| m.content.conclusion.contains("/srv/proxy")
                && m.content.conclusion.contains("autostart"))
    );
    assert!(
        !results
            .iter()
            .any(|m| m.content.conclusion.contains("old.example"))
    );
    assert_eq!(changed.len(), 3);
}

#[tokio::test]
async fn related_revision_conflict_rolls_back_the_whole_proposal() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let old = prior(&store).await;
    let (lease, _) = candidate(&store).await;
    let decisions = serde_json::from_value(json!([{"candidate":0,"action":"create","target":null,"expected_revision":null,"content":null,"reason":"Replacement","related":[{"id":old.id,"revision":0,"action":"conflict","reason":"Conflicting claims","retained":null,"forget_request":null}]}])).unwrap();
    assert!(
        store
            .complete_consolidation(&lease, decisions, now())
            .await
            .is_err()
    );
    assert_eq!(store.list("p", "", 20, false).await.unwrap().len(), 1);
    assert_eq!(store.show("p", &old.id, None).await.unwrap().revision, 1);
    assert_eq!(
        store.work_details(lease.id).await.unwrap()["state"],
        "running"
    );
}

#[tokio::test]
async fn replacement_cannot_drop_prior_evidence_without_a_retained_record() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let old = prior(&store).await;
    let (lease, _) = candidate(&store).await;
    let decisions = serde_json::from_value(json!([{"candidate":0,"action":"create","target":null,"expected_revision":null,"content":null,"reason":"Replacement","related":[{"id":old.id,"revision":old.revision,"action":"supersede","reason":"Replacement","retained":null,"forget_request":null}]}])).unwrap();
    let error = store
        .complete_consolidation(&lease, decisions, now())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("prior evidence"));
}

#[tokio::test]
async fn unresolved_conflict_keeps_both_claims_searchable() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let old = prior(&store).await;
    let (lease, _) = candidate(&store).await;
    let decisions = serde_json::from_value(json!([{"candidate":0,"action":"create","target":null,"expected_revision":null,"content":null,"reason":"Unresolved disagreement","related":[{"id":old.id,"revision":old.revision,"action":"conflict","reason":"No proven replacement","retained":null,"forget_request":null}]}])).unwrap();
    store
        .complete_consolidation(&lease, decisions, now())
        .await
        .unwrap();
    assert!(
        store
            .list("p", "", 20, false)
            .await
            .unwrap()
            .iter()
            .all(|m| m.status == Status::Conflicted)
    );
}

#[tokio::test]
async fn forgetting_requires_an_explicit_current_user_request_and_suppresses_replay() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let old = prior(&store).await;
    let (lease, _) = candidate(&store).await;
    let proposal = |auth: Value| json!([{"candidate":0,"action":"discard","target":null,"expected_revision":null,"content":null,"reason":"Explicit forgetting","related":[{"id":old.id,"revision":old.revision,"action":"forget","reason":"User requested removal","retained":null,"forget_request":auth}]}]);
    assert!(
        store
            .complete_consolidation(
                &lease,
                serde_json::from_value(proposal(Value::Null)).unwrap(),
                now()
            )
            .await
            .is_err()
    );
    store.complete_consolidation(&lease, serde_json::from_value(json!([{"candidate":0,"action":"discard","target":null,"expected_revision":null,"content":null,"reason":"No mutation","related":[]}])).unwrap(), now()).await.unwrap();
    let text = format!("Forget memory {}.", old.id);
    let extraction = source(&store, "forget", &text).await;
    store
        .complete_extraction(&extraction, vec![input("forget", &text)], now())
        .await
        .unwrap();
    let lease = store
        .claim_work("worker", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    let auth = json!({"receipt_id":"forget","message_id":"message","quote":text});
    store
        .complete_consolidation(
            &lease,
            serde_json::from_value(proposal(auth)).unwrap(),
            now(),
        )
        .await
        .unwrap();
    assert_eq!(
        store.show("p", &old.id, None).await.unwrap().status,
        Status::Forgotten
    );
    assert!(store.create("p", old.content, Actor::Agent).await.is_err());
}

#[tokio::test]
async fn negative_or_quoted_forgetting_is_not_authorization() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let old = prior(&store).await;
    let text = format!("Do not forget memory {}.", old.id);
    let extraction = source(&store, "negative", &text).await;
    store
        .complete_extraction(&extraction, vec![input("negative", &text)], now())
        .await
        .unwrap();
    let lease = store
        .claim_work("worker", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    let proposal = json!([{"candidate":0,"action":"discard","target":null,"expected_revision":null,"content":null,"reason":"Misclassified request","related":[{"id":old.id,"revision":1,"action":"forget","reason":"Misclassified request","retained":null,"forget_request":{"receipt_id":"negative","message_id":"message","quote":text}}]}]);
    assert!(
        store
            .complete_consolidation(&lease, serde_json::from_value(proposal).unwrap(), now())
            .await
            .is_err()
    );
    assert_eq!(
        store.show("p", &old.id, None).await.unwrap().status,
        Status::Active
    );
}

#[tokio::test]
async fn one_proposal_can_retire_a_shared_match_and_keep_its_snapshot_for_another_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let old = prior(&store).await;
    let text = "Replace old.example with new.example; keep deployment and autostart. Also enable monitoring.";
    let extraction = source(&store, "new", text).await;
    let mut first = input("new", text);
    first.conclusion = "Deploy at /srv/proxy, enable autostart, and use new.example.".into();
    first.evidence.extend(old.content.evidence.clone());
    let mut second = input("new", text);
    second.title = "Monitoring policy".into();
    second.conclusion = "Enable monitoring.".into();
    store
        .complete_extraction(&extraction, vec![first, second], now())
        .await
        .unwrap();
    let lease = store
        .claim_work("worker", &AgentConfig::default(), now())
        .await
        .unwrap()
        .unwrap();
    let decisions = serde_json::from_value(json!([
        {"candidate":0,"action":"create","target":null,"expected_revision":null,"content":null,"reason":"Replacement","related":[{"id":old.id,"revision":1,"action":"supersede","reason":"Preserved deployment and evidence","retained":null,"forget_request":null}]},
        {"candidate":1,"action":"create","target":null,"expected_revision":null,"content":null,"reason":"Independent policy","related":[{"id":old.id,"revision":1,"action":"keep","reason":"No further mutation for this independent candidate","retained":null,"forget_request":null}]}
    ])).unwrap();
    store
        .complete_consolidation(&lease, decisions, now())
        .await
        .unwrap();
    assert_eq!(
        store.show("p", &old.id, None).await.unwrap().status,
        Status::Superseded
    );
    assert_eq!(store.list("p", "", 20, false).await.unwrap().len(), 2);
}
