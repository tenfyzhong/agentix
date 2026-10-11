//! Typed, extensible decision events and bounded best-effort persistence.
//! No routing, memory, lifecycle, or report logic belongs in this module.
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{path::PathBuf, time::Duration};

/// A stable `snake_case` identifier; new kinds need no writer/report registration.
fn valid_kind(kind: &str) -> bool {
    !kind.is_empty()
        && kind.len() <= 64
        && kind.as_bytes()[0].is_ascii_lowercase()
        && kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

#[derive(Debug, Deserialize)]
pub(crate) struct AnswerMetric {
    pub question: String,
    pub subject_id: Option<String>,
    pub choice: Option<String>,
    pub confidence: Option<f64>,
    pub probability: Option<f64>,
    pub margin: Option<f64>,
    pub valid: i64,
    pub issue: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DecisionMetric {
    pub kind: String,
    pub started_at: i64,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub project_id: Option<String>,
    pub model: String,
    pub threshold: f64,
    pub duration_ms: f64,
    pub called: bool,
    pub action: String,
    pub reason: Option<String>,
    pub answers: Vec<AnswerMetric>,
}

impl DecisionMetric {
    pub fn new(kind: &str, model: &str, threshold: f64) -> Self {
        Self {
            kind: kind.into(),
            started_at: i64::try_from(
                time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000,
            )
            .unwrap_or(0),
            session_id: None,
            turn_id: None,
            project_id: None,
            model: model.into(),
            threshold,
            duration_ms: 0.0,
            called: false,
            action: "agent".into(),
            reason: Some("preparation_unavailable".into()),
            answers: Vec::new(),
        }
    }
}

/// Owns the persistence deadline; callers never need to catch storage failures.
pub(crate) struct MetricsCollector {
    path: PathBuf,
}

impl MetricsCollector {
    pub fn from_env() -> Option<Self> {
        std::env::var("TASKIX_JEV_METRICS_ENABLED")
            .ok()
            .is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1"))
            .then(database_path)
            .and_then(Result::ok)
            .map(Self::new)
    }

    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub async fn record(&self, event: &DecisionMetric) -> bool {
        matches!(
            tokio::time::timeout(Duration::from_millis(250), append_metric(&self.path, event))
                .await,
            Ok(Ok(()))
        )
    }
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

// Share the plugin's v2 protocol and atomic v1 migration. No source text is stored.
async fn append_metric(path: &std::path::Path, event: &DecisionMetric) -> Result<()> {
    ensure!(valid_kind(&event.kind), "invalid metrics decision kind");
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
        for statement in include_str!("../../../../plugins/taskix-manager/metrics-schema.sql")
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
    sqlx::query("INSERT INTO requests (id,started_at,session_id,turn_id,project_id,model,threshold,duration_ms,called,accepted,action,reason,answer_count,kind) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(&id).bind(event.started_at)
        .bind(&event.session_id).bind(&event.turn_id).bind(&event.project_id)
        .bind(&event.model).bind(event.threshold).bind(event.duration_ms)
        .bind(event.called).bind(event.called && event.action != "agent")
        .bind(&event.action).bind(&event.reason).bind(i64::try_from(event.answers.len())?)
        .bind(&event.kind).execute(&mut *tx).await?;
    for answer in &event.answers {
        sqlx::query("INSERT INTO answers VALUES (?,?,?,?,?,?,?,?,?)")
            .bind(&id)
            .bind(&answer.question)
            .bind(&answer.subject_id)
            .bind(&answer.choice)
            .bind(answer.confidence)
            .bind(answer.probability)
            .bind(answer.margin)
            .bind(answer.valid)
            .bind(&answer.issue)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    db.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sqlx::Row;
    #[tokio::test]
    async fn collector_rejects_invalid_kind_without_creating_storage() {
        let dir = tempfile::tempdir().unwrap();
        for kind in [
            "",
            "Risk",
            "risk-review",
            "risk\nreview",
            "私密",
            &"x".repeat(65),
        ] {
            let path = dir.path().join("missing/metrics.db");
            let collector = MetricsCollector::new(path.clone());
            assert!(
                !collector
                    .record(&DecisionMetric::new(kind, "jev-test", 0.65))
                    .await
            );
            assert!(!path.exists());
        }
    }

    #[tokio::test]
    async fn generic_collector_rolls_back_all_answers_on_duplicate_question() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.db");
        let mut metric = DecisionMetric::new("risk_review", "jev-test", 0.65);
        // Exercise transaction correctness independently of the best-effort deadline.
        append_metric(&path, &metric).await.unwrap();
        for _ in 0..2 {
            metric.answers.push(AnswerMetric {
                question: "risk".into(),
                subject_id: None,
                choice: Some("allow".into()),
                confidence: Some(0.99),
                probability: Some(0.99),
                margin: Some(0.98),
                valid: 1,
                issue: None,
            });
        }
        let error = append_metric(&path, &metric).await.unwrap_err();
        assert!(
            error.to_string().contains("UNIQUE constraint failed"),
            "{error}"
        );
        let mut db = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM requests")
                .fetch_one(&mut db)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM answers")
                .fetch_one(&mut db)
                .await
                .unwrap(),
            0
        );
        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn generic_decision_preserves_kind_and_multiple_answers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.db");
        let event = json!({"kind":"risk_review","started_at":1,"model":"jev-test","threshold":0.65,
        "duration_ms":2,"called":true,"action":"allow","reason":null,
        "answers":[
            {"question":"risk","subject_id":null,"choice":"allow","confidence":0.99,"probability":0.99,"margin":0.98,"valid":1,"issue":null},
            {"question":"scope","subject_id":null,"choice":"allow","confidence":0.99,"probability":0.99,"margin":0.98,"valid":1,"issue":null}
        ]});
        append_metric(&path, &serde_json::from_value(event.clone()).unwrap())
            .await
            .unwrap();
        let mut db = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
        let row = sqlx::query("SELECT kind,answer_count,accepted FROM requests")
            .fetch_one(&mut db)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("kind"), "risk_review");
        assert_eq!(row.get::<i64, _>("answer_count"), 2);
        assert_eq!(row.get::<i64, _>("accepted"), 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM answers")
                .fetch_one(&mut db)
                .await
                .unwrap(),
            2
        );
        db.close().await.unwrap();
    }

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
        let schema = include_str!("../../../../plugins/taskix-manager/metrics-schema.sql")
            .replace("user_version=2", "user_version=1")
            .replace(", kind TEXT NOT NULL DEFAULT 'routing'", "");
        sqlx::raw_sql(&schema).execute(&mut db).await.unwrap();
        db.close().await.unwrap();
        let event = json!({"started_at":1,"model":"jev-test","threshold":0.65,"duration_ms":1,"called":true,"action":"skip","reason":null,"kind":"memory_triage","answers":[]});
        append_metric(&path, &serde_json::from_value(event.clone()).unwrap())
            .await
            .unwrap();
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
        assert!(
            append_metric(&path, &serde_json::from_value(event.clone()).unwrap())
                .await
                .is_err()
        );
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
        let schema = include_str!("../../../../plugins/taskix-manager/metrics-schema.sql")
            .replace("user_version=2", "user_version=1")
            .replace(", kind TEXT NOT NULL DEFAULT 'routing'", "");
        sqlx::raw_sql(&schema).execute(&mut db).await.unwrap();
        sqlx::query("CREATE TRIGGER reject_append BEFORE INSERT ON requests BEGIN SELECT RAISE(FAIL,'fixture'); END").execute(&mut db).await.unwrap();
        assert!(
            append_metric(
                &path,
                &DecisionMetric::new("memory_triage", "jev-test", 0.65)
            )
            .await
            .is_err()
        );
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
