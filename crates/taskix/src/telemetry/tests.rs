use super::*;
use agentix_metrics::{Metrics, Service};
use sqlx::{Connection, Executor, SqliteConnection, sqlite::SqliteConnectOptions};
use tempfile::tempdir;

#[tokio::test]
async fn missing_sources_are_not_created_by_collection() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("absent.db");
    let mut collector = Collector::new(
        path.clone(),
        path.clone(),
        path.clone(),
        Metrics::new(Service::Taskix, "test"),
    );
    assert!(collector.refresh_tasks().await.is_err());
    assert!(collector.refresh_memory().await.is_err());
    assert!(collector.refresh_jev().await.is_err());
    assert!(!path.exists());
}

#[tokio::test]
async fn task_snapshot_observes_external_writes_and_omits_identifiers() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("tasks.db");
    let _store = agentix_task::Store::open(&path).await.unwrap();
    let mut db = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    db.execute("INSERT INTO tasks(id,data) VALUES('secret-task','{\"status\":\"BLOCKED\",\"updated_at\":100}')").await.unwrap();
    let metrics = Metrics::new(Service::Taskix, "test");
    let mut collector = Collector::new(path.clone(), path.clone(), path, metrics.clone());
    collector.refresh_tasks().await.unwrap();
    let text = metrics.encode().unwrap();
    assert!(
        text.contains("taskix_entities{kind=\"task\",state=\"blocked\"} 1"),
        "{text}"
    );
    assert!(!text.contains("secret-task"));
    db.execute("UPDATE tasks SET data='{\"status\":\"DONE\",\"updated_at\":200}'")
        .await
        .unwrap();
    collector.refresh_tasks().await.unwrap();
    assert!(
        metrics
            .encode()
            .unwrap()
            .contains("taskix_entities{kind=\"task\",state=\"done\"} 1")
    );
    let mut readonly = read_only(&collector.tasks).await.unwrap();
    assert!(readonly.execute("DELETE FROM tasks").await.is_err());
}

#[tokio::test]
async fn incremental_jev_does_not_replay_history_or_double_count_and_reads_late_reviews() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("jev.db");
    let mut db = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true),
    )
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../plugins/taskix-manager/metrics-schema.sql"
    ))
    .execute(&mut db)
    .await
    .unwrap();
    let insert = "INSERT INTO requests(id,started_at,model,threshold,duration_ms,called,accepted,action,answer_count,kind) VALUES(?,1,'private-model',0.65,25,1,1,'new_job',0,'routing')";
    sqlx::query(insert)
        .bind("old")
        .execute(&mut db)
        .await
        .unwrap();
    let metrics = Metrics::new(Service::Taskix, "test");
    let mut collector = Collector::new(path.clone(), path.clone(), path, metrics.clone());
    collector.refresh_jev().await.unwrap();
    assert!(
        !metrics
            .encode()
            .unwrap()
            .contains("taskix_jev_decisions_total")
    );
    sqlx::query(insert)
        .bind("new")
        .execute(&mut db)
        .await
        .unwrap();
    collector.refresh_jev().await.unwrap();
    collector.refresh_jev().await.unwrap();
    let text = metrics.encode().unwrap();
    assert!(
        text.contains("taskix_jev_decisions_total{kind=\"routing\",outcome=\"accepted\"} 1"),
        "{text}"
    );
    assert!(text.contains("# TYPE taskix_jev_duration_seconds histogram"));
    assert!(text.contains("taskix_jev_duration_seconds_bucket{kind=\"routing\",le=\"0.01\"} 0"));
    assert!(text.contains("taskix_jev_duration_seconds_bucket{kind=\"routing\",le=\"0.025\"} 1"));
    assert!(text.contains("taskix_jev_duration_seconds_bucket{kind=\"routing\",le=\"+Inf\"} 1"));
    assert!(text.contains("taskix_jev_duration_seconds_sum{kind=\"routing\"} 0.025"));
    assert!(text.contains("taskix_jev_duration_seconds_count{kind=\"routing\"} 1"));
    assert!(!text.contains("private-model"));
    for (id, duration) in [
        ("zero", 0.0),
        ("boundary", 100.0),
        ("overflow", 4_000_000.0),
        ("invalid", -1.0),
    ] {
        sqlx::query(insert).bind(id).execute(&mut db).await.unwrap();
        sqlx::query("UPDATE requests SET duration_ms=? WHERE id=?")
            .bind(duration)
            .bind(id)
            .execute(&mut db)
            .await
            .unwrap();
    }
    collector.refresh_jev().await.unwrap();
    collector.refresh_jev().await.unwrap();
    let histogram = metrics.encode().unwrap();
    for sample in [
        "taskix_jev_duration_seconds_bucket{kind=\"routing\",le=\"0.01\"} 1",
        "taskix_jev_duration_seconds_bucket{kind=\"routing\",le=\"0.025\"} 2",
        "taskix_jev_duration_seconds_bucket{kind=\"routing\",le=\"0.1\"} 3",
        "taskix_jev_duration_seconds_bucket{kind=\"routing\",le=\"3600\"} 3",
        "taskix_jev_duration_seconds_bucket{kind=\"routing\",le=\"+Inf\"} 4",
        "taskix_jev_duration_seconds_count{kind=\"routing\"} 4",
    ] {
        assert!(histogram.contains(sample), "{sample}: {histogram}");
    }
    if let Ok(path) = std::env::var("JEV_METRICS_FIXTURE_FILE") {
        std::fs::write(path, histogram).unwrap();
    }

    db.execute("UPDATE requests SET review='incorrect' WHERE id='new'")
        .await
        .unwrap();
    collector.refresh_jev().await.unwrap();
    assert!(
        metrics
            .encode()
            .unwrap()
            .contains("taskix_jev_reviewed{kind=\"routing\",result=\"incorrect\"} 1")
    );
}

#[tokio::test]
async fn memory_snapshot_has_bounded_states_and_no_source_text() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let _store = agentix_memory::MemoryStore::open(&path).await.unwrap();
    let mut db = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    db.execute("INSERT INTO sources VALUES('receipt','project','instance','{}')")
        .await
        .unwrap();
    db.execute("INSERT INTO work_items(project_id,receipt_id,kind,state,payload,available_at,attempts) VALUES('project','receipt','extract','pending','{\"text\":\"private\"}',1,2)").await.unwrap();
    let metrics = Metrics::new(Service::Taskix, "test");
    let collector = Collector::new(path.clone(), path.clone(), path, metrics.clone());
    collector.refresh_memory().await.unwrap();
    let text = metrics.encode().unwrap();
    assert!(
        text.contains("taskix_memory_work_items{kind=\"extract\",state=\"pending\"} 1"),
        "{text}"
    );
    assert!(!text.contains("private"));
    assert!(!text.contains("project=\""));
}

#[test]
fn taskix_metrics_configuration_has_its_own_default_port_and_board_compatibility() {
    let dir = tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".obsidian")).unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path,format!("schema_version=1\n[storage]\npath={:?}\n[documents]\nroot={:?}\ndirectory='Projects'\n[metrics]\nenabled=true\n",dir.path().join("tasks.db"),dir.path())).unwrap();
    let config = load_config(&path).unwrap();
    assert!(config.enabled);
    assert_eq!(config.listen.port(), 9465);
    agentix_task::Config::load(&path).unwrap();
}

#[tokio::test]
async fn sqlite_progress_budget_interrupts_expensive_reads() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("tasks.db");
    let _store = agentix_task::Store::open(&path).await.unwrap();
    let mut db = read_only(&path).await.unwrap();
    let result=tokio::time::timeout(Duration::from_secs(2),sqlx::query_scalar::<_,i64>("WITH RECURSIVE numbers(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM numbers WHERE n<1000000000) SELECT sum(n) FROM numbers").fetch_one(&mut db)).await;
    assert!(
        result
            .expect("SQLite must interrupt before the outer timeout")
            .is_err()
    );
}

#[tokio::test]
async fn pruned_history_at_startup_does_not_disable_current_state_snapshots() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("tasks.db");
    let _store = agentix_task::Store::open(&path).await.unwrap();
    let mut db = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    db.execute("UPDATE event_retention SET pruned_through=100 WHERE id=1")
        .await
        .unwrap();
    let metrics = Metrics::new(Service::Taskix, "test");
    let mut collector = Collector::new(path.clone(), path.clone(), path, metrics.clone());
    collector.refresh_tasks().await.unwrap();
    collector.refresh_tasks().await.unwrap();
    assert!(metrics.encode().unwrap().contains("taskix_entities"));
}

#[tokio::test]
async fn jev_v1_and_database_replacement_rebaseline_without_replay() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("jev.db");
    let mut db = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true),
    )
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../plugins/taskix-manager/metrics-schema.sql"
    ))
    .execute(&mut db)
    .await
    .unwrap();
    db.execute("ALTER TABLE requests DROP COLUMN kind")
        .await
        .unwrap();
    db.execute("PRAGMA user_version=1").await.unwrap();
    let insert = "INSERT INTO requests(id,started_at,model,threshold,duration_ms,called,accepted,action,answer_count) VALUES(?,1,'private-model',0.65,25,1,1,'new_job',0)";
    sqlx::query(insert)
        .bind("original")
        .execute(&mut db)
        .await
        .unwrap();
    let metrics = Metrics::new(Service::Taskix, "test");
    let mut collector = Collector::new(path.clone(), path.clone(), path, metrics.clone());
    collector.refresh_jev().await.unwrap();
    db.execute("DELETE FROM requests").await.unwrap();
    sqlx::query(insert)
        .bind("replacement")
        .execute(&mut db)
        .await
        .unwrap();
    assert!(collector.refresh_jev().await.is_err());
    collector.refresh_jev().await.unwrap();
    assert!(
        !metrics
            .encode()
            .unwrap()
            .contains("taskix_jev_decisions_total")
    );
    sqlx::query(insert)
        .bind("next")
        .execute(&mut db)
        .await
        .unwrap();
    collector.refresh_jev().await.unwrap();
    assert!(
        metrics
            .encode()
            .unwrap()
            .contains("taskix_jev_decisions_total{kind=\"routing\",outcome=\"accepted\"} 1")
    );
}

#[tokio::test]
async fn empty_memory_queues_publish_zero_and_unknown_states_are_aggregated() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let _store = agentix_memory::MemoryStore::open(&path).await.unwrap();
    let metrics = Metrics::new(Service::Taskix, "test");
    let collector = Collector::new(path.clone(), path.clone(), path, metrics.clone());
    collector.refresh_memory().await.unwrap();
    assert!(
        metrics
            .encode()
            .unwrap()
            .contains("taskix_memory_work_items{kind=\"extract\",state=\"pending\"} 0")
    );
    assert!(
        metrics
            .encode()
            .unwrap()
            .contains("taskix_memory_pending_embeddings 0")
    );
}

#[tokio::test]
async fn replaced_task_database_rebaselines_even_with_same_event_head() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("tasks.db");
    let _store = agentix_task::Store::open(&path).await.unwrap();
    let metrics = Metrics::new(Service::Taskix, "test");
    let mut collector = Collector::new(path.clone(), path.clone(), path.clone(), metrics.clone());
    collector.refresh_tasks().await.unwrap();
    let mut db = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path))
        .await
        .unwrap();
    db.execute("UPDATE memory_source_identity SET instance_id='replacement'")
        .await
        .unwrap();
    assert!(collector.refresh_tasks().await.is_err());
    assert!(!metrics.encode().unwrap().contains("taskix_entities"));
    collector.refresh_tasks().await.unwrap();
    assert!(metrics.encode().unwrap().contains("taskix_entities"));
}

#[tokio::test]
#[ignore = "manual paired SQLite contention benchmark; no developer databases"]
async fn application_snapshot_contention_benchmark() {
    let sample_size: usize =
        std::env::var("TASKIX_METRICS_BENCH_WRITES").map_or(2000, |value| value.parse().unwrap());
    assert!(sample_size >= 100);
    let dir = tempdir().unwrap();
    let path = dir.path().join("tasks.db");
    let _store = agentix_task::Store::open(&path).await.unwrap();
    let mut writer = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    writer.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) INSERT INTO tasks(id,data) SELECT 'task_'||x,'{\"status\":\"IN_PROGRESS\",\"updated_at\":100}' FROM n").await.unwrap();
    let mut samples = Vec::new();
    for round in 0..7 {
        for enabled in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let metrics = Metrics::new(Service::Taskix, "benchmark");
            let mut collector = Collector::new(path.clone(), path.clone(), path.clone(), metrics);
            let collection = enabled.then(|| {
                tokio::spawn(async move {
                    // One snapshot overlaps each measured burst, more often than the 60s production cadence.
                    collector.refresh_tasks().await.unwrap();
                })
            });
            let mut timings = Vec::new();
            let total = std::time::Instant::now();
            for index in 0..sample_size {
                let at = std::time::Instant::now();
                sqlx::query("UPDATE tasks SET data=json_set(data,'$.updated_at',?) WHERE id=?")
                    .bind(
                        round * 1_000_000_i64
                            + i64::from(enabled) * 100_000
                            + i64::try_from(index).unwrap()
                            + 1,
                    )
                    .bind(format!("task_{}", index % 10_000 + 1))
                    .execute(&mut writer)
                    .await
                    .unwrap();
                timings.push(at.elapsed().as_nanos());
            }
            let seconds = total.elapsed().as_secs_f64();
            if let Some(collection) = collection {
                collection.await.unwrap();
            }
            timings.sort_unstable();
            samples.push(serde_json::json!({"enabled":enabled,"seconds":seconds,"writes_per_second":f64::from(u32::try_from(sample_size).unwrap())/seconds,"p95_ns":timings[sample_size * 95 / 100]}));
        }
    }
    println!(
        "SNAPSHOT_BENCHMARK {}",
        serde_json::json!({"profile":if cfg!(debug_assertions) {"debug"} else {"release"},"rows":10000,"writes_per_sample":sample_size,"samples":samples})
    );
}

#[tokio::test]
async fn entity_aggregation_avoids_per_row_case_conversion() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let mut db = SqliteConnection::connect("sqlite::memory:").await.unwrap();
    db.execute("CREATE TABLE tasks(data TEXT NOT NULL)")
        .await
        .unwrap();
    db.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000) INSERT INTO tasks SELECT json_object('status',CASE x%3 WHEN 0 THEN 'TODO' WHEN 1 THEN 'todo' ELSE 'Todo' END,'updated_at',x) FROM n").await.unwrap();
    let steps = Arc::new(AtomicUsize::new(0));
    let counted = steps.clone();
    db.lock_handle()
        .await
        .unwrap()
        .set_progress_handler(1, move || {
            counted.fetch_add(1, Ordering::Relaxed);
            true
        });
    let reference = sqlx::query("SELECT lower(json_extract(data,'$.status')) AS state,count(*) AS count,min(COALESCE(json_extract(data,'$.updated_at'),json_extract(data,'$.created_at'),?)) AS oldest FROM tasks GROUP BY 1")
        .bind(2000_i64).fetch_all(&mut db).await.unwrap();
    let before = steps.swap(0, Ordering::Relaxed);
    let groups = entity_groups(&mut db, "tasks", 2000).await.unwrap();
    let after = steps.load(Ordering::Relaxed);
    println!("ENTITY_VM_STEPS before={before} after={after}");
    assert_eq!(groups["todo"], (1000, 1));
    assert_eq!(reference[0].get::<i64, _>("count"), groups["todo"].0);
    assert!(
        after < before,
        "scan VM operations must decrease: {before} -> {after}"
    );
}

#[tokio::test]
#[ignore = "manual SQL aggregation measurement"]
async fn entity_aggregation_benchmark() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("tasks.db");
    let _store = agentix_task::Store::open(&path).await.unwrap();
    let mut db = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    sqlx::query("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) INSERT INTO tasks(id,data) SELECT 'task_'||x,json_object('status',CASE x%10 WHEN 0 THEN 'IN_PROGRESS' ELSE 'DONE' END,'updated_at',x,'title',?) FROM n").bind("x".repeat(2048)).execute(&mut db).await.unwrap();
    let before = "SELECT lower(json_extract(data,'$.status')) AS state,count(*) AS count,min(COALESCE(json_extract(data,'$.updated_at'),json_extract(data,'$.created_at'),?)) AS oldest FROM tasks GROUP BY 1";
    let mut samples = Vec::new();
    for round in 0..7 {
        for optimized in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let started = std::time::Instant::now();
            for _ in 0..10 {
                if optimized {
                    let groups = entity_groups(&mut db, "tasks", 20000).await.unwrap();
                    assert_eq!(groups["done"], (9000, 1));
                    assert_eq!(groups["in_progress"], (1000, 10));
                } else {
                    let rows = sqlx::query(before)
                        .bind(20000_i64)
                        .fetch_all(&mut db)
                        .await
                        .unwrap();
                    let mut groups: BTreeMap<&str, (i64, i64)> =
                        STATES.iter().map(|s| (*s, (0, 20000))).collect();
                    for row in rows {
                        let state: String = row.get("state");
                        let group = groups.get_mut(normalized(&state, STATES)).unwrap();
                        group.0 += row.get::<i64, _>("count");
                        group.1 = group.1.min(row.get("oldest"));
                    }
                    assert_eq!(groups["done"], (9000, 1));
                    assert_eq!(groups["in_progress"], (1000, 10));
                }
            }
            samples.push(serde_json::json!({"optimized":optimized,"round":round,"snapshots":10,"elapsed_ns":started.elapsed().as_nanos()}));
        }
    }
    println!(
        "ENTITY_BENCHMARK {}",
        serde_json::json!({"rows":10000,"title_bytes":2048,"samples":samples})
    );
}

#[tokio::test]
async fn entity_groups_merge_unknown_and_null_statuses_without_exposing_them() {
    let mut db = SqliteConnection::connect("sqlite::memory:").await.unwrap();
    db.execute("CREATE TABLE tasks(data TEXT NOT NULL)")
        .await
        .unwrap();
    db.execute(r#"INSERT INTO tasks VALUES('{"status":1,"updated_at":20}'),('{"status":"PRIVATE","updated_at":10}'),('{"created_at":30}')"#).await.unwrap();
    let groups = entity_groups(&mut db, "tasks", 100).await.unwrap();
    assert_eq!(groups["other"], (3, 10));
    assert_eq!(groups.len(), STATES.len());
}

#[tokio::test]
async fn decisions_group_by_provider_protocol_and_keep_legacy_typesafe_only() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("decisions.db");
    let mut db = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true),
    )
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../plugins/taskix-manager/metrics-schema.sql"
    ))
    .execute(&mut db)
    .await
    .unwrap();
    let metrics = Metrics::new(Service::Taskix, "test");
    let mut collector = Collector::new(path.clone(), path.clone(), path, metrics.clone());
    collector.refresh_jev().await.unwrap();
    for (id, provider, protocol) in [
        ("a", "typesafe", "typesafe"),
        ("b", "openai", "openai_decisions"),
        ("c", "acme", "openai_decisions"),
    ] {
        sqlx::query("INSERT INTO requests(id,started_at,model,threshold,duration_ms,called,accepted,action,answer_count,kind,provider,protocol,review) VALUES(?,1,'model',.65,25,1,1,'new_job',0,'routing',?,?,'correct')").bind(id).bind(provider).bind(protocol).execute(&mut db).await.unwrap();
    }
    collector.refresh_jev().await.unwrap();
    collector.refresh_jev().await.unwrap();
    let text = metrics.encode().unwrap();
    for (provider, protocol) in [
        ("typesafe", "typesafe"),
        ("openai", "openai_decisions"),
        ("acme", "openai_decisions"),
    ] {
        assert!(text.contains(&format!("taskix_decision_decisions_total{{provider=\"{provider}\",protocol=\"{protocol}\",kind=\"routing\",outcome=\"accepted\"}} 1")), "{text}");
        assert!(text.contains(&format!("taskix_decision_duration_seconds_bucket{{provider=\"{provider}\",protocol=\"{protocol}\",kind=\"routing\",le=\"0.025\"}} 1")), "{text}");
        assert!(text.contains(&format!("taskix_decision_reviewed{{provider=\"{provider}\",protocol=\"{protocol}\",kind=\"routing\",result=\"correct\"}} 1")), "{text}");
    }
    assert!(
        text.contains("taskix_jev_decisions_total{kind=\"routing\",outcome=\"accepted\"} 1"),
        "{text}"
    );
    assert_eq!(
        text.matches("# TYPE taskix_decision_decisions_total counter")
            .count(),
        1
    );
    assert!(!text.contains("model="));
}

#[test]
fn decision_metric_groups_are_bounded_without_losing_builtin_identities() {
    let mut groups = BTreeSet::new();
    for i in 0..MAX_DECISION_GROUPS {
        assert_eq!(
            decision_group(&format!("custom_{i}"), "typesafe", &mut groups).0,
            format!("custom_{i}")
        );
    }
    assert_eq!(
        decision_group("overflow", "openai_decisions", &mut groups),
        ("other".into(), "other".into())
    );
    assert_eq!(
        decision_group("typesafe", "typesafe", &mut groups),
        ("typesafe".into(), "typesafe".into())
    );
    assert_eq!(
        decision_group("openai", "openai_decisions", &mut groups),
        ("openai".into(), "openai_decisions".into())
    );
    assert_eq!(
        decision_group("bad\"label", "typesafe", &mut groups),
        ("other".into(), "other".into())
    );
    assert_eq!(
        decision_group("acme", "unknown", &mut groups),
        ("other".into(), "other".into())
    );
}
