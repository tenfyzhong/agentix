use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn batch_activity_seeks_maxima_without_scanning_task_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("tasks.sqlite3"))
        .await
        .unwrap();
    let mut conn = store.pool.acquire().await.unwrap();
    sqlx::query("INSERT INTO projects(id,data) VALUES('p',json_object('root','/p'))")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO jobs(id,data) VALUES('j',json_object('project_id','p','updated_at',1))",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("WITH RECURSIVE n(value) AS (VALUES(1) UNION ALL SELECT value+1 FROM n WHERE value<10000)
        INSERT INTO tasks(id,data) SELECT 't'||value,json_object('id','t'||value,'project_id','p','job_id','j','updated_at',value) FROM n")
        .execute(&mut *conn).await.unwrap();
    let steps = Arc::new(AtomicUsize::new(0));
    let measured = steps.clone();
    conn.lock_handle()
        .await
        .unwrap()
        .set_progress_handler(100, move || {
            measured.fetch_add(100, Ordering::Relaxed);
            true
        });
    let row = sqlx::query(PROJECT_ACTIVITY_QUERY)
        .bind("[\"p\"]")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    conn.lock_handle().await.unwrap().remove_progress_handler();
    assert_eq!(row.get::<i64, _>("updated_at"), 10000);
    assert!(
        steps.load(Ordering::Relaxed) < 1000,
        "indexed maxima should not scan 10000 Tasks: {} VM steps",
        steps.load(Ordering::Relaxed)
    );
}
