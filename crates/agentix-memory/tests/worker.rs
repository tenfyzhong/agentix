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
