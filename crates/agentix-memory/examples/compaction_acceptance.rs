//! Opt-in live acceptance on a temporary copy of one Project, never the original database.
//! Usage: `compaction_acceptance CONFIG PROJECT_ID RETIRED_VALUE CURRENT_VALUE`
use agentix_memory::{
    Actor, DeepQuery, HttpModel, HttpProvider, MemoryConfig, MemoryLocation, MemoryStore,
    MemoryWorker, Model, ModelReply, ModelRequest, ProjectRepository,
};
use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use serde_json::json;
use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Instant};

struct Repository(PathBuf);
#[async_trait]
impl ProjectRepository for Repository {
    async fn root(&self, _: &str) -> Result<Option<PathBuf>> {
        Ok(Some(self.0.clone()))
    }
}

struct LiveModel(HttpModel);
#[async_trait]
impl Model for LiveModel {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelReply> {
        let started = Instant::now();
        eprintln!(
            "model request: {} messages, {} tools",
            request.history.len(),
            request.tools.len()
        );
        let result = self.0.complete(request).await;
        match &result {
            Ok(reply) => eprintln!(
                "model response: {} ms, tools {:?}",
                started.elapsed().as_millis(),
                reply
                    .calls
                    .iter()
                    .map(|call| &call.name)
                    .collect::<Vec<_>>()
            ),
            Err(error) => eprintln!("model error: {} ms, {error}", started.elapsed().as_millis()),
        }
        result
    }
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 4,
        "usage: compaction_acceptance CONFIG PROJECT_ID RETIRED_VALUE CURRENT_VALUE"
    );
    let config = MemoryConfig::load(std::path::Path::new(&args[0]))?;
    let location = MemoryLocation::load(std::path::Path::new(&args[0]))?;
    let original = MemoryStore::open_read_only(&location.path).await?;
    let memories = original.list(&args[1], "", 100, false).await?;
    ensure!(
        !memories.is_empty() && memories.len() < 100,
        "acceptance requires a complete Project page smaller than 100 memories"
    );
    let temp = tempfile::tempdir()?;
    let store = MemoryStore::open(&temp.path().join("memory.db")).await?;
    let mut receipts = HashSet::new();
    for memory in &memories {
        for evidence in &memory.content.evidence {
            if receipts.insert(evidence.receipt_id.clone()) {
                let mut source = original.source(&args[1], &evidence.receipt_id).await?;
                source.project_id = "acceptance".into();
                store.ingest(&source).await?;
            }
        }
    }
    while let Some(lease) = store.claim_work("setup", &config.agent, now()).await? {
        store.complete_extraction(&lease, vec![], now()).await?;
    }
    let mut copied = Vec::new();
    for memory in &memories {
        copied.push(
            store
                .create("acceptance", memory.content.clone(), Actor::Agent)
                .await?,
        );
    }
    let provider = Arc::new(HttpProvider::new(
        config
            .providers
            .get(&config.agent.provider)
            .context("missing provider")?
            .clone(),
    )?);
    let model = Arc::new(LiveModel(HttpModel::new(provider, config.agent.clone())?));
    let repository = Arc::new(Repository(temp.path().into()));
    let worker = MemoryWorker::new(
        store.clone(),
        model.clone(),
        config.agent.clone(),
        repository.clone(),
    );
    let page = store
        .schedule_compaction("acceptance", "", 100, true, 0, now())
        .await?;
    let started = Instant::now();
    for _ in 0..page.scheduled {
        if let Err(error) = worker.run_once("acceptance").await {
            for id in &page.work_ids {
                let details = store.work_details(*id).await?;
                eprintln!(
                    "work {}: {}, error {}",
                    id, details["state"], details["error"]
                );
            }
            return Err(error);
        }
    }
    let current = store.list("acceptance", "", 100, false).await?;
    let stale = current
        .iter()
        .filter(|m| m.content.conclusion.contains(&args[2]))
        .count();
    let current_value = current
        .iter()
        .any(|m| m.content.conclusion.contains(&args[3]));
    let mut history_retained = true;
    for memory in &copied {
        history_retained &=
            store.show("acceptance", &memory.id, Some(1)).await?.content == memory.content;
    }
    let deep = DeepQuery::new(store.clone(), model, config.agent.clone(), repository, 1);
    let answer = deep.ask("acceptance", &format!("What is the current REALITY camouflage domain? Distinguish the obsolete value {} from its replacement {}. Cite the original replacement evidence.",args[2],args[3])).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"copied_memories":copied.len(),"scheduled":page.scheduled,"elapsed_ms":started.elapsed().as_millis(),"active_memories":current.len(),"stale_conclusions":stale,"current_value_present":current_value,"history_retained":history_retained,"work":store.work_counts().await?,"answer":answer,"current_summaries":current.iter().map(|m| json!({"id":m.id,"revision":m.revision,"status":m.status,"conclusion":m.content.conclusion})).collect::<Vec<_>>()})
        )?
    );
    ensure!(
        stale == 0
            && current_value
            && history_retained
            && !answer.insufficient_evidence
            && answer.answer.contains(&args[3]),
        "live semantic compaction acceptance failed"
    );
    Ok(())
}
