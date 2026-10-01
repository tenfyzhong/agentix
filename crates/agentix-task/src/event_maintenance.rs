//! Compact event records and durable sequence receipts independent of retention.
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use sqlx::{Row, SqliteConnection};

use crate::Store;

pub(crate) use crate::event_payload::compact_payload;

pub(crate) async fn migrate(conn: &mut SqliteConnection) -> Result<()> {
    // sqlite_sequence also preserves the global receipt if events were removed earlier.
    sqlx::query("INSERT INTO event_watermarks(scope,sequence) VALUES ('',MAX(COALESCE((SELECT MAX(sequence) FROM task_events),0),COALESCE((SELECT seq FROM sqlite_sequence WHERE name='task_events'),0))) ON CONFLICT(scope) DO UPDATE SET sequence=MAX(sequence,excluded.sequence)")
        .execute(&mut *conn).await?;
    sqlx::query("INSERT INTO event_watermarks(scope,sequence) SELECT json_extract(data,'$.project_id'),MAX(sequence) FROM task_events WHERE json_extract(data,'$.project_id') IS NOT NULL GROUP BY json_extract(data,'$.project_id') ON CONFLICT(scope) DO UPDATE SET sequence=MAX(sequence,excluded.sequence)")
        .execute(&mut *conn).await?;
    Ok(())
}

#[cfg(test)]
#[path = "event_maintenance_tests.rs"]
mod tests;

impl Store {
    /// Preview historical compaction and optional age-based pruning.
    pub async fn maintain_events(
        &self,
        retain_days: i64,
        prune: bool,
        apply: bool,
    ) -> Result<Value> {
        ensure!(
            (1..=36500).contains(&retain_days),
            "invalid: retain_days must be between 1 and 36500"
        );
        let cutoff = self.now().saturating_sub(retain_days * 86400);
        let mut tx = if apply {
            self.pool.begin_with("BEGIN IMMEDIATE").await?
        } else {
            self.pool.begin().await?
        };
        let mut report = MaintenanceReport::default();
        let mut after = 0;
        loop {
            let rows = sqlx::query("SELECT sequence,data FROM task_events WHERE sequence>? ORDER BY sequence LIMIT 128").bind(after).fetch_all(&mut *tx).await?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                after = row.get("sequence");
                let data: String = row.get("data");
                let mut event: Value = serde_json::from_str(&data)?;
                let expired = event["occurred_at"]
                    .as_i64()
                    .is_some_and(|stamp| stamp < cutoff);
                if prune && expired {
                    report.deleted_events += 1;
                    report.reclaimed_payload_bytes += data.len();
                    if apply {
                        sqlx::query("UPDATE event_retention SET pruned_through=MAX(pruned_through,?) WHERE id=1").bind(after).execute(&mut *tx).await?;
                        sqlx::query("DELETE FROM task_events WHERE sequence=?")
                            .bind(after)
                            .execute(&mut *tx)
                            .await?;
                    }
                } else {
                    let compact = compact_payload(&event["payload"]);
                    if compact != event["payload"] {
                        event["payload"] = compact;
                        let compact = serde_json::to_string(&event)?;
                        report.compacted_events += 1;
                        report.reclaimed_payload_bytes += data.len().saturating_sub(compact.len());
                        if apply {
                            sqlx::query("UPDATE task_events SET data=? WHERE sequence=?")
                                .bind(compact)
                                .bind(after)
                                .execute(&mut *tx)
                                .await?;
                        }
                    }
                }
            }
        }
        tx.commit().await?;
        Ok(
            json!({"applied":apply,"prune":prune,"retain_days":retain_days,"cutoff":cutoff,
            "compacted_events":report.compacted_events,"deleted_events":report.deleted_events,
            "reclaimed_payload_bytes":report.reclaimed_payload_bytes}),
        )
    }

    /// Reclaim freed database pages explicitly, outside maintenance transactions.
    pub async fn vacuum_events(&self) -> Result<()> {
        sqlx::query("VACUUM").execute(&self.pool).await?;
        Ok(())
    }
}

#[derive(Default)]
struct MaintenanceReport {
    compacted_events: usize,
    deleted_events: usize,
    reclaimed_payload_bytes: usize,
}
