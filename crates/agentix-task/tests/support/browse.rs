use super::incremental::connection;
use super::*;
use agentix_task::BrowseScope;

#[tokio::test]
async fn project_and_detail_scopes_ignore_other_projects_and_plan_bodies() {
    let f = Fixture::new().await;
    let task = f.task("Target").await;
    f.start(&task, "target").await;
    let sibling = f.task("Sibling").await;
    let other_root = tempfile::tempdir().unwrap();
    let project = f
        .service
        .execute(
            json!({"command":"project.register","name":"Other","root":other_root.path()}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let job = f
        .service
        .execute(
            json!({"command":"job.create","project":project,"title":"Other"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut conn = connection(&f).await;
    sqlx::query("UPDATE jobs SET data=json_remove(data,'$.title') WHERE id=?")
        .bind(job)
        .execute(&mut conn)
        .await
        .unwrap();
    sqlx::query("UPDATE plans SET data=json_remove(data,'$.hash')")
        .execute(&mut conn)
        .await
        .unwrap();
    for scope in [
        BrowseScope::Project("demo"),
        BrowseScope::Job(&f.job),
        BrowseScope::Task(&task),
    ] {
        let state = f.service.store().browse_snapshot(scope).await.unwrap();
        assert_eq!(state.projects.len(), 1);
        assert_eq!(state.projects[0].id, f.project);
        assert_eq!(state.jobs.len(), 1);
        assert_eq!(state.jobs[0].id, f.job);
        assert!(state.plans.is_empty());
        assert!(state.inboxes.is_empty());
        assert_eq!(state.leases.len(), 1);
        assert_eq!(state.leases[0].task_id, task);
    }
    let board = f
        .service
        .store()
        .browse_snapshot(BrowseScope::Project(&f.project))
        .await
        .unwrap();
    assert_eq!(
        board
            .tasks
            .iter()
            .map(|t| t.id.as_str())
            .collect::<Vec<_>>(),
        [task.as_str(), sibling.as_str()]
    );
    let detail = f
        .service
        .store()
        .browse_snapshot(BrowseScope::Task(&task))
        .await
        .unwrap();
    assert_eq!(detail.tasks.len(), 1);
    assert_eq!(detail.tasks[0].id, task);
}

#[tokio::test]
async fn session_board_keeps_expired_lease_associations_and_excludes_archives() {
    let f = Fixture::new().await;
    let task = f.task("Target").await;
    f.claim(&task, "lease-session").await;
    let mut conn = connection(&f).await;
    // Exercise the lease association independently of last_session, including
    // expiry: read-only browsing must neither reap leases nor hide history.
    sqlx::query(
        "UPDATE tasks SET data=json_set(data,'$.last_session','history-session') WHERE id=?",
    )
    .bind(&task)
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::query("UPDATE task_leases SET data=json_set(data,'$.lease_expires_at',0) WHERE id=?")
        .bind(&task)
        .execute(&mut conn)
        .await
        .unwrap();
    for session in ["lease-session", "history-session"] {
        let board = f
            .service
            .store()
            .browse_snapshot(BrowseScope::Session(session))
            .await
            .unwrap();
        assert_eq!(board.tasks.len(), 1);
        assert_eq!(board.jobs[0].id, f.job);
        assert_eq!(board.leases[0].lease_expires_at, 0);
    }
    assert!(
        f.service
            .store()
            .browse_snapshot(BrowseScope::Session("unknown"))
            .await
            .unwrap()
            .jobs
            .is_empty()
    );
    for (table, id) in [("jobs", f.job.as_str()), ("projects", f.project.as_str())] {
        sqlx::query(&format!(
            "UPDATE {table} SET data=json_set(data,'$.archived_at',1) WHERE id=?"
        ))
        .bind(id)
        .execute(&mut conn)
        .await
        .unwrap();
        assert!(
            f.service
                .store()
                .browse_snapshot(BrowseScope::Session("history-session"))
                .await
                .unwrap()
                .jobs
                .is_empty()
        );
        let project = f
            .service
            .store()
            .browse_snapshot(BrowseScope::Project("demo"))
            .await
            .unwrap();
        assert_eq!(project.projects.len(), 1);
        assert!(project.tasks.is_empty());
        // Explicit detail navigation remains available for archived work.
        assert_eq!(
            f.service
                .store()
                .browse_snapshot(BrowseScope::Job(&f.job))
                .await
                .unwrap()
                .tasks
                .len(),
            1
        );
        sqlx::query(&format!(
            "UPDATE {table} SET data=json_remove(data,'$.archived_at') WHERE id=?"
        ))
        .bind(id)
        .execute(&mut conn)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn task_detail_reads_dependency_status_without_dependency_bodies() {
    let f = Fixture::new().await;
    let dependency = f.task("Dependency").await;
    let task = f.task("Dependent").await;
    f.service
        .execute(
            json!({"command":"task.depend","task":task,"dependency":dependency}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let mut conn = connection(&f).await;
    sqlx::query("UPDATE tasks SET data=json_remove(data,'$.title') WHERE id=?")
        .bind(&dependency)
        .execute(&mut conn)
        .await
        .unwrap();
    let state = f
        .service
        .store()
        .browse_snapshot(BrowseScope::Task(&task))
        .await
        .unwrap();
    assert_eq!(state.tasks.len(), 1);
    assert!(!state.dependencies_done(&state.tasks[0]));
    sqlx::query("UPDATE tasks SET data=json_set(data,'$.status','DONE') WHERE id=?")
        .bind(&dependency)
        .execute(&mut conn)
        .await
        .unwrap();
    let state = f
        .service
        .store()
        .browse_snapshot(BrowseScope::Task(&task))
        .await
        .unwrap();
    assert_eq!(state.tasks.len(), 1);
    assert!(state.dependencies_done(&state.tasks[0]));
}
