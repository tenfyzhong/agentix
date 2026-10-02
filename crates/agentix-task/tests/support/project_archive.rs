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
    let archived = f
        .service
        .config()
        .documents
        .root
        .join("Archived Projects/demo");
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
        fs::read_to_string(
            f.service
                .config()
                .document_path(std::path::Path::new(&after.plans[0].path))
                .unwrap()
        )
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
    let archive_root = &service.config().documents.root;
    fs::create_dir_all(archive_root.join("History/Projects/demo")).unwrap();
    fs::write(
        archive_root.join("History/Projects/demo/unmanaged.txt"),
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
        fs::read_to_string(archive_root.join("History/Projects/demo/unmanaged.txt")).unwrap(),
        "Keep conflict"
    );
    fs::remove_dir_all(archive_root.join("History/Projects/demo")).unwrap();
    service.sync().await.unwrap();
    assert!(archive_root.join("History/Projects/demo/Board.md").exists());
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
    let archive_root = &f.service.config().documents.root;
    fs::create_dir(archive_root.join("Archived Projects")).unwrap();
    fs::rename(
        output.join("Projects/demo"),
        archive_root.join("Archived Projects/demo"),
    )
    .unwrap();
    let reopened = Service::open(f.service.config().clone()).await.unwrap();
    reopened.sync_pending_documents().await.unwrap();
    assert!(
        archive_root
            .join("Archived Projects/demo/Board.md")
            .exists()
    );
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
async fn project_archive_configuration_change_resolves_moved_archives() {
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
    fs::create_dir_all(config.archive_dir().parent().unwrap()).unwrap();
    fs::rename(f.service.config().archive_dir(), config.archive_dir()).unwrap();
    let service = Service::new(config, f.service.store().clone()).unwrap();
    service.sync_pending_documents().await.unwrap();
    let output = service.config().output_dir();
    let archive_root = &service.config().documents.root;
    assert!(archive_root.join("History/Projects/demo/Board.md").exists());
    assert!(!archive_root.join("Archived Projects/demo").exists());
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
    assert!(archive_root.join("History/Projects/demo/Board.md").exists());
    fs::remove_dir_all(output.join("Projects/demo")).unwrap();
    service.sync().await.unwrap();
    assert!(output.join("Projects/demo/Board.md").exists());
}

#[tokio::test]
async fn project_archive_migrates_legacy_output_relative_folder() {
    let f = Fixture::new().await;
    f.service
        .execute(
            json!({"command":"job.cancel","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let output = f.service.config().output_dir();
    let legacy = output.join("Archived Projects/demo");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::rename(output.join("Projects/demo"), &legacy).unwrap();
    fs::write(legacy.join("attachment.txt"), "Keep legacy attachment").unwrap();
    f.service
        .store()
        .execute(
            json!({"command":"project.archive","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.service.store().execute(json!({"command":"project.relocate","project":f.project,"name":"demo","directory":"Archived Projects/demo"}), WriteOptions::default()).await.unwrap();
    f.service.sync().await.unwrap();
    let archived = f
        .service
        .config()
        .documents
        .root
        .join("Archived Projects/demo");
    assert_eq!(
        fs::read_to_string(archived.join("attachment.txt")).unwrap(),
        "Keep legacy attachment"
    );
    assert!(!legacy.exists());
    let board = fs::read_to_string(archived.join("Board.md")).unwrap();
    assert!(board.contains("Archived Projects/demo/Jobs"));
    assert!(!board.contains("Tasks \u{2603}/Archived Projects"));
    assert!(!board.contains(f.service.config().documents.root.to_str().unwrap()));
    fs::rename(&archived, archived.with_file_name("renamed")).unwrap();
    f.service.sync_pending_documents().await.unwrap();
    let project = f
        .service
        .store()
        .snapshot()
        .await
        .unwrap()
        .projects
        .remove(0);
    assert_eq!(project.key, "renamed");
    f.service
        .execute(
            json!({"command":"project.unarchive","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(output.join("Projects/renamed/attachment.txt").exists());
}

#[tokio::test]
async fn project_archive_configuration_rejects_active_output_and_database_overlap() {
    let f = Fixture::new().await;
    let mut config = f.service.config().clone();
    for directory in [
        config.documents.directory.clone(),
        config.documents.directory.join("Projects"),
        config.documents.directory.join("Projects/Archive"),
        config.documents.directory.join("Projects/demo/Archive"),
    ] {
        config.documents.archive_directory = directory;
        assert!(
            config.validate().is_err(),
            "{:?}",
            config.documents.archive_directory
        );
    }
    config.documents.archive_directory = "History".into();
    config.storage.path = config.documents.root.join("History/tasks.sqlite3");
    assert!(config.validate().is_err());
}

#[tokio::test]
async fn project_archive_migrates_relative_path_without_moving_same_directory() {
    let f = Fixture::new().await;
    f.service
        .execute(
            json!({"command":"job.cancel","job":f.job}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let mut config = f.service.config().clone();
    config.documents.directory = ".".into();
    let archived = config.documents.root.join("Archived Projects/demo");
    fs::create_dir_all(archived.parent().unwrap()).unwrap();
    fs::rename(
        f.service.config().output_dir().join("Projects/demo"),
        &archived,
    )
    .unwrap();
    f.service
        .store()
        .execute(
            json!({"command":"project.archive","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    f.service.store().execute(json!({"command":"project.relocate","project":f.project,"name":"demo","directory":"Archived Projects/demo"}), WriteOptions::default()).await.unwrap();
    let service = Service::new(config, f.service.store().clone()).unwrap();
    service.sync().await.unwrap();
    assert!(archived.join("Board.md").exists());
    assert!(
        !std::path::Path::new(
            &service.store().snapshot().await.unwrap().projects[0].document_directory()
        )
        .is_absolute()
    );
    service
        .execute(
            json!({"command":"project.unarchive","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        service
            .config()
            .documents
            .root
            .join("Projects/demo/Board.md")
            .exists()
    );
}
