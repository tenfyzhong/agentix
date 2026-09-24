//! Transactionally maintained directory and Unicode name indexes.
use anyhow::{Context, Result};
use sqlx::{Row, SqliteConnection};
use std::path::Path;

pub(crate) const BY_ROOT: &str = "SELECT p.id FROM project_lookup l JOIN projects p ON p.id=l.project_id WHERE l.canonical_root=? ORDER BY p.rowid LIMIT 1";

pub(crate) fn canonical_root(root: &str) -> String {
    Path::new(root)
        .canonicalize()
        .map_or_else(|_| root.to_owned(), |p| p.to_string_lossy().into_owned())
}

pub(crate) async fn upsert(
    conn: &mut SqliteConnection,
    id: &str,
    root: &str,
    key: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO project_lookup(project_id,canonical_root,folded_key) VALUES (?,?,?) ON CONFLICT(project_id) DO UPDATE SET canonical_root=excluded.canonical_root,folded_key=excluded.folded_key")
        .bind(id).bind(canonical_root(root)).bind(key.to_lowercase()).execute(conn).await?;
    Ok(())
}

pub(crate) async fn migrate(conn: &mut SqliteConnection) -> Result<()> {
    let rows = sqlx::query(
        "SELECT id,root,json_extract(data,'$.key') AS key FROM projects ORDER BY rowid",
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in rows {
        let id: String = row.try_get("id")?;
        let root: Option<String> = row.try_get("root")?;
        let key: Option<String> = row.try_get("key")?;
        upsert(
            conn,
            &id,
            &root.context("Project root is missing during lookup migration")?,
            &key.context("Project key is missing during lookup migration")?,
        )
        .await?;
    }
    Ok(())
}

pub(crate) async fn available_key(conn: &mut SqliteConnection, name: &str) -> Result<String> {
    let base = crate::naming::short_name(name);
    let mut key = base.clone();
    let mut suffix = 2;
    while sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM project_lookup WHERE folded_key=?)",
    )
    .bind(key.to_lowercase())
    .fetch_one(&mut *conn)
    .await?
    {
        key = format!("{base}-{suffix}");
        suffix += 1;
    }
    Ok(key)
}
