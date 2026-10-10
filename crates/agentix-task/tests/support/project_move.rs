use super::*;

#[tokio::test]
async fn project_move_preserves_work_and_updates_directory_lookup_and_board() {
    let f = Fixture::new().await;
    let task = f.task("Keep history").await;
    f.claim(&task, "move-test").await;
    f.plan(&task).await;
    let before = f.service.store().snapshot().await.unwrap();
    let destination = tempfile::tempdir().unwrap();
    let request = json!({"command":"project.move","project":f.project,"root":destination.path(),"remote":"git@example.com:renamed/repository.git"});
    let options = WriteOptions {
        expected_revision: Some(1),
        idempotency_key: Some("move-once".into()),
        ..WriteOptions::default()
    };
    let moved = f
        .service
        .execute(request.clone(), options.clone())
        .await
        .unwrap();
    assert!(moved.projection_pending.is_none());
    assert_eq!(moved.result["id"], f.project);
    assert_eq!(moved.result["revision"], 2);
    let after = f.service.store().snapshot().await.unwrap();
    assert_eq!(before.jobs, after.jobs);
    assert_eq!(before.tasks, after.tasks);
    assert_eq!(before.plans, after.plans);
    assert_eq!(before.leases, after.leases);
    assert_eq!(before.projects[0].key, after.projects[0].key);
    assert_eq!(
        before.projects[0].document_directory(),
        after.projects[0].document_directory()
    );
    assert!(
        f.service
            .store()
            .project_by_root(f.dir.path().canonicalize().unwrap().to_str().unwrap())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.service
            .project_for_session(Some(destination.path()), None)
            .await
            .unwrap()
            .unwrap()
            .id,
        f.project
    );
    let board = std::fs::read_to_string(
        f.service
            .config()
            .output_dir()
            .join("Projects/demo/Board.md"),
    )
    .unwrap();
    assert!(board.contains(destination.path().canonicalize().unwrap().to_str().unwrap()));
    assert!(board.contains("git@example.com:renamed/repository.git"));
    assert_eq!(
        f.service.execute(request, options).await.unwrap().result,
        moved.result
    );
}

#[tokio::test]
async fn project_move_rejects_registered_destination_and_stale_revision_atomically() {
    let f = Fixture::new().await;
    let destination = tempfile::tempdir().unwrap();
    f.service
        .execute(
            json!({"command":"project.register","name":"Other","root":destination.path()}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let before = f.service.store().project_result(&f.project).await.unwrap();
    let error = f.service.execute(json!({"command":"project.move","project":f.project,"root":destination.path().join("."),"remote":"new"}), WriteOptions::default()).await.unwrap_err();
    assert!(error.to_string().contains("already registered"), "{error}");
    let fresh = tempfile::tempdir().unwrap();
    let error = f
        .service
        .execute(
            json!({"command":"project.move","project":f.project,"root":fresh.path()}),
            WriteOptions {
                expected_revision: Some(0),
                ..WriteOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("revision changed"), "{error}");
    assert_eq!(
        f.service.store().project_result(&f.project).await.unwrap(),
        before
    );
    assert_eq!(
        f.service
            .store()
            .project_by_root(f.dir.path().canonicalize().unwrap().to_str().unwrap())
            .await
            .unwrap()
            .unwrap()
            .id,
        f.project
    );
    assert!(
        f.service
            .store()
            .project_by_root(fresh.path().canonicalize().unwrap().to_str().unwrap())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn project_move_validates_destination_and_remote_and_can_clear_remote() {
    let f = Fixture::new().await;
    let original = f.service.store().project_result(&f.project).await.unwrap();
    for root in [
        f.dir.path().join("missing"),
        f.service.config().storage.path.clone(),
    ] {
        let error = f
            .service
            .execute(
                json!({"command":"project.move","project":f.project,"root":root}),
                WriteOptions::default(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("directory"), "{error}");
    }
    let error = f
        .service
        .execute(
            json!({"command":"project.move","project":f.project,"root":f.dir.path(),"remote":" "}),
            WriteOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("remote"));
    assert_eq!(
        f.service.store().project_result(&f.project).await.unwrap(),
        original
    );
    for remote in [json!("https://example.com/new.git"), Value::Null] {
        let moved = f.service.execute(json!({"command":"project.move","project":f.project,"root":f.dir.path(),"remote":remote}), WriteOptions::default()).await.unwrap();
        assert_eq!(moved.result["remote"], remote);
    }
}

#[tokio::test]
async fn project_move_serializes_competing_destinations_without_loading_unrelated_projects() {
    use super::incremental::connection;
    let f = Fixture::new().await;
    let other_root = tempfile::tempdir().unwrap();
    let other = f
        .service
        .store()
        .execute(
            json!({"command":"project.register","name":"Other","root":other_root.path()}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let corrupt_root = tempfile::tempdir().unwrap();
    let corrupt = f
        .service
        .store()
        .execute(
            json!({"command":"project.register","name":"Unrelated","root":corrupt_root.path()}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    sqlx::query("UPDATE projects SET data=json_remove(data,'$.created_at') WHERE id=?")
        .bind(corrupt["id"].as_str().unwrap())
        .execute(&mut connection(&f).await)
        .await
        .unwrap();
    let destination = tempfile::tempdir().unwrap();
    let (first, second) = tokio::join!(
        f.service.store().execute(
            json!({"command":"project.move","project":f.project,"root":destination.path()}),
            WriteOptions::default()
        ),
        f.service.store().execute(
            json!({"command":"project.move","project":other["id"],"root":destination.path()}),
            WriteOptions::default()
        ),
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let (winner, error) = if let Ok(winner) = first {
        (winner.result, second.unwrap_err())
    } else {
        (second.unwrap().result, first.unwrap_err())
    };
    assert!(error.to_string().contains("already registered"), "{error}");
    assert_eq!(
        f.service
            .store()
            .project_by_root(destination.path().canonicalize().unwrap().to_str().unwrap())
            .await
            .unwrap()
            .unwrap()
            .id,
        winner["id"].as_str().unwrap()
    );
}

#[tokio::test]
async fn project_move_preserves_archival_state_and_recovers_pending_board_projection() {
    let f = Fixture::new().await;
    f.service
        .execute(
            json!({"command":"job.cancel","job":f.job}),
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
    let original = f.service.store().project_result(&f.project).await.unwrap();
    let board = f
        .service
        .config()
        .document_path(std::path::Path::new(&format!(
            "{}/Board.md",
            original.document_directory()
        )))
        .unwrap();
    let source = std::fs::read_to_string(&board).unwrap();
    std::fs::remove_file(&board).unwrap();
    std::fs::create_dir(&board).unwrap();
    let destination = tempfile::tempdir().unwrap();
    let moved = f
        .service
        .execute(
            json!({"command":"project.move","project":f.project,"root":destination.path()}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(moved.projection_pending.is_some());
    let current = f.service.store().project_result(&f.project).await.unwrap();
    assert_eq!(current.archived_at, original.archived_at);
    assert_eq!(current.document_directory(), original.document_directory());
    assert_eq!(
        current.root,
        destination.path().canonicalize().unwrap().to_string_lossy()
    );
    std::fs::remove_dir(&board).unwrap();
    std::fs::write(&board, source).unwrap();
    f.service.sync_pending_documents().await.unwrap();
    assert!(
        std::fs::read_to_string(&board)
            .unwrap()
            .contains(&current.root)
    );
    assert!(!f.service.store().has_pending_documents().await.unwrap());
}
