use crate::{
    Actor, HttpEmbedding, MemoryInput, MemoryStore, ProjectTools, RequestHandler, RetrievalConfig,
    SemanticRetrieval, ServiceConfig, Status, ToolSet,
};
use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemoryRequest {
    ProjectionStatus {
        project: String,
    },
    Status {
        project: Option<String>,
    },
    Search {
        project: String,
        query: String,
        #[serde(default = "default_limit")]
        limit: i64,
    },
    Show {
        project: String,
        id: String,
        revision: Option<i64>,
    },
    List {
        project: String,
        #[serde(default)]
        after: String,
        #[serde(default = "default_limit")]
        limit: i64,
        #[serde(default)]
        all: bool,
    },
    Context {
        project: String,
        session: String,
        turn: String,
        query: String,
        #[serde(default)]
        budget: Option<usize>,
    },
    Source {
        project: String,
        receipt_id: String,
        message_id: Option<String>,
        #[serde(default)]
        offset: usize,
    },
    Create {
        project: String,
        content: MemoryInput,
        actor: Actor,
    },
    Update {
        project: String,
        id: String,
        revision: i64,
        content: MemoryInput,
        actor: Actor,
    },
    SetStatus {
        project: String,
        id: String,
        revision: i64,
        status: Status,
        reason: String,
        actor: Actor,
    },
    Reindex {
        project: String,
        #[serde(default)]
        after: String,
        #[serde(default = "default_limit")]
        limit: i64,
    },
    Compact {
        project: String,
        #[serde(default)]
        after: String,
        #[serde(default = "default_limit")]
        limit: i64,
    },
    Work {
        project: String,
        id: i64,
    },
    Retry {
        project: String,
        id: i64,
    },
    Receipt {
        project: String,
        receipt_id: String,
    },
    Ask {
        project: String,
        query: String,
    },
}
fn default_limit() -> i64 {
    10
}

pub struct MemoryApi {
    store: MemoryStore,
    retrieval: SemanticRetrieval,
    config: RetrievalConfig,
    queries: Semaphore,
    deep: Option<crate::DeepQuery>,
}
impl MemoryApi {
    #[must_use]
    pub fn new(store: MemoryStore, config: RetrievalConfig, service: ServiceConfig) -> Self {
        Self {
            retrieval: SemanticRetrieval::new(store.clone(), None, config.query_timeout_ms),
            store,
            config,
            queries: Semaphore::new(service.max_query_concurrency),
            deep: None,
        }
    }
    #[must_use]
    pub fn with_embedding(mut self, embedding: Arc<HttpEmbedding>) -> Self {
        self.retrieval = SemanticRetrieval::new(
            self.store.clone(),
            Some(embedding),
            self.config.query_timeout_ms,
        );
        self
    }
    #[must_use]
    pub fn with_deep_query(mut self, deep: crate::DeepQuery) -> Self {
        self.deep = Some(deep);
        self
    }

    #[allow(clippy::too_many_lines)] // Exhaustive API dispatch keeps operation guards visible.
    async fn execute(&self, request: MemoryRequest) -> Result<Value> {
        match request {
            MemoryRequest::ProjectionStatus { project } => {
                self.store.projection_status(&project).await
            }
            MemoryRequest::Status { project } => self.store.memory_status(project.as_deref()).await,
            MemoryRequest::Search {
                project,
                query,
                limit,
            } => Ok(serde_json::to_value(
                self.retrieval.search(&project, &query, limit).await?,
            )?),
            MemoryRequest::Show {
                project,
                id,
                revision,
            } => Ok(serde_json::to_value(
                self.store.show(&project, &id, revision).await?,
            )?),
            MemoryRequest::List {
                project,
                after,
                limit,
                all,
            } => Ok(serde_json::to_value(
                self.store.list(&project, &after, limit, all).await?,
            )?),
            MemoryRequest::Context {
                project,
                session,
                turn,
                query,
                budget,
            } => {
                crate::retrieval::query(&project, &query)?;
                let budget = budget
                    .unwrap_or(self.config.max_context_bytes)
                    .min(self.config.max_context_bytes);
                if let Some(packet) = self
                    .store
                    .cached_context(&project, &session, &turn, budget)
                    .await?
                {
                    let mut value = serde_json::to_value(packet)?;
                    value["mode"] = json!("context_cache");
                    return Ok(value);
                }
                let results = self
                    .retrieval
                    // Hosts have a 1500 ms budget including CLI/IPC and FTS.
                    .search_with_timeout(
                        &project,
                        &query,
                        i64::try_from(self.config.max_items)?,
                        750,
                    )
                    .await?;
                let packet = self
                    .store
                    .context(&project, &session, &turn, results.memories, budget)
                    .await?;
                let mut value = serde_json::to_value(packet)?;
                value["mode"] = json!(results.mode);
                Ok(value)
            }
            MemoryRequest::Source {
                project,
                receipt_id,
                message_id,
                offset,
            } => {
                if let Some(message_id) = message_id {
                    ProjectTools::new(self.store.clone(),project,None)?.execute("source_read",json!({"receipt_id":receipt_id,"message_id":message_id,"offset":offset})).await
                } else {
                    let source = self.store.source(&project, &receipt_id).await?;
                    ensure!(
                        offset <= source.messages.len(),
                        "invalid source message offset"
                    );
                    Ok(
                        json!({"receipt_id":receipt_id,"revision":source.revision,"session_id":source.session_id,"turn_id":source.turn_id,"messages":source.messages.iter().skip(offset).take(50).map(|m|json!({"id":m.id,"role":m.role,"bytes":m.text.len()})).collect::<Vec<_>>(),"next_offset":if offset+50<source.messages.len(){Some(offset+50)}else{None}}),
                    )
                }
            }
            MemoryRequest::Create {
                project,
                content,
                actor,
            } => Ok(serde_json::to_value(
                self.store.create(&project, content, actor).await?,
            )?),
            MemoryRequest::Update {
                project,
                id,
                revision,
                content,
                actor,
            } => Ok(serde_json::to_value(
                self.store
                    .update(&project, &id, revision, content, actor)
                    .await?,
            )?),
            MemoryRequest::SetStatus {
                project,
                id,
                revision,
                status,
                reason,
                actor,
            } => Ok(serde_json::to_value(
                self.store
                    .set_status(&project, &id, revision, status, &reason, actor)
                    .await?,
            )?),
            MemoryRequest::Reindex {
                project,
                after,
                limit,
            } => Ok(serde_json::to_value(
                self.store.reindex_fts(&project, &after, limit).await?,
            )?),
            MemoryRequest::Work { project, id } => {
                let work = self.store.work_details(id).await?;
                ensure!(work["project_id"] == project, "not_found: memory work");
                Ok(work)
            }
            MemoryRequest::Compact {
                project,
                after,
                limit,
            } => Ok(serde_json::to_value(
                self.store
                    .schedule_compaction(
                        &project,
                        &after,
                        limit,
                        true,
                        0,
                        time::OffsetDateTime::now_utc().unix_timestamp(),
                    )
                    .await?,
            )?),
            MemoryRequest::Retry { project, id } => {
                self.store.retry_work(&project, id).await?;
                self.store.work_details(id).await
            }
            MemoryRequest::Receipt {
                project,
                receipt_id,
            } => self.store.receipt_status(&project, &receipt_id).await,
            MemoryRequest::Ask { .. } => {
                bail!("deep memory queries require a configured Agent model")
            }
        }
    }
}
#[async_trait]
impl RequestHandler for MemoryApi {
    async fn handle(&self, request: Value) -> Result<Value> {
        let request: MemoryRequest = serde_json::from_value(request)?;
        if let MemoryRequest::Ask { project, query } = request {
            return Ok(serde_json::to_value(
                self.deep
                    .as_ref()
                    .context("deep memory queries require a configured Agent model")?
                    .ask(&project, &query)
                    .await?,
            )?);
        }
        let _permit = self
            .queries
            .try_acquire()
            .context("busy: memory query limit reached")?;
        self.execute(request).await
    }
}

impl MemoryStore {
    pub async fn memory_status(&self, project: Option<&str>) -> Result<Value> {
        let memories: i64 =
            sqlx::query_scalar("SELECT count(*) FROM memories WHERE ? IS NULL OR project_id=?")
                .bind(project)
                .bind(project)
                .fetch_one(&self.pool)
                .await?;
        let sources: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sources WHERE ? IS NULL OR project_id=?")
                .bind(project)
                .bind(project)
                .fetch_one(&self.pool)
                .await?;
        let work: Vec<(String, i64)> = sqlx::query_as(
            "SELECT state,count(*) FROM work_items WHERE ? IS NULL OR project_id=? GROUP BY state",
        )
        .bind(project)
        .bind(project)
        .fetch_all(&self.pool)
        .await?;
        let work: serde_json::Map<String, Value> = work
            .into_iter()
            .map(|(key, count)| (key, json!(count)))
            .collect();
        let profile = if let Some(project) = project {
            self.embedding_profile(project).await?
        } else {
            None
        };
        let indexed:i64=sqlx::query_scalar("SELECT count(*) FROM memory_vectors v JOIN embedding_profiles p ON p.project_id=v.project_id AND p.generation=v.generation JOIN memories m ON m.id=v.memory_id AND m.revision=v.revision WHERE (? IS NULL OR v.project_id=?) AND m.status='active' AND (m.valid_until IS NULL OR m.valid_until>unixepoch())").bind(project).bind(project).fetch_one(&self.pool).await?;
        // Offline readers may open a v1 snapshot before the daemon has installed
        // the additive retry-state table. Reads must not migrate that snapshot.
        let has_failures: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='embedding_failures')",
        )
        .fetch_one(&self.pool)
        .await?;
        let failures: Vec<String> = if has_failures {
            sqlx::query_scalar("SELECT json_object('memory_id',f.memory_id,'revision',f.revision,'generation',f.generation,'attempts',f.attempts,'available_at',f.available_at,'error',f.error) FROM embedding_failures f JOIN memories m ON m.id=f.memory_id AND m.revision=f.revision JOIN embedding_profiles p ON p.project_id=m.project_id AND p.generation=f.generation WHERE (? IS NULL OR m.project_id=?) AND m.status='active' AND (m.valid_until IS NULL OR m.valid_until>unixepoch()) ORDER BY f.memory_id LIMIT 20")
            .bind(project).bind(project).fetch_all(&self.pool).await?
        } else {
            Vec::new()
        };
        let failures: Vec<Value> = failures
            .iter()
            .map(|data| serde_json::from_str(data))
            .collect::<Result<_, _>>()?;
        Ok(
            json!({"project":project,"memories":memories,"sources":sources,"work":work,"embedding_profile":profile,"indexed":indexed,"embedding_failures":failures}),
        )
    }
    pub async fn receipt_status(&self, project: &str, receipt: &str) -> Result<Value> {
        self.source(project, receipt).await?;
        let states:Vec<(String,i64)>=sqlx::query_as("SELECT state,count(*) FROM work_items WHERE project_id=? AND receipt_id=? GROUP BY state").bind(project).bind(receipt).fetch_all(&self.pool).await?;
        let complete = states.iter().all(|(state, _)| state == "done");
        Ok(
            json!({"receipt_id":receipt,"complete":complete,"states":states.into_iter().collect::<std::collections::BTreeMap<_,_>>()}),
        )
    }
    pub async fn retry_work(&self, project: &str, id: i64) -> Result<()> {
        let result=sqlx::query("UPDATE work_items SET state='pending',attempts=0,max_attempts=NULL,available_at=0,error=NULL WHERE project_id=? AND id=? AND state='failed' AND (json_type(payload,'$.review')='object' OR receipt_id IN (SELECT receipt_id FROM source_heads WHERE project_id=?))")
            .bind(project).bind(id).bind(project).execute(&self.pool).await?;
        ensure!(
            result.rows_affected() == 1,
            "conflict: only failed work with a current source can retry"
        );
        self.notify_work();
        Ok(())
    }
}
