use super::*;
use std::{fs, path::Path};

#[tokio::test]
async fn abbreviated_workspace_roots_preserve_board_identity_and_archive_recovery() {
    let f = Fixture::new().await;
    let root = agentix_task::expand_home(Path::new("~/taskix-isolated-workspace-test")).unwrap();
    let project = f
        .service
        .execute(
            json!({"command":"project.register","root":root,"name":"Home"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let id = project["id"].as_str().unwrap();
    let key = project["key"].as_str().unwrap();
    let source = f
        .service
        .config()
        .output_dir()
        .join(format!("Projects/{key}"));
    let moved = f
        .service
        .config()
        .output_dir()
        .join("Projects/Renamed home");
    assert_eq!(
        board_root(&source.join("Board.md")),
        "~/taskix-isolated-workspace-test"
    );
    fs::write(source.join("attachment.txt"), "Keep attachment").unwrap();
    fs::rename(source, &moved).unwrap();
    f.service.sync().await.unwrap();
    let project = f.service.store().project_result(id).await.unwrap();
    assert_eq!(project.root, root.to_string_lossy());
    assert_eq!(project.key, "Renamed home");
    for command in ["project.archive", "project.unarchive"] {
        f.service
            .execute(
                json!({"command":command,"project":id}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        fs::read_to_string(moved.join("attachment.txt")).unwrap(),
        "Keep attachment"
    );
    let read_only = Store::open_read_only(&f.service.config().storage.path)
        .await
        .unwrap();
    assert_eq!(
        read_only
            .project_by_root(&root.to_string_lossy())
            .await
            .unwrap()
            .unwrap()
            .id,
        id
    );
}

fn board_root(path: &Path) -> Value {
    let document = fs::read_to_string(path).unwrap();
    let properties: Value = serde_yaml::from_str(document.split("---").nth(1).unwrap()).unwrap();
    properties["root"].clone()
}

#[tokio::test]
async fn home_board_roots_format_exact_home_and_preserve_sibling_prefixes() {
    let f = Fixture::new().await;
    let home = agentix_task::expand_home(Path::new("~")).unwrap();
    let home = home
        .to_string_lossy()
        .trim_end_matches(['/', '\\'])
        .to_owned();
    for (name, root, expected) in [
        ("Exact home", home.clone(), "~".to_owned()),
        (
            "Sibling home",
            format!("{home}-sibling/workspace"),
            format!("{home}-sibling/workspace"),
        ),
        (
            "Outside home",
            "/external-workspace".to_owned(),
            "/external-workspace".to_owned(),
        ),
    ] {
        let project = f
            .service
            .execute(
                json!({"command":"project.register","root":root,"name":name}),
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .result;
        let path = f.service.config().output_dir().join(format!(
            "Projects/{}/Board.md",
            project["key"].as_str().unwrap()
        ));
        assert_eq!(board_root(&path), expected);
        assert_eq!(project["root"], root);
    }
}

fn set_board_root(path: &Path, root: &str) {
    let source = fs::read_to_string(path).unwrap();
    assert!(source.lines().any(|line| line.starts_with("root: ")));
    let source = source
        .lines()
        .map(|line| {
            if line.starts_with("root: ") {
                format!("root: {}", json!(root))
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(path, format!("{source}\n")).unwrap();
}

async fn register_home_project(f: &Fixture) -> Value {
    f.service
        .execute(
            json!({"command":"project.register","root":"~/taskix-isolated-workspace-test","name":"Home"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result
}

#[tokio::test]
async fn home_board_roots_preserve_archive_move_receipt_recovery() {
    for legacy in [false, true] {
        let f = Fixture::new().await;
        let project = register_home_project(&f).await;
        let active = f.service.config().output_dir().join("Projects/Home");
        let archive = f.service.config().archive_dir().join("Home");
        let root = if legacy {
            project["root"].as_str().unwrap()
        } else {
            "~/taskix-isolated-workspace-test"
        };
        set_board_root(&active.join("Board.md"), root);
        fs::write(active.join("attachment.txt"), "Keep attachment").unwrap();
        for (command, source, destination) in [
            ("project.archive", &active, &archive),
            ("project.unarchive", &archive, &active),
        ] {
            f.service
                .store()
                .execute(
                    json!({"command":command,"project":project["id"]}),
                    WriteOptions::default(),
                )
                .await
                .unwrap();
            set_board_root(&source.join("Board.md"), root);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::rename(source, destination).unwrap();
            let reopened = Service::open(f.service.config().clone()).await.unwrap();
            reopened.sync_pending_documents().await.unwrap();
            assert_eq!(
                fs::read_to_string(destination.join("attachment.txt")).unwrap(),
                "Keep attachment"
            );
            let stored = reopened
                .store()
                .project_result(project["id"].as_str().unwrap())
                .await
                .unwrap();
            assert_eq!(stored.root, project["root"].as_str().unwrap());
        }
    }
}

#[tokio::test]
async fn home_board_roots_preserve_legacy_archive_folder_migration() {
    let f = Fixture::new().await;
    let project = register_home_project(&f).await;
    let output = f.service.config().output_dir();
    let legacy = output.join("Archived Projects/Home");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::rename(output.join("Projects/Home"), &legacy).unwrap();
    set_board_root(&legacy.join("Board.md"), "~/taskix-isolated-workspace-test");
    fs::write(legacy.join("attachment.txt"), "Keep attachment").unwrap();
    for request in [
        json!({"command":"project.archive","project":project["id"]}),
        json!({"command":"project.relocate","project":project["id"],"name":"Home","directory":"Archived Projects/Home"}),
    ] {
        f.service
            .store()
            .execute(request, WriteOptions::default())
            .await
            .unwrap();
    }
    f.service.sync().await.unwrap();
    assert!(!legacy.exists());
    assert_eq!(
        fs::read_to_string(f.service.config().archive_dir().join("Home/attachment.txt")).unwrap(),
        "Keep attachment"
    );
}

#[tokio::test]
async fn home_board_roots_reject_a_different_workspace_after_folder_rename() {
    let f = Fixture::new().await;
    let project = register_home_project(&f).await;
    let output = f.service.config().output_dir();
    let moved = output.join("Projects/Renamed home");
    fs::rename(output.join("Projects/Home"), &moved).unwrap();
    set_board_root(&moved.join("Board.md"), "~/different-workspace");
    let error = f.service.sync().await.unwrap_err().to_string();
    assert!(error.contains("workspace root does not match"), "{error}");
    assert!(moved.join("Board.md").exists());
    assert!(!output.join("Projects/Home").exists());
    assert_eq!(
        f.service
            .store()
            .project_result(project["id"].as_str().unwrap())
            .await
            .unwrap()
            .key,
        "Home"
    );
}

#[tokio::test]
async fn home_board_roots_accept_legacy_absolute_roots_after_folder_rename() {
    let f = Fixture::new().await;
    let project = register_home_project(&f).await;
    let output = f.service.config().output_dir();
    let moved = output.join("Projects/Renamed home");
    fs::rename(output.join("Projects/Home"), &moved).unwrap();
    set_board_root(&moved.join("Board.md"), project["root"].as_str().unwrap());
    f.service.sync().await.unwrap();
    let stored = f
        .service
        .store()
        .project_result(project["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(stored.key, "Renamed home");
    assert_eq!(stored.root, project["root"].as_str().unwrap());
    assert_eq!(
        board_root(&moved.join("Board.md")),
        "~/taskix-isolated-workspace-test"
    );
}

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
