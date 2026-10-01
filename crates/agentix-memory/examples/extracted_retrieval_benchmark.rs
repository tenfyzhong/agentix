//! Query a consistent snapshot of an extracted memory store using production retrieval.
use agentix_memory::{
    EmbeddingConfig, EmbeddingIndex, HttpEmbedding, HttpProvider, MemoryStore, ProviderConfig,
    ProviderProtocol, SemanticRetrieval,
};
use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::json;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufWriter, Write},
    path::Path,
    sync::Arc,
    time::Instant,
};

#[derive(Deserialize)]
struct Query {
    id: String,
    project: String,
    question: String,
}

async fn snapshot_database(source: &Path, target: &Path) -> Result<()> {
    ensure!(source.is_file(), "source database missing");
    // Reserve the destination atomically; SQLite accepts an empty destination.
    File::options().write(true).create_new(true).open(target)?;
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(source).read_only(true),
    )
    .await?;
    sqlx::query("VACUUM INTO ?")
        .bind(target.to_string_lossy().as_ref())
        .execute(&mut connection)
        .await?;
    connection.close().await?;
    Ok(())
}

fn require_mode(expected: &str, actual: &str) -> Result<()> {
    ensure!(
        expected == actual,
        "requested {expected}, got {actual}; incomplete benchmark"
    );
    Ok(())
}

fn embedding_index(
    store: &MemoryStore,
    endpoint: &str,
) -> Result<(Arc<HttpEmbedding>, EmbeddingIndex)> {
    let config = EmbeddingConfig {
        enabled: true,
        provider: "benchmark-ollama".into(),
        model: "bge-m3".into(),
        dimensions: Some(1024),
        request_timeout_seconds: 120,
        ..EmbeddingConfig::default()
    };
    let provider = Arc::new(HttpProvider::new(ProviderConfig {
        protocol: ProviderProtocol::Ollama,
        base_url: endpoint.to_owned(),
        api_key_env: None,
        max_in_flight: 2,
    })?);
    let model = Arc::new(HttpEmbedding::new(provider, config.clone())?);
    let index = EmbeddingIndex::new(store.clone(), model.clone(), config);
    Ok((model, index))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 5,
        "usage: extracted_retrieval_benchmark DATABASE QUESTIONS OLLAMA_URL_OR_DASH NEW_OUTPUT_DIRECTORY"
    );
    let questions: Vec<Query> = serde_json::from_reader(File::open(&args[2])?)?;
    ensure!(!questions.is_empty(), "no questions");
    let ids: BTreeSet<_> = questions.iter().map(|q| &q.id).collect();
    ensure!(ids.len() == questions.len(), "duplicate query IDs");
    let output = Path::new(&args[4]);
    std::fs::create_dir(output)?;
    let database = output.join("memory.sqlite3");
    snapshot_database(Path::new(&args[1]), &database).await?;
    let store = MemoryStore::open(&database).await?;
    let counts = store.work_counts().await?;
    ensure!(
        counts.pending == 0 && counts.running == 0 && counts.failed == 0,
        "extraction work unfinished"
    );
    let projects: BTreeSet<_> = questions.iter().map(|q| q.project.clone()).collect();
    let mut check = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&database)
            .read_only(true),
    )
    .await?;
    for project in &projects {
        let sources: i64 = sqlx::query_scalar("SELECT count(*) FROM sources WHERE project_id=?")
            .bind(project)
            .fetch_one(&mut check)
            .await?;
        ensure!(
            sources > 0,
            "query project has no extracted sources: {project}"
        );
    }
    let mut embedding = None;
    let start = Instant::now();
    if args[3] != "-" {
        let (model, index) = embedding_index(&store, &args[3])?;
        for project in &projects {
            let generation = index.activate(project).await?;
            while index.step(project, generation).await? > 0 {}
            let missing: i64 = sqlx::query_scalar("SELECT count(*) FROM memories m WHERE project_id=? AND status IN ('active','conflicted') AND (valid_until IS NULL OR valid_until>unixepoch()) AND NOT EXISTS(SELECT 1 FROM memory_vectors v WHERE v.memory_id=m.id AND v.revision=m.revision AND v.generation=?)")
                .bind(project).bind(generation).fetch_one(&mut check).await?;
            ensure!(missing == 0, "incomplete vectors for {project}: {missing}");
        }
        embedding = Some(model);
    }
    let index_ms = start.elapsed().as_secs_f64() * 1000.0;
    check.close().await?;
    let fts = SemanticRetrieval::new(store.clone(), None, 120_000);
    let hybrid = SemanticRetrieval::new(store, embedding.clone(), 120_000);
    let mut results = BufWriter::new(File::create(output.join("results.jsonl"))?);
    for question in &questions {
        for (mode, retrieval) in [("fts", &fts), ("hybrid", &hybrid)] {
            if mode == "hybrid" && embedding.is_none() {
                continue;
            }
            let start = Instant::now();
            let result = retrieval
                .search(&question.project, &question.question, 20)
                .await?;
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            let evidence: Vec<Vec<_>> = result
                .memories
                .iter()
                .map(|m| {
                    m.content
                        .evidence
                        .iter()
                        .map(|e| e.message_id.clone())
                        .collect()
                })
                .collect();
            writeln!(
                results,
                "{}",
                json!({"id":question.id,"project":question.project,
                "mode":mode,"actual_mode":result.mode,"retrieval_ms":elapsed,
                "evidence":evidence,"memories":result.memories})
            )?;
            results.flush()?;
            require_mode(mode, &result.mode)?;
        }
    }
    std::fs::write(
        output.join("completion.json"),
        serde_json::to_vec_pretty(&json!({
        "questions":questions.len(),"projects":projects,"index_ms":index_ms,
        "embedding_model":embedding.as_ref().map(|_|"bge-m3"),"dimensions":1024,
        "query_latency_includes_embedding":true,"limit":20}))?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentix_memory::{Actor, MemoryInput, MemoryStore};
    use serde_json::json;

    #[tokio::test]
    async fn snapshot_preserves_wal_content_and_does_not_modify_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.db");
        let store = MemoryStore::open(&source).await.unwrap();
        let input: MemoryInput = serde_json::from_value(json!({
            "title":"Cobalt decision", "conclusion":"Keep Cobalt offline", "rationale":"Contract",
            "scope":"production", "tags":[], "kind":"user_decision", "evidence":[]
        }))
        .unwrap();
        let memory = store.create("p", input, Actor::Human).await.unwrap();
        let target = dir.path().join("snapshot.db");
        snapshot_database(&source, &target).await.unwrap();
        let snapshot = MemoryStore::open(&target).await.unwrap();
        let found = snapshot.search("p", "Cobalt", 20).await.unwrap();
        assert_eq!(found[0].id, memory.id);
        assert_eq!(found[0].content.rationale, "Contract");
        assert!(
            snapshot
                .search("other", "Cobalt", 20)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(snapshot_database(&source, &target).await.is_err());
        assert!(store.embedding_profile("p").await.unwrap().is_none());
    }

    #[test]
    fn hybrid_fallback_cannot_be_reported_as_success() {
        assert!(require_mode("hybrid", "hybrid").is_ok());
        assert!(require_mode("hybrid", "fts_fallback").is_err());
    }
}
