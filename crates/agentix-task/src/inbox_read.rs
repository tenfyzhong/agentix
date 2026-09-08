//! Scoped Inbox reads for IM browsing and source reconciliation.
use anyhow::Result;
use sqlx::Row;

use crate::{InboxEntry, Store};

/// Metadata needed to refresh a published Inbox submission's source message.
#[derive(Debug, PartialEq, Eq)]
pub struct InboxSource {
    pub id: String,
    pub source: String,
    pub source_version: i64,
}

const SOURCE_LOOKUP: &str =
    "SELECT id FROM inbox_entries WHERE json_extract(data,'$.source')=? ORDER BY rowid LIMIT 1";

impl Store {
    /// Read an exact Inbox ID, including deleted or unpublished entries.
    pub async fn inbox_record(&self, id: &str) -> Result<Option<InboxEntry>> {
        let data: Option<String> = sqlx::query_scalar("SELECT data FROM inbox_entries WHERE id=?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        data.map(|data| serde_json::from_str(&data).map_err(Into::into))
            .transpose()
    }

    /// Locate the first source match without loading its content or lease.
    pub async fn inbox_id_by_source(&self, source: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(SOURCE_LOOKUP)
            .bind(source)
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Refreshable sources in insertion order, without authored content.
    pub async fn inbox_sources(&self) -> Result<Vec<InboxSource>> {
        let rows = sqlx::query(
            "SELECT e.id, json_extract(e.data,'$.source') AS source,
                COALESCE(json_extract(e.data,'$.source_version'),0) AS version
             FROM inbox_entries e LEFT JOIN projects p ON p.id=e.project_id
             WHERE json_extract(e.data,'$.published')=1
               AND json_extract(e.data,'$.deleted') IS NOT 1
               AND json_extract(e.data,'$.source') IS NOT NULL
               AND json_extract(p.data,'$.archived_at') IS NULL
             ORDER BY e.rowid",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| InboxSource {
                id: row.get("id"),
                source: row.get("source"),
                source_version: row.get("version"),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn source_reads_filter_visibility_and_ignore_bodies() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("tasks.sqlite3"))
            .await
            .unwrap();
        for (id, archived) in [("prj_live", None), ("prj_archived", Some(1))] {
            sqlx::query("INSERT INTO projects(id,data) VALUES(?,?)")
                .bind(id)
                .bind(json!({"archived_at":archived}).to_string())
                .execute(&store.pool)
                .await
                .unwrap();
        }
        for (id, project, published, deleted, source) in [
            ("inbox_z", "prj_live", true, false, Some("shared")),
            ("inbox_a", "prj_live", true, false, Some("shared")),
            ("inbox_hidden", "prj_live", false, false, Some("hidden")),
            ("inbox_deleted", "prj_live", true, true, Some("deleted")),
            (
                "inbox_archived",
                "prj_archived",
                true,
                false,
                Some("archived"),
            ),
            ("inbox_local", "prj_live", true, false, None),
        ] {
            let data = json!({"project_id":project,"published":published,"deleted":deleted,
                "source":source,"content":{"invalid":"unused body"},"lease":{"invalid":"unused lease"}});
            sqlx::query("INSERT INTO inbox_entries(id,data) VALUES(?,?)")
                .bind(id)
                .bind(data.to_string())
                .execute(&store.pool)
                .await
                .unwrap();
        }
        assert_eq!(
            store.inbox_sources().await.unwrap(),
            vec![
                InboxSource {
                    id: "inbox_z".into(),
                    source: "shared".into(),
                    source_version: 0
                },
                InboxSource {
                    id: "inbox_a".into(),
                    source: "shared".into(),
                    source_version: 0
                },
            ]
        );
        assert_eq!(
            store.inbox_id_by_source("shared").await.unwrap().as_deref(),
            Some("inbox_z")
        );
        assert_eq!(
            store
                .inbox_id_by_source("deleted")
                .await
                .unwrap()
                .as_deref(),
            Some("inbox_deleted")
        );
        assert!(store.inbox_id_by_source("missing").await.unwrap().is_none());
        assert!(store.inbox_record("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn source_lookup_uses_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("tasks.sqlite3"))
            .await
            .unwrap();
        let rows = sqlx::query(&format!("EXPLAIN QUERY PLAN {SOURCE_LOOKUP}"))
            .bind("source")
            .fetch_all(&store.pool)
            .await
            .unwrap();
        let details: Vec<String> = rows.iter().map(|row| row.get("detail")).collect();
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("SEARCH") && detail.contains("inbox_by_source")),
            "{details:?}"
        );
    }
}
