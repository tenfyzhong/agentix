//! Configuration-independent migration of persisted document locations.
use std::{
    collections::BTreeMap,
    path::{Component, Path},
};

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sqlx::{Row, SqliteConnection};

use crate::Project;

pub(crate) async fn migrate(conn: &mut SqliteConnection) -> Result<()> {
    let projects: Vec<String> = sqlx::query_scalar("SELECT data FROM projects")
        .fetch_all(&mut *conn)
        .await?;
    let mut prefixes = BTreeMap::new();
    for data in projects {
        let mut project: Project = serde_json::from_str(&data)?;
        let old = project.document_directory();
        validate_source(&old)?;
        if !is_absolute_document_path(&old) {
            continue;
        }
        ensure!(
            project.archived_at.is_some(),
            "invalid: absolute active Project document directory"
        );
        let new = format!("Archived Projects/{}", project.key);
        validate(&new)?;
        prefixes.insert(old, new.clone());
        project.document_directory = Some(new);
        sqlx::query("UPDATE projects SET data=? WHERE id=?")
            .bind(serde_json::to_string(&project)?)
            .bind(&project.id)
            .execute(&mut *conn)
            .await?;
    }
    for (table, field) in [("jobs", "document_path"), ("plans", "path")] {
        let rows = sqlx::query(&format!("SELECT id,data FROM {table}"))
            .fetch_all(&mut *conn)
            .await?;
        for row in rows {
            let original: String = row.get("data");
            let mut value: Value = serde_json::from_str(&original)?;
            value[field] = Value::String(relative(
                value[field].as_str().context("missing document path")?,
                &prefixes,
            )?);
            let data = value.to_string();
            if data != original {
                sqlx::query(&format!("UPDATE {table} SET data=? WHERE id=?"))
                    .bind(data)
                    .bind(row.get::<String, _>("id"))
                    .execute(&mut *conn)
                    .await?;
            }
        }
    }
    let rows = sqlx::query("SELECT key,path FROM document_registry")
        .fetch_all(&mut *conn)
        .await?;
    for row in rows {
        let old: String = row.get("path");
        let new = relative(&old, &prefixes)?;
        if old != new {
            sqlx::query("UPDATE document_registry SET path=? WHERE key=?")
                .bind(new)
                .bind(row.get::<String, _>("key"))
                .execute(&mut *conn)
                .await?;
        }
    }
    let rows = sqlx::query("SELECT id,data FROM document_deletions")
        .fetch_all(&mut *conn)
        .await?;
    for row in rows {
        let mut cleanup: crate::deletion::Cleanup =
            serde_json::from_str(&row.get::<String, _>("data"))?;
        cleanup.files = cleanup
            .files
            .iter()
            .map(|p| relative(p, &prefixes))
            .collect::<Result<_>>()?;
        cleanup.directories = cleanup
            .directories
            .iter()
            .map(|p| relative_directory(p, &prefixes))
            .collect::<Result<_>>()?;
        cleanup.candidates = cleanup
            .candidates
            .into_iter()
            .map(|(p, ids)| Ok((relative(&p, &prefixes)?, ids)))
            .collect::<Result<_>>()?;
        sqlx::query("UPDATE document_deletions SET data=? WHERE id=?")
            .bind(serde_json::to_string(&cleanup)?)
            .bind(row.get::<String, _>("id"))
            .execute(&mut *conn)
            .await?;
    }
    migrate_generated_records(conn, &prefixes).await
}

async fn migrate_generated_records(
    conn: &mut SqliteConnection,
    prefixes: &BTreeMap<String, String>,
) -> Result<()> {
    // Replayed results and generated event fields must use the same path contract.
    for (table, id, field) in [
        ("idempotency_keys", "key", "result"),
        ("task_events", "sequence", "data"),
    ] {
        let rows = sqlx::query(&format!("SELECT CAST({id} AS TEXT) AS id,{field} AS data FROM {table} WHERE instr({field},'\"path\":')>0 OR instr({field},'\"document_path\":')>0 OR instr({field},'\"document_directory\":')>0"))
            .fetch_all(&mut *conn).await?;
        for row in rows {
            let mut value: Value = serde_json::from_str(&row.get::<String, _>("data"))?;
            normalize_generated_fields(&mut value, prefixes)?;
            sqlx::query(&format!("UPDATE {table} SET {field}=? WHERE {id}=?"))
                .bind(value.to_string())
                .bind(row.get::<String, _>("id"))
                .execute(&mut *conn)
                .await?;
        }
    }
    Ok(())
}

fn normalize_generated_fields(
    value: &mut Value,
    prefixes: &BTreeMap<String, String>,
) -> Result<()> {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(
                    key.as_str(),
                    "path" | "document_path" | "document_directory"
                ) {
                    if let Some(path) = value.as_str() {
                        *value = Value::String(if key == "document_directory" {
                            relative_directory(path, prefixes)?
                        } else {
                            relative(path, prefixes)?
                        });
                    }
                } else if !matches!(key.as_str(), "conversation" | "messages") {
                    normalize_generated_fields(value, prefixes)?;
                }
            }
        }
        Value::Array(array) => {
            for value in array {
                normalize_generated_fields(value, prefixes)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn relative_directory(path: &str, prefixes: &BTreeMap<String, String>) -> Result<String> {
    validate_source(path)?;
    if !is_absolute_document_path(path) {
        return relative(path, prefixes);
    }
    if let Some(mapped) = prefixes.get(path) {
        return Ok(mapped.clone());
    }
    let name = Path::new(path)
        .file_name()
        .and_then(|p| p.to_str())
        .context("invalid archived directory")?;
    let new = format!("Archived Projects/{name}");
    validate(&new)?;
    Ok(new)
}

fn relative(path: &str, prefixes: &BTreeMap<String, String>) -> Result<String> {
    validate_source(path)?;
    if !is_absolute_document_path(path) {
        validate(path)?;
        return Ok(path.to_owned());
    }
    for (old, new) in prefixes {
        if path == old {
            return Ok(new.clone());
        }
        if let Some(suffix) = path.strip_prefix(&format!("{old}/")) {
            let new = format!("{new}/{suffix}");
            validate(&new)?;
            return Ok(new);
        }
    }
    // Deleted Projects have no entity left; only their managed cleanup paths remain.
    let path = Path::new(path);
    let directory = path
        .ancestors()
        .skip(1)
        .find(|p| {
            p.file_name()
                .is_some_and(|name| name == "Jobs" || name == "Tasks")
        })
        .and_then(Path::parent)
        .or_else(|| path.parent())
        .context("invalid archived document path")?;
    let name = directory
        .file_name()
        .and_then(|p| p.to_str())
        .context("invalid archived Project name")?;
    let new = format!(
        "Archived Projects/{name}/{}",
        path.strip_prefix(directory)?.to_string_lossy()
    );
    validate(&new)?;
    Ok(new)
}

fn validate(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty()
            && !is_absolute_document_path(path)
            && Path::new(path)
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "invalid relative document path"
    );
    Ok(())
}

fn is_absolute_document_path(path: &str) -> bool {
    Path::new(path).is_absolute()
        || path.starts_with('/')
        || path.starts_with('\\')
        || (path.as_bytes().get(1) == Some(&b':')
            && path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic))
}

fn validate_source(path: &str) -> Result<()> {
    ensure!(
        !Path::new(path)
            .components()
            .any(|c| c == Component::ParentDir),
        "invalid document path traversal"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_archive_directory_fields_use_the_project_name() {
        let mut value = serde_json::json!({"payload":{"document_directory":"/old-vault/History/Projects/demo","root":"/workspace/demo"}});
        normalize_generated_fields(&mut value, &BTreeMap::new()).unwrap();
        assert_eq!(
            value["payload"]["document_directory"],
            "Archived Projects/demo"
        );
        assert_eq!(value["payload"]["root"], "/workspace/demo");
    }

    #[test]
    fn migration_rejects_traversal_in_old_absolute_paths() {
        assert!(relative("/vault/Archive/../demo/Jobs/job.md", &BTreeMap::new()).is_err());
        assert!(relative_directory("/vault/Archive/../demo", &BTreeMap::new()).is_err());
    }
}
