use serde_json::Value;
use sqlx::{Connection, sqlite::SqliteConnectOptions};
use std::process::Command;
use tempfile::TempDir;

fn command(dir: &TempDir, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_taskix"))
        .env("TASKIX_JEV_METRICS_DB", dir.path().join("metrics.sqlite"))
        .env("TASKIX_CONFIG", dir.path().join("missing.toml"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn metrics_missing_database_does_not_create_database_or_require_task_config() {
    let dir = TempDir::new().unwrap();
    let output = command(&dir, &["routing", "metrics", "report", "--json"]);
    let body: Value = serde_json::from_slice(&output.stdout).expect("structured metrics error");
    assert_eq!(body["ok"], false);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("metrics")
    );
    assert!(!dir.path().join("metrics.sqlite").exists());
}

#[tokio::test]
async fn metrics_report_list_and_label_without_task_database() {
    let dir = TempDir::new().unwrap();
    let mut db = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(dir.path().join("metrics.sqlite"))
            .create_if_missing(true),
    )
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../plugins/taskix-manager/metrics-schema.sql"
    ))
    .execute(&mut db)
    .await
    .unwrap();
    sqlx::raw_sql("        INSERT INTO requests VALUES ('r1', 1, 's', 't', 'p', 'jev-test', 0.9, 5.0, 1, 1, 'new_job', NULL, NULL, 1, 'routing');
        INSERT INTO answers VALUES ('r1', 'route', NULL, 'new_job', 0.99, 0.99, 0.98, 1, NULL);
        INSERT INTO requests VALUES ('r2', 2, 's', 't2', 'p', 'jev-test', 0.9, 7.0, 1, 0, 'agent', 'uncertain_or_conflicting', NULL, 2, 'routing');
        INSERT INTO answers VALUES ('r2', 'route', NULL, 'new_job', 0.99, 0.99, 0.98, 1, NULL);
        INSERT INTO answers VALUES ('r2', 'inbox_0', 'inbox_a', 'unrelated', 0.87, 0.99, 0.98, 1, 'low_confidence');").execute(&mut db).await.unwrap();
    db.close().await.unwrap();
    let report = command(&dir, &["routing", "metrics", "report", "--json"]);
    assert!(
        report.status.success(),
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
    let body: Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(body["result"]["totals"][0]["adoption_rate"], 0.5);
    assert_eq!(
        body["result"]["totals"][0]["reviewed_accuracy"],
        Value::Null
    );
    let gates = body["result"]["score_gates"].as_array().unwrap();
    let thresholds = [0.5, 0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85, 0.9, 0.95];
    assert_eq!(gates.len(), thresholds.len());
    for (gate, threshold) in gates.iter().zip(thresholds) {
        assert_eq!(gate["model"], "jev-test");
        assert_eq!(gate["threshold"], threshold);
        assert_eq!(gate["requests"], 2);
        assert_eq!(
            gate["score_gate_pass"],
            if threshold <= 0.87 { 2 } else { 1 }
        );
    }
    assert_compact_report(&dir);
    let text = command(&dir, &["routing", "metrics", "report", "--details"]);
    assert!(text.status.success());
    let stdout = String::from_utf8_lossy(&text.stdout);
    let gate_lines: Vec<_> = stdout
        .split_once(
            "Score gates (not predicted adoption): model  threshold  score_gate_pass  requests\n",
        )
        .unwrap()
        .1
        .lines()
        .take_while(|line| !line.is_empty())
        .collect();
    let expected_lines: Vec<_> = thresholds
        .iter()
        .map(|threshold| {
            let pass = if *threshold <= 0.87 { 2 } else { 1 };
            format!("jev-test  {threshold}  {pass}  2")
        })
        .collect();
    assert_eq!(gate_lines, expected_lines);
    assert!(String::from_utf8_lossy(&text.stdout).contains("PREP_MS"));
    assert!(
        body["result"]["note"]
            .as_str()
            .unwrap()
            .contains("excludes metrics writing and subsequent Agent handling")
    );
    assert!(String::from_utf8_lossy(&text.stdout).contains("50.0%"));
    let list = command(
        &dir,
        &["routing", "metrics", "list", "--limit", "1", "--json"],
    );
    let body: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(body["result"].as_array().unwrap().len(), 1);
    assert_eq!(body["result"][0]["id"], "r2");
    assert_eq!(body["result"][0]["answers"][0]["subject_id"], "inbox_a");
    assert!(
        command(&dir, &["routing", "metrics", "label", "r1", "incorrect"])
            .status
            .success()
    );
    let report = command(&dir, &["routing", "metrics", "report", "--json"]);
    let body: Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(body["result"]["totals"][0]["reviewed_accuracy"], 0.0);
    assert!(
        !command(&dir, &["routing", "metrics", "label", "missing", "correct"])
            .status
            .success()
    );
    assert!(!dir.path().join("missing.toml").exists());
}

#[tokio::test]
async fn metrics_rejects_unknown_schema_before_reading_or_labeling() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("metrics.sqlite");
    let mut db = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true),
    )
    .await
    .unwrap();
    sqlx::raw_sql("PRAGMA user_version=99; PRAGMA application_id=0x544A4556; CREATE TABLE requests(id TEXT, review TEXT); INSERT INTO requests VALUES ('r1',NULL);").execute(&mut db).await.unwrap();
    db.close().await.unwrap();
    let before = std::fs::read(&path).unwrap();
    for args in [
        vec!["routing", "metrics", "report", "--json"],
        vec!["routing", "metrics", "label", "r1", "correct", "--json"],
    ] {
        let output = command(&dir, &args);
        assert!(!output.status.success());
        let body: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("Unsupported Jev metrics schema")
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[tokio::test]
async fn metrics_groups_decision_kinds_and_questions_without_join_inflation() {
    let dir = TempDir::new().unwrap();
    let mut db = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(dir.path().join("metrics.sqlite"))
            .create_if_missing(true),
    )
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../plugins/taskix-manager/metrics-schema.sql"
    ))
    .execute(&mut db)
    .await
    .unwrap();
    for (id, kind, accepted, review) in [
        ("r1", "routing", 1, Some("correct")),
        ("r2", "routing", 0, None),
        ("r3", "completion", 1, Some("incorrect")),
        ("r4", "completion", 0, None),
        ("r5", "recovery", 0, None),
    ] {
        sqlx::query("INSERT INTO requests (id, started_at, model, threshold, duration_ms, called, accepted, action, answer_count, review, kind) VALUES (?, 1, 'jev-test', 0.65, 10, 1, ?, 'agent', 0, ?, ?)")
            .bind(id).bind(accepted).bind(review).bind(kind).execute(&mut db).await.unwrap();
    }
    sqlx::raw_sql(
        "INSERT INTO answers VALUES ('r1','inbox_0',NULL,'match',0.99,0.99,0.99,1,NULL);
        INSERT INTO answers VALUES ('r1','inbox_1',NULL,'match',0.2,0.2,0.1,1,'low_confidence');
        INSERT INTO answers VALUES ('r2','inbox_0',NULL,NULL,NULL,NULL,NULL,0,'invalid_answer');",
    )
    .execute(&mut db)
    .await
    .unwrap();
    db.close().await.unwrap();
    let before = std::fs::read(dir.path().join("metrics.sqlite")).unwrap();
    let output = command(&dir, &["routing", "metrics", "report", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let kinds = body["result"]["by_kind"].as_array().unwrap();
    let routing = kinds.iter().find(|v| v["kind"] == "routing").unwrap();
    assert_eq!(routing["requests"], 2);
    assert_eq!(routing["adoption_rate"], 0.5);
    assert_eq!(routing["reviewed_accuracy"], 1.0);
    let completion = kinds.iter().find(|v| v["kind"] == "completion").unwrap();
    assert_eq!(completion["requests"], 2);
    assert_eq!(completion["reviewed_accuracy"], 0.0);
    let questions = body["result"]["by_question"].as_array().unwrap();
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0]["question"], "inbox");
    assert_eq!(questions[0]["answers"], 3);
    assert_eq!(questions[0]["passed"], 1);
    assert_eq!(
        std::fs::read(dir.path().join("metrics.sqlite")).unwrap(),
        before
    );
}

#[tokio::test]
async fn metrics_legacy_read_is_compatible_and_full_summary_fits_a_screen() {
    let dir = TempDir::new().unwrap();
    let mut db = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(dir.path().join("metrics.sqlite"))
            .create_if_missing(true),
    )
    .await
    .unwrap();
    let legacy = include_str!("../../../plugins/taskix-manager/metrics-schema.sql")
        .replace("PRAGMA user_version=2", "PRAGMA user_version=1")
        .replace(", kind TEXT NOT NULL DEFAULT 'routing'", "");
    sqlx::raw_sql(&legacy).execute(&mut db).await.unwrap();
    sqlx::raw_sql("INSERT INTO requests VALUES ('old',1,NULL,NULL,NULL,'jev',0.65,1,1,1,'new_job',NULL,'correct',0)")
        .execute(&mut db).await.unwrap();
    db.close().await.unwrap();
    let path = dir.path().join("metrics.sqlite");
    let before = std::fs::read(&path).unwrap();
    let output = command(&dir, &["routing", "metrics", "report", "--json"]);
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["result"]["by_kind"][0]["kind"], "routing");
    assert_eq!(body["result"]["by_kind"][0]["reviewed_accuracy"], 1.0);
    assert_eq!(std::fs::read(&path).unwrap(), before);

    let mut db = sqlx::SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    sqlx::raw_sql("ALTER TABLE requests ADD COLUMN kind TEXT NOT NULL DEFAULT 'routing'; PRAGMA user_version=2")
        .execute(&mut db).await.unwrap();
    for kind in [
        "routing",
        "discussion",
        "recovery",
        "outcome",
        "review_policy",
        "completion",
    ] {
        sqlx::query("INSERT INTO requests VALUES (?,1,NULL,NULL,NULL,'jev',0.65,1,1,1,'selected',NULL,NULL,0,?)")
            .bind(kind).bind(kind).execute(&mut db).await.unwrap();
    }
    for question in [
        "intent",
        "route",
        "review_policy",
        "recovery",
        "outcome",
        "completion",
        "turn_0",
    ] {
        sqlx::query("INSERT INTO answers VALUES ('routing',?,NULL,'keep',0.9,0.9,0.9,1,NULL)")
            .bind(question)
            .execute(&mut db)
            .await
            .unwrap();
    }
    for i in 0..100 {
        sqlx::query("INSERT INTO answers VALUES ('routing',?,NULL,'match',0.9,0.9,0.9,1,NULL)")
            .bind(format!("inbox_{i}"))
            .execute(&mut db)
            .await
            .unwrap();
    }
    db.close().await.unwrap();
    let output = command(&dir, &["routing", "metrics", "report"]);
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.lines().count() <= 23, "{text}");
    assert!(text.lines().all(|line| line.len() <= 80), "{text}");
    for kind in [
        "routing",
        "discussion",
        "recovery",
        "outcome",
        "review_policy",
        "completion",
    ] {
        assert!(text.contains(kind), "{text}");
    }
}

fn assert_compact_report(dir: &TempDir) {
    let compact = command(dir, &["routing", "metrics", "report"]);
    let compact = String::from_utf8_lossy(&compact.stdout);
    assert!(compact.contains(&format!(
        "{:<13} {:>8} {:>7} {:>8} {:>7} {:>8.1}",
        "routing", 2, 2, 1, "50.0%", 6.0
    )));
    assert!(compact.lines().count() <= 20);
    assert!(!compact.contains("Score gates"));
}
