//! Fact identity, immutable values and one active version per Project.
use crate::{Actor, Memory, MemoryInput, MemoryStore, Source, Status};
use anyhow::{Context, Result, ensure};
use sqlx::SqliteConnection;

pub(crate) struct ActiveFactMatch {
    pub id: String,
    pub revision: i64,
    pub actor: String,
    pub same_value: bool,
}

impl MemoryStore {
    /// Only correction metadata for one indexed active identity, without quotation bodies.
    pub(crate) async fn active_fact_match(
        &self,
        project: &str,
        content: &MemoryInput,
    ) -> Result<Option<ActiveFactMatch>> {
        let fact = content.fact.as_ref().context("fact identity required")?;
        let row: Option<(String, i64, String, String)> = sqlx::query_as("SELECT m.id,m.revision,json_extract(m.data,'$.actor'),json_extract(m.data,'$.content.fact.value') FROM memory_facts f JOIN memories m ON m.id=f.memory_id WHERE f.project_id=? AND f.fact_key=? AND f.status='active' LIMIT 1")
            .bind(project).bind(fact.key()?).fetch_optional(&self.pool).await?;
        Ok(row.map(|(id, revision, actor, value)| ActiveFactMatch {
            id,
            revision,
            actor,
            same_value: value == fact.value,
        }))
    }

    /// Conflict diagnostics are separate from ordinary effective-memory retrieval.
    pub async fn conflicts(&self, project: &str, after: &str, limit: i64) -> Result<Vec<Memory>> {
        ensure!((1..=100).contains(&limit), "invalid: conflict page");
        let rows: Vec<String> = sqlx::query_scalar("SELECT data FROM memories WHERE project_id=? AND status='conflicted' AND id>? ORDER BY id LIMIT ?")
            .bind(project).bind(after).bind(limit).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|data| Ok(serde_json::from_str(&data)?))
            .collect()
    }

    /// One-to-many lineage for a legacy record split into facts.
    pub async fn fact_derivatives(&self, project: &str, source_id: &str) -> Result<Vec<Memory>> {
        let rows: Vec<String> = sqlx::query_scalar("SELECT DISTINCT m.data FROM memory_fact_origins o JOIN memories m ON m.id=o.memory_id WHERE m.project_id=? AND o.source_memory_id=? ORDER BY m.id LIMIT 100")
            .bind(project).bind(source_id).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|data| Ok(serde_json::from_str(&data)?))
            .collect()
    }

    /// Exact indexed lookup includes expired and conflicted versions for reconciliation.
    pub async fn fact_versions(&self, project: &str, content: &MemoryInput) -> Result<Vec<Memory>> {
        let fact = content.fact.as_ref().context("fact identity required")?;
        let rows: Vec<String> = sqlx::query_scalar("SELECT m.data FROM memory_facts f JOIN memories m ON m.id=f.memory_id WHERE f.project_id=? AND f.fact_key=? AND f.status IN ('active','conflicted') ORDER BY m.id LIMIT 16")
            .bind(project).bind(fact.key()?).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|data| Ok(serde_json::from_str(&data)?))
            .collect()
    }
}

pub(crate) async fn validate_version(conn: &mut SqliteConnection, memory: &Memory) -> Result<()> {
    let prior: Option<String> = sqlx::query_scalar("SELECT data FROM memories WHERE id=?")
        .bind(&memory.id)
        .fetch_optional(&mut *conn)
        .await?;
    if let Some(prior) = prior {
        let prior: Memory = serde_json::from_str(&prior)?;
        if let Some(fact) = &prior.content.fact {
            ensure!(
                memory
                    .content
                    .fact
                    .as_ref()
                    .is_some_and(|current| current.key().ok() == fact.key().ok()
                        && current.value == fact.value),
                "fact identity and value are immutable; supersede with a new version instead"
            );
        }
    }
    Ok(())
}

pub(crate) async fn index(conn: &mut SqliteConnection, memory: &Memory) -> Result<()> {
    if let Some(fact) = &memory.content.fact {
        let key = fact.key()?;
        if memory.status == Status::Active {
            let occupied: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM memory_facts WHERE project_id=? AND fact_key=? AND status='active' AND memory_id<>?)")
                .bind(&memory.project_id).bind(&key).bind(&memory.id).fetch_one(&mut *conn).await?;
            ensure!(
                !occupied,
                "conflict: fact already has an active version; reconcile before writing"
            );
        }
        let status = serde_json::to_value(memory.status)?;
        sqlx::query("INSERT INTO memory_facts(memory_id,project_id,fact_key,status) VALUES(?,?,?,?) ON CONFLICT(memory_id) DO UPDATE SET status=excluded.status")
            .bind(&memory.id).bind(&memory.project_id).bind(key).bind(status.as_str()).execute(&mut *conn).await?;
    }
    Ok(())
}

pub(crate) fn same_identity(a: &MemoryInput, b: &MemoryInput) -> Result<bool> {
    match (&a.fact, &b.fact) {
        (Some(a), Some(b)) => Ok(a.key()? == b.key()?),
        (None, None) => Ok(true),
        _ => Ok(false),
    }
}

pub(crate) async fn source_order(
    conn: &mut SqliteConnection,
    project: &str,
    content: &MemoryInput,
) -> Result<(i64, i64)> {
    let mut order = (0, 0);
    for quote in &content.evidence {
        let data: String =
            sqlx::query_scalar("SELECT data FROM sources WHERE receipt_id=? AND project_id=?")
                .bind(&quote.receipt_id)
                .bind(project)
                .fetch_one(&mut *conn)
                .await?;
        let source: Source = serde_json::from_str(&data)?;
        let message = source
            .messages
            .iter()
            .find(|m| m.id == quote.message_id)
            .context("evidence message missing")?;
        order = order.max((
            message.recorded_at.unwrap_or(source.recorded_at),
            source.sequence,
        ));
    }
    Ok(order)
}

pub(crate) async fn validate_replacement(
    conn: &mut SqliteConnection,
    prior: &Memory,
    content: &MemoryInput,
) -> Result<()> {
    if prior.content.fact.is_some() || content.fact.is_some() {
        ensure!(
            same_identity(&prior.content, content)?,
            "replacement cannot combine different facts"
        );
        if prior.actor == Actor::Agent
            && prior.content.fact.as_ref().map(|f| &f.value)
                != content.fact.as_ref().map(|f| &f.value)
        {
            ensure!(
                source_order(conn, &prior.project_id, content).await?
                    >= source_order(conn, &prior.project_id, &prior.content).await?,
                "stale fact evidence cannot replace newer knowledge"
            );
        }
    }
    Ok(())
}

/// No confirmed active fact can coexist with unresolved claims of that identity.
pub(crate) async fn check_settled(conn: &mut SqliteConnection, changed: &[Memory]) -> Result<()> {
    for memory in changed.iter().filter(|m| m.status == Status::Active) {
        if let Some(fact) = &memory.content.fact {
            let conflicted: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM memory_facts WHERE project_id=? AND fact_key=? AND status='conflicted')")
                .bind(&memory.project_id).bind(fact.key()?).fetch_one(&mut *conn).await?;
            ensure!(
                !conflicted,
                "unresolved fact conflicts must be reconciled before creating an active version"
            );
        }
    }
    Ok(())
}

pub(crate) fn same_value(a: &MemoryInput, b: &MemoryInput) -> Result<bool> {
    Ok(same_identity(a, b)?
        && a.fact.as_ref().map(|f| &f.value) == b.fact.as_ref().map(|f| &f.value))
}
