//! Opt-in measurements always copy an offline backup into a temporary directory.
use crate::{Store, WriteOptions};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Row};
use std::{path::PathBuf, time::Instant};

async fn digest(db: &mut sqlx::SqliteConnection, exclude: &str) -> Vec<u8> {
    let mut hash = Sha256::new();
    for table in [
        "projects",
        "jobs",
        "tasks",
        "plans",
        "task_leases",
        "inbox_entries",
    ] {
        for row in sqlx::query(&format!("SELECT data FROM {table} WHERE id!=? AND COALESCE(json_extract(data,'$.project_id'),'')!=? ORDER BY id")).bind(exclude).bind(exclude).fetch_all(&mut *db).await.unwrap() {
            hash.update(row.get::<String, _>("data"));
        }
    }
    hash.finalize().to_vec()
}
fn percentiles(mut samples: Vec<u128>) -> Value {
    samples.sort_unstable();
    json!({"samples":samples.len(),"p50_us":samples[samples.len()/2],"p95_us":samples[samples.len()*95/100],"p99_us":samples[samples.len()*99/100],"max_us":samples.last()})
}
async fn writes(store: &Store, job: &str, prefix: &str) -> Value {
    let mut samples = Vec::new();
    for i in 0..200 {
        let started = Instant::now();
        store
            .execute(
                json!({"command":"job.update","job":job,"title":format!("{prefix}-{i}")}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        samples.push(started.elapsed().as_micros());
    }
    percentiles(samples)
}

#[tokio::test]
#[ignore = "requires TASKIX_BENCHMARK_BACKUP pointing to an offline SQLite backup"]
async fn real_backup_retention_and_foreground_latency() {
    let source =
        PathBuf::from(std::env::var_os("TASKIX_BENCHMARK_BACKUP").expect("offline backup path"));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("copy.sqlite3");
    std::fs::copy(&source, &path).unwrap();
    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(&path);
    let mut db = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let before_hash = digest(&mut db, "").await;
    let before_bytes = std::fs::metadata(&path).unwrap().len();
    db.close().await.unwrap();
    let start = Instant::now();
    let store = Store::open(&path).await.unwrap();
    store.set_background_maintenance(false);
    let migration_ms = start.elapsed().as_millis();
    let mut db = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    assert_eq!(digest(&mut db, "").await, before_hash);
    let original_events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_events")
        .fetch_one(&mut db)
        .await
        .unwrap();
    let project = store
        .execute(
            json!({"command":"project.register","root":dir.path(),"name":"Benchmark"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let job = store
        .execute(
            json!({"command":"job.create","project":project,"title":"Measured job"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let baseline = writes(&store, &job, "baseline").await;
    let before_cleanup = digest(&mut db, &project).await;
    let start = Instant::now();
    let worker = store.clone();
    let handle = tokio::spawn(async move {
        worker.run_event_maintenance().await.unwrap();
    });
    let concurrent = writes(&store, &job, "concurrent").await;
    handle.await.unwrap();
    let cleanup_ms = start.elapsed().as_millis();
    assert_eq!(digest(&mut db, &project).await, before_cleanup);
    let check: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(check, "ok");
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&mut db)
        .await
        .unwrap();
    eprintln!(
        "{}",
        json!({"migration_ms":migration_ms,"cleanup_ms":cleanup_ms,"before_bytes":before_bytes,"after_bytes":std::fs::metadata(&path).unwrap().len(),"events":original_events,"foreground_baseline":baseline,"foreground_concurrent":concurrent,"policy":store.event_policy(None,None,None).await.unwrap(),"integrity_check":check})
    );
}
