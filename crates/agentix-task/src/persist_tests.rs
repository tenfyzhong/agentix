use super::*;

thread_local! {
    pub(crate) static VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

async fn fixture() -> (tempfile::TempDir, Store, Snapshot) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    let project = store
        .execute(
            json!({"command":"project.register","root":dir.path(),"name":"Test"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let job = store
        .execute(
            json!({"command":"job.create","project":project["id"],"title":"Job"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    store
        .execute(
            json!({"command":"task.add","job":job["id"],"title":"Task"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let state = store.snapshot().await.unwrap();
    (dir, store, state)
}

#[tokio::test]
async fn schema_fifteen_event_retention_database_upgrades_with_memory_sources() {
    assert_schema_fifteen_upgrade(false).await;
}

#[tokio::test]
async fn schema_fifteen_memory_database_upgrades_with_event_watermarks() {
    assert_schema_fifteen_upgrade(true).await;
}

#[tokio::test]
async fn schema_sixteen_upgrades_to_protect_project_document_locations() {
    let (dir, store, snapshot) = fixture().await;
    sqlx::query("PRAGMA user_version=16")
        .execute(&store.pool)
        .await
        .unwrap();
    store.pool.close().await;
    store.maintenance_pool.close().await;
    let reopened = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&reopened.pool)
        .await
        .unwrap();
    assert_eq!(version, 17);
    assert_eq!(
        reopened.snapshot().await.unwrap().projects,
        snapshot.projects
    );
}

async fn assert_schema_fifteen_upgrade(has_memory: bool) {
    let (dir, store, snapshot) = fixture().await;
    store.set_background_maintenance(false);
    let project = &snapshot.projects[0].id;
    store.execute(
        json!({"command":"session.record","project":project,"session":"migration","turn_id":"decision","messages":[{"id":"user","role":"user","text":"Keep the external deployment decision"}]}),
        WriteOptions { session_ref: Some("migration".into()), ..WriteOptions::default() },
    ).await.unwrap();
    let sources = store.memory_sources(0, 20).await.unwrap();
    assert_eq!(sources.len(), 1);
    let instance = store.memory_source_instance().await.unwrap();
    let sequence = store.latest_sequence().await.unwrap();
    let project_sequence: i64 =
        sqlx::query_scalar("SELECT sequence FROM event_watermarks WHERE scope=?")
            .bind(project)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    let drops = if has_memory {
        "DROP TRIGGER event_watermark_insert; DROP TABLE event_watermarks; DROP TABLE event_retention; DROP INDEX events_by_age;"
    } else {
        "DROP TABLE memory_source_outbox; DROP TABLE memory_source_heads; DROP TABLE memory_source_identity;"
    };
    sqlx::raw_sql(&format!("{drops} PRAGMA user_version=15;"))
        .execute(&store.pool)
        .await
        .unwrap();
    store.pool.close().await;
    store.maintenance_pool.close().await;
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    store.set_background_maintenance(false);
    assert_eq!(store.snapshot().await.unwrap().jobs, snapshot.jobs);
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(version, 17);
    assert!(
        store.event_policy(None, None, None).await.unwrap()["enabled"]
            .as_bool()
            .unwrap()
    );
    assert_eq!(store.latest_sequence().await.unwrap(), sequence);
    let restored_sequence: i64 =
        sqlx::query_scalar("SELECT sequence FROM event_watermarks WHERE scope=?")
            .bind(project)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert_eq!(restored_sequence, project_sequence);
    let new_instance = store.memory_source_instance().await.unwrap();
    assert!(!new_instance.is_empty());
    if has_memory {
        assert_eq!(new_instance, instance);
        assert_eq!(
            serde_json::to_value(store.memory_sources(0, 20).await.unwrap()).unwrap(),
            serde_json::to_value(sources).unwrap()
        );
    } else {
        assert!(store.memory_sources(0, 20).await.unwrap().is_empty());
    }
    let read_only = Store::open_read_only(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    assert_eq!(
        read_only.project_result(project).await.unwrap().id,
        *project
    );
}

#[tokio::test]
async fn wide_unchanged_state_does_not_rescan_entities_for_each_row() {
    let (_dir, store, mut state) = fixture().await;
    let job = state.jobs[0].clone();
    let task = state.tasks[0].clone();
    for index in 0..1000 {
        let mut job = job.clone();
        job.id = format!("job_wide_{index}");
        let mut task = task.clone();
        task.id = format!("task_wide_{index}");
        task.job_id.clone_from(&job.id);
        state.jobs.push(job);
        state.tasks.push(task);
    }
    let sequence = store.latest_sequence().await.unwrap();
    let mut tx = store.pool.begin().await.unwrap();
    VISITS.set(0);
    persist(
        &mut tx,
        &state,
        &state,
        "project.archive",
        &WriteOptions::default(),
        store.now(),
    )
    .await
    .unwrap();
    let visits = VISITS.get();
    eprintln!("Wide unchanged-state entity visits: {visits}");
    assert!(visits > 0, "work counter must include index construction");
    tx.commit().await.unwrap();
    assert_eq!(store.latest_sequence().await.unwrap(), sequence);
    assert!(
        visits < state.tasks.len() * 16,
        "unchanged state comparison visited {visits} identifiers for {} tasks",
        state.tasks.len()
    );
}

#[tokio::test]
async fn indexed_changes_keep_event_order_and_latest_session_ties() {
    use crate::{JobStatus, TaskStatus};
    for explicit in [None, Some("explicit")] {
        let (_dir, store, mut before) = fixture().await;
        for (index, session, updated) in
            [(1, Some("first"), 10), (2, Some("last"), 10), (3, None, 20)]
        {
            let mut task = before.tasks[0].clone();
            task.id = format!("task_event_{index}");
            task.last_session = session.map(str::to_owned);
            task.updated_at = updated;
            before.tasks.push(task);
        }
        before.jobs[0].status = JobStatus::PendingReview;
        let mut after = before.clone();
        after.jobs[0].status = JobStatus::Active;
        after.jobs[0].revision += 1;
        after.tasks[0].status = TaskStatus::Blocked;
        after.tasks[0].revision += 1;
        let sequence = store.latest_sequence().await.unwrap();
        let options = WriteOptions {
            session_ref: explicit.map(str::to_owned),
            ..WriteOptions::default()
        };
        let mut tx = store.pool.begin().await.unwrap();
        persist(
            &mut tx,
            &before,
            &after,
            "inbox.set-status",
            &options,
            store.now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let events = store.events(None, sequence, 10).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "job.rejected");
        assert_eq!(
            events[0].session_ref.as_deref(),
            Some(explicit.unwrap_or("last"))
        );
        assert_eq!(events[1].event_type, "task.blocked");
        assert_eq!(
            events[1].task_id.as_deref(),
            Some(after.tasks[0].id.as_str())
        );
        assert_eq!(events[1].session_ref.as_deref(), explicit);
    }
}

#[tokio::test]
async fn events_do_not_duplicate_job_bodies_and_keep_notification_fields() {
    let (_dir, store, state) = fixture().await;
    let job = &state.jobs[0];
    store.execute(json!({"command":"job.update","job":job.id,"prompt":"large prompt".repeat(1000),"title":"Notice title"}), WriteOptions::default()).await.unwrap();
    let events = store.events(Some(&job.id), 0, 100).await.unwrap();
    let event = events.last().unwrap();
    assert_eq!(event.payload["title"], "Notice title");
    assert!(
        event.payload.get("prompt").is_none(),
        "events must not duplicate authored bodies"
    );
    assert!(event.payload.get("conversation").is_none());
    assert!(serde_json::to_vec(event).unwrap().len() < 2048);
    assert_eq!(store.snapshot().await.unwrap().jobs[0].prompt.len(), 12000);
}

#[tokio::test]
async fn deleting_retained_events_does_not_regress_global_or_project_receipts() {
    let (_dir, store, state) = fixture().await;
    let latest = store.latest_sequence().await.unwrap();
    let receipt = store.project_receipt(&state.projects[0]).await.unwrap();
    sqlx::query("DELETE FROM task_events")
        .execute(&store.pool)
        .await
        .unwrap();
    assert_eq!(store.latest_sequence().await.unwrap(), latest);
    assert_eq!(
        store.project_receipt(&state.projects[0]).await.unwrap(),
        receipt
    );
    let outcome = store
        .execute(
            json!({"command":"job.update","job":state.jobs[0].id,"title":"Later"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(outcome.sequence > latest);
}
