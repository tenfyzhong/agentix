use super::*;
use std::{fs, path::Path};

async fn archived_fixture() -> Fixture {
    let f = Fixture::new().await;
    let task = f.task("Preserve moved plan").await;
    let claim = f.start(&task, "relative-path-owner").await;
    f.service
        .execute(json!({"command":"task.done","task":task}), owner(&claim))
        .await
        .unwrap();
    f.approve().await;
    f.service
        .execute(
            json!({"command":"project.archive","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    let archive = f.service.config().archive_dir().join("demo");
    fs::write(archive.join("attachment.txt"), "Keep attachment").unwrap();
    f
}

async fn assert_relative_documents(service: &Service) -> agentix_task::Snapshot {
    let state = service.store().snapshot().await.unwrap();
    for project in &state.projects {
        assert!(!Path::new(&project.document_directory()).is_absolute());
    }
    for path in state
        .jobs
        .iter()
        .map(|j| &j.document_path)
        .chain(state.plans.iter().map(|p| &p.path))
    {
        assert!(!Path::new(path).is_absolute(), "{path}");
    }
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(&service.config().storage.path),
    )
    .await
    .unwrap();
    let paths: Vec<String> = sqlx::query_scalar("SELECT path FROM document_registry")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(
        paths.iter().all(|path| !Path::new(path).is_absolute()),
        "{paths:?}"
    );
    pool.close().await;
    state
}

#[tokio::test]
async fn relative_document_paths_survive_two_vault_moves_without_rebasing() {
    let f = archived_fixture().await;
    let before = assert_relative_documents(&f.service).await;
    let mut config = f.service.config().clone();
    for name in ["second-vault", "third-vault"] {
        let root = f.dir.path().join(name);
        fs::rename(&config.documents.root, &root).unwrap();
        config.documents.root = root;
        let service = Service::new(config.clone(), f.service.store().clone()).unwrap();
        service.sync_pending_documents().await.unwrap();
        let after = assert_relative_documents(&service).await;
        assert_eq!(before.projects, after.projects);
        assert_eq!(before.jobs, after.jobs);
        assert_eq!(before.plans, after.plans);
        let task = &after.tasks[0].id;
        assert!(
            service.plan(task).await.unwrap()["body"]
                .as_str()
                .unwrap()
                .contains("Plan")
        );
        assert_eq!(
            fs::read_to_string(config.archive_dir().join("demo/attachment.txt")).unwrap(),
            "Keep attachment"
        );
        let note = service.obsidian_note(task).await.unwrap();
        assert!(
            note["path"]
                .as_str()
                .unwrap()
                .starts_with("Archived Projects/demo/Tasks/")
        );
    }
    let service = Service::new(config, f.service.store().clone()).unwrap();
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
            .output_dir()
            .join("Projects/demo/attachment.txt")
            .exists()
    );
    assert_relative_documents(&service).await;
}

#[tokio::test]
async fn relative_document_paths_use_current_active_and_archive_directories() {
    let f = archived_fixture().await;
    let before = assert_relative_documents(&f.service).await;
    let mut config = f.service.config().clone();
    config.documents.archive_directory = "History/Projects".into();
    config.documents.directory = "Other-Agents".into();
    fs::create_dir_all(config.archive_dir().parent().unwrap()).unwrap();
    fs::rename(f.service.config().archive_dir(), config.archive_dir()).unwrap();
    fs::rename(f.service.config().output_dir(), config.output_dir()).unwrap();
    let service = Service::new(config.clone(), f.service.store().clone()).unwrap();
    service.sync().await.unwrap();
    let after = assert_relative_documents(&service).await;
    assert_eq!(before.projects, after.projects);
    assert_eq!(before.jobs, after.jobs);
    assert_eq!(before.plans[0].path, after.plans[0].path);
    assert!(
        service.plan(&after.tasks[0].id).await.unwrap()["body"]
            .as_str()
            .unwrap()
            .contains("Plan")
    );
    let note = service.obsidian_note(&after.tasks[0].id).await.unwrap();
    assert!(
        note["path"]
            .as_str()
            .unwrap()
            .starts_with("History/Projects/demo/Tasks/")
    );
    service
        .execute(
            json!({"command":"project.unarchive","project":f.project}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        config
            .output_dir()
            .join("Projects/demo/attachment.txt")
            .exists()
    );
}

#[tokio::test]
async fn relative_document_paths_migrate_schema_eighteen_absolute_archives_after_a_move() {
    let f = archived_fixture().await;
    let state = f.service.store().snapshot().await.unwrap();
    let absolute = f
        .service
        .config()
        .archive_dir()
        .join("demo")
        .to_string_lossy()
        .replace('\\', "/");
    let old = state.projects[0].document_directory();
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(&f.service.config().storage.path),
    )
    .await
    .unwrap();
    sqlx::query("PRAGMA user_version=18")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE projects SET data=json_set(data,'$.document_directory',?) WHERE id=?")
        .bind(&absolute)
        .bind(&f.project)
        .execute(&pool)
        .await
        .unwrap();
    for job in &state.jobs {
        let path = format!(
            "{absolute}{}",
            job.document_path.strip_prefix(&old).unwrap()
        );
        sqlx::query("UPDATE jobs SET data=json_set(data,'$.document_path',?) WHERE id=?")
            .bind(path)
            .bind(&job.id)
            .execute(&pool)
            .await
            .unwrap();
    }
    for plan in &state.plans {
        let path = format!("{absolute}{}", plan.path.strip_prefix(&old).unwrap());
        sqlx::query("UPDATE plans SET data=json_set(data,'$.path',?) WHERE id=?")
            .bind(path)
            .bind(&plan.id)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE document_registry SET path=?||substr(path,length(?)+1) WHERE substr(path,1,length(?))=?")
        .bind(&absolute).bind(&old).bind(&old).bind(&old).execute(&pool).await.unwrap();
    sqlx::query("PRAGMA user_version=18")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let mut config = f.service.config().clone();
    let root = f.dir.path().join("migrated-vault");
    fs::rename(&config.documents.root, &root).unwrap();
    config.documents.root = root;
    let service = Service::open(config.clone()).await.unwrap();
    assert_relative_documents(&service).await;
    service.sync_pending_documents().await.unwrap();
    assert_eq!(
        fs::read_to_string(config.archive_dir().join("demo/attachment.txt")).unwrap(),
        "Keep attachment"
    );
    service.plan(&state.tasks[0].id).await.unwrap();
}
