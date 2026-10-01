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
    /// Record human assessment of the full proposed route and Inbox selection.
    Label {
        request_id: String,
        #[arg(value_parser = ["correct", "incorrect"])]
        review: String,
    },
}

pub(crate) fn database_path() -> Result<PathBuf> {
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
        MetricsCommand::Report { .. } => report(db, path).await,
    }
}

async fn report(db: &mut SqliteConnection, path: &std::path::Path) -> Result<Value> {
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *db)
        .await?;
    let kind = if version == 1 { "'routing'" } else { "kind" };
    let by_kind = rows(db, &format!("SELECT {kind} AS kind, COUNT(*) AS requests,
        SUM(called) AS called, SUM(accepted) AS accepted, AVG(accepted) AS adoption_rate,
        AVG(duration_ms) AS mean_duration_ms,
        SUM(CASE WHEN accepted=1 AND review IS NOT NULL THEN 1 ELSE 0 END) AS reviewed_accepted,
        AVG(CASE WHEN accepted=1 AND review IS NOT NULL THEN review='correct' END) AS reviewed_accuracy
        FROM requests GROUP BY {kind} ORDER BY kind")).await?;
    let memory_triage = rows(
        db,
        &format!(
            "SELECT COUNT(*) AS requests,
        coalesce(SUM(action='skip' AND accepted=1),0) AS skipped,
        coalesce(SUM(action='extract' AND accepted=1),0) AS extract,
        coalesce(SUM(accepted=0),0) AS fallback FROM requests WHERE {kind}='memory_triage'"
        ),
    )
    .await?
    .into_iter()
    .next()
    .unwrap_or(Value::Null);
    let by_question = rows(
        db,
        "SELECT
        CASE WHEN question GLOB 'inbox_[0-9]*' THEN 'inbox'
             WHEN question GLOB 'turn_[0-9]*' THEN 'discussion' ELSE question END AS question,
        COUNT(*) AS answers,
        SUM(CASE WHEN valid=1 AND choice!='uncertain' AND confidence>=r.threshold
            AND probability>=r.threshold AND margin>=0.2 THEN 1 ELSE 0 END) AS passed
        FROM answers a JOIN requests r ON r.id=a.request_id GROUP BY 1 ORDER BY 1",
    )
    .await?;
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
        json!({"path":path,"memory_triage":memory_triage,"totals":totals,"by_kind":by_kind,"by_question":by_question,"reasons":reasons,"issues":issues,"response_quality":response_quality,"score_gates":score_gates,
        "note":"Score gates are not predicted adoption: assignment/revision checks may still reject. Accuracy includes only manually reviewed accepted requests. mean_duration_ms measures preparation and Jev; it excludes metrics writing and subsequent Agent handling. Metrics writes are best-effort. Memory triage counts classification decisions, not committed work outcomes or saved model calls."}),
    )
}

fn percent(value: &Value) -> String {
    value
        .as_f64()
        .map_or_else(|| "n/a".to_string(), |v| format!("{:.1}%", v * 100.0))
}

pub fn print_report(value: &Value, details: bool) {
    print_summary(value);
    if !details {
        return;
    }
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

fn print_summary(value: &Value) {
    println!("Jev metrics");
    println!(
        "{:<13} {:>8} {:>7} {:>8} {:>7} {:>8}",
        "KIND", "REQUESTS", "CALLED", "ACCEPTED", "RATE", "PREP_MS"
    );
    let rows = value["by_kind"].as_array().into_iter().flatten();
    let mut requests = 0;
    let mut accepted = 0;
    for row in rows {
        requests += row["requests"].as_u64().unwrap_or(0);
        accepted += row["accepted"].as_u64().unwrap_or(0);
        println!(
            "{:<13} {:>8} {:>7} {:>8} {:>7} {:>8.1}",
            row["kind"].as_str().unwrap_or("unknown"),
            row["requests"].as_u64().unwrap_or(0),
            row["called"].as_u64().unwrap_or(0),
            row["accepted"].as_u64().unwrap_or(0),
            percent(&row["adoption_rate"]),
            row["mean_duration_ms"].as_f64().unwrap_or(0.0)
        );
    }
    if value["memory_triage"]["requests"].as_u64().unwrap_or(0) > 0 {
        println!(
            "Memory triage: skipped {}, extract {}, fallback {}",
            value["memory_triage"]["skipped"],
            value["memory_triage"]["extract"],
            value["memory_triage"]["fallback"]
        );
    }
    println!(
        "Total: {accepted}/{requests} accepted ({})",
        percent(&ratio(&json!(accepted), &json!(requests)))
    );
    println!(
        "{:<13} {:>8} {:>7} {:>8}",
        "QUESTION", "ANSWERS", "PASSED", "RATE"
    );
    for row in value["by_question"].as_array().into_iter().flatten() {
        let rate = ratio(&row["passed"], &row["answers"]);
        println!(
            "{:<13} {:>8} {:>7} {:>8}",
            row["question"].as_str().unwrap_or("unknown"),
            row["answers"].as_u64().unwrap_or(0),
            row["passed"].as_u64().unwrap_or(0),
            percent(&rate)
        );
    }
    println!("Question pass rate is a score gate, not adoption or accuracy.");
    println!("PREP_MS excludes metrics writes and subsequent Agent handling.");
    println!("Details: --details | Full data: --json | Collection is best-effort.");
}

fn ratio(numerator: &Value, denominator: &Value) -> Value {
    match (numerator.as_f64(), denominator.as_f64()) {
        (Some(n), Some(d)) if d > 0.0 => json!(n / d),
        _ => Value::Null,
    }
}

// Share the plugin's v2 protocol and atomic v1 migration. No source text is stored.
pub(crate) async fn append_memory_metric(path: &std::path::Path, event: &Value) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let mut directories = tokio::fs::DirBuilder::new();
    directories.recursive(true);
    #[cfg(unix)]
    directories.mode(0o700);
    directories.create(parent).await?;
    let mut file = tokio::fs::OpenOptions::new();
    file.create(true).append(true);
    #[cfg(unix)]
    file.mode(0o600);
    drop(file.open(path).await?);
    let mut db = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .busy_timeout(Duration::from_millis(25)),
    )
    .await?;
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *tx)
        .await?;
    let application: i64 = sqlx::query_scalar("PRAGMA application_id")
        .fetch_one(&mut *tx)
        .await?;
    let tables: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'")
            .fetch_one(&mut *tx)
            .await?;
    if version == 0 && application == 0 && tables == 0 {
        for statement in include_str!("../../../plugins/taskix-manager/metrics-schema.sql")
            .split(';')
            .filter(|s| !s.trim().is_empty())
        {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
    } else if !matches!(version, 1 | 2) || application != 0x544A_4556 {
        bail!("unsupported Jev metrics schema");
    }
    if version == 1 {
        sqlx::query("ALTER TABLE requests ADD COLUMN kind TEXT NOT NULL DEFAULT 'routing'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("PRAGMA user_version=2")
            .execute(&mut *tx)
            .await?;
    }
    let id: String = sqlx::query_scalar("SELECT lower(hex(randomblob(16)))")
        .fetch_one(&mut *tx)
        .await?;
    let action = event["action"].as_str().unwrap_or("agent");
    let called = event["called"].as_bool().unwrap_or(false);
    let answer = &event["answer"];
    sqlx::query("INSERT INTO requests (id,started_at,session_id,turn_id,project_id,model,threshold,duration_ms,called,accepted,action,reason,answer_count,kind) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,'memory_triage')")
        .bind(&id).bind(event["started_at"].as_i64().unwrap_or(0))
        .bind(event["session_id"].as_str()).bind(event["turn_id"].as_str()).bind(event["project_id"].as_str())
        .bind(event["model"].as_str().unwrap_or("jev-latest")).bind(event["threshold"].as_f64().unwrap_or(0.65))
        .bind(event["duration_ms"].as_f64().unwrap_or(0.0)).bind(called).bind(called && action!="agent")
        .bind(action).bind(event["reason"].as_str()).bind(i64::from(answer.is_object())).execute(&mut *tx).await?;
    if answer.is_object() {
        sqlx::query("INSERT INTO answers VALUES (?,'memory_triage',?,?,?,?,?,?,?)")
            .bind(&id)
            .bind(event["subject_id"].as_str())
            .bind(answer["choice"].as_str())
            .bind(answer["confidence"].as_f64())
            .bind(answer["probability"].as_f64())
            .bind(answer["margin"].as_f64())
            .bind(answer["valid"].as_i64().unwrap_or(0))
            .bind(answer["issue"].as_str())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    db.close().await?;
    Ok(())
}

#[cfg(test)]
mod memory_writer_tests {
    use super::*;
    #[tokio::test]
    async fn memory_metrics_writer_migrates_v1_and_refuses_foreign_databases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.db");
        let mut db = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        let schema = include_str!("../../../plugins/taskix-manager/metrics-schema.sql")
            .replace("user_version=2", "user_version=1")
            .replace(", kind TEXT NOT NULL DEFAULT 'routing'", "");
        sqlx::raw_sql(&schema).execute(&mut db).await.unwrap();
        db.close().await.unwrap();
        let event = json!({"started_at":1,"model":"jev-test","threshold":0.65,"duration_ms":1,"called":true,"action":"skip","reason":null,"answer":null});
        append_memory_metric(&path, &event).await.unwrap();
        let mut db = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA user_version")
                .fetch_one(&mut db)
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT kind FROM requests")
                .fetch_one(&mut db)
                .await
                .unwrap(),
            "memory_triage"
        );
        sqlx::query("PRAGMA application_id=42")
            .execute(&mut db)
            .await
            .unwrap();
        db.close().await.unwrap();
        assert!(append_memory_metric(&path, &event).await.is_err());
    }
    #[tokio::test]
    async fn memory_metrics_failed_append_rolls_back_v1_migration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.db");
        let mut db = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        let schema = include_str!("../../../plugins/taskix-manager/metrics-schema.sql")
            .replace("user_version=2", "user_version=1")
            .replace(", kind TEXT NOT NULL DEFAULT 'routing'", "");
        sqlx::raw_sql(&schema).execute(&mut db).await.unwrap();
        sqlx::query("CREATE TRIGGER reject_append BEFORE INSERT ON requests BEGIN SELECT RAISE(FAIL,'fixture'); END").execute(&mut db).await.unwrap();
        assert!(append_memory_metric(&path, &json!({})).await.is_err());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA user_version")
                .fetch_one(&mut db)
                .await
                .unwrap(),
            1
        );
        db.close().await.unwrap();
    }
}
