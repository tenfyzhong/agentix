use super::incremental::connection;
use super::*;
use agentix_task::{BrowseScope, TaskStatus};

fn status_rank(status: TaskStatus) -> usize {
    match status {
        TaskStatus::InProgress => 0,
        TaskStatus::Blocked => 1,
        TaskStatus::WaitingUser => 2,
        TaskStatus::Todo => 3,
        TaskStatus::Failed => 4,
        TaskStatus::Done => 5,
        TaskStatus::Cancelled => 6,
    }
}

#[tokio::test]
async fn task_pages_match_full_scope_order_counts_and_lease_expiry() {
    let f = Fixture::new("markdown").await;
    let seed = f.task("Current task").await;
    f.claim(&seed, "target").await;
    let mut conn = connection(&f).await;
    sqlx::query("UPDATE tasks SET data=json_set(data,'$.position',999) WHERE id=?")
        .bind(&seed)
        .execute(&mut conn)
        .await
        .unwrap();
    for (index, status) in TaskStatus::ALL.into_iter().cycle().take(14).enumerate() {
        sqlx::query("INSERT INTO tasks(id,data) SELECT ?,json_set(data,'$.id',?,'$.status',?,
            '$.title',?,'$.position',?,'$.reason','Reason','$.last_session',NULL) FROM tasks WHERE id=?")
            .bind(format!("task_page_{index}")).bind(format!("task_page_{index}"))
            .bind(status.to_string()).bind(format!("Task {index}"))
            .bind(i64::try_from(index / 2).unwrap()).bind(&seed).execute(&mut conn).await.unwrap();
    }
    for expired in [false, true] {
        if expired {
            f.clock.fetch_add(901, Ordering::SeqCst);
        }
        for scope in [
            BrowseScope::Project("demo"),
            BrowseScope::Session("target"),
            BrowseScope::Job(&f.job[..12]),
        ] {
            let state = f.service.store().browse_snapshot(scope).await.unwrap();
            let mut expected: Vec<_> = state.tasks.iter().collect();
            if matches!(scope, BrowseScope::Job(_)) {
                expected.sort_by_key(|task| (task.position, &task.id));
            } else {
                expected.sort_by_key(|task| {
                    (
                        status_rank(task.status),
                        expired || task.id != seed,
                        task.position,
                        &task.id,
                    )
                });
            }
            for requested in [0, 1, 3, usize::MAX] {
                let result = f
                    .service
                    .store()
                    .browse_task_page(scope, Some("target"), requested, 4, 1)
                    .await
                    .unwrap();
                assert_eq!(
                    (result.total, result.job_count, result.pages, result.page),
                    (15, 1, 4, requested.min(3))
                );
                let expected: Vec<_> = expected.iter().skip(result.page * 4).take(4).collect();
                assert_eq!(result.tasks.len(), expected.len());
                for (item, task) in result.tasks.iter().zip(expected) {
                    assert_eq!(
                        (&item.id, &item.title, item.status, item.phase, &item.reason),
                        (&task.id, &task.title, task.status, task.phase, &task.reason)
                    );
                    assert_eq!(item.current, !expired && task.id == seed);
                }
                for (status, count) in result.status_counts {
                    assert_eq!(
                        count,
                        state
                            .tasks
                            .iter()
                            .filter(|task| task.status == status)
                            .count()
                    );
                }
            }
        }
    }
    // Extra Markdown pages must not repeat the final task buttons.
    let result = f
        .service
        .store()
        .browse_task_page(BrowseScope::Job(&f.job), None, usize::MAX, 4, 8)
        .await
        .unwrap();
    assert_eq!((result.page, result.pages, result.total), (7, 8, 15));
    assert!(result.tasks.is_empty());
}

#[tokio::test]
async fn task_pages_preserve_archive_visibility_empty_pages_and_resolution() {
    let f = Fixture::new("markdown").await;
    let task = f.task("Target").await;
    f.claim(&task, "target").await;
    let mut conn = connection(&f).await;
    for table in ["projects", "jobs"] {
        sqlx::query(&format!(
            "UPDATE {table} SET data=json_set(data,'$.archived_at',1)"
        ))
        .execute(&mut conn)
        .await
        .unwrap();
        for scope in [BrowseScope::Project("demo"), BrowseScope::Session("target")] {
            let result = f
                .service
                .store()
                .browse_task_page(scope, Some("target"), usize::MAX, 6, 1)
                .await
                .unwrap();
            assert_eq!(
                (result.total, result.job_count, result.page, result.pages),
                (0, 0, 0, 1)
            );
            assert!(result.tasks.is_empty());
            if matches!(scope, BrowseScope::Project(_)) {
                assert_eq!(result.project.unwrap().id, f.project);
            }
        }
        let jobs = f
            .service
            .store()
            .session_job_page("target", 0, 6)
            .await
            .unwrap();
        assert_eq!(jobs.total, 0);
        let detail = f
            .service
            .store()
            .browse_task_page(BrowseScope::Job(&f.job), None, 0, 6, 1)
            .await
            .unwrap();
        assert_eq!(detail.total, 1);
        sqlx::query(&format!(
            "UPDATE {table} SET data=json_remove(data,'$.archived_at')"
        ))
        .execute(&mut conn)
        .await
        .unwrap();
    }
    assert!(
        f.service
            .store()
            .browse_task_page(BrowseScope::Project("unknown"), None, 0, 6, 1)
            .await
            .is_err()
    );
    assert!(
        f.service
            .store()
            .browse_task_page(BrowseScope::Session("target"), None, 0, 0, 1)
            .await
            .is_err()
    );
    let empty = f
        .service
        .store()
        .session_job_page("unknown", usize::MAX, 6)
        .await
        .unwrap();
    assert_eq!((empty.total, empty.page, empty.pages), (0, 0, 1));
    assert!(empty.jobs.is_empty());
}

#[tokio::test]
async fn session_job_pages_keep_insertion_order_and_only_read_selected_titles() {
    let f = Fixture::new("markdown").await;
    let seed = f.task("Target").await;
    f.claim(&seed, "target").await;
    let mut conn = connection(&f).await;
    for index in (0..20).rev() {
        let job = format!("job_page_{index:02}");
        let task = format!("task_page_{index:02}");
        sqlx::query("INSERT INTO jobs(id,data) SELECT ?,json_set(data,'$.id',?,'$.title',?) FROM jobs WHERE id=?")
            .bind(&job).bind(&job).bind(format!("Job {index}")).bind(&f.job).execute(&mut conn).await.unwrap();
        sqlx::query("INSERT INTO tasks(id,data) SELECT ?,json_set(data,'$.id',?,'$.job_id',?) FROM tasks WHERE id=?")
            .bind(&task).bind(&task).bind(&job).bind(&seed).execute(&mut conn).await.unwrap();
    }
    let expected = f
        .service
        .store()
        .browse_snapshot(BrowseScope::Session("target"))
        .await
        .unwrap();
    for requested in [0, 1, 2, 3, usize::MAX] {
        let result = f
            .service
            .store()
            .session_job_page("target", requested, 6)
            .await
            .unwrap();
        assert_eq!(
            (result.total, result.page, result.pages),
            (21, requested.min(3), 4)
        );
        for (item, job) in result
            .jobs
            .iter()
            .zip(expected.jobs.iter().skip(result.page * 6).take(6))
        {
            assert_eq!(
                (&item.id, &item.title, item.status, item.task_count),
                (&job.id, &job.title, job.status, 1)
            );
        }
    }
    sqlx::query("UPDATE jobs SET data=json_remove(data,'$.title') WHERE id IN (SELECT id FROM jobs ORDER BY rowid LIMIT -1 OFFSET 6)")
        .execute(&mut conn).await.unwrap();
    sqlx::query("UPDATE tasks SET data=json_remove(data,'$.title')")
        .execute(&mut conn)
        .await
        .unwrap();
    let result = f
        .service
        .store()
        .session_job_page("target", 0, 6)
        .await
        .unwrap();
    assert_eq!(result.jobs.len(), 6);
    assert_eq!(result.total, 21);
    assert!(result.jobs.iter().all(|job| job.task_count == 1));
}
