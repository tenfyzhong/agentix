//! Atomic ID migration preserves rowids and immutable evidence text.
use std::collections::HashMap;

use anyhow::Result;
use serde_json::Value;
use sqlx::SqliteConnection;

use crate::{Memory, ids};

pub(crate) async fn migrate(conn: &mut SqliteConnection) -> Result<()> {
    sqlx::query("PRAGMA defer_foreign_keys=ON")
        .execute(&mut *conn)
        .await?;
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT id,data FROM memories ORDER BY id")
        .fetch_all(&mut *conn)
        .await?;
    let mut mapping = HashMap::new();
    for (id, data) in &rows {
        if let Some(suffix) = ids::legacy_suffix(id) {
            let memory: Memory = serde_json::from_str(data)?;
            mapping.insert(id.clone(), ids::timestamp_id(memory.created_at, suffix)?);
        }
    }
    for (old, new) in &mapping {
        // Updating the primary key in place keeps the FTS rowid and vector contents.
        sqlx::query("UPDATE memories SET id=? WHERE id=?")
            .bind(new)
            .bind(old)
            .execute(&mut *conn)
            .await?;
        for table in [
            "memory_versions",
            "suppressions",
            "memory_vectors",
            "embedding_failures",
            "memory_projection",
            "memory_reviews",
            "context_deliveries",
        ] {
            sqlx::query(&format!("UPDATE {table} SET memory_id=? WHERE memory_id=?"))
                .bind(new)
                .bind(old)
                .execute(&mut *conn)
                .await?;
        }
        sqlx::query("INSERT INTO memory_id_renames(old_id,memory_id) VALUES(?,?)")
            .bind(old)
            .bind(new)
            .execute(&mut *conn)
            .await?;
    }
    if !mapping.is_empty() {
        for table in ["memories", "memory_versions"] {
            let rows: Vec<(i64, String)> =
                sqlx::query_as(&format!("SELECT rowid,data FROM {table}"))
                    .fetch_all(&mut *conn)
                    .await?;
            for (rowid, data) in rows {
                let mut memory: Memory = serde_json::from_str(&data)?;
                rename(&mut memory.id, &mapping);
                for reference in [&mut memory.supersedes, &mut memory.superseded_by]
                    .into_iter()
                    .flatten()
                {
                    rename(reference, &mapping);
                }
                let migrated = serde_json::to_string(&memory)?;
                if migrated != data {
                    sqlx::query(&format!("UPDATE {table} SET data=? WHERE rowid=?"))
                        .bind(migrated)
                        .bind(rowid)
                        .execute(&mut *conn)
                        .await?;
                }
            }
        }
        migrate_context(conn, &mapping).await?;
        // Only current structured review IDs and completed decision targets are references.
        // Source snapshots, evidence, model transcripts and audit text stay verbatim.
        migrate_work(conn, &mapping).await?;
        // Supersession links can change in notes whose IDs already have a timestamp.
        sqlx::query("UPDATE memory_projection SET published_revision=0,prepared_revision=0,prepared_hash=''")
            .execute(&mut *conn).await?;
    }
    sqlx::query("PRAGMA user_version=2").execute(conn).await?;
    Ok(())
}

async fn migrate_work(
    conn: &mut SqliteConnection,
    mapping: &HashMap<String, String>,
) -> Result<()> {
    let work: Vec<(i64, String, Option<String>)> =
        sqlx::query_as("SELECT id,payload,result FROM work_items WHERE kind='consolidate'")
            .fetch_all(&mut *conn)
            .await?;
    for (id, payload, result) in work {
        let mut payload: Value = serde_json::from_str(&payload)?;
        if let Some(review) = payload.get_mut("review") {
            let mut memory: Memory = serde_json::from_value(review.clone())?;
            rename(&mut memory.id, mapping);
            for reference in [&mut memory.supersedes, &mut memory.superseded_by]
                .into_iter()
                .flatten()
            {
                rename(reference, mapping);
            }
            *review = serde_json::to_value(memory)?;
        }
        let result = result
            .map(|text| -> Result<String> {
                let mut value: Value = serde_json::from_str(&text)?;
                if let Some(decisions) = value.as_array_mut() {
                    for decision in decisions {
                        if let Some(Value::String(target)) = decision.get_mut("target") {
                            rename(target, mapping);
                        }
                    }
                }
                Ok(serde_json::to_string(&value)?)
            })
            .transpose()?;
        sqlx::query("UPDATE work_items SET payload=?,result=?,state=CASE WHEN state='running' THEN 'pending' ELSE state END,generation=generation+CASE WHEN state='running' THEN 1 ELSE 0 END,attempts=max(0,attempts-CASE WHEN state='running' THEN 1 ELSE 0 END),owner=NULL,lease_until=NULL WHERE id=?")
                .bind(payload.to_string()).bind(result).bind(id).execute(&mut *conn).await?;
    }
    Ok(())
}

fn rename(id: &mut String, mapping: &HashMap<String, String>) {
    if let Some(new) = mapping.get(id) {
        id.clone_from(new);
    }
}

async fn migrate_context(
    conn: &mut SqliteConnection,
    mapping: &HashMap<String, String>,
) -> Result<()> {
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT rowid,data FROM context_receipts")
        .fetch_all(&mut *conn)
        .await?;
    for (rowid, data) in rows {
        let mut packet: crate::ContextPacket = serde_json::from_str(&data)?;
        for item in &mut packet.items {
            rename(&mut item.id, mapping);
        }
        let mut text = String::new();
        for line in packet.text.split_inclusive('\n') {
            if let Ok(mut value) = serde_json::from_str::<Value>(line)
                && let Some(Value::String(id)) = value.get_mut("id")
            {
                rename(id, mapping);
                text.push_str(&serde_json::to_string(&value)?);
                if line.ends_with('\n') {
                    text.push('\n');
                }
                continue;
            }
            text.push_str(line);
        }
        packet.text = text;
        sqlx::query("UPDATE context_receipts SET data=? WHERE rowid=?")
            .bind(serde_json::to_string(&packet)?)
            .bind(rowid)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}
