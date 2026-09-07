use super::*;
use std::{fs, time::SystemTime};

async fn connection(f: &Fixture) -> sqlx::SqliteConnection {
    use sqlx::Connection;
    sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&f.service.config().storage.path),
    )
    .await
    .unwrap()
}

async fn other_job(f: &Fixture) -> (String, String) {
    let job = f
        .service
        .execute(
            json!({"command":"job.create","project":f.project,"title":"Unrelated"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let task = f
        .service
        .execute(
            json!({"command":"task.add","job":job,"title":"Other task"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (job, task)
}

#[tokio::test]
async fn status_write_does_not_deserialize_unrelated_records() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Target").await;
    let (job, other) = other_job(&f).await;
    let mut conn = connection(&f).await;
    // Keep valid JSON and indexed relationships, but make unrelated domain rows
    // unreadable. Neither the transaction nor its projection needs their bodies.
    sqlx::query("UPDATE tasks SET data=json_remove(data,'$.title') WHERE id=?")
        .bind(other)
        .execute(&mut conn)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET data=json_remove(data,'$.title') WHERE id=?")
        .bind(job)
        .execute(&mut conn)
        .await
        .unwrap();
    let outcome = f
        .service
        .execute(
            json!({"command":"task.block","task":task,"reason":"Needs input"}),
            WriteOptions::default(),
        )
        .await;
    assert!(
        outcome.is_ok(),
        "unrelated rows must not be read: {outcome:?}"
    );
    let outcome = outcome.unwrap();
    assert_eq!(outcome.result["status"], "BLOCKED");
    assert!(
        outcome.projection_pending.is_none(),
        "{:?}",
        outcome.projection_pending
    );
}

#[tokio::test]
async fn status_write_leaves_unrelated_documents_and_dynamic_views_untouched() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Target").await;
    let (_, other) = other_job(&f).await;
    let note = f.service.obsidian_note(&other).await.unwrap();
    let path = f
        .service
        .config()
        .documents
        .root
        .join(note["path"].as_str().unwrap());
    // An unrelated document may be temporarily malformed in an open editor.
    fs::write(&path, "---\nunclosed: [\n---\nHuman draft\n").unwrap();
    let views = ["Dashboard.base", "Recent Jobs.base"];
    for relative in views {
        // Windows requires a writable handle to update file timestamps.
        fs::File::options()
            .write(true)
            .open(f.service.config().output_dir().join(relative))
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
            .unwrap();
    }
    let outcome = f
        .service
        .execute(
            json!({"command":"task.block","task":task,"reason":"Needs input"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        outcome.projection_pending.is_none(),
        "{:?}",
        outcome.projection_pending
    );
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "---\nunclosed: [\n---\nHuman draft\n"
    );
    for relative in views {
        assert_eq!(
            fs::metadata(f.service.config().output_dir().join(relative))
                .unwrap()
                .modified()
                .unwrap(),
            SystemTime::UNIX_EPOCH
        );
    }
}

#[tokio::test]
async fn job_approval_preserves_registered_bases_saved_without_comments() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Finish").await;
    let claim = f.start(&task, "base-review").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    let views = ["Dashboard.base", "Recent Jobs.base"];
    let saved: Vec<_> = views
        .iter()
        .map(|relative| {
            let path = f.service.config().output_dir().join(relative);
            let value: Value = serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            // Obsidian can serialize view settings without retaining generated comments.
            let contents = serde_yaml::to_string(&value).unwrap();
            fs::write(&path, &contents).unwrap();
            // Windows requires a writable handle to update file timestamps.
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
                .unwrap();
            (path, contents)
        })
        .collect();
    let outcome = f
        .service
        .execute(
            json!({"command":"job.approve","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(outcome.result["status"], "COMPLETED");
    assert!(
        outcome.projection_pending.is_none(),
        "{:?}",
        outcome.projection_pending
    );
    assert!(!f.service.store().has_pending_documents().await.unwrap());
    for (path, contents) in &saved {
        assert_eq!(&fs::read_to_string(path).unwrap(), contents);
        assert_eq!(
            fs::metadata(path).unwrap().modified().unwrap(),
            SystemTime::UNIX_EPOCH
        );
    }
    f.service.sync().await.unwrap();
    for (path, _) in saved {
        assert!(
            fs::read_to_string(path)
                .unwrap()
                .starts_with("# taskcli-generated:")
        );
    }
}

#[tokio::test]
async fn incremental_base_sync_recreates_missing_views_and_protects_unmanaged_files() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Target").await;
    let path = f.service.config().output_dir().join("Recent Jobs.base");
    fs::remove_file(&path).unwrap();
    let outcome = f
        .service
        .execute(
            json!({"command":"task.block","task":task,"reason":"Input"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(outcome.projection_pending.is_none());
    assert!(path.exists());
    let mut conn = connection(&f).await;
    sqlx::query("DELETE FROM document_registry WHERE key='pending-review'")
        .execute(&mut conn)
        .await
        .unwrap();
    fs::write(&path, "views: []\n").unwrap();
    let outcome = f
        .service
        .execute(
            json!({"command":"task.update","task":task,"title":"Changed"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        outcome
            .projection_pending
            .unwrap()
            .contains("unmanaged document")
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "views: []\n");
    fs::remove_file(&path).unwrap();
    f.service.sync_pending_documents().await.unwrap();
    assert!(path.exists());
    assert!(!f.service.store().has_pending_documents().await.unwrap());
}

#[tokio::test]
async fn claiming_one_task_does_not_rewrite_other_leases() {
    let f = Fixture::new("obsidian").await;
    let a = f.task("A").await;
    let b = f.task("B").await;
    let held = f.claim(&a, "first").await;
    let mut conn = connection(&f).await;
    sqlx::raw_sql("CREATE TRIGGER preserve_lease BEFORE DELETE ON task_leases BEGIN SELECT RAISE(ABORT, 'unrelated lease deleted'); END;")
        .execute(&mut conn).await.unwrap();
    let outcome = f
        .service
        .execute(
            json!({"command":"task.claim","task":b,"executor":"agent:second","session":"second"}),
            WriteOptions::default(),
        )
        .await;
    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(
        f.service.store().task_result(&a).await.unwrap()["lease"],
        held["lease"]
    );
}

#[tokio::test]
async fn next_write_recovers_committed_projection_after_restart() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Target").await;
    f.service
        .store()
        .execute(
            json!({"command":"task.block","task":task,"reason":"Pending publication"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let service = Service::open(f.service.config().clone()).await.unwrap();
    let outcome = service
        .execute(
            json!({"command":"job.update","job":f.job,"title":"Updated"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(outcome.projection_pending.is_none());
    let note = service.obsidian_note(&task).await.unwrap();
    let contents = fs::read_to_string(
        service
            .config()
            .documents
            .root
            .join(note["path"].as_str().unwrap()),
    )
    .unwrap();
    let properties: Value = serde_yaml::from_str(contents.split("---").nth(1).unwrap()).unwrap();
    assert_eq!(properties["status"], "BLOCKED");
}

#[tokio::test]
async fn creation_and_rename_read_names_without_loading_unrelated_bodies() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Target").await;
    let (job, other) = other_job(&f).await;
    let mut conn = connection(&f).await;
    sqlx::query("UPDATE tasks SET data=json_remove(data,'$.title') WHERE id=?")
        .bind(other)
        .execute(&mut conn)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET data=json_remove(data,'$.title') WHERE id=?")
        .bind(job)
        .execute(&mut conn)
        .await
        .unwrap();
    for request in [
        json!({"command":"task.update","task":task,"name":"Other task"}),
        json!({"command":"task.add","job":f.job,"title":"Other task"}),
        json!({"command":"job.update","job":f.job,"name":"Unrelated"}),
        json!({"command":"job.create","project":f.project,"title":"Unrelated"}),
    ] {
        let outcome = f
            .service
            .execute(request.clone(), WriteOptions::default())
            .await;
        assert!(outcome.is_ok(), "{request}: {outcome:?}");
        let outcome = outcome.unwrap();
        assert!(
            outcome.projection_pending.is_none(),
            "{:?}",
            outcome.projection_pending
        );
        assert!(
            outcome.result["name"].as_str().unwrap().contains('-'),
            "collision must still be detected: {:?}",
            outcome.result
        );
    }
}

#[tokio::test]
async fn a_write_never_changes_another_projects_board_receipt() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Target").await;
    f.service
        .execute(
            json!({"command":"project.register","name":"other","root":f.dir.path().join("other")}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let path = f
        .service
        .config()
        .output_dir()
        .join("Projects/other/Board.md");
    let before = fs::read_to_string(&path).unwrap();
    f.clock.fetch_add(10, Ordering::SeqCst);
    f.service
        .execute(
            json!({"command":"task.block","task":task,"reason":"Blocked"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    // A repair must produce the same per-project receipt as incremental writes.
    f.service.sync().await.unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
}

#[tokio::test]
async fn failed_backlog_does_not_prevent_other_documents_from_publishing() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Target").await;
    let (job, _) = other_job(&f).await;
    let note = f.service.obsidian_note(&job).await.unwrap();
    let bad = f
        .service
        .config()
        .documents
        .root
        .join(note["path"].as_str().unwrap());
    fs::write(&bad, "---\nbroken: [\n---\n").unwrap();
    f.service
        .store()
        .execute(
            json!({"command":"job.update","job":job,"title":"Pending"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let outcome = f
        .service
        .execute(
            json!({"command":"task.block","task":task,"reason":"Publish independently"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(outcome.projection_pending.is_some());
    let note = f.service.obsidian_note(&task).await.unwrap();
    let source = fs::read_to_string(
        f.service
            .config()
            .documents
            .root
            .join(note["path"].as_str().unwrap()),
    )
    .unwrap();
    let properties: Value = serde_yaml::from_str(source.split("---").nth(1).unwrap()).unwrap();
    assert_eq!(properties["status"], "BLOCKED");
}

#[tokio::test]
async fn acknowledging_an_older_generation_preserves_a_newer_publication() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Target").await;
    let mut conn = connection(&f).await;
    sqlx::query(&format!("CREATE TRIGGER concurrent_publication AFTER UPDATE ON document_registry WHEN NEW.key='task:{task}' BEGIN UPDATE pending_documents SET generation='newer' WHERE key=NEW.key; END"))
        .execute(&mut conn).await.unwrap();
    f.service
        .execute(
            json!({"command":"task.block","task":task,"reason":"Changed"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let generation: String =
        sqlx::query_scalar("SELECT generation FROM pending_documents WHERE key=?")
            .bind(format!("task:{task}"))
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(generation, "newer");
    sqlx::query("DROP TRIGGER concurrent_publication")
        .execute(&mut conn)
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"job.update","job":f.job,"title":"Retry pending"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pending_documents")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn readiness_checks_do_not_deserialize_sibling_tasks() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Target").await;
    let other = f.task("Incomplete sibling").await;
    let claim = f.start(&task, "worker").await;
    let mut conn = connection(&f).await;
    sqlx::query("UPDATE tasks SET data=json_remove(data,'$.title') WHERE id=?")
        .bind(other)
        .execute(&mut conn)
        .await
        .unwrap();
    f.service
        .store()
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    let status: String =
        sqlx::query_scalar("SELECT json_extract(data,'$.status') FROM jobs WHERE id=?")
            .bind(&f.job)
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(status, "ACTIVE");
}

#[tokio::test]
async fn job_events_keep_the_related_session_with_scoped_reads() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Target").await;
    let claim = f.start(&task, "worker").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    f.approve().await;
    let events = f.service.store().events(None, 0, 1000).await.unwrap();
    let event = events
        .iter()
        .find(|e| e.event_type == "job.completed")
        .unwrap();
    assert_eq!(event.session_ref.as_deref(), Some("worker"));
}

#[tokio::test]
async fn session_heartbeat_does_not_reload_completed_work() {
    let f = Fixture::new("obsidian").await;
    let historical = f.task("Historical").await;
    let active = f.task("Active").await;
    let claim = f.start(&historical, "worker").await;
    f.service
        .execute(
            json!({"command":"task.done","task":historical}),
            owner(&claim),
        )
        .await
        .unwrap();
    f.start(&active, "worker").await;
    let mut conn = connection(&f).await;
    sqlx::query("UPDATE tasks SET data=json_remove(data,'$.title') WHERE id=?")
        .bind(historical)
        .execute(&mut conn)
        .await
        .unwrap();
    let outcome = f
        .service
        .execute(
            json!({"command":"session.heartbeat","session":"worker"}),
            WriteOptions::default(),
        )
        .await;
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[tokio::test]
#[ignore = "explicit large-database acceptance test"]
async fn status_write_with_one_hundred_thousand_unrelated_tasks() {
    use sqlx::Connection;
    let f = Fixture::new("obsidian").await;
    let task = f.task("Target").await;
    let mut conn = connection(&f).await;
    let mut tx = conn.begin().await.unwrap();
    // Construct the large fixture in SQLite, without allocating a Rust snapshot.
    sqlx::query("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) INSERT INTO jobs(id,data) SELECT 'job_scale_'||x,json_set(json_remove(j.data,'$.title'),'$.id','job_scale_'||x,'$.name','Unrelated '||x) FROM n,jobs j WHERE j.id=?")
        .bind(&f.job).execute(&mut *tx).await.unwrap();
    sqlx::query("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) INSERT INTO tasks(id,data) SELECT 'task_scale_'||x,json_set(json_remove(t.data,'$.title'),'$.id','task_scale_'||x,'$.job_id','job_scale_'||CAST((x+9)/10 AS INTEGER),'$.name','Unrelated '||x) FROM n,tasks t WHERE t.id=?")
        .bind(&task).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let started = std::time::Instant::now();
    let outcome = f
        .service
        .execute(
            json!({"command":"task.block","task":task,"reason":"Scale check"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    eprintln!(
        "Status write with 100,000 unrelated Tasks: {:?}",
        started.elapsed()
    );
    assert!(
        outcome.projection_pending.is_none(),
        "{:?}",
        outcome.projection_pending
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM document_registry WHERE key LIKE 'task:%'")
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(count, 1, "unrelated Tasks must not be projected");
}
