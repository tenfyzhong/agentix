//! Opt-in live acceptance on a temporary copy of one Project, never the original database.
//! Usage: `compaction_acceptance CONFIG PROJECT_ID RETIRED_VALUE CURRENT_VALUE [--codex-luna]`
#[path = "support/codex_model.rs"]
mod codex_model;
use agentix_memory::{
    Actor, DeepQuery, HttpModel, HttpProvider, Memory, MemoryConfig, MemoryLocation, MemoryStore,
    MemoryWorker, Model, ModelReply, ModelRequest, ProjectRepository,
};
use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use codex_model::CodexModel;
use serde_json::json;
use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Instant};

struct Repository(PathBuf);
#[async_trait]
impl ProjectRepository for Repository {
    async fn root(&self, _: &str) -> Result<Option<PathBuf>> {
        Ok(Some(self.0.clone()))
    }
}

struct LiveModel(Arc<dyn Model>);
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
                "model response: {} ms, input/output tokens {}/{}, tools {:?}",
                started.elapsed().as_millis(),
                reply.usage.input_tokens,
                reply.usage.output_tokens,
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

async fn acceptance_model(
    config: &MemoryConfig,
    directory: &std::path::Path,
    codex_luna: bool,
) -> Result<Arc<dyn Model>> {
    let inner: Arc<dyn Model> = if codex_luna {
        eprintln!(
            "using Codex subscription model gpt-6-luna with configured request/task deadlines"
        );
        Arc::new(
            CodexModel::new(
                directory.join("model"),
                "gpt-6-luna",
                0,
                std::time::Duration::from_secs(config.agent.request_timeout_seconds),
            )
            .await?,
        )
    } else {
        let provider = Arc::new(HttpProvider::new(
            config
                .providers
                .get(&config.agent.provider)
                .context("missing provider")?
                .clone(),
        )?);
        Arc::new(HttpModel::new(provider, config.agent.clone())?)
    };
    Ok(inner)
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 4 || (args.len() == 5 && args[4] == "--codex-luna"),
        "usage: compaction_acceptance CONFIG PROJECT_ID RETIRED_VALUE CURRENT_VALUE [--codex-luna]"
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
    let model = Arc::new(LiveModel(
        acceptance_model(&config, temp.path(), args.len() == 5).await?,
    ));
    let repository = Arc::new(Repository(temp.path().into()));
    let worker = MemoryWorker::new(
        store.clone(),
        model.clone(),
        config.agent.clone(),
        repository.clone(),
    );
    let work_ids = schedule_legacy(&store, &copied).await?;
    let started = Instant::now();
    drain_compaction(&store, &worker, &work_ids).await?;
    let current = store.list("acceptance", "", 100, false).await?;
    let stale = current
        .iter()
        .filter(|m| m.content.conclusion.contains(&args[2]))
        .count();
    let canonical = check_atomic(&store, &copied, &current, &args[3]).await?;
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
            &json!({"copied_memories":copied.len(),"scheduled":work_ids.len(),"elapsed_ms":started.elapsed().as_millis(),"active_memories":current.len(),"stale_conclusions":stale,"current_domain_records":canonical.current_domain_records,"legacy_retired":canonical.legacy_retired,"atomic_memories":canonical.atomic_memories,"one_active_per_fact":canonical.one_active_per_fact,"evidence_retained":canonical.evidence_retained,"history_retained":history_retained,"work":store.work_counts().await?,"answer":answer,"current_summaries":current.iter().map(|m| json!({"id":m.id,"revision":m.revision,"status":m.status,"fact":m.content.fact,"conclusion":m.content.conclusion})).collect::<Vec<_>>()})
        )?
    );
    ensure!(
        stale == 0
            && canonical.current_domain_records == 1
            && canonical.legacy_retired
            && canonical.atomic_memories == current.len()
            && canonical.one_active_per_fact
            && canonical.evidence_retained
            && history_retained
            && !answer.insufficient_evidence
            && answer.answer.contains(&args[3]),
        "live semantic compaction acceptance failed"
    );
    Ok(())
}

struct AtomicCheck {
    current_domain_records: usize,
    legacy_retired: bool,
    atomic_memories: usize,
    one_active_per_fact: bool,
    evidence_retained: bool,
}

async fn check_atomic(
    store: &MemoryStore,
    copied: &[Memory],
    current: &[Memory],
    current_value: &str,
) -> Result<AtomicCheck> {
    let current_domain_records = current
        .iter()
        .filter(|m| {
            m.content.fact.as_ref().is_some_and(|fact| {
                fact.attribute.contains("domain") && fact.value == current_value
            })
        })
        .count();
    let atomic_memories = current.iter().filter(|m| m.content.fact.is_some()).count();
    let mut one_active_per_fact = true;
    for memory in current {
        one_active_per_fact &= memory.content.fact.is_some()
            && store
                .fact_versions("acceptance", &memory.content)
                .await?
                .len()
                == 1;
    }
    let mut legacy_retired = true;
    let mut evidence_retained = true;
    for memory in copied {
        let updated = store.show("acceptance", &memory.id, None).await?;
        if memory.content.fact.is_none() {
            let derivatives = store.fact_derivatives("acceptance", &memory.id).await?;
            legacy_retired &=
                updated.status == agentix_memory::Status::Superseded && !derivatives.is_empty();
            evidence_retained &= memory.content.evidence.iter().all(|quote| {
                derivatives.iter().any(|part| {
                    part.content.evidence.iter().any(|retained| {
                        retained.receipt_id == quote.receipt_id
                            && retained.message_id == quote.message_id
                            && retained.quote.contains(&quote.quote)
                    })
                })
            });
        }
    }
    Ok(AtomicCheck {
        current_domain_records,
        legacy_retired,
        atomic_memories,
        one_active_per_fact,
        evidence_retained,
    })
}

async fn drain_compaction(
    store: &MemoryStore,
    worker: &MemoryWorker,
    work_ids: &[i64],
) -> Result<()> {
    loop {
        let counts = store.work_counts().await?;
        ensure!(
            counts.failed == 0,
            "acceptance work exhausted the configured retry budget"
        );
        if counts.pending == 0 && counts.running == 0 {
            break;
        }
        match worker.run_once("acceptance").await {
            Ok(true) => {}
            Ok(false) => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
            Err(error) => {
                eprintln!(
                    "attempt error: {error}; inspect durable work and honor configured retries"
                );
                for id in work_ids {
                    let details = store.work_details(*id).await?;
                    eprintln!(
                        "work {}: {}, attempts {}, error {}",
                        id, details["state"], details["attempts"], details["error"]
                    );
                    ensure!(
                        details["state"] != "failed",
                        "acceptance work failed: {error}"
                    );
                }
            }
        }
    }
    Ok(())
}

async fn schedule_legacy(store: &MemoryStore, copied: &[Memory]) -> Result<Vec<i64>> {
    let mut ordered: Vec<_> = copied.iter().collect();
    ordered.sort_by(|a, b| a.id.cmp(&b.id));
    let mut after = String::new();
    let mut work_ids = Vec::new();
    for memory in ordered {
        if memory.content.fact.is_none() {
            let page = store
                .schedule_compaction("acceptance", &after, 1, true, 0, now())
                .await?;
            ensure!(
                page.scanned == 1 && page.next_after == memory.id,
                "acceptance cursor did not select its legacy seed"
            );
            work_ids.extend(page.work_ids);
        }
        after.clone_from(&memory.id);
    }
    Ok(work_ids)
}
