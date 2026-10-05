//! Opt-in execution of explicitly queued work using an existing Codex subscription.
//! Usage: `compact_codex CONFIG PROJECT_ID REPOSITORY_ROOT ARTIFACT_DIRECTORY WORK_ID...|--enqueue-legacy`
#[path = "support/codex_model.rs"]
mod codex_model;

use agentix_memory::{MemoryConfig, MemoryLocation, MemoryStore, MemoryWorker, ProjectRepository};
use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use std::{path::PathBuf, sync::Arc, time::Duration};

struct Repository(PathBuf);
impl Repository {
    fn new(root: &std::path::Path) -> Result<Self> {
        ensure!(root.is_dir(), "Project repository directory unavailable");
        Ok(Self(root.canonicalize()?))
    }
}
#[async_trait]
impl ProjectRepository for Repository {
    async fn root(&self, _: &str) -> Result<Option<PathBuf>> {
        Ok(Some(self.0.clone()))
    }
}

async fn schedule_legacy(store: &MemoryStore, project: &str) -> Result<Vec<i64>> {
    let mut memories = store.list(project, "", 100, true).await?;
    ensure!(
        memories.len() < 100,
        "one-shot legacy selection requires a complete page below 100 records"
    );
    memories.sort_by(|a, b| a.id.cmp(&b.id));
    let mut after = String::new();
    let mut ids = Vec::new();
    for memory in memories {
        if memory.actor == agentix_memory::Actor::Agent
            && memory.status == agentix_memory::Status::Active
            && memory.content.fact.is_none()
        {
            let page = store
                .schedule_compaction(
                    project,
                    &after,
                    1,
                    true,
                    0,
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                )
                .await?;
            ensure!(
                page.scanned == 1 && page.next_after == memory.id,
                "legacy selection changed during enqueue"
            );
            ids.extend(page.work_ids);
        }
        after = memory.id;
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentix_memory::Actor;
    use serde_json::json;

    #[tokio::test]
    async fn explicit_repository_root_is_validated_before_enqueue() {
        let dir = tempfile::tempdir().unwrap();
        let repository = Repository::new(dir.path()).unwrap();
        assert_eq!(
            repository.root("target").await.unwrap(),
            Some(dir.path().canonicalize().unwrap())
        );
        assert!(Repository::new(&dir.path().join("missing")).is_err());
    }

    #[tokio::test]
    async fn legacy_selection_queues_only_active_mixed_agent_records_in_requested_project() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&dir.path().join("memory.db"))
            .await
            .unwrap();
        for project in ["target", "other"] {
            let receipt = format!("r-{project}");
            let source = serde_json::from_value(json!({"instance_id":"db","receipt_id":receipt,"sequence":1,"project_id":project,"session_id":"s","turn_id":project,"revision":1,"job_id":null,"recorded_at":1,"messages":[{"id":"m","role":"user","text":"Deployment path is /srv and autostart is enabled."}]})).unwrap();
            store.ingest(&source).await.unwrap();
            for (actor, fact) in [
                (Actor::Agent, serde_json::Value::Null),
                (
                    Actor::Agent,
                    json!({"entity":"service","attribute":"autostart","qualifiers":[],"value":"enabled"}),
                ),
                (Actor::Human, serde_json::Value::Null),
            ] {
                let content = serde_json::from_value(json!({"title":"Deployment","conclusion":"Deployment path is /srv and autostart is enabled.","rationale":"External deployment","scope":"service","conditions":[],"tags":[],"kind":"user_decision","fact":fact,"evidence":[{"receipt_id":receipt,"message_id":"m","quote":"Deployment path is /srv and autostart is enabled."}]})).unwrap();
                store.create(project, content, actor).await.unwrap();
            }
        }
        let ids = schedule_legacy(&store, "target").await.unwrap();
        assert_eq!(ids.len(), 1);
        let item = store.work_details(ids[0]).await.unwrap();
        assert_eq!(item["project_id"], "target");
        assert_eq!(item["kind"], "consolidate");
        assert!(schedule_legacy(&store, "target").await.unwrap().is_empty());
        assert_eq!(store.work_counts().await.unwrap().pending, 3);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        args.len() >= 5,
        "usage: compact_codex CONFIG PROJECT_ID REPOSITORY_ROOT ARTIFACT_DIRECTORY WORK_ID...|--enqueue-legacy"
    );
    let config = MemoryConfig::load(std::path::Path::new(&args[0]))?;
    let location = MemoryLocation::load(std::path::Path::new(&args[0]))?;
    let repository = Repository::new(std::path::Path::new(&args[2]))?;
    let enqueue_legacy = args.len() == 5 && args[4] == "--enqueue-legacy";
    let mut ids: Vec<i64> = if enqueue_legacy {
        Vec::new()
    } else {
        args[4..]
            .iter()
            .map(|id| id.parse())
            .collect::<std::result::Result<_, _>>()?
    };
    let original = MemoryStore::open_read_only(&location.path).await?;
    for id in &ids {
        let item = original.work_details(*id).await?;
        ensure!(
            item["project_id"] == args[1] && item["kind"] == "consolidate",
            "selected work is outside the requested Project or is not consolidation"
        );
        ensure!(
            matches!(item["state"].as_str(), Some("pending" | "running")),
            "selected work is not executable"
        );
    }
    drop(original);
    let artifacts = PathBuf::from(&args[3]);
    ensure!(!artifacts.exists(), "artifact directory must be new");
    let codex_home = std::env::var_os("CODEX_HOME").map_or(
        PathBuf::from(std::env::var_os("HOME").context("missing Codex home")?).join(".codex"),
        PathBuf::from,
    );
    let model = codex_model::CodexModel::new(
        artifacts,
        "gpt-6-luna",
        0,
        Duration::from_secs(config.agent.request_timeout_seconds),
        Some(&codex_home.join("models_cache.json")),
    )
    .await?;
    let store = MemoryStore::open(&location.path).await?;
    if enqueue_legacy {
        ids = schedule_legacy(&store, &args[1]).await?;
    }
    let worker = MemoryWorker::new(
        store.clone(),
        Arc::new(model),
        config.agent,
        Arc::new(repository),
    );
    eprintln!(
        "Executing {} selected work items using gpt-6-luna, reasoning low",
        ids.len()
    );
    for id in ids {
        loop {
            let item = store.work_details(id).await?;
            eprintln!(
                "work {id}: state {}, attempts {}, error {}",
                item["state"], item["attempts"], item["error"]
            );
            match item["state"].as_str() {
                Some("done") => break,
                Some("failed" | "cancelled") => {
                    anyhow::bail!("selected work {id} stopped: {}", item["error"])
                }
                _ => {}
            }
            match worker.run_work_item("manual-codex-luna", id).await {
                Ok(true) => {}
                Ok(false) => tokio::time::sleep(Duration::from_millis(250)).await,
                Err(error) => eprintln!("work {id} attempt failed: {error}"),
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({"project_id":args[1],"model":"gpt-6-luna","reasoning_effort":"low","completed":true})
    );
    Ok(())
}
