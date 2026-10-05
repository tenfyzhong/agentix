use crate::{EmbeddingConfig, HttpEmbedding, Memory, MemoryStore};
use anyhow::{Result, ensure};
use serde::Serialize;
use sqlx::Row;
use std::{
    collections::{HashMap, VecDeque},
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
        match self
            .embed_and_write(project, generation, &pending, &texts)
            .await
        {
            Ok(written) => Ok(written),
            Err(error) => {
                // Isolate document-specific rejections. Do not amplify an outage,
                // authentication failure or rate limit into one request per item.
                let isolate = pending.len() > 1
                    && error
                        .downcast_ref::<crate::providers::ProviderHttpError>()
                        .is_some_and(|status| matches!(status.0, 400 | 413 | 422));
                for (memory, text) in pending.iter().zip(&texts) {
                    if isolate {
                        match self
                            .embed_and_write(
                                project,
                                generation,
                                std::slice::from_ref(memory),
                                std::slice::from_ref(text),
                            )
                            .await
                        {
                            Ok(_) => {}
                            Err(error) => {
                                self.store
                                    .record_embedding_failure(
                                        memory,
                                        generation,
                                        &error.to_string(),
                                    )
                                    .await?;
                            }
                        }
                    } else {
                        self.store
                            .record_embedding_failure(memory, generation, &error.to_string())
                            .await?;
                    }
                }
                Err(error)
            }
        }
    }

    async fn embed_and_write(
        &self,
        project: &str,
        generation: i64,
        pending: &[Memory],
        texts: &[String],
    ) -> Result<usize> {
        let vectors = self.embedding.documents(texts).await?;
        self.write_vectors(project, generation, pending, vectors)
            .await
    }

    async fn write_vectors(
        &self,
        project: &str,
        generation: i64,
        pending: &[Memory],
        vectors: Vec<Vec<f32>>,
    ) -> Result<usize> {
        if !self
            .store
            .resolve_dimensions(project, generation, vectors[0].len())
            .await?
        {
            let profile = self.store.embedding_profile(project).await?;
            ensure!(
                profile.is_none_or(|p| p.generation != generation),
                "invalid: embedding response dimension differs from current profile"
            );
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

impl MemoryStore {
    async fn record_embedding_failure(
        &self,
        memory: &Memory,
        generation: i64,
        error: &str,
    ) -> Result<()> {
        let error: String = error.chars().take(512).collect();
        // A delayed provider failure must not suppress a newer memory or model.
        sqlx::query("INSERT INTO embedding_failures(memory_id,generation,revision,attempts,available_at,error) SELECT m.id,p.generation,m.revision,1,unixepoch()+2,? FROM memories m JOIN embedding_profiles p ON p.project_id=m.project_id WHERE m.id=? AND m.project_id=? AND m.revision=? AND p.generation=? AND m.status='active' ON CONFLICT(memory_id,generation,revision) DO UPDATE SET attempts=embedding_failures.attempts+1,available_at=unixepoch()+min(300,1 << min(embedding_failures.attempts+1,8)),error=excluded.error")
            .bind(error).bind(&memory.id).bind(&memory.project_id).bind(memory.revision).bind(generation).execute(&self.pool).await?;
        Ok(())
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
    queries: Arc<Mutex<QueryCache>>,
}
type QueryKey = (String, i64, String);
type QueryReply = std::result::Result<Vec<f32>, String>;
#[derive(Default)]
struct QueryCache {
    completed: VecDeque<CachedQuery>,
    pending: HashMap<
        QueryKey,
        (
            tokio::sync::watch::Receiver<Option<QueryReply>>,
            tokio::task::AbortHandle,
        ),
    >,
}
impl Drop for QueryCache {
    fn drop(&mut self) {
        for (_, handle) in self.pending.values() {
            handle.abort();
        }
    }
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
            queries: Arc::new(Mutex::new(QueryCache::default())),
        }
    }
    pub async fn search(&self, project: &str, query: &str, limit: i64) -> Result<SearchResult> {
        self.search_with_timeout(project, query, limit, self.timeout_ms)
            .await
    }

    async fn query_vector(
        &self,
        embedding: Arc<HttpEmbedding>,
        project: &str,
        generation: i64,
        query: &str,
    ) -> Result<Vec<f32>> {
        let key = (project.to_owned(), generation, query.to_owned());
        let mut receiver = {
            let mut cache = self.queries.lock().expect("query cache lock");
            cache
                .completed
                .retain(|entry| entry.expires > Instant::now());
            if let Some(entry) = cache.completed.iter().find(|entry| {
                entry.project == project && entry.generation == generation && entry.query == query
            }) {
                return Ok(entry.vector.clone());
            }
            if let Some((receiver, _)) = cache.pending.get(&key) {
                receiver.clone()
            } else {
                ensure!(cache.pending.len() < 64, "query embedding capacity reached");
                let (sender, receiver) = tokio::sync::watch::channel(None);
                let weak = Arc::downgrade(&self.queries);
                let task_key = key.clone();
                let timeout = self.timeout_ms;
                let task = tokio::spawn(async move {
                    let result = match tokio::time::timeout(
                        Duration::from_millis(timeout),
                        embedding.query(&task_key.2),
                    )
                    .await
                    {
                        Ok(result) => result.map_err(|e| e.to_string()),
                        Err(_) => Err("query embedding timeout".into()),
                    };
                    if let Some(shared) = weak.upgrade() {
                        let mut cache = shared.lock().expect("query cache lock");
                        cache.pending.remove(&task_key);
                        if let Ok(vector) = &result {
                            if cache.completed.len() >= 64 {
                                cache.completed.pop_front();
                            }
                            cache.completed.push_back(CachedQuery {
                                project: task_key.0,
                                generation: task_key.1,
                                query: task_key.2,
                                vector: vector.clone(),
                                expires: Instant::now() + Duration::from_mins(1),
                            });
                        }
                        sender.send_replace(Some(result));
                    }
                });
                cache
                    .pending
                    .insert(key, (receiver.clone(), task.abort_handle()));
                receiver
            }
        };
        loop {
            if let Some(result) = receiver.borrow_and_update().clone() {
                return result.map_err(anyhow::Error::msg);
            }
            receiver
                .changed()
                .await
                .map_err(|_| anyhow::anyhow!("query embedding cancelled"))?;
        }
    }

    /// A caller may reserve part of its end-to-end budget for lexical fallback
    /// and transport without extending the configured semantic deadline.
    pub async fn search_with_timeout(
        &self,
        project: &str,
        query: &str,
        limit: i64,
        timeout_ms: u64,
    ) -> Result<SearchResult> {
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
                let vector = self
                    .query_vector(embedding.clone(), project, profile.generation, query)
                    .await?;
                Ok::<_, anyhow::Error>(Some(
                    self.store
                        .hybrid_search(project, query, profile.generation, &vector, limit)
                        .await?,
                ))
            };
            if let Ok(Ok(Some(memories))) = tokio::time::timeout(
                Duration::from_millis(self.timeout_ms.min(timeout_ms)),
                semantic,
            )
            .await
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
