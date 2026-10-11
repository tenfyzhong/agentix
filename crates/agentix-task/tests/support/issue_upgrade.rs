use super::*;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

async fn connection(f: &Fixture) -> SqliteConnection {
    SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(&f.service.config().storage.path),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn fresh_issue_database_has_no_retired_intake_entity() {
    let f = Fixture::new().await;
    let mut conn = connection(&f).await;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sqlite_schema WHERE name LIKE 'inbox_%'")
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(count, 0);
    let state = serde_json::to_value(f.service.store().snapshot().await.unwrap()).unwrap();
    assert!(state.get("inboxes").is_none());
    assert!(
        !f.service
            .config()
            .output_dir()
            .join("Projects/demo/Inbox.md")
            .exists()
    );
}

#[tokio::test]
async fn removed_commands_and_job_association_fields_are_rejected() {
    let f = Fixture::new().await;
    for request in [
        json!({"command":"inbox.add","project":f.project,"content":"Retired"}),
        json!({"command":"inbox.list","project":f.project}),
        json!({"command":"job.create","project":f.project,"title":"Old client","inbox_ids":[]}),
        json!({"command":"job.update","job":f.job,"inbox_ids":[]}),
    ] {
        assert!(
            f.service
                .execute(request.clone(), WriteOptions::default())
                .await
                .is_err(),
            "{request}"
        );
    }
    assert!(f.service.obsidian_note("inbox_removed").await.is_err());
}

async fn seed_retired_document(f: &Fixture) -> std::path::PathBuf {
    let mut conn = connection(f).await;
    sqlx::raw_sql("CREATE TABLE IF NOT EXISTS inbox_entries(id TEXT PRIMARY KEY, data TEXT NOT NULL); PRAGMA user_version = 22;")
        .execute(&mut conn).await.unwrap();
    sqlx::query("INSERT INTO inbox_entries(id,data) VALUES('inbox_retired',?)")
        .bind(json!({"id":"inbox_retired","project_id":f.project,"content":"Old requirement","status":"TODO"}).to_string())
        .execute(&mut conn).await.unwrap();
    let relative = "Projects/demo/Inbox.md";
    sqlx::query("INSERT OR REPLACE INTO document_registry(key,path) VALUES(?,?)")
        .bind(format!("inbox:{}", f.project))
        .bind(relative)
        .execute(&mut conn)
        .await
        .unwrap();
    let path = f.service.config().output_dir().join(relative);
    std::fs::write(&path, "# Retired generated document\n").unwrap();
    path
}

async fn reopen(f: &Fixture) -> Service {
    let now = f.clock.clone();
    Service::new(
        f.service.config().clone(),
        Store::open_with_clock(
            &f.service.config().storage.path,
            Arc::new(move || now.load(Ordering::SeqCst)),
        )
        .await
        .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn issue_upgrade_removes_registered_intake_without_converting_or_losing_work() {
    let f = Fixture::new().await;
    let task = f.task("Preserved execution").await;
    f.claim(&task, "upgrade").await;
    f.plan(&task).await;
    let before = f.service.store().snapshot().await.unwrap();
    let path = seed_retired_document(&f).await;
    let mut conn = connection(&f).await;
    let event = json!({"event_id":"event_legacy_job","event_type":"inbox.claim-next","project_id":f.project,"job_id":f.job,"task_id":null,"revision":1,"actor_ref":"agent:codex","session_ref":"upgrade","occurred_at":f.clock.load(Ordering::SeqCst),"payload":{"id":f.job,"title":"Feature"}});
    sqlx::query("INSERT INTO task_events(event_id,job_id,data) VALUES(?,?,?)")
        .bind("event_legacy_job")
        .bind(&f.job)
        .bind(event.to_string())
        .execute(&mut conn)
        .await
        .unwrap();
    let archived_relative = "Archived Projects/retired/Inbox.md";
    let archived = f.service.config().documents.root.join(archived_relative);
    std::fs::create_dir_all(archived.parent().unwrap()).unwrap();
    std::fs::write(&archived, "# Retired archive").unwrap();
    // Registry paths include documents whose Project was archived before upgrade.
    sqlx::query("INSERT INTO document_registry(key,path) VALUES('inbox:archived',?)")
        .bind(archived_relative)
        .execute(&mut conn)
        .await
        .unwrap();
    let personal = f.service.config().output_dir().join("Personal/Inbox.md");
    std::fs::create_dir_all(personal.parent().unwrap()).unwrap();
    std::fs::write(&personal, "# Personal note").unwrap();
    let service = reopen(&f).await;
    service.sync().await.unwrap();
    service.sync().await.unwrap();
    let after = service.store().snapshot().await.unwrap();
    assert_eq!(before.jobs, after.jobs);
    assert_eq!(before.tasks, after.tasks);
    assert_eq!(before.plans, after.plans);
    assert!(after.issues.is_empty());
    assert!(!path.exists());
    assert!(!archived.exists());
    let history: i64 =
        sqlx::query_scalar("SELECT count(*) FROM task_events WHERE event_id='event_legacy_job'")
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(
        history, 1,
        "preserve Job history even when the retired intake triggered it"
    );
    assert_eq!(
        std::fs::read_to_string(personal).unwrap(),
        "# Personal note"
    );
    let mut conn = connection(&f).await;
    let retired: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sqlite_schema WHERE name='inbox_entries'")
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(retired, 0);
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(version, 25);
}

#[cfg(unix)]
#[tokio::test]
async fn issue_upgrade_never_follows_a_registered_symlink_outside_the_vault() {
    let f = Fixture::new().await;
    let path = seed_retired_document(&f).await;
    let outside = f.dir.path().join("private.md");
    std::fs::write(&outside, "Private content").unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    let service = reopen(&f).await;
    assert!(service.sync().await.is_err());
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "Private content"
    );
    std::fs::remove_file(&path).unwrap();
    service.sync().await.unwrap();
    assert!(!path.exists());
    assert!(outside.exists());
}

#[tokio::test]
async fn issue_upgrade_retries_failed_registered_document_cleanup() {
    let f = Fixture::new().await;
    let path = seed_retired_document(&f).await;
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let service = reopen(&f).await;
    assert!(service.sync().await.is_err());
    let mut conn = connection(&f).await;
    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pending_documents WHERE key='rebuild'")
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(pending, 1);
    std::fs::remove_dir(&path).unwrap();
    service.sync().await.unwrap();
    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pending_documents WHERE key='rebuild'")
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(pending, 0);
    assert!(!path.exists());
}

#[tokio::test]
async fn issue_upgrade_recognizes_legacy_publication_receipts_without_path_registry() {
    let f = Fixture::new().await;
    let path = seed_retired_document(&f).await;
    let mut conn = connection(&f).await;
    sqlx::query("DELETE FROM document_registry WHERE key LIKE 'inbox:%'")
        .execute(&mut conn)
        .await
        .unwrap();
    f.service
        .store()
        .set_metadata(&format!("inbox_initialized:{}", f.project), &json!(true))
        .await
        .unwrap();
    std::fs::write(&path, format!("---\nproject_id: {}\ntags: [agent/inbox]\n---\n<!-- taskix:inbox:start project={} -->\nOld requirement\n<!-- taskix:inbox:end -->\n", f.project, f.project)).unwrap();
    let service = reopen(&f).await;
    service.sync().await.unwrap();
    assert!(
        !path.exists(),
        "old publication used an initialization receipt instead of a path registry"
    );
    assert!(
        service
            .store()
            .metadata(&format!("inbox_initialized:{}", f.project))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn issue_upgrade_preserves_user_replacement_at_a_legacy_receipt_path() {
    let f = Fixture::new().await;
    let path = seed_retired_document(&f).await;
    let mut conn = connection(&f).await;
    sqlx::query("DELETE FROM document_registry WHERE key LIKE 'inbox:%'")
        .execute(&mut conn)
        .await
        .unwrap();
    f.service
        .store()
        .set_metadata(&format!("inbox_initialized:{}", f.project), &json!(true))
        .await
        .unwrap();
    std::fs::write(&path, "# Personal replacement\nKeep it.").unwrap();
    let service = reopen(&f).await;
    service.sync().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "# Personal replacement\nKeep it."
    );
    assert!(
        service
            .store()
            .metadata(&format!("inbox_initialized:{}", f.project))
            .await
            .unwrap()
            .is_none()
    );
}
