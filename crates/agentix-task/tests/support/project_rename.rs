use super::*;
use std::fs;

#[tokio::test]
async fn project_folder_rename_preserves_identity_content_and_tracking() {
    let f = Fixture::new().await;
    let task = f.task("Continue working").await;
    let claim = f.start(&task, "rename-session").await;
    let before = f.service.store().snapshot().await.unwrap();
    let output = f.service.config().output_dir();
    let old = output.join("Projects/demo");
    let new = output.join("Projects/视频项目");
    let job_path = &before.jobs[0].document_path;
    let job_body = fs::read_to_string(output.join(job_path)).unwrap();
    fs::write(
        output.join(job_path),
        job_body.replace(
            "<!-- taskix:notes:start -->",
            "<!-- taskix:notes:start -->\nKeep my notes.",
        ),
    )
    .unwrap();
    let plan_path = &before.plans[0].path;
    let plan_body = fs::read_to_string(output.join(plan_path)).unwrap();
    fs::write(
        output.join(plan_path),
        format!("{plan_body}\nUnpublished plan edit.\n"),
    )
    .unwrap();
    let inbox = fs::read_to_string(old.join("Inbox.md")).unwrap();
    fs::write(
        old.join("Inbox.md"),
        inbox.replace(
            "<!-- taskix:inbox:end -->",
            "- [ ] Preserve this request\n<!-- taskix:inbox:end -->",
        ),
    )
    .unwrap();
    fs::write(old.join("attachment.txt"), "keep attachment").unwrap();
    fs::rename(&old, &new).unwrap();

    // The next agent write must handle the rename before publishing old paths.
    let outcome = f
        .service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    assert!(
        outcome.projection_pending.is_none(),
        "{:?}",
        outcome.projection_pending
    );
    f.service.sync().await.unwrap();
    let after = f.service.store().snapshot().await.unwrap();
    assert_eq!(after.projects[0].id, f.project);
    assert_eq!(after.projects[0].root, before.projects[0].root);
    assert_eq!(after.projects[0].name, "视频项目");
    assert_eq!(after.projects[0].key, "视频项目");
    assert_eq!(after.jobs[0].id, f.job);
    assert_eq!(after.tasks[0].id, task);
    assert_eq!(after.tasks[0].status, agentix_task::TaskStatus::Done);
    assert!(!old.exists(), "do not recreate the old folder");
    assert_eq!(
        fs::read_to_string(new.join("attachment.txt")).unwrap(),
        "keep attachment"
    );
    assert!(
        fs::read_to_string(output.join(&after.jobs[0].document_path))
            .unwrap()
            .contains("Keep my notes.")
    );
    assert!(
        fs::read_to_string(output.join(&after.plans[0].path))
            .unwrap()
            .contains("Unpublished plan edit.")
    );
    assert_eq!(after.inboxes[0].content, "Preserve this request");
    for relative in [&after.jobs[0].document_path, &after.plans[0].path] {
        let document = fs::read_to_string(output.join(relative)).unwrap();
        assert!(relative.starts_with("Projects/视频项目/"));
        assert!(!document.contains("Projects/demo/"), "{document}");
    }
    let board = fs::read_to_string(new.join("Board.md")).unwrap();
    assert!(board.contains("name: \"视频项目\""));
    assert!(board.contains("视频项目 — Board"));
    let resolved = f
        .service
        .project_for_session(Some(f.dir.path()), Some("rename-session"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.id, f.project);
    assert_eq!(resolved.name, "视频项目");
    let directory = agentix_task::ProjectDirectory::discover(f.dir.path()).unwrap();
    assert_eq!(
        f.service
            .ensure_directory_project(&directory)
            .await
            .unwrap()
            .unwrap()
            .id,
        f.project
    );
}

#[tokio::test]
async fn project_folder_rename_before_plan_revision_uses_moved_body() {
    let f = Fixture::new().await;
    let task = f.task("Plan rename").await;
    let claim = f.claim(&task, "planner").await;
    f.plan(&task).await;
    let output = f.service.config().output_dir();
    fs::rename(output.join("Projects/demo"), output.join("Projects/Demo")).unwrap();
    let result = f
        .service
        .execute(
            json!({"command":"plan.revise","task":task,"body":"# Revised after rename"}),
            owner(&claim),
        )
        .await
        .unwrap();
    assert!(result.projection_pending.is_none());
    assert!(
        result.result["path"]
            .as_str()
            .unwrap()
            .starts_with("Projects/Demo/")
    );
    assert_eq!(
        f.service.plan(&task).await.unwrap()["body"]
            .as_str()
            .unwrap()
            .trim(),
        "# Revised after rename"
    );
}

#[tokio::test]
async fn project_folder_rename_rejects_ambiguous_boards_without_recreating_old_folder() {
    let f = Fixture::new().await;
    let output = f.service.config().output_dir();
    fs::rename(output.join("Projects/demo"), output.join("Projects/First")).unwrap();
    fs::create_dir(output.join("Projects/Second")).unwrap();
    fs::copy(
        output.join("Projects/First/Board.md"),
        output.join("Projects/Second/Board.md"),
    )
    .unwrap();
    assert!(f.service.sync().await.is_err());
    assert!(!output.join("Projects/demo").exists());
    assert_eq!(
        f.service.store().snapshot().await.unwrap().projects[0].key,
        "demo"
    );
}

#[tokio::test]
async fn project_folder_rename_sync_recovers_after_reopening_service() {
    let f = Fixture::new().await;
    let output = f.service.config().output_dir();
    fs::rename(
        output.join("Projects/demo"),
        output.join("Projects/New name"),
    )
    .unwrap();
    let service = Service::open(f.service.config().clone()).await.unwrap();
    service.sync().await.unwrap();
    assert_eq!(
        service.store().snapshot().await.unwrap().projects[0].name,
        "New name"
    );
    assert!(!output.join("Projects/demo").exists());
    let revision = service.store().snapshot().await.unwrap().projects[0].revision;
    service.sync().await.unwrap();
    assert_eq!(
        service.store().snapshot().await.unwrap().projects[0].revision,
        revision
    );
}

#[tokio::test]
async fn project_folder_rename_keeps_archived_jobs_and_cross_job_dependencies() {
    let f = Fixture::new().await;
    let task = f.task("Archived prerequisite").await;
    let claim = f.start(&task, "archived").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    f.approve().await;
    f.service
        .execute(
            json!({"command":"job.archive","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let job = f
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
            json!({"command":"task.add","job":job["id"],"title":"Dependent task"}),
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
    let output = f.service.config().output_dir();
    fs::rename(
        output.join("Projects/demo"),
        output.join("Projects/Renamed"),
    )
    .unwrap();
    f.service.sync_pending_documents().await.unwrap();
    let state = f.service.store().snapshot().await.unwrap();
    let archived = state.jobs.iter().find(|job| job.id == f.job).unwrap();
    assert!(archived.archived_at.is_some());
    assert!(
        archived
            .document_path
            .starts_with("Projects/Renamed/Jobs/Archived/")
    );
    assert!(output.join(&archived.document_path).is_file());
    let dependent = state
        .tasks
        .iter()
        .find(|t| t.id == dependent["id"])
        .unwrap();
    assert_eq!(dependent.dependencies, [task]);
    for job in &state.jobs {
        let body = fs::read_to_string(output.join(&job.document_path)).unwrap();
        assert!(!body.contains("Projects/demo/"));
    }
}

#[tokio::test]
async fn project_folder_rename_retries_publication_without_losing_authored_content() {
    let f = Fixture::new().await;
    let job = f.service.store().job_record(&f.job).await.unwrap();
    let output = f.service.config().output_dir();
    let source = fs::read_to_string(output.join(&job.document_path)).unwrap();
    fs::rename(output.join("Projects/demo"), output.join("Projects/Retry")).unwrap();
    let moved = output.join(
        job.document_path
            .replace("Projects/demo/", "Projects/Retry/"),
    );
    fs::write(
        &moved,
        source.replace("<!-- taskix:notes:end -->", "missing marker"),
    )
    .unwrap();
    assert!(f.service.sync().await.is_err());
    let renamed = f.service.store().snapshot().await.unwrap().projects[0].clone();
    assert_eq!(renamed.key, "Retry");
    fs::write(
        &moved,
        source.replace(
            "<!-- taskix:notes:start -->",
            "<!-- taskix:notes:start -->\nPreserved on retry.",
        ),
    )
    .unwrap();
    let service = Service::open(f.service.config().clone()).await.unwrap();
    service.sync_pending_documents().await.unwrap();
    assert!(
        fs::read_to_string(moved)
            .unwrap()
            .contains("Preserved on retry.")
    );
    assert!(!output.join("Projects/demo").exists());
    assert_eq!(
        service.store().snapshot().await.unwrap().projects[0].revision,
        renamed.revision
    );
}

#[tokio::test]
async fn project_folder_copy_does_not_rename_registered_project() {
    let f = Fixture::new().await;
    let output = f.service.config().output_dir();
    fs::create_dir(output.join("Projects/Copy")).unwrap();
    fs::copy(
        output.join("Projects/demo/Board.md"),
        output.join("Projects/Copy/Board.md"),
    )
    .unwrap();
    f.service.sync().await.unwrap();
    assert_eq!(
        f.service.store().snapshot().await.unwrap().projects[0].key,
        "demo"
    );
}

#[tokio::test]
async fn project_folder_rename_rejects_registered_name_and_invalid_name() {
    for name in ["Taken", "Bad#name"] {
        let f = Fixture::new().await;
        let other_root = f.dir.path().join("other");
        fs::create_dir(&other_root).unwrap();
        f.service
            .execute(
                json!({"command":"project.register","name":"Taken","root":other_root}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        let output = f.service.config().output_dir();
        fs::rename(output.join("Projects/Taken"), output.join("Projects/Saved")).unwrap();
        fs::rename(
            output.join("Projects/demo"),
            output.join(format!("Projects/{name}")),
        )
        .unwrap();
        let error = f.service.sync().await.unwrap_err().to_string();
        assert!(
            error.contains("already registered") || error.contains("portable name"),
            "{error}"
        );
        assert_eq!(
            f.service.store().snapshot().await.unwrap().projects[0].key,
            "demo"
        );
        assert!(!output.join("Projects/demo").exists());
    }
}
