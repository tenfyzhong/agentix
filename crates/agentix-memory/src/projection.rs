//! Read-only notes are regenerated from `SQLite`; publication retains durable receipts.
use crate::{Memory, MemoryStore, retrieval::digest};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::Row;
use std::path::{Component, Path, PathBuf};
mod document;
mod files;

#[derive(Clone)]
pub struct MemoryProjection {
    store: MemoryStore,
    root: PathBuf,
    directory: PathBuf,
}
#[derive(Default, Serialize)]
pub struct ProjectionPage {
    pub published: usize,
    pub imported: usize,
    pub conflicts: Vec<Value>,
    pub next_cursor: String,
    pub complete: bool,
}
impl MemoryProjection {
    pub fn new(store: MemoryStore, root: &Path, directory: &Path) -> Result<Self> {
        ensure!(
            root.is_absolute()
                && !directory.as_os_str().is_empty()
                && directory
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
            "invalid projection directory"
        );
        Ok(Self {
            store,
            root: root.to_owned(),
            directory: directory.to_owned(),
        })
    }
    pub fn path(&self, id: &str) -> Result<PathBuf> {
        ensure!(
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            "invalid memory document ID"
        );
        Ok(self.root.join(&self.directory).join(format!("{id}.md")))
    }
    /// One bounded page, repairing files that differ from the authoritative database.
    /// The service serializes publishers; database writers remain independent.
    pub async fn sync(&self, project: &str, after: &str, limit: i64) -> Result<ProjectionPage> {
        let memories = self.store.list(project, after, limit, true).await?;
        let mut result = ProjectionPage {
            next_cursor: after.into(),
            complete: memories.len() < usize::try_from(limit)?,
            ..ProjectionPage::default()
        };
        for memory in memories {
            result.next_cursor.clone_from(&memory.id);
            match self.sync_one(&memory).await {
                Ok((imported, published)) => {
                    result.imported += usize::from(imported);
                    result.published += usize::from(published);
                }
                Err(error) => {
                    let message: String = error.to_string().chars().take(1024).collect();
                    sqlx::query("UPDATE memory_projection SET error=? WHERE memory_id=?")
                        .bind(&message)
                        .bind(&memory.id)
                        .execute(&self.store.pool)
                        .await?;
                    result
                        .conflicts
                        .push(json!({"id":memory.id,"error":message}));
                }
            }
        }
        Ok(result)
    }
    async fn sync_one(&self, initial: &Memory) -> Result<(bool, bool)> {
        let path = self.path(&initial.id)?;
        files::prepare_directory(&self.root, &self.directory)?;
        let text = files::read(&path)?;
        let current = self
            .store
            .show(&initial.project_id, &initial.id, None)
            .await?;
        let output = document::render(&current)?;
        let output_hash = digest(&output);
        if text.as_deref() == Some(&output) {
            self.acknowledge(&current.id, current.revision, &output_hash)
                .await?;
            return Ok((false, false));
        }
        sqlx::query(
            "UPDATE memory_projection SET prepared_revision=?,prepared_hash=? WHERE memory_id=?",
        )
        .bind(current.revision)
        .bind(&output_hash)
        .bind(&current.id)
        .execute(&self.store.pool)
        .await?;
        files::publish(&path, text.as_deref(), &output)?;
        self.acknowledge(&current.id, current.revision, &output_hash)
            .await?;
        Ok((false, true))
    }
    async fn acknowledge(&self, id: &str, revision: i64, hash: &str) -> Result<()> {
        sqlx::query("UPDATE memory_projection SET published_revision=?,published_hash=?,imported_hash='',prepared_revision=0,prepared_hash='',error=NULL WHERE memory_id=? AND published_revision<=?")
            .bind(revision).bind(hash).bind(id).bind(revision).execute(&self.store.pool).await?;
        Ok(())
    }
}
impl MemoryStore {
    /// Read the authoritative document without requiring a running service.
    pub async fn rendered_note(&self, id: &str) -> Result<(Memory, String)> {
        let data: String = sqlx::query_scalar("SELECT data FROM memories WHERE id=?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .context("not_found: memory")?;
        let memory: Memory = serde_json::from_str(&data)?;
        let text = document::render(&memory)?;
        Ok((memory, text))
    }

    pub async fn projection_status(&self, project: &str) -> Result<Value> {
        let row = sqlx::query("SELECT count(*) total,coalesce(sum(p.published_revision<m.revision OR p.error IS NOT NULL),0) pending FROM memories m JOIN memory_projection p ON p.memory_id=m.id WHERE m.project_id=?")
            .bind(project).fetch_one(&self.pool).await?;
        let errors = sqlx::query("SELECT m.id,p.error FROM memories m JOIN memory_projection p ON p.memory_id=m.id WHERE m.project_id=? AND p.error IS NOT NULL ORDER BY m.id LIMIT 20")
            .bind(project).fetch_all(&self.pool).await?;
        Ok(
            json!({"total":row.get::<i64,_>("total"),"pending":row.get::<i64,_>("pending"),"errors":errors.iter().map(|r|json!({"id":r.get::<String,_>("id"),"error":r.get::<String,_>("error")})).collect::<Vec<_>>()}),
        )
    }
}
