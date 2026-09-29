use agentix_memory::{
    AgentConfig, MemoryStore, MemoryWorker, Model, ModelReply, ModelRequest, ProjectRepository,
    Source, TokenUsage, ToolCall,
};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

struct Repositories(PathBuf);

struct CrossTurnModel;
#[async_trait]
impl Model for CrossTurnModel {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelReply> {
        let outputs = request
            .history
            .iter()
            .filter_map(|m| match m {
                agentix_memory::Message::Tool { output, .. } => {
                    Some(serde_json::from_str::<serde_json::Value>(output).unwrap())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let (name, arguments) = match outputs.len() {
            0 => ("source_neighbors", json!({"receipt_id":"choice"})),
            1 => {
                assert_eq!(outputs[0]["sources"][0]["receipt_id"], "proposal");
                (
                    "source_read",
                    json!({"receipt_id":"proposal","message_id":null,"offset":0}),
                )
            }
            2 => (
                "source_read",
                json!({"receipt_id":"proposal","message_id":outputs[1]["messages"][0]["id"],"offset":0}),
            ),
            3 => {
                assert_eq!(outputs[2]["page"]["text"], "Option B keeps data offline.");
                ("repo_search", json!({"query":"offline"}))
            }
            _ => (
                "submit_candidates",
                json!({"candidates":[{"title":"Offline storage chosen","conclusion":"Keep data offline","rationale":"User selected option B","scope":"project","conditions":[],"valid_until":null,"tags":[],"kind":"user_decision","evidence":[{"receipt_id":"choice","message_id":"message","quote":"Choose option B."},{"receipt_id":"proposal","message_id":"message","quote":"Option B keeps data offline."}]}]}),
            ),
        };
        Ok(ModelReply {
            continuation: json!([]),
            calls: vec![ToolCall {
                id: format!("call-{}", outputs.len()),
                name: name.into(),
                arguments,
            }],
            text: String::new(),
            usage: TokenUsage::default(),
        })
    }
}

#[tokio::test]
async fn extraction_resolves_a_choice_using_a_prior_legacy_message() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    let mut proposal = source("p");
    proposal.receipt_id = "proposal".into();
    proposal.turn_id = "legacy:job:proposal".into();
    proposal.messages[0].role = "assistant".into();
    proposal.messages[0].text = "Option B keeps data offline.".into();
    store.ingest(&proposal).await.unwrap();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let prior = store
        .claim_work("prior", &AgentConfig::default(), now)
        .await
        .unwrap()
        .unwrap();
    store
        .complete_extraction(&prior, vec![], now)
        .await
        .unwrap();
    let mut choice = source("p");
    choice.receipt_id = "choice".into();
    choice.sequence = 2;
    choice.messages[0].text = "Choose option B.".into();
    store.ingest(&choice).await.unwrap();
    let worker = MemoryWorker::new(
        store.clone(),
        Arc::new(CrossTurnModel),
        AgentConfig::default(),
        Arc::new(Repositories(temp.path().into())),
    );
    assert!(worker.run_once("extract").await.unwrap());
    let consolidation = store
        .claim_work("consolidate", &AgentConfig::default(), now)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(consolidation.kind, agentix_memory::WorkKind::Consolidate);
    assert_eq!(
        consolidation.payload[0]["evidence"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let decisions = serde_json::from_value(json!([{"candidate":0,"action":"create","target":null,"expected_revision":null,"content":null,"reason":"Explicit choice with supporting proposal"}])).unwrap();
    let memories = store
        .complete_consolidation(&consolidation, decisions, now)
        .await
        .unwrap();
    assert_eq!(memories[0].content.kind, agentix_memory::Kind::UserDecision);
}
#[async_trait]
impl ProjectRepository for Repositories {
    async fn root(&self, _project: &str) -> Result<Option<PathBuf>> {
        Ok(Some(self.0.clone()))
    }
}
struct BlockingModel {
    entered: Semaphore,
    release: Semaphore,
}
#[async_trait]
impl Model for BlockingModel {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelReply> {
        let call = if request.history.len() == 1 {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
            ToolCall {
                id: "read".into(),
                name: "repo_search".into(),
                arguments: json!({"query":"external decision"}),
            }
        } else {
            ToolCall {
                id: "finish".into(),
                name: "submit_candidates".into(),
                arguments: json!({"candidates":[]}),
            }
        };
        Ok(ModelReply {
            continuation: json!([{"type":"function_call","call_id":call.id,"name":call.name,"arguments":call.arguments.to_string()}]),
            calls: vec![call],
            text: String::new(),
            usage: TokenUsage::default(),
        })
    }
}
fn source(project: &str) -> Source {
    serde_json::from_value(json!({"instance_id":"db","receipt_id":project,"sequence":1,"project_id":project,"session_id":project,"turn_id":"turn","revision":1,"job_id":null,"recorded_at":1,"messages":[{"id":"message","role":"user","text":"external decision"}]})).unwrap()
}
#[tokio::test]
async fn model_loops_run_in_parallel_without_holding_database_transactions() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "Repository information").unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    store.ingest(&source("a")).await.unwrap();
    store.ingest(&source("b")).await.unwrap();
    let model = Arc::new(BlockingModel {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let worker = Arc::new(MemoryWorker::new(
        store.clone(),
        model.clone(),
        AgentConfig::default(),
        Arc::new(Repositories(repo)),
    ));
    let a = worker.clone();
    let a = tokio::spawn(async move { a.run_once("a").await });
    let b = worker.clone();
    let b = tokio::spawn(async move { b.run_once("b").await });
    tokio::time::timeout(Duration::from_secs(2), model.entered.acquire_many(2))
        .await
        .unwrap()
        .unwrap()
        .forget();
    tokio::time::timeout(Duration::from_millis(500), store.ingest(&source("c")))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(store.work_counts().await.unwrap().running, 2);
    model.release.add_permits(2);
    assert!(a.await.unwrap().unwrap());
    assert!(b.await.unwrap().unwrap());
    assert_eq!(store.work_counts().await.unwrap().done, 2);
    assert_eq!(store.work_counts().await.unwrap().pending, 1);
}

#[path = "support/http.rs"]
mod http;

#[tokio::test]
async fn real_http_agent_pipeline_extracts_consolidates_and_preserves_provenance() {
    fn response(name: &str, args: &serde_json::Value) -> (u16, serde_json::Value) {
        (
            200,
            json!({"status":"completed","output":[{"type":"function_call","call_id":name,"name":name,"arguments":args.to_string()}]}),
        )
    }
    let candidate = json!({"title":"External decision","conclusion":"external decision","rationale":"User instruction","scope":"project","conditions":[],"valid_until":null,"tags":[],"kind":"user_decision","evidence":[{"receipt_id":"a","message_id":"message","quote":"external decision"}]});
    let server=http::MockHttp::start(vec![
        response("repo_search",&json!({"query":"external decision"})),
        response("submit_candidates",&json!({"candidates":[candidate]})),
        response("memory_search",&json!({"query":"external decision"})),
        response("submit_decisions",&json!({"decisions":[{"candidate":0,"action":"create","target":null,"expected_revision":null,"content":null,"reason":"New external constraint"}]})),
    ]).await;
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "Repository information").unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    store.ingest(&source("a")).await.unwrap();
    let provider = agentix_memory::HttpProvider::new(agentix_memory::ProviderConfig {
        base_url: server.url.clone(),
        protocol: agentix_memory::ProviderProtocol::Openai,
        api_key_env: None,
        max_in_flight: 4,
    })
    .unwrap();
    let model = agentix_memory::HttpModel::new(Arc::new(provider), AgentConfig::default()).unwrap();
    let worker = MemoryWorker::new(
        store.clone(),
        Arc::new(model),
        AgentConfig::default(),
        Arc::new(Repositories(repo)),
    );
    assert!(worker.run_once("extract").await.unwrap());
    assert!(worker.run_once("consolidate").await.unwrap());
    assert!(!worker.run_once("idle").await.unwrap());
    let memories = store.search("a", "external", 10).await.unwrap();
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].content.evidence[0].receipt_id, "a");
    assert_eq!(store.work_counts().await.unwrap().done, 2);
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[2].1["input"].as_array().unwrap().len(), 1);
    assert_eq!(requests[0].1["store"], false);
}

#[tokio::test]
async fn completed_extraction_keeps_repository_inspection_audit() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "A checked repository document").unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    store.ingest(&source("a")).await.unwrap();
    let model = Arc::new(BlockingModel {
        entered: Semaphore::new(0),
        release: Semaphore::new(1),
    });
    let worker = MemoryWorker::new(
        store.clone(),
        model,
        AgentConfig::default(),
        Arc::new(Repositories(repo)),
    );
    worker.run_once("audit").await.unwrap();
    let details = store.work_details(1).await.unwrap();
    assert!(details["audit"]["inspections"].as_array().unwrap().len() == 1);
    assert_eq!(details["audit"]["inspections"][0]["tool"], "repo_search");
    assert!(
        details["audit"]["inspections"][0]["digest"]
            .as_str()
            .unwrap()
            .len()
            == 64
    );
}

struct ReviewModel {
    edit: Option<(MemoryStore, agentix_memory::Memory)>,
    quote: &'static str,
}
#[async_trait]
impl Model for ReviewModel {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelReply> {
        if let Some((store, memory)) = &self.edit {
            store
                .update(
                    &memory.project_id,
                    &memory.id,
                    memory.revision,
                    memory.content.clone(),
                    agentix_memory::Actor::Human,
                )
                .await?;
        }
        let call = ToolCall {
            id: "finish".into(),
            name: "submit_review".into(),
            arguments: json!({"archive":true,"path":"README.md","offset":0,"quote":self.quote,"reason":"The decision is now documented in README.md"}),
        };
        assert!(request.instructions.contains("Review"));
        Ok(ModelReply {
            continuation: json!([]),
            calls: vec![call],
            text: String::new(),
            usage: TokenUsage::default(),
        })
    }
}

#[tokio::test]
async fn repository_review_archives_documented_memory_and_preserves_human_edits() {
    use agentix_memory::{Actor, MemoryInput, Status};
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "external decision is recorded here").unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    store.ingest(&source("a")).await.unwrap();
    let config = AgentConfig::default();
    let lease = store
        .claim_work("extract", &config, 1)
        .await
        .unwrap()
        .unwrap();
    store.complete_extraction(&lease, vec![], 2).await.unwrap();
    let input: MemoryInput = serde_json::from_value(json!({"title":"Decision","conclusion":"external decision","rationale":"User requirement","scope":"project","tags":[],"kind":"user_decision","evidence":[{"receipt_id":"a","message_id":"message","quote":"external decision"}]})).unwrap();
    let memory = store
        .create("a", input.clone(), Actor::Agent)
        .await
        .unwrap();
    let human = store.create("a", input, Actor::Human).await.unwrap();
    assert_eq!(store.schedule_reviews("a", "day:1", 10).await.unwrap(), 1);
    assert_eq!(store.schedule_reviews("a", "day:1", 10).await.unwrap(), 0);
    let worker = MemoryWorker::new(
        store.clone(),
        Arc::new(ReviewModel {
            edit: None,
            quote: "external decision",
        }),
        config,
        Arc::new(Repositories(repo.clone())),
    );
    worker.run_once("review").await.unwrap();
    assert_eq!(
        store.show("a", &memory.id, None).await.unwrap().status,
        Status::Archived
    );
    assert_eq!(
        store.show("a", &human.id, None).await.unwrap().status,
        Status::Active
    );
    assert_eq!(store.schedule_reviews("a", "day:2", 10).await.unwrap(), 0);
    let race = store
        .create("a", human.content.clone(), Actor::Agent)
        .await
        .unwrap();
    store.schedule_reviews("a", "day:2", 10).await.unwrap();
    let worker = MemoryWorker::new(
        store.clone(),
        Arc::new(ReviewModel {
            edit: Some((store.clone(), race.clone())),
            quote: "external decision",
        }),
        AgentConfig::default(),
        Arc::new(Repositories(repo.clone())),
    );
    worker.run_once("race").await.unwrap();
    assert_eq!(
        store.show("a", &race.id, None).await.unwrap().actor,
        Actor::Human
    );
    assert_eq!(
        store.show("a", &race.id, None).await.unwrap().status,
        Status::Active
    );
    let forged = store
        .create("a", human.content, Actor::Agent)
        .await
        .unwrap();
    store.schedule_reviews("a", "day:3", 10).await.unwrap();
    let worker = MemoryWorker::new(
        store.clone(),
        Arc::new(ReviewModel {
            edit: None,
            quote: "fabricated citation",
        }),
        AgentConfig::default(),
        Arc::new(Repositories(repo)),
    );
    assert!(worker.run_once("forged").await.is_err());
    assert_eq!(
        store.show("a", &forged.id, None).await.unwrap().status,
        Status::Active
    );
}

struct EmptyModel(std::sync::atomic::AtomicUsize);
#[async_trait]
impl Model for EmptyModel {
    async fn complete(&self, _request: &ModelRequest) -> Result<ModelReply> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(ModelReply {
            continuation: json!([]),
            calls: vec![ToolCall {
                id: "done".into(),
                name: "submit_candidates".into(),
                arguments: json!({"candidates":[]}),
            }],
            text: String::new(),
            usage: TokenUsage::default(),
        })
    }
}
struct Gate(Option<bool>);
#[async_trait]
impl agentix_memory::ExtractionGate for Gate {
    async fn evaluate(
        &self,
        source: &Source,
        lease: &agentix_memory::WorkLease,
    ) -> Result<agentix_memory::TriageDecision> {
        assert_eq!(source.receipt_id, lease.receipt_id);
        let skip = self.0.ok_or_else(|| anyhow::anyhow!("unavailable"))?;
        Ok(agentix_memory::TriageDecision {
            skip,
            audit: json!({"action":if skip {"skip"} else {"extract"}}),
        })
    }
}
#[tokio::test]
async fn extraction_gate_skips_only_explicit_skip_and_falls_back_on_error() {
    for decision in [Some(true), Some(false), None] {
        let temp = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&temp.path().join("memory.db"))
            .await
            .unwrap();
        store.ingest(&source("project")).await.unwrap();
        let model = Arc::new(EmptyModel(std::sync::atomic::AtomicUsize::new(0)));
        let worker = MemoryWorker::new(
            store.clone(),
            model.clone(),
            AgentConfig::default(),
            Arc::new(Repositories(temp.path().to_owned())),
        )
        .with_extraction_gate(Arc::new(Gate(decision)));
        assert!(worker.run_once("worker").await.unwrap());
        assert_eq!(
            model.0.load(std::sync::atomic::Ordering::SeqCst),
            usize::from(decision != Some(true))
        );
        assert_eq!(store.work_counts().await.unwrap().done, 1);
        assert!(store.work_details(1).await.unwrap()["audit"]["triage"].is_object());
    }
}

struct SupersedingGate(MemoryStore);
#[async_trait]
impl agentix_memory::ExtractionGate for SupersedingGate {
    async fn evaluate(
        &self,
        source: &Source,
        _lease: &agentix_memory::WorkLease,
    ) -> Result<agentix_memory::TriageDecision> {
        let mut newer = source.clone();
        newer.revision += 1;
        newer.sequence += 1;
        newer.receipt_id = "newer-receipt".into();
        newer.messages[0].text = "New decision supersedes the pending snapshot".into();
        self.0.ingest(&newer).await?;
        Ok(agentix_memory::TriageDecision {
            skip: true,
            audit: json!({"action":"skip"}),
        })
    }
}
#[tokio::test]
async fn screening_cannot_skip_a_newer_source_revision() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    store.ingest(&source("project")).await.unwrap();
    let model = Arc::new(EmptyModel(std::sync::atomic::AtomicUsize::new(0)));
    let worker = MemoryWorker::new(
        store.clone(),
        model.clone(),
        AgentConfig::default(),
        Arc::new(Repositories(temp.path().to_owned())),
    )
    .with_extraction_gate(Arc::new(SupersedingGate(store.clone())));
    assert!(worker.run_once("worker").await.is_err());
    assert_eq!(store.work_details(1).await.unwrap()["state"], "cancelled");
    assert_eq!(store.work_counts().await.unwrap().pending, 1);
    assert_eq!(model.0.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn rapid_source_revisions_coalesce_before_calling_model() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    store.ingest(&source("p")).await.unwrap();
    let model = Arc::new(BlockingModel {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let worker = Arc::new(MemoryWorker::new(
        store.clone(),
        model.clone(),
        AgentConfig::default(),
        Arc::new(Repositories(temp.path().into())),
    ));
    let child = worker.clone();
    let running = tokio::spawn(async move { child.run_once("worker").await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while store.work_counts().await.unwrap().running == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        model.entered.available_permits(),
        0,
        "rapid updates should settle before a paid request"
    );
    let mut revised = source("p");
    revised.receipt_id = "new".into();
    revised.revision = 2;
    revised.sequence = 2;
    revised.messages[0].text.push_str(" with new context");
    store.ingest(&revised).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), running)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    );
    assert_eq!(model.entered.available_permits(), 0);
    assert_eq!(store.work_counts().await.unwrap().pending, 1);
    assert!(store.source("p", "p").await.is_ok());
    model.release.add_permits(1);
    assert!(worker.run_once("replacement").await.unwrap());
    assert_eq!(store.work_counts().await.unwrap().done, 1);
    assert_eq!(model.entered.available_permits(), 1);
}

#[tokio::test]
async fn superseded_source_cancels_a_blocked_model_request() {
    let temp = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(&temp.path().join("memory.db"))
        .await
        .unwrap();
    store.ingest(&source("p")).await.unwrap();
    let model = Arc::new(BlockingModel {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let worker = Arc::new(MemoryWorker::new(
        store.clone(),
        model.clone(),
        AgentConfig::default(),
        Arc::new(Repositories(temp.path().into())),
    ));
    let running = tokio::spawn(async move { worker.run_once("worker").await });
    tokio::time::timeout(Duration::from_secs(3), model.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let mut revised = source("p");
    revised.receipt_id = "new".into();
    revised.revision = 2;
    revised.sequence = 2;
    store.ingest(&revised).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), running)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    );
    assert_eq!(store.work_counts().await.unwrap().cancelled, 1);
    assert_eq!(store.work_counts().await.unwrap().pending, 1);
}
