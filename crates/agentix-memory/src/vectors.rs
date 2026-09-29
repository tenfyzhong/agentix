use std::collections::HashMap;

use anyhow::{Result, ensure};
use sqlx::Row;

use crate::{Memory, MemoryStore};

const VECTOR_PAGE_SQL: &str = "SELECT v.memory_id,v.vector FROM memory_vectors v JOIN memories m ON m.id=v.memory_id JOIN embedding_profiles p ON p.project_id=v.project_id AND p.generation=v.generation WHERE v.project_id=? AND v.generation=? AND v.memory_id>? AND v.revision=m.revision AND m.status IN ('active','conflicted') AND (m.valid_until IS NULL OR m.valid_until>unixepoch()) ORDER BY v.memory_id LIMIT 256";

fn normalize(vector: &[f32]) -> Result<Vec<f64>> {
    ensure!(
        !vector.is_empty() && vector.len() <= 16384 && vector.iter().all(|v| v.is_finite()),
        "invalid: embedding values"
    );
    let norm = vector
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>()
        .sqrt();
    ensure!(norm > 0.0 && norm.is_finite(), "invalid: zero embedding");
    Ok(vector.iter().map(|v| f64::from(*v) / norm).collect())
}

impl MemoryStore {
    /// A configuration change starts a new generation, even when returning to an old profile.
    pub async fn configure_embedding(
        &self,
        project: &str,
        fingerprint: &str,
        dimensions: usize,
    ) -> Result<i64> {
        ensure!(
            (1..=16384).contains(&dimensions),
            "invalid: embedding dimensions"
        );
        self.reserve_embedding(project, fingerprint, Some(dimensions))
            .await
    }

    /// Reserve the generation before any remote call; unknown dimensions are resolved later.
    pub async fn reserve_embedding(
        &self,
        project: &str,
        fingerprint: &str,
        dimensions: Option<usize>,
    ) -> Result<i64> {
        ensure!(
            !project.is_empty()
                && !fingerprint.is_empty()
                && fingerprint.len() <= 4096
                && dimensions.is_none_or(|d| (1..=16384).contains(&d)),
            "invalid: embedding profile"
        );
        let dimensions = dimensions.map(i64::try_from).transpose()?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let prior = sqlx::query(
            "SELECT generation,fingerprint,dimensions FROM embedding_profiles WHERE project_id=?",
        )
        .bind(project)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(row) = &prior
            && row.get::<String, _>("fingerprint") == fingerprint
            && dimensions.is_none_or(|d| row.get::<i64, _>("dimensions") == d)
        {
            return Ok(row.get("generation"));
        }
        let generation = prior.map_or(1, |r| r.get::<i64, _>("generation") + 1);
        sqlx::query("INSERT INTO embedding_profiles(project_id,generation,fingerprint,dimensions) VALUES (?,?,?,?) ON CONFLICT(project_id) DO UPDATE SET generation=excluded.generation,fingerprint=excluded.fingerprint,dimensions=excluded.dimensions")
            .bind(project).bind(generation).bind(fingerprint).bind(dimensions.unwrap_or(0)).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(generation)
    }

    pub async fn embedding_pending(
        &self,
        project: &str,
        generation: i64,
        after: &str,
        limit: i64,
    ) -> Result<Vec<Memory>> {
        ensure!((1..=100).contains(&limit), "invalid: embedding page size");
        let rows:Vec<String>=sqlx::query_scalar("SELECT m.data FROM memories m JOIN embedding_profiles p ON p.project_id=m.project_id WHERE m.project_id=? AND p.generation=? AND m.id>? AND m.status IN ('active','conflicted') AND (m.valid_until IS NULL OR m.valid_until>unixepoch()) AND NOT EXISTS(SELECT 1 FROM memory_vectors v WHERE v.memory_id=m.id AND v.generation=p.generation AND v.revision=m.revision) ORDER BY m.id LIMIT ?")
            .bind(project).bind(generation).bind(after).bind(limit).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|v| Ok(serde_json::from_str(&v)?))
            .collect()
    }

    pub async fn put_embedding(
        &self,
        project: &str,
        id: &str,
        revision: i64,
        generation: i64,
        vector: &[f32],
    ) -> Result<bool> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let expected:Option<i64>=sqlx::query_scalar("SELECT p.dimensions FROM embedding_profiles p JOIN memories m ON m.project_id=p.project_id WHERE p.project_id=? AND p.generation=? AND m.id=? AND m.revision=? AND m.status IN ('active','conflicted') AND (m.valid_until IS NULL OR m.valid_until>unixepoch())")
            .bind(project).bind(generation).bind(id).bind(revision).fetch_optional(&mut *tx).await?;
        let Some(expected) = expected else {
            return Ok(false);
        };
        ensure!(
            i64::try_from(vector.len())? == expected,
            "invalid: embedding dimension changed"
        );
        let normalized = normalize(vector)?;
        let bytes: Vec<u8> = normalized.iter().flat_map(|v| v.to_le_bytes()).collect();
        sqlx::query("INSERT INTO memory_vectors(memory_id,project_id,generation,revision,vector) VALUES (?,?,?,?,?) ON CONFLICT(memory_id,generation) DO UPDATE SET revision=excluded.revision,vector=excluded.vector")
            .bind(id).bind(project).bind(generation).bind(revision).bind(bytes).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Independent lexical and semantic recall followed by reciprocal rank fusion.
    pub async fn hybrid_search(
        &self,
        project: &str,
        query: &str,
        generation: i64,
        vector: &[f32],
        limit: i64,
    ) -> Result<Vec<Memory>> {
        ensure!((1..=100).contains(&limit), "invalid: memory search limit");
        let candidates = (limit * 4).min(100);
        let lexical = self.search(project, query, candidates).await?;
        let semantic = self
            .vector_candidates(project, generation, vector, usize::try_from(candidates)?)
            .await?;
        let mut scores = HashMap::<String, f64>::new();
        for ranking in [
            lexical.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
            semantic,
        ] {
            for (rank, id) in ranking.into_iter().enumerate() {
                *scores.entry(id).or_default() +=
                    1.0 / (60.0 + f64::from(u32::try_from(rank)?) + 1.0);
            }
        }
        let ids: Vec<_> = scores.keys().collect();
        // Recheck visibility after recall; an intervening forget must not leak an old result.
        let rows:Vec<String>=sqlx::query_scalar("SELECT data FROM memories WHERE project_id=? AND status IN ('active','conflicted') AND (valid_until IS NULL OR valid_until>unixepoch()) AND id IN (SELECT value FROM json_each(?))")
            .bind(project).bind(serde_json::to_string(&ids)?).fetch_all(&self.pool).await?;
        let mut results: Vec<Memory> = rows
            .into_iter()
            .map(|s| serde_json::from_str(&s))
            .collect::<Result<_, _>>()?;
        results.sort_by(|a, b| {
            scores[&b.id]
                .total_cmp(&scores[&a.id])
                .then_with(|| a.id.cmp(&b.id))
        });
        results.truncate(usize::try_from(limit)?);
        Ok(results)
    }

    async fn vector_candidates(
        &self,
        project: &str,
        generation: i64,
        vector: &[f32],
        limit: usize,
    ) -> Result<Vec<String>> {
        let dimensions: Option<i64> = sqlx::query_scalar(
            "SELECT dimensions FROM embedding_profiles WHERE project_id=? AND generation=?",
        )
        .bind(project)
        .bind(generation)
        .fetch_optional(&self.pool)
        .await?;
        let Some(dimensions) = dimensions else {
            return Ok(Vec::new());
        };
        ensure!(
            i64::try_from(vector.len())? == dimensions,
            "invalid: query embedding dimension"
        );
        let vector = normalize(vector)?;
        let mut after = String::new();
        let mut top = Vec::<(String, f64)>::new();
        loop {
            // Short pages release the connection, including while the worker pool is active.
            let rows = sqlx::query(VECTOR_PAGE_SQL)
                .bind(project)
                .bind(generation)
                .bind(&after)
                .fetch_all(&self.pool)
                .await?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                let id: String = row.get("memory_id");
                after.clone_from(&id);
                let bytes: Vec<u8> = row.get("vector");
                ensure!(
                    bytes.len() == vector.len() * 8,
                    "invalid: stored embedding dimension"
                );
                let mut similarity = 0.0;
                for (value, chunk) in vector.iter().zip(bytes.as_chunks::<8>().0) {
                    let element = f64::from_le_bytes(*chunk);
                    ensure!(element.is_finite(), "invalid: stored embedding value");
                    similarity += value * element;
                }
                if similarity <= 0.0 {
                    continue;
                }
                top.push((id, similarity));
                top.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                top.truncate(limit);
            }
        }
        Ok(top.into_iter().map(|(id, _)| id).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn vector_page_uses_project_generation_range_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&dir.path().join("memory.db"))
            .await
            .unwrap();
        let plan = sqlx::query(&format!("EXPLAIN QUERY PLAN {VECTOR_PAGE_SQL}"))
            .bind("project")
            .bind(1_i64)
            .bind("")
            .fetch_all(&store.pool)
            .await
            .unwrap();
        let details = plan
            .iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(details.contains("vectors_by_project"), "{details}");
        assert!(!details.contains("SCAN v"), "{details}");
    }
}
