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
    sqlx::raw_sql("        INSERT INTO requests VALUES ('r1', 1, 's', 't', 'p', 'jev-test', 0.9, 5.0, 1, 1, 'new_job', NULL, NULL, 1);
        INSERT INTO answers VALUES ('r1', 'route', NULL, 'new_job', 0.99, 0.99, 0.98, 1, NULL);
        INSERT INTO requests VALUES ('r2', 2, 's', 't2', 'p', 'jev-test', 0.9, 7.0, 1, 0, 'agent', 'uncertain_or_conflicting', NULL, 2);
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
    let text = command(&dir, &["routing", "metrics", "report"]);
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
