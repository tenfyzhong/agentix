//! Read-only aggregation and terminal presentation for every recorded decision kind.
use super::{row_json, rows};
use anyhow::Result;
use serde_json::{Value, json};
use sqlx::SqliteConnection;

pub(super) async fn report(db: &mut SqliteConnection, path: &std::path::Path) -> Result<Value> {
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
        CASE WHEN question GLOB 'turn_[0-9]*' THEN 'discussion' ELSE question END AS question,
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
