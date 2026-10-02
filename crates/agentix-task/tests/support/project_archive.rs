use super::*;
use std::fs;

#[tokio::test]
async fn project_archive_moves_entire_folder_and_restores_paths() {
    let f = Fixture::new().await;
    let task = f.task("Archive history").await;
    let claim = f.start(&task, "archiver").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    f.approve().await;
    let output = f.service.config().output_dir();
    let before = f.service.store().snapshot().await.unwrap();
    let old = output.join("Projects/demo");
    fs::write(old.join("attachment.txt"), "Keep attachment").unwrap();
    let plan = output.join(&before.plans[0].path);
    fs::write(
        &plan,
        format!(
            "{}\nAuthored plan edit.\n",
            fs::read_to_string(&plan).unwrap()
        ),
    )
    .unwrap();
    let outcome = f
        .service
        .execute(
            json!({"command":"project.archive","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        outcome.projection_pending.is_none(),
        "{:?}",
        outcome.projection_pending
    );
    assert!(!old.exists(), "archived project must leave Projects");
    let archived = output.join("Archived Projects/demo");
    assert_eq!(
        fs::read_to_string(archived.join("attachment.txt")).unwrap(),
        "Keep attachment"
    );
    let after = f.service.store().snapshot().await.unwrap();
    assert_eq!(outcome.result["revision"], after.projects[0].revision);
    assert_eq!(after.projects[0].root, before.projects[0].root);
    assert!(
        after.jobs[0]
            .document_path
            .starts_with("Archived Projects/demo/")
    );
    assert!(
        fs::read_to_string(output.join(&after.plans[0].path))
            .unwrap()
            .contains("Authored plan edit.")
    );
    let note = f.service.obsidian_note(&task).await.unwrap();
    assert!(
        note["path"]
            .as_str()
            .unwrap()
            .contains("Archived Projects/demo/Tasks/")
    );
    let board = fs::read_to_string(archived.join("Board.md")).unwrap();
    assert!(board.contains("Archived Projects/demo/Inbox"));
    let reopened = Service::open(f.service.config().clone()).await.unwrap();
    reopened.sync().await.unwrap();
    reopened
        .execute(
            json!({"command":"project.unarchive","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(old.join("attachment.txt").exists());
    assert!(!archived.exists());
    let restored = reopened.store().snapshot().await.unwrap();
    assert_eq!(restored.jobs[0].document_path, before.jobs[0].document_path);
    assert_eq!(restored.plans[0].path, before.plans[0].path);
}

#[tokio::test]
async fn project_archive_uses_configured_folder_and_rejects_conflicts() {
    let f = Fixture::new().await;
    f.service
        .execute(
            json!({"command":"job.cancel","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let mut value = serde_json::to_value(f.service.config()).unwrap();
    value["documents"]["archive_directory"] = json!("History/Projects");
    let config: Config = serde_json::from_value(value).unwrap();
    let service = Service::new(config, f.service.store().clone()).unwrap();
    let output = service.config().output_dir();
    fs::create_dir_all(output.join("History/Projects/demo")).unwrap();
    fs::write(
        output.join("History/Projects/demo/unmanaged.txt"),
        "Keep conflict",
    )
    .unwrap();
    let outcome = service
        .execute(
            json!({"command":"project.archive","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(outcome.projection_pending.is_some());
    assert!(output.join("Projects/demo/Board.md").exists());
    assert_eq!(
        fs::read_to_string(output.join("History/Projects/demo/unmanaged.txt")).unwrap(),
        "Keep conflict"
    );
    fs::remove_dir_all(output.join("History/Projects/demo")).unwrap();
    service.sync().await.unwrap();
    assert!(output.join("History/Projects/demo/Board.md").exists());
    assert!(!output.join("Projects/demo").exists());
}

#[tokio::test]
async fn project_archive_configuration_rejects_unsafe_locations() {
    let f = Fixture::new().await;
    for directory in [
        "",
        "../Archive",
        "/Archive",
        "Projects",
        "Projects/Archive",
        ".",
    ] {
        let mut value = serde_json::to_value(f.service.config()).unwrap();
        value["documents"]["archive_directory"] = json!(directory);
        let config: Config = serde_json::from_value(value).unwrap();
        assert!(config.validate().is_err(), "{directory}");
    }
}

#[tokio::test]
async fn project_archive_recovers_a_move_before_the_database_receipt() {
    let f = Fixture::new().await;
    f.service
        .execute(
            json!({"command":"job.cancel","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.service
        .store()
        .execute(
            json!({"command":"project.archive","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let output = f.service.config().output_dir();
    fs::create_dir(output.join("Archived Projects")).unwrap();
    fs::rename(
        output.join("Projects/demo"),
        output.join("Archived Projects/demo"),
    )
    .unwrap();
    let reopened = Service::open(f.service.config().clone()).await.unwrap();
    reopened.sync_pending_documents().await.unwrap();
    assert!(output.join("Archived Projects/demo/Board.md").exists());
    assert!(!output.join("Projects/demo").exists());
    assert!(
        reopened
            .store()
            .job_record(&f.job)
            .await
            .unwrap()
            .document_path
            .starts_with("Archived Projects/demo/")
    );
}

#[tokio::test]
async fn project_archive_configuration_change_moves_existing_archives() {
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
    let mut config = f.service.config().clone();
    config.documents.archive_directory = "History/Projects".into();
    let service = Service::new(config, f.service.store().clone()).unwrap();
    service.sync_pending_documents().await.unwrap();
    let output = service.config().output_dir();
    assert!(output.join("History/Projects/demo/Board.md").exists());
    assert!(!output.join("Archived Projects/demo").exists());
    fs::create_dir_all(output.join("Projects/demo")).unwrap();
    fs::write(
        output.join("Projects/demo/personal.txt"),
        "Keep restore conflict",
    )
    .unwrap();
    let result = service
        .execute(
            json!({"command":"project.unarchive","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.projection_pending.is_some());
    assert!(output.join("History/Projects/demo/Board.md").exists());
    fs::remove_dir_all(output.join("Projects/demo")).unwrap();
    service.sync().await.unwrap();
    assert!(output.join("Projects/demo/Board.md").exists());
}
