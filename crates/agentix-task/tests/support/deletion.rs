use super::*;

#[tokio::test]
async fn job_deletion_cascades_and_preserves_other_jobs() {
    for format in ["markdown", "obsidian"] {
        for archived in [false, true] {
            let f = Fixture::new(format).await;
            let task = f.task("Delete this plan").await;
            let claim = f.start(&task, "delete").await;
            f.service
                .execute(json!({"command":"task.done","task":task}), owner(&claim))
                .await
                .unwrap();
            if archived {
                f.service
                    .execute(
                        json!({"command":"job.archive","job":f.job}),
                        WriteOptions::default(),
                    )
                    .await
                    .unwrap();
            }
            let other = f
                .service
                .execute(
                    json!({"command":"job.create","project":f.project,"title":"Keep job"}),
                    WriteOptions::default(),
                )
                .await
                .unwrap()
                .result;
            f.service
                .execute(
                    json!({"command":"task.add","job":other["id"],"title":"Keep task"}),
                    WriteOptions::default(),
                )
                .await
                .unwrap();
            let before = f.service.store().snapshot().await.unwrap();
            let request = json!({"command":"job.delete","job":f.job});
            let options = WriteOptions {
                idempotency_key: Some("delete-once".into()),
                ..WriteOptions::default()
            };
            let deleted = f
                .service
                .execute(request.clone(), options.clone())
                .await
                .unwrap();
            assert_eq!(deleted.result["deleted"], true);
            assert!(deleted.projection_pending.is_none());
            let after = f.service.store().snapshot().await.unwrap();
            assert_eq!(after.jobs.len(), 1);
            assert_eq!(after.jobs[0].id, other["id"]);
            assert_eq!(after.tasks.len(), 1);
            assert!(after.plans.is_empty());
            let root = f.service.config().output_dir();
            assert!(!root.join(&before.jobs[0].document_path).exists());
            assert!(!root.join(&before.plans[0].path).exists());
            assert!(root.join(&after.jobs[0].document_path).exists());
            let board = std::fs::read_to_string(root.join("Projects/demo/Board.md")).unwrap();
            assert!(!board.contains("Delete this plan"));
            assert!(
                root.join("Projects/demo/Tasks/260905-0002-Keep task.md")
                    .exists()
            );
            let events = f.service.store().events(None, 0, 1000).await.unwrap();
            assert!(
                events
                    .iter()
                    .any(|e| e.event_type == "job.deleted" && e.job_id.as_deref() == Some(&f.job))
            );
            assert_eq!(
                f.service.execute(request, options).await.unwrap().result,
                deleted.result
            );
            assert_eq!(f.service.store().snapshot().await.unwrap(), after);
        }
    }
}

#[tokio::test]
async fn deletion_and_claim_race_without_leaving_orphaned_work() {
    for _ in 0..5 {
        let f = Fixture::new("markdown").await;
        let task = f.task("Race").await;
        let clock = f.clock.clone();
        let other = Store::open_with_clock(
            &f.service.config().storage.path,
            Arc::new(move || clock.load(Ordering::SeqCst)),
        )
        .await
        .unwrap();
        let (deleted, claimed) = tokio::join!(
            f.service.store().execute(json!({"command":"job.delete","job":f.job}),WriteOptions::default()),
            other.execute(json!({"command":"task.claim","task":task,"executor":"agent:racer","session":"racer"}),WriteOptions::default()),
        );
        assert_ne!(deleted.is_ok(), claimed.is_ok());
        let state = f.service.store().snapshot().await.unwrap();
        if deleted.is_ok() {
            assert!(state.jobs.is_empty());
            assert!(state.tasks.is_empty());
            assert!(state.leases.is_empty());
        } else {
            assert_eq!(state.jobs.len(), 1);
            assert_eq!(state.tasks.len(), 1);
            assert_eq!(state.leases.len(), 1);
        }
        f.service.sync().await.unwrap();
    }
}

#[tokio::test]
async fn deletion_checks_revisions_leases_and_external_dependencies() {
    let f = Fixture::new("markdown").await;
    let task = f.task("Owned work").await;
    let claim = f.claim(&task, "owner").await;
    let before = f.service.store().snapshot().await.unwrap();
    for request in [
        json!({"command":"job.delete","job":f.job}),
        json!({"command":"project.delete","project":f.project}),
    ] {
        let error = f
            .service
            .execute(request, WriteOptions::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("lease"), "{error}");
        assert_eq!(f.service.store().snapshot().await.unwrap(), before);
    }
    f.service
        .execute(
            json!({"command":"task.release","task":task,"reason":"allow deletion"}),
            owner(&claim),
        )
        .await
        .unwrap();
    let error = f
        .service
        .execute(
            json!({"command":"job.delete","job":f.job}),
            WriteOptions {
                expected_revision: Some(-1),
                ..WriteOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("revision"));
    let other = f
        .service
        .execute(
            json!({"command":"job.create","project":f.project,"title":"Dependent job"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let dependent = f
        .service
        .execute(
            json!({"command":"task.add","job":other["id"],"title":"Dependent task"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    f.service
        .execute(
            json!({"command":"task.depend","task":dependent["id"],"dependency":task}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let before = f.service.store().snapshot().await.unwrap();
    let error = f
        .service
        .execute(
            json!({"command":"job.delete","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("depend"), "{error}");
    assert_eq!(f.service.store().snapshot().await.unwrap(), before);
    f.service
        .execute(
            json!({"command":"project.delete","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        f.service
            .store()
            .snapshot()
            .await
            .unwrap()
            .projects
            .is_empty()
    );
}

#[tokio::test]
async fn failed_job_cleanup_retries_after_restart_without_reusing_numbers() {
    let f = Fixture::new("markdown").await;
    let task = f.task("Remove plan").await;
    let claim = f.start(&task, "delete").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    let before = f.service.store().snapshot().await.unwrap();
    let path = f
        .service
        .config()
        .output_dir()
        .join(&before.jobs[0].document_path);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let request = json!({"command":"job.delete","job":f.job});
    let options = WriteOptions {
        idempotency_key: Some("retry-delete".into()),
        ..WriteOptions::default()
    };
    let deleted = f
        .service
        .execute(request.clone(), options.clone())
        .await
        .unwrap();
    assert!(deleted.projection_pending.is_some());
    assert!(f.service.store().snapshot().await.unwrap().jobs.is_empty());
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, "Cleanup still required").unwrap();
    let store = Store::open_with_clock(&f.service.config().storage.path, {
        let now = f.clock.clone();
        Arc::new(move || now.load(Ordering::SeqCst))
    })
    .await
    .unwrap();
    let service = Service::new(f.service.config().clone(), store).unwrap();
    assert!(
        service
            .execute(request, options)
            .await
            .unwrap()
            .projection_pending
            .is_none()
    );
    assert!(!path.exists());
    assert!(
        !service
            .config()
            .output_dir()
            .join(&before.plans[0].path)
            .exists()
    );
    let job = service
        .execute(
            json!({"command":"job.create","project":f.project,"title":"Feature"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(job["sequence"], 2);
    let task = service
        .execute(
            json!({"command":"task.add","job":job["id"],"title":"Next"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(task["sequence"], 2);
}

#[tokio::test]
async fn project_deletion_removes_its_entire_output_directory_only() {
    let f = Fixture::new("obsidian").await;
    let task = f.task("Owned plan").await;
    let claim = f.start(&task, "delete").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"job.archive","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.service
        .execute(
            json!({"command":"project.archive","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let root = f.service.config().output_dir();
    let project_dir = root.join("Projects/demo");
    std::fs::create_dir_all(project_dir.join("Attachments/nested")).unwrap();
    std::fs::write(
        project_dir.join("Attachments/nested/user.txt"),
        "Project attachment",
    )
    .unwrap();
    let repository_file = f.dir.path().join("source.rs");
    std::fs::write(&repository_file, "Keep repository").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(f.dir.path(), project_dir.join("external")).unwrap();
    let other = f
        .service
        .execute(
            json!({"command":"project.register","name":"other","root":f.dir.path().join("other")}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    f.service
        .execute(
            json!({"command":"job.create","project":other["id"],"title":"Unrelated"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let result = f
        .service
        .execute(
            json!({"command":"project.delete","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.projection_pending.is_none(), "{result:?}");
    assert!(!project_dir.exists());
    assert_eq!(
        std::fs::read_to_string(repository_file).unwrap(),
        "Keep repository"
    );
    assert!(root.join("Projects/other/Board.md").exists());
    let state = f.service.store().snapshot().await.unwrap();
    assert_eq!(state.projects.len(), 1);
    assert_eq!(state.jobs.len(), 1);
    assert!(state.tasks.is_empty());
    assert!(state.plans.is_empty());
    assert!(
        !std::fs::read_to_string(root.join(f.dashboard_file()))
            .unwrap()
            .contains("[demo](")
    );
}

#[tokio::test]
async fn pending_project_cleanup_reserves_its_directory_until_sync() {
    let f = Fixture::new("markdown").await;
    f.service
        .store()
        .execute(
            json!({"command":"project.delete","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let request = json!({"command":"project.register","name":"demo","root":f.dir.path()});
    let error = f
        .service
        .store()
        .execute(request.clone(), WriteOptions::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("cleanup"), "{error}");
    f.service.sync().await.unwrap();
    let new = f
        .service
        .execute(request, WriteOptions::default())
        .await
        .unwrap()
        .result;
    assert_ne!(new["id"], f.project);
    assert!(
        f.service
            .config()
            .output_dir()
            .join("Projects/demo/Board.md")
            .exists()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn deletion_does_not_follow_directory_symlinks_within_the_output() {
    let f = Fixture::new("markdown").await;
    let root = f.service.config().output_dir();
    let project = root.join("Projects/demo");
    let saved = root.join("saved-demo");
    let other = root.join("Projects/other");
    std::fs::create_dir_all(other.join("Jobs")).unwrap();
    let job = f.service.store().snapshot().await.unwrap().jobs[0].clone();
    let filename = job.document_path.rsplit('/').next().unwrap();
    let innocent = other.join("Jobs").join(filename);
    std::fs::write(&innocent, "Another project's document").unwrap();
    std::fs::rename(&project, &saved).unwrap();
    std::os::unix::fs::symlink(&other, &project).unwrap();
    let result = f
        .service
        .execute(
            json!({"command":"project.delete","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.projection_pending.is_some());
    assert_eq!(
        std::fs::read_to_string(&innocent).unwrap(),
        "Another project's document"
    );
    std::fs::remove_file(&project).unwrap();
    std::fs::rename(&saved, &project).unwrap();
    f.service.sync().await.unwrap();
    assert!(!project.exists());
    assert!(innocent.exists());
}
