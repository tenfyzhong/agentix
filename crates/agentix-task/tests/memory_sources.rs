use agentix_task::{Store, WriteOptions};
use serde_json::{Value, json};

fn options() -> WriteOptions {
    WriteOptions {
        session_ref: Some("memory-session".into()),
        actor_ref: "agent:codex".into(),
        ..WriteOptions::default()
    }
}

async fn project(store: &Store, root: &std::path::Path) -> Value {
    store
        .execute(
            json!({"command":"project.register","name":"Memory","root":root}),
            options(),
        )
        .await
        .unwrap()
        .result["id"]
        .clone()
}

fn capture(project: &Value, text: &str) -> Value {
    json!({"command":"session.record","project":project,"session":"memory-session",
        "turn_id":"decision","messages":[{"id":"user","role":"user","text":text}]})
}

#[tokio::test]
async fn sources_are_immutable_versioned_and_acknowledged_individually() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let p = project(&store, dir.path()).await;
    store
        .execute(capture(&p, "Use local storage for offline use"), options())
        .await
        .unwrap();
    store
        .execute(capture(&p, "Use local storage for offline use"), options())
        .await
        .unwrap();
    store
        .execute(
            capture(&p, "Use remote storage after deployment changes"),
            options(),
        )
        .await
        .unwrap();
    let page = store.memory_sources(0, 10).await.unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].revision, 1);
    assert_eq!(page[1].revision, 2);
    assert_eq!(
        page[0].messages[0]["text"],
        "Use local storage for offline use"
    );
    assert_eq!(page[0].project_id, p.as_str().unwrap());
    assert_eq!(page[0].instance_id, page[1].instance_id);
    assert!(page[0].job_id.is_none());
    store
        .acknowledge_memory_source(&page[1].instance_id, &page[1].receipt_id)
        .await
        .unwrap();
    assert_eq!(store.memory_sources(0, 10).await.unwrap().len(), 1);
    assert!(
        store
            .acknowledge_memory_source("wrong-instance", &page[0].receipt_id)
            .await
            .is_err()
    );
    drop(store);
    let store = Store::open(&path).await.unwrap();
    let pending = store.memory_sources(0, 10).await.unwrap();
    assert_eq!(pending[0].instance_id, page[0].instance_id);
    assert_eq!(pending[0].sequence, page[0].sequence);
}

#[tokio::test]
async fn invalid_project_and_conflicting_reassignment_roll_back_capture() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    assert!(
        store
            .execute(capture(&json!("absent"), "Decision"), options())
            .await
            .is_err()
    );
    let p = project(&store, dir.path()).await;
    store
        .execute(capture(&p, "Decision"), options())
        .await
        .unwrap();
    let other = project(&store, &dir.path().join("other")).await;
    assert!(
        store
            .execute(capture(&other, "Changed"), options())
            .await
            .is_err()
    );
    let pending = store.memory_sources(0, 10).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].messages[0]["text"], "Decision");
}

#[tokio::test]
async fn unknown_ownership_never_uses_the_latest_job() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    let p = project(&store, dir.path()).await;
    store
        .execute(
            json!({"command":"job.create","project":p,"title":"Other delivery"}),
            options(),
        )
        .await
        .unwrap();
    store
        .execute(capture(&Value::Null, "Unassigned discussion"), options())
        .await
        .unwrap();
    assert!(store.memory_sources(0, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn attachment_assigns_sources_atomically_and_rejects_foreign_project() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    let p = project(&store, dir.path()).await;
    store
        .execute(
            capture(&Value::Null, "Decision before registration"),
            options(),
        )
        .await
        .unwrap();
    let job = store
        .execute(
            json!({"command":"job.create","project":p,"title":"Delivery",
        "conversation_turns":["decision"]}),
            options(),
        )
        .await
        .unwrap()
        .result;
    let pending = store.memory_sources(0, 10).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].job_id.as_deref(), job["id"].as_str());
    store
        .execute(capture(&Value::Null, "Late correction"), options())
        .await
        .unwrap();
    assert_eq!(store.memory_sources(0, 10).await.unwrap().len(), 2);

    let other = project(&store, &dir.path().join("other")).await;
    let mut next = capture(&p, "Keep project ownership");
    next["turn_id"] = json!("next");
    store.execute(next, options()).await.unwrap();
    assert!(
        store
            .execute(
                json!({"command":"job.create","project":other,"title":"Wrong",
        "conversation_turns":["next"]}),
                options()
            )
            .await
            .is_err()
    );
    assert_eq!(store.snapshot().await.unwrap().jobs.len(), 1);
}

#[tokio::test]
async fn source_evidence_survives_discussion_expiry_and_migration_is_idempotent() {
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks.sqlite3");
    let now = Arc::new(AtomicI64::new(1_800_000_000));
    let clock = now.clone();
    let store = Store::open_with_clock(&path, Arc::new(move || clock.load(Ordering::SeqCst)))
        .await
        .unwrap();
    let p = project(&store, dir.path()).await;
    store
        .execute(capture(&p, "Keep evidence"), options())
        .await
        .unwrap();
    let first = store.memory_sources(0, 10).await.unwrap();
    now.fetch_add(31 * 24 * 60 * 60, Ordering::SeqCst);
    store
        .execute(
            json!({"command":"session.start","session":"memory-session"}),
            options(),
        )
        .await
        .unwrap();
    assert!(
        store
            .discussion_list("memory-session", 0, 10)
            .await
            .unwrap()["turns"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.memory_sources(0, 10).await.unwrap()[0].messages,
        first[0].messages
    );
    drop(store);
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", path.display()))
        .await
        .unwrap();
    sqlx::query("PRAGMA user_version=14")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let reopened = Store::open(&path).await.unwrap();
    let sources = reopened.memory_sources(0, 10).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].instance_id, first[0].instance_id);
}

#[tokio::test]
async fn restored_source_cannot_acknowledge_a_different_event_at_reused_sequence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let p = project(&store, dir.path()).await;
    let restored_path = dir.path().join("restored.sqlite3");
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", path.display()))
        .await
        .unwrap();
    sqlx::query("VACUUM INTO ?")
        .bind(restored_path.to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    store
        .execute(capture(&p, "Original branch of history"), options())
        .await
        .unwrap();
    let old = store.memory_sources(0, 10).await.unwrap().remove(0);
    let restored = Store::open(&restored_path).await.unwrap();
    restored
        .execute(
            capture(&p, "Different decision after restoration"),
            options(),
        )
        .await
        .unwrap();
    let new = restored.memory_sources(0, 10).await.unwrap().remove(0);
    assert_eq!(old.instance_id, new.instance_id);
    assert_eq!(old.sequence, new.sequence);
    assert_ne!(old.receipt_id, new.receipt_id);
    assert!(
        restored
            .acknowledge_memory_source(&old.instance_id, &old.receipt_id)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn legacy_history_backfill_is_scoped_paged_and_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    let p = project(&store, dir.path()).await;
    let job = store
        .execute(
            json!({"command":"job.create","project":p,"title":"Legacy"}),
            options(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    store
        .execute(
            json!({"command":"session.record","job":job,"session":"memory-session","messages":[
                {"id":"a","role":"user","text":"First confirmed constraint"},
                {"id":"b","role":"assistant","text":"Acknowledged"},
                {"id":"c","role":"user","text":"Another decision"}
            ]}),
            options(),
        )
        .await
        .unwrap();
    assert!(store.memory_sources(0, 10).await.unwrap().is_empty());
    let first = store
        .backfill_memory_job(p.as_str().unwrap(), &job, 0, 2)
        .await
        .unwrap();
    assert_eq!(first.next_offset, 2);
    assert!(!first.complete);
    store
        .backfill_memory_job(p.as_str().unwrap(), &job, 0, 2)
        .await
        .unwrap();
    let second = store
        .backfill_memory_job(p.as_str().unwrap(), &job, 2, 2)
        .await
        .unwrap();
    assert_eq!(second.next_offset, 3);
    assert!(second.complete);
    let sources = store.memory_sources(0, 10).await.unwrap();
    assert_eq!(sources.len(), 3);
    assert_eq!(sources[0].messages[0]["text"], "First confirmed constraint");
    assert!(sources.iter().all(|s| s.job_id.as_deref() == Some(&job)));
    let other = project(&store, &dir.path().join("other")).await;
    assert!(
        store
            .backfill_memory_job(other.as_str().unwrap(), &job, 0, 2)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn schema_fourteen_upgrade_preserves_history_without_automatic_backfill() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let p = project(&store, dir.path()).await;
    let job = store
        .execute(
            json!({"command":"job.create","project":p,"title":"Before memory"}),
            options(),
        )
        .await
        .unwrap()
        .result;
    let before = store.snapshot().await.unwrap();
    drop(store);
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", path.display()))
        .await
        .unwrap();
    sqlx::raw_sql("DROP TABLE memory_source_outbox; DROP TABLE memory_source_heads; DROP TABLE memory_source_identity; PRAGMA user_version=14;")
        .execute(&pool).await.unwrap();
    pool.close().await;
    let upgraded = Store::open(&path).await.unwrap();
    assert_eq!(upgraded.snapshot().await.unwrap().jobs, before.jobs);
    assert!(upgraded.memory_sources(0, 10).await.unwrap().is_empty());
    assert_eq!(
        upgraded.snapshot().await.unwrap().jobs[0].id,
        job["id"].as_str().unwrap()
    );
    upgraded
        .execute(capture(&p, "New decision"), options())
        .await
        .unwrap();
    assert_eq!(upgraded.memory_sources(0, 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn directory_hints_cannot_reassign_late_messages_to_another_project() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    let first = project(&store, dir.path()).await;
    let second = project(&store, other.path()).await;
    let mut request = capture(&first, "Original decision");
    request.as_object_mut().unwrap().remove("project");
    request["project_hint"] = first.clone();
    store.execute(request.clone(), options()).await.unwrap();
    request["project_hint"] = second;
    request["messages"][0]["text"] = json!("Late clarified decision");
    store.execute(request, options()).await.unwrap();
    let sources = store.memory_sources(0, 100).await.unwrap();
    assert_eq!(sources.len(), 2);
    assert!(
        sources
            .iter()
            .all(|s| s.project_id == first.as_str().unwrap())
    );
}

#[tokio::test]
async fn recovery_replays_acknowledged_receipts_and_validates_the_source_instance() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    let p = project(&store, dir.path()).await;
    store
        .execute(capture(&p, "Offline recovery"), options())
        .await
        .unwrap();
    let source = store.memory_sources(0, 1).await.unwrap().remove(0);
    store
        .acknowledge_memory_source(&source.instance_id, &source.receipt_id)
        .await
        .unwrap();
    assert!(store.memory_sources(0, 1).await.unwrap().is_empty());
    let replay = store.replay_memory_sources(0, 1).await.unwrap();
    assert_eq!(replay[0].receipt_id, source.receipt_id);
    assert_eq!(
        store.memory_source_instance().await.unwrap(),
        source.instance_id
    );
    store.verify_memory_sources(&replay).await.unwrap();
    let mut forged = source;
    forged.messages[0]["text"] = json!("Forked history");
    assert!(store.verify_memory_sources(&[forged]).await.is_err());
}

#[tokio::test]
async fn read_only_project_lookup_does_not_migrate_or_wait_for_a_writer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let p = project(&store, dir.path()).await;
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    let _writer = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let reader = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        Store::open_read_only(&path),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        reader.project_result(p.as_str().unwrap()).await.unwrap().id,
        p.as_str().unwrap()
    );
    assert!(
        reader
            .execute(capture(&p, "read-only cannot write"), options())
            .await
            .is_err()
    );
    assert!(
        Store::open_read_only(&dir.path().join("missing.sqlite3"))
            .await
            .is_err()
    );
}
