use std::{path::PathBuf, time::Duration};

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
    Report,
    /// Show recent requests and their individual answer scores.
    List {
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=1000))]
        limit: u32,
    },
    /// Record human assessment of the full proposed route and Inbox selection.
    Label {
        request_id: String,
        #[arg(value_parser = ["correct", "incorrect"])]
        review: String,
    },
}

fn database_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("TASKIX_JEV_METRICS_DB")
        && !path.trim().is_empty()
    {
        return Ok(PathBuf::from(path.trim()));
    }
    let root = std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map_or_else(
            || dirs::home_dir().map(|home| home.join(".local/state")),
            Some,
        )
        .context("Cannot resolve Jev metrics directory")?;
    Ok(root.join("taskix/jev-metrics.sqlite"))
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
        if version != 1 || application != 0x544A_4556 {
            bail!("Unsupported Jev metrics schema (application_id={application}, version={version}); expected Taskix Jev metrics v1. Use a compatible version or a new TASKIX_JEV_METRICS_DB path.");
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
        MetricsCommand::Report => report(db, path).await,
    }
}

async fn report(db: &mut SqliteConnection, path: &std::path::Path) -> Result<Value> {
    let totals = rows(db, "SELECT model, threshold, COUNT(*) AS requests, SUM(called) AS called,
        SUM(accepted) AS accepted, AVG(accepted) AS adoption_rate, AVG(duration_ms) AS mean_duration_ms,
        SUM(CASE WHEN accepted=1 AND review IS NOT NULL THEN 1 ELSE 0 END) AS reviewed_accepted,
        AVG(CASE WHEN accepted=1 AND review IS NOT NULL THEN review='correct' END) AS reviewed_accuracy
        FROM requests GROUP BY model, threshold").await?;
    let reasons = rows(
        db,
        "SELECT reason, COUNT(*) AS requests FROM requests WHERE accepted=0 GROUP BY reason",
    )
    .await?;
    let issues = rows(db, "SELECT question, issue, COUNT(*) AS answers FROM answers WHERE issue IS NOT NULL GROUP BY question, issue").await?;
    let mut score_gates = Vec::new();
    for threshold in [0.5, 0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85, 0.9, 0.95] {
        let data = sqlx::query("SELECT model, COUNT(*) AS requests,
            SUM(CASE WHEN answer_count>0 AND NOT EXISTS (SELECT 1 FROM answers a WHERE a.request_id=r.id AND
                (valid=0 OR choice='uncertain' OR confidence<? OR probability<? OR margin<0.2)) THEN 1 ELSE 0 END) AS score_gate_pass
            FROM requests r GROUP BY model").bind(threshold).bind(threshold).fetch_all(&mut *db).await?;
        for row in data {
            let mut value = row_json(&row)?;
            value["threshold"] = json!(threshold);
            score_gates.push(value);
        }
    }
    let response_quality = rows(db, "SELECT model,
        SUM(CASE WHEN answer_count>0 AND NOT EXISTS (SELECT 1 FROM answers a WHERE a.request_id=r.id AND valid=0) THEN 1 ELSE 0 END) AS valid_responses,
        SUM(CASE WHEN answer_count>0 AND NOT EXISTS (SELECT 1 FROM answers a WHERE a.request_id=r.id AND valid=0)
            AND EXISTS (SELECT 1 FROM answers a WHERE a.request_id=r.id AND issue='low_confidence') THEN 1 ELSE 0 END) AS responses_with_low_confidence
        FROM requests r GROUP BY model").await?;
    Ok(
        json!({"path":path,"totals":totals,"reasons":reasons,"issues":issues,"response_quality":response_quality,"score_gates":score_gates,
        "note":"Score gates are not predicted adoption: assignment/revision checks may still reject. Accuracy includes only manually reviewed accepted requests. mean_duration_ms measures preparation and Jev; it excludes metrics writing and subsequent Agent handling. Metrics writes are best-effort."}),
    )
}

fn percent(value: &Value) -> String {
    value
        .as_f64()
        .map_or_else(|| "n/a".to_string(), |v| format!("{:.1}%", v * 100.0))
}

pub fn print_report(value: &Value) {
    println!(
        "Jev routing metrics: {}",
        value["path"].as_str().unwrap_or("")
    );
    println!("MODEL  THRESHOLD  REQUESTS  ACCEPTED  ADOPTION  REVIEWED  ACCURACY  PREP_MS");
    for row in value["totals"].as_array().into_iter().flatten() {
        println!(
            "{}  {}  {}  {}  {}  {}  {}  {:.1}",
            row["model"].as_str().unwrap_or(""),
            row["threshold"],
            row["requests"],
            row["accepted"],
            percent(&row["adoption_rate"]),
            row["reviewed_accepted"],
            percent(&row["reviewed_accuracy"]),
            row["mean_duration_ms"].as_f64().unwrap_or(0.0)
        );
    }
    for (key, heading, columns) in [
        ("reasons", "Fallback reasons", &["reason", "requests"][..]),
        (
            "issues",
            "Answer issues",
            &["question", "issue", "answers"][..],
        ),
        (
            "response_quality",
            "Response quality",
            &["model", "valid_responses", "responses_with_low_confidence"][..],
        ),
        (
            "score_gates",
            "Score gates (not predicted adoption)",
            &["model", "threshold", "score_gate_pass", "requests"][..],
        ),
    ] {
        println!("\n{heading}: {}", columns.join("  "));
        for row in value[key].as_array().into_iter().flatten() {
            println!(
                "{}",
                columns
                    .iter()
                    .map(|key| row[key]
                        .as_str()
                        .map_or_else(|| row[key].to_string(), str::to_string))
                    .collect::<Vec<_>>()
                    .join("  ")
            );
        }
    }
    println!("\n{}", value["note"].as_str().unwrap_or(""));
}
