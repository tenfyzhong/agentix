//! Home abbreviations at the persistence boundary; document paths stay independent.
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sqlx::{Row, SqliteConnection};

pub(crate) fn abbreviate_home(path: &str) -> Result<String> {
    let home = dirs::home_dir().context("home directory unavailable")?;
    Ok(Path::new(path).strip_prefix(home).map_or_else(
        |_| path.to_owned(),
        |relative| {
            if relative.as_os_str().is_empty() {
                "~".to_owned()
            } else {
                Path::new("~")
                    .join(relative)
                    .to_string_lossy()
                    .replace('\\', "/")
            }
        },
    ))
}

pub(crate) fn expand_home(path: &str) -> Result<String> {
    if Path::new(path) == Path::new("~") {
        return Ok(dirs::home_dir()
            .context("home directory unavailable")?
            .to_string_lossy()
            .into_owned());
    }
    Ok(crate::config::expand_home(Path::new(path))?
        .to_string_lossy()
        .into_owned())
}

pub(crate) fn metadata_key(key: &str) -> Result<String> {
    match key.strip_prefix("agentix:cursor:") {
        Some(path) => Ok(format!("agentix:cursor:{}", abbreviate_home(path)?)),
        None => Ok(key.to_owned()),
    }
}

fn map_roots(value: &mut Value, map: fn(&str) -> Result<String>) -> Result<()> {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if key == "root" {
                    if let Some(path) = value.as_str() {
                        *value = Value::String(map(path)?);
                    }
                } else if !matches!(key.as_str(), "conversation" | "messages") {
                    map_roots(value, map)?;
                }
            }
        }
        Value::Array(array) => {
            for value in array {
                map_roots(value, map)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn expand_roots(value: &mut Value) -> Result<()> {
    map_roots(value, expand_home)
}

pub(crate) fn to_string(value: &impl Serialize) -> Result<String> {
    let mut value = serde_json::to_value(value)?;
    map_roots(&mut value, abbreviate_home)?;
    Ok(value.to_string())
}

pub(crate) fn from_str<T: DeserializeOwned>(data: &str) -> Result<T> {
    if !data.contains("\"root\"") {
        return Ok(serde_json::from_str(data)?);
    }
    let mut value: Value = serde_json::from_str(data)?;
    expand_roots(&mut value)?;
    Ok(serde_json::from_value(value)?)
}

pub(crate) async fn migrate(conn: &mut SqliteConnection) -> Result<()> {
    for (table, id, field) in [
        ("projects", "id", "data"),
        ("task_events", "sequence", "data"),
        ("idempotency_keys", "key", "result"),
    ] {
        let rows = sqlx::query(&format!(
            "SELECT CAST({id} AS TEXT) AS id,{field} AS data FROM {table} WHERE instr({field},'\"root\"')>0"
        ))
        .fetch_all(&mut *conn)
        .await?;
        for row in rows {
            let original: String = row.get("data");
            let value: Value = serde_json::from_str(&original)?;
            let data = to_string(&value)?;
            if data != original {
                sqlx::query(&format!("UPDATE {table} SET {field}=? WHERE {id}=?"))
                    .bind(data)
                    .bind(row.get::<String, _>("id"))
                    .execute(&mut *conn)
                    .await?;
            }
        }
    }
    let rows = sqlx::query("SELECT project_id,canonical_root FROM project_lookup")
        .fetch_all(&mut *conn)
        .await?;
    for row in rows {
        let old: String = row.get("canonical_root");
        let new = abbreviate_home(&old)?;
        if new != old {
            sqlx::query("UPDATE project_lookup SET canonical_root=? WHERE project_id=?")
                .bind(new)
                .bind(row.get::<String, _>("project_id"))
                .execute(&mut *conn)
                .await?;
        }
    }
    migrate_cursors(conn).await
}

async fn migrate_cursors(conn: &mut SqliteConnection) -> Result<()> {
    let rows =
        sqlx::query("SELECT key,value FROM projection_state WHERE key LIKE 'agentix:cursor:%'")
            .fetch_all(&mut *conn)
            .await?;
    for row in rows {
        let old: String = row.get("key");
        let new = metadata_key(&old)?;
        if new == old {
            continue;
        }
        let mut value: String = row.get("value");
        let existing: Option<String> =
            sqlx::query_scalar("SELECT value FROM projection_state WHERE key=?")
                .bind(&new)
                .fetch_optional(&mut *conn)
                .await?;
        if let Some(existing) = existing {
            match (value.parse::<i64>(), existing.parse::<i64>()) {
                (Ok(left), Ok(right)) => value = left.max(right).to_string(),
                _ => ensure!(
                    value == existing,
                    "conflict: incompatible legacy sync cursors"
                ),
            }
        }
        sqlx::query("DELETE FROM projection_state WHERE key=?")
            .bind(old)
            .execute(&mut *conn)
            .await?;
        sqlx::query("INSERT INTO projection_state(key,value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value")
            .bind(new).bind(value).execute(&mut *conn).await?;
    }
    Ok(())
}
