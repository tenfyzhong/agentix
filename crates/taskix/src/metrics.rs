pub(crate) mod collector;
mod report;
pub(crate) use collector::database_path;
pub use report::print_report;

use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde_json::{Value, json};
use sqlx::{
    Column, Connection, Row, SqliteConnection, TypeInfo, ValueRef,
    sqlite::{SqliteConnectOptions, SqliteRow},
};

#[derive(Subcommand)]
pub enum MetricsCommand {
    /// Show adoption, fallback reasons, reviewed accuracy, and threshold comparisons.
    Report {
        /// Include fallback reasons, answer issues, and all score gates.
        #[arg(long)]
        details: bool,
    },
    /// Show recent requests and their individual answer scores.
    List {
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=1000))]
        limit: u32,
    },
    /// Record human assessment of the full proposed route and Issue association.
    Label {
        request_id: String,
        #[arg(value_parser = ["correct", "incorrect"])]
        review: String,
    },
}

fn row_json(row: &SqliteRow) -> Result<Value> {
    let mut result = serde_json::Map::new();
    for column in row.columns() {
        let name = column.name();
        let raw = row.try_get_raw(name)?;
        let value = if raw.is_null() {
            Value::Null
        } else {
            match raw.type_info().name() {
                "INTEGER" => json!(row.try_get::<i64, _>(name)?),
                "REAL" => json!(row.try_get::<f64, _>(name)?),
                _ => json!(row.try_get::<String, _>(name)?),
            }
        };
        result.insert(name.to_string(), value);
    }
    Ok(Value::Object(result))
}

async fn rows(db: &mut SqliteConnection, sql: &str) -> Result<Vec<Value>> {
    sqlx::query(sql)
        .fetch_all(db)
        .await?
        .iter()
        .map(row_json)
        .collect()
}

pub async fn run(command: &MetricsCommand) -> Result<Value> {
    let path = database_path()?;
    if !path.is_file() {
        bail!(
            "not_found: Jev metrics database {}. Enable TASKIX_JEV_METRICS_ENABLED and collect routing requests first.",
            path.display()
        );
    }
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(false)
        .read_only(!matches!(command, MetricsCommand::Label { .. }))
        .busy_timeout(Duration::from_millis(25));
    let mut db = SqliteConnection::connect_with(&options)
        .await
        .context("Cannot open Jev metrics database")?;
    let result = async {
        let version: i64 = sqlx::query_scalar("PRAGMA user_version").fetch_one(&mut db).await?;
        let application: i64 = sqlx::query_scalar("PRAGMA application_id").fetch_one(&mut db).await?;
        if !matches!(version, 1 | 2) || application != 0x544A_4556 {
            bail!("Unsupported Jev metrics schema (application_id={application}, version={version}); expected Taskix Jev metrics v1 or v2. Use a compatible version or a new TASKIX_JEV_METRICS_DB path.");
        }
        query(&mut db, command, &path).await
    }.await;
    db.close().await?;
    result
}

async fn query(
    db: &mut SqliteConnection,
    command: &MetricsCommand,
    path: &std::path::Path,
) -> Result<Value> {
    match command {
        MetricsCommand::Label { request_id, review } => {
            let changed = sqlx::query("UPDATE requests SET review=? WHERE id=?")
                .bind(review)
                .bind(request_id)
                .execute(db)
                .await?
                .rows_affected();
            if changed == 0 {
                bail!("not_found: Unknown metrics request ID");
            }
            Ok(json!({"id":request_id,"review":review}))
        }
        MetricsCommand::List { limit } => {
            let data =
                sqlx::query("SELECT * FROM requests ORDER BY started_at DESC, rowid DESC LIMIT ?")
                    .bind(i64::from(*limit))
                    .fetch_all(&mut *db)
                    .await?;
            let mut result = Vec::new();
            for row in data {
                let mut value = row_json(&row)?;
                let answers = sqlx::query("SELECT question, subject_id, choice, confidence, probability, margin, valid, issue FROM answers WHERE request_id=? ORDER BY question")
                    .bind(row.try_get::<String, _>("id")?).fetch_all(&mut *db).await?;
                value["answers"] = json!(answers.iter().map(row_json).collect::<Result<Vec<_>>>()?);
                result.push(value);
            }
            Ok(json!(result))
        }
        MetricsCommand::Report { .. } => report::report(db, path).await,
    }
}
