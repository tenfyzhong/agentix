//! Benchmark adapter using the production `MemoryStore`, not a second search implementation.
use agentix_memory::{Actor, MemoryInput, MemoryStore};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    time::Instant,
};

#[derive(Deserialize)]
struct Document {
    id: String,
    project: String,
    text: String,
}

#[derive(Deserialize)]
struct Query {
    id: String,
    project: String,
    question: String,
}

#[derive(Deserialize)]
struct Embedding {
    sha256: String,
    vector: Vec<f32>,
}

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

async fn import_documents(
    store: &MemoryStore,
    documents: &[Document],
    vectors: &HashMap<String, Vec<f32>>,
) -> Result<HashMap<String, String>> {
    let mut mapping = HashMap::new();
    let mut generations = HashMap::new();
    for document in documents {
        let input: MemoryInput = serde_json::from_value(json!({
            "title": document.id, "conclusion": document.text,
            "rationale": "Raw dialogue retrieval baseline", "scope": "project",
            "tags": [], "kind": "observation", "evidence": []
        }))?;
        let memory = store.create(&document.project, input, Actor::Human).await?;
        if !vectors.is_empty() {
            let generation = if let Some(generation) = generations.get(&document.project) {
                *generation
            } else {
                let generation = store
                    .configure_embedding(&document.project, "bge-m3", 1024)
                    .await?;
                generations.insert(document.project.clone(), generation);
                generation
            };
            let vector = vectors
                .get(&digest(&document.text))
                .context("missing document vector")?;
            ensure!(
                store
                    .put_embedding(
                        &document.project,
                        &memory.id,
                        memory.revision,
                        generation,
                        vector
                    )
                    .await?,
                "vector rejected"
            );
        }
        mapping.insert(memory.id, document.id.clone());
    }
    Ok(mapping)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 5,
        "usage: retrieval_benchmark CORPUS QUESTIONS VECTORS_JSONL_OR_DASH OUTPUT_JSONL"
    );
    let documents: Vec<Document> = serde_json::from_reader(File::open(&args[1])?)?;
    let questions: Vec<Query> = serde_json::from_reader(File::open(&args[2])?)?;
    let mut vectors = HashMap::new();
    if args[3] != "-" {
        for line in BufReader::new(File::open(&args[3])?).lines() {
            let row: Embedding = serde_json::from_str(&line?)?;
            vectors.insert(row.sha256, row.vector);
        }
    }
    // Refuse accidental overwrite of a completed run.
    let output = File::options()
        .write(true)
        .create_new(true)
        .open(&args[4])?;
    let mut output = BufWriter::new(output);
    let dir = tempfile::tempdir()?;
    let store = MemoryStore::open(&dir.path().join("memory.db")).await?;
    let start = Instant::now();
    let mapping = import_documents(&store, &documents, &vectors).await?;
    eprintln!(
        "indexed {} documents in {:.3}s",
        documents.len(),
        start.elapsed().as_secs_f64()
    );
    for question in questions {
        for mode in ["fts", "hybrid"] {
            if mode == "hybrid" && vectors.is_empty() {
                continue;
            }
            let start = Instant::now();
            let results = if mode == "fts" {
                store
                    .search(&question.project, &question.question, 20)
                    .await?
            } else {
                let vector = vectors
                    .get(&digest(&question.question))
                    .context("missing query vector")?;
                store
                    .hybrid_search(&question.project, &question.question, 1, vector, 20)
                    .await?
            };
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            let ids: Vec<_> = results.iter().map(|r| &mapping[&r.id]).collect();
            writeln!(
                output,
                "{}",
                json!({"id":question.id,"mode":mode,"ids":ids,"retrieval_ms":elapsed})
            )?;
        }
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn adapter_preserves_ids_and_project_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&dir.path().join("memory.db"))
            .await
            .unwrap();
        let documents = vec![
            Document {
                id: "a".into(),
                project: "p".into(),
                text: "chosen cobalt deployment".into(),
            },
            Document {
                id: "b".into(),
                project: "other".into(),
                text: "chosen cobalt deployment".into(),
            },
        ];
        let mapping = import_documents(&store, &documents, &HashMap::new())
            .await
            .unwrap();
        let result = store.search("p", "cobalt", 10).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(mapping[&result[0].id], "a");
    }
}
