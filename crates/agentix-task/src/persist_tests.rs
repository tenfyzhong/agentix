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
