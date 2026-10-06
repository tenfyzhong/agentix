//! Replay source receipts through the production memory worker.
#[path = "support/codex_model.rs"]
mod codex_model;
#[path = "support/replay_state.rs"]
mod replay_state;
use codex_model::CodexModel;
use replay_state::{ReplayState, file_digest, repository_digest};

use agentix_memory::{AgentConfig, MemoryStore, MemoryWorker, ProjectRepository, Source};
use anyhow::{Result, ensure};
use async_trait::async_trait;
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::process::Command;

struct Repository(PathBuf);
#[async_trait]
impl ProjectRepository for Repository {
    async fn root(&self, _: &str) -> Result<Option<PathBuf>> {
        Ok(Some(self.0.clone()))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 4 || (args.len() == 5 && args[4] == "--resume"),
        "usage: extraction_benchmark SOURCES_JSON REPOSITORY OUTPUT_DIRECTORY [--resume]"
    );
    let sources: Vec<Source> = serde_json::from_slice(&tokio::fs::read(&args[1]).await?)?;
    let directory = PathBuf::from(&args[3]);
    let repository = PathBuf::from(&args[2]).canonicalize()?;
    ensure!(
        !directory.starts_with(&repository),
        "output must be outside fixture repository"
    );
    let codex_version = Command::new("codex").arg("--version").output().await?;
    ensure!(
        codex_version.status.success(),
        "cannot identify Codex version"
    );
    let manifest = json!({"sources_sha256":file_digest(std::path::Path::new(&args[1]))?,
        "binary_sha256":file_digest(&std::env::current_exe()?)?,
        "repository_sha256":repository_digest(&repository)?, "repository":repository,
        "model":"gpt-6-astra", "codex_version":String::from_utf8(codex_version.stdout)?});
    let mut state = ReplayState::open(&directory, &manifest, args.len() == 5)?;
    ensure!(
        state.completed <= sources.len(),
        "checkpoint exceeds input count"
    );
    let directory = directory.canonicalize()?;
    let store = MemoryStore::open(&directory.join("memory.sqlite3")).await?;
    let config = AgentConfig {
        extraction_debounce_ms: 0,
        task_timeout_seconds: 1200,
        lease_seconds: 1260,
        ..AgentConfig::default()
    };
    let model = Arc::new(
        CodexModel::new(
            directory.clone(),
            "gpt-6-astra",
            state.next_request,
            Duration::from_mins(3),
            None,
        )
        .await?,
    );
    let worker = MemoryWorker::new(
        store.clone(),
        model,
        config,
        Arc::new(Repository(repository)),
    );
    // Drain after each chronological receipt: no future source leakage.
    for (index, source) in sources.into_iter().enumerate().skip(state.completed) {
        store.ingest(&source).await?;
        loop {
            let counts = store.work_counts().await?;
            ensure!(counts.failed == 0, "failed work in extraction run");
            if counts.pending == 0 && counts.running == 0 {
                break;
            }
            match worker.run_once("benchmark").await {
                Ok(true) => {}
                Ok(false) => tokio::time::sleep(Duration::from_millis(250)).await,
                Err(error) => eprintln!("worker attempt failed: {error:#}"),
            }
        }
        state.checkpoint(index + 1)?;
        eprintln!("completed {} / {}", source.project_id, source.receipt_id);
    }
    let counts = store.work_counts().await?;
    tokio::fs::write(
        directory.join("completion.json"),
        serde_json::to_vec_pretty(&counts)?,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn replay_resume_reingestion_does_not_duplicate_work() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&dir.path().join("memory.db"))
            .await
            .unwrap();
        let source: Vec<Source> =
            serde_json::from_str(include_str!("../tests/fixtures/quality/smoke-sources.json"))
                .unwrap();
        assert!(store.ingest(&source[0]).await.unwrap());
        let before = store.work_counts().await.unwrap();
        assert!(!store.ingest(&source[0]).await.unwrap());
        let after = store.work_counts().await.unwrap();
        assert_eq!(before.pending, after.pending);
    }

    #[test]
    fn replay_resume_preserves_progress_and_rejects_changed_inputs_or_live_owner() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("run");
        let manifest = json!({"sources":"hash-a","binary":"hash-b"});
        let mut state = ReplayState::open(&output, &manifest, false).unwrap();
        assert!(ReplayState::open(&output, &manifest, true).is_err());
        state.checkpoint(2).unwrap();
        std::fs::write(output.join("request-000004.input.json"), "{}").unwrap();
        drop(state);
        let state = ReplayState::open(&output, &manifest, true).unwrap();
        assert_eq!(state.completed, 2);
        assert_eq!(state.next_request, 5);
        drop(state);
        assert!(ReplayState::open(&output, &json!({"sources":"changed"}), true).is_err());
        assert!(ReplayState::open(&output, &manifest, false).is_err());
    }
}
