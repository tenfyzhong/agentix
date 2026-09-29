use crate::{EmbeddingConfig, HttpEmbedding, Memory, MemoryStore};
use anyhow::{Result, ensure};
use serde::Serialize;
use sqlx::Row;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Debug, Serialize)]
pub struct EmbeddingProfile {
    pub generation: i64,
    pub fingerprint: String,
    pub dimensions: usize,
}
impl MemoryStore {
    pub async fn embedding_profile(&self, project: &str) -> Result<Option<EmbeddingProfile>> {
        let row = sqlx::query(
            "SELECT generation,fingerprint,dimensions FROM embedding_profiles WHERE project_id=?",
        )
        .bind(project)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            Ok(EmbeddingProfile {
                generation: r.try_get("generation")?,
                fingerprint: r.try_get("fingerprint")?,
                dimensions: usize::try_from(r.try_get::<i64, _>("dimensions")?)?,
            })
        })
        .transpose()
    }
    async fn resolve_dimensions(
        &self,
        project: &str,
        generation: i64,
        dimensions: usize,
    ) -> Result<bool> {
        ensure!(
            (1..=16384).contains(&dimensions),
            "invalid embedding dimensions"
        );
        Ok(sqlx::query("UPDATE embedding_profiles SET dimensions=? WHERE project_id=? AND generation=? AND dimensions IN (0,?)")
            .bind(i64::try_from(dimensions)?).bind(project).bind(generation).bind(i64::try_from(dimensions)?).execute(&self.pool).await?.rows_affected()==1)
    }
}

pub struct EmbeddingIndex {
    store: MemoryStore,
    embedding: Arc<HttpEmbedding>,
    config: EmbeddingConfig,
}
impl EmbeddingIndex {
    #[must_use]
    pub fn new(store: MemoryStore, embedding: Arc<HttpEmbedding>, config: EmbeddingConfig) -> Self {
        Self {
            store,
            embedding,
            config,
        }
    }
    /// Call when installing a configuration, never from a delayed batch result.
    pub async fn activate(&self, project: &str) -> Result<i64> {
        self.store
            .reserve_embedding(
                project,
                &self.embedding.fingerprint(),
                self.config.dimensions,
            )
            .await
    }
    pub async fn step(&self, project: &str, generation: i64) -> Result<usize> {
        let Some(profile) = self.store.embedding_profile(project).await? else {
            return Ok(0);
        };
        if profile.generation != generation || profile.fingerprint != self.embedding.fingerprint() {
            return Ok(0);
        }
        let pending = self
            .store
            .embedding_pending(
                project,
                generation,
                "",
                i64::try_from(self.config.batch_size)?,
            )
            .await?;
        if pending.is_empty() {
            return Ok(0);
        }
        let texts = pending
            .iter()
            .map(|m| {
                format!(
                    "{}\n{}\n{}\n{}\n{}\n{}",
                    m.content.title,
                    m.content.conclusion,
                    m.content.rationale,
                    m.content.scope,
                    m.content.conditions.join("\n"),
                    m.content.tags.join(" ")
                )
            })
            .collect::<Vec<_>>();
        // No connection or transaction is retained during the provider call.
        let vectors = self.embedding.documents(&texts).await?;
        if !self
            .store
            .resolve_dimensions(project, generation, vectors[0].len())
            .await?
        {
            return Ok(0);
        }
        let mut written = 0;
        for (memory, vector) in pending.iter().zip(vectors) {
            written += usize::from(
                self.store
                    .put_embedding(project, &memory.id, memory.revision, generation, &vector)
                    .await?,
            );
        }
        Ok(written)
    }
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub mode: String,
    pub memories: Vec<Memory>,
}
pub struct SemanticRetrieval {
    store: MemoryStore,
    embedding: Option<Arc<HttpEmbedding>>,
    timeout_ms: u64,
    queries: Mutex<VecDeque<CachedQuery>>,
}
struct CachedQuery {
    project: String,
    generation: i64,
    query: String,
    vector: Vec<f32>,
    expires: Instant,
}
impl SemanticRetrieval {
    #[must_use]
    pub fn new(store: MemoryStore, embedding: Option<Arc<HttpEmbedding>>, timeout_ms: u64) -> Self {
        Self {
            store,
            embedding,
            timeout_ms,
            queries: Mutex::new(VecDeque::new()),
        }
    }
    pub async fn search(&self, project: &str, query: &str, limit: i64) -> Result<SearchResult> {
        // Validate the query before paying for an embedding.
        crate::retrieval::query(project, query)?;
        ensure!((1..=100).contains(&limit), "invalid memory search limit");
        if let Some(embedding) = &self.embedding {
            let semantic = async {
                let Some(profile) = self.store.embedding_profile(project).await? else {
                    return Ok(None);
                };
                if profile.fingerprint != embedding.fingerprint() || profile.dimensions == 0 {
                    return Ok(None);
                }
                // Cache only embeddings, never search results or lifecycle visibility.
                // Capacity 64 and dimensions <=16384 bound retained vectors to 4 MiB.
                let cached = self
                    .queries
                    .lock()
                    .expect("query cache lock")
                    .iter()
                    .find(|entry| {
                        entry.project == project
                            && entry.generation == profile.generation
                            && entry.query == query
                            && entry.expires > Instant::now()
                    })
                    .map(|entry| entry.vector.clone());
                let vector = if let Some(vector) = cached {
                    vector
                } else {
                    let vector = embedding.query(query).await?;
                    let mut cache = self.queries.lock().expect("query cache lock");
                    cache.retain(|entry| entry.expires > Instant::now());
                    if cache.len() >= 64 {
                        cache.pop_front();
                    }
                    cache.push_back(CachedQuery {
                        project: project.into(),
                        generation: profile.generation,
                        query: query.into(),
                        vector: vector.clone(),
                        expires: Instant::now() + Duration::from_mins(1),
                    });
                    vector
                };
                Ok::<_, anyhow::Error>(Some(
                    self.store
                        .hybrid_search(project, query, profile.generation, &vector, limit)
                        .await?,
                ))
            };
            if let Ok(Ok(Some(memories))) =
                tokio::time::timeout(Duration::from_millis(self.timeout_ms), semantic).await
            {
                return Ok(SearchResult {
                    mode: "hybrid".into(),
                    memories,
                });
            }
        }
        Ok(SearchResult {
            mode: if self.embedding.is_some() {
                "fts_fallback"
            } else {
                "fts"
            }
            .into(),
            memories: self.store.search(project, query, limit).await?,
        })
    }
}
