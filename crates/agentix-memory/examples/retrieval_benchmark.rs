//! Benchmark adapter using the production `MemoryStore`, not a second search implementation.
use agentix_memory::{Actor, MemoryInput, MemoryStore};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    path::Path,
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

fn lexical_query(query: &str, variant: &str) -> String {
    if !variant.ends_with("-stop") {
        return query.to_owned();
    }
    let words: Vec<_> = query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
        .filter(|word| {
            !matches!(
                word.to_ascii_lowercase().as_str(),
                "a" | "an"
                    | "the"
                    | "what"
                    | "which"
                    | "who"
                    | "whom"
                    | "whose"
                    | "when"
                    | "where"
                    | "why"
                    | "how"
                    | "is"
                    | "are"
                    | "was"
                    | "were"
                    | "be"
                    | "been"
                    | "being"
                    | "do"
                    | "does"
                    | "did"
                    | "has"
                    | "have"
                    | "had"
                    | "would"
                    | "could"
                    | "should"
                    | "can"
                    | "will"
                    | "shall"
                    | "of"
                    | "to"
                    | "in"
                    | "on"
                    | "at"
                    | "for"
                    | "from"
                    | "with"
                    | "by"
                    | "about"
                    | "and"
                    | "or"
                    | "it"
                    | "its"
                    | "he"
                    | "his"
                    | "she"
                    | "her"
                    | "they"
                    | "their"
                    | "them"
                    | "you"
                    | "your"
                    | "i"
                    | "my"
                    | "we"
                    | "our"
                    | "s"
            )
        })
        .collect();
    if words.is_empty() {
        query.to_owned()
    } else {
        words.join(" ")
    }
}

// Experiment only: the caller creates an empty temporary store for this run.
async fn configure_lexical_experiment(path: &Path, variant: &str) -> Result<()> {
    ensure!(
        matches!(
            variant,
            "baseline" | "porter" | "baseline-stop" | "porter-stop"
        ),
        "unknown lexical experiment"
    );
    if variant.starts_with("baseline") {
        return Ok(());
    }
    let mut connection =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path)).await?;
    let mut tx = connection.begin().await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM memories")
        .fetch_one(&mut *tx)
        .await?;
    ensure!(count == 0, "lexical experiment requires an empty store");
    sqlx::raw_sql("DROP TABLE memory_fts; CREATE VIRTUAL TABLE memory_fts USING fts5(project_token,title,body,tags,scope,tokenize='porter unicode61');")
        .execute(&mut *tx).await?;
    tx.commit().await?;
    connection.close().await?;
    Ok(())
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
        (5..=6).contains(&args.len()),
        "usage: retrieval_benchmark CORPUS QUESTIONS VECTORS_JSONL_OR_DASH OUTPUT_JSONL [baseline|porter|baseline-stop|porter-stop]"
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
    let variant = args.get(5).map_or("baseline", String::as_str);
    configure_lexical_experiment(&dir.path().join("memory.db"), variant).await?;
    let start = Instant::now();
    let mapping = import_documents(&store, &documents, &vectors).await?;
    eprintln!(
        "indexed {} documents in {:.3}s",
        documents.len(),
        start.elapsed().as_secs_f64()
    );
    for question in questions {
        let lexical = lexical_query(&question.question, variant);
        for mode in ["fts", "hybrid"] {
            if mode == "hybrid" && vectors.is_empty() {
                continue;
            }
            let start = Instant::now();
            let results = if mode == "fts" {
                store.search(&question.project, &lexical, 20).await?
            } else {
                let vector = vectors
                    .get(&digest(&question.question))
                    .context("missing query vector")?;
                store
                    .hybrid_search(&question.project, &lexical, 1, vector, 20)
                    .await?
            };
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            let ids: Vec<_> = results.iter().map(|r| &mapping[&r.id]).collect();
            writeln!(
                output,
                "{}",
                json!({"id":question.id,"mode":mode,"ids":ids,"retrieval_ms":elapsed,"lexical_variant":variant})
            )?;
        }
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopword_experiment_preserves_identifiers_negation_and_nonempty_queries() {
        assert_eq!(
            lexical_query("Where has Melanie camped?", "porter-stop"),
            "Melanie camped"
        );
        assert_eq!(
            lexical_query("Why not use cloud_embedding?", "baseline-stop"),
            "not use cloud_embedding"
        );
        assert_eq!(lexical_query("why", "porter-stop"), "why");
        assert_eq!(lexical_query("中文决定", "porter-stop"), "中文决定");
        assert_eq!(
            lexical_query("Where has Melanie camped?", "porter"),
            "Where has Melanie camped?"
        );
    }

    #[tokio::test]
    async fn porter_experiment_matches_inflections_without_cross_project_results() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        let store = MemoryStore::open(&path).await.unwrap();
        configure_lexical_experiment(&path, "porter").await.unwrap();
        let documents = vec![Document {
            id: "camp".into(),
            project: "p".into(),
            text: "We enjoy camping".into(),
        }];
        import_documents(&store, &documents, &HashMap::new())
            .await
            .unwrap();
        assert_eq!(store.search("p", "camped", 10).await.unwrap().len(), 1);
        assert!(
            store
                .search("other", "camped", 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(configure_lexical_experiment(&path, "porter").await.is_err());
    }

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
