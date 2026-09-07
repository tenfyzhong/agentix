use super::*;

#[tokio::test]
async fn markdown_dashboard_does_not_query_event_history() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        schema_version: 1,
        storage: crate::StorageConfig {
            path: dir.path().join("tasks.sqlite3"),
        },
        documents: crate::DocumentConfig {
            format: DocumentFormat::Markdown,
            root: dir.path().to_owned(),
            directory: "notes".into(),
        },
    };
    let service = Service::open(config).await.unwrap();
    let project = service
        .store
        .execute(
            json!({"command":"project.register","name":"Target","root":dir.path()}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .as_str()
        .unwrap()
        .to_owned();
    service
        .store
        .execute(
            json!({"command":"job.create","project":project,"title":"Activity"}),
            WriteOptions::default(),
        )
        .await
        .unwrap();
    // Incremental dashboard rendering only has Project records.
    let state = Snapshot {
        projects: service.store.projects().await.unwrap(),
        ..Snapshot::default()
    };
    sqlx::query("DROP TABLE task_events")
        .execute(&service.store.pool)
        .await
        .unwrap();
    let result = service
        .dashboard(&state, state.projects[0].created_at)
        .await;
    assert!(
        result.is_ok(),
        "activity needs no event history: {result:?}"
    );
    assert!(result.unwrap().1.contains("Target"));
}

thread_local! {
    pub(super) static FRONTMATTER_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

async fn export_fixture() -> (tempfile::TempDir, Service, String) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".obsidian")).unwrap();
    let service = Service::open(Config {
        schema_version: 1,
        storage: crate::StorageConfig {
            path: dir.path().join("tasks.sqlite3"),
        },
        documents: crate::DocumentConfig {
            format: DocumentFormat::Obsidian,
            root: dir.path().to_owned(),
            directory: "notes".into(),
        },
    })
    .await
    .unwrap();
    let project = service
        .store
        .execute(
            json!({"command":"project.register","name":"Export","root":dir.path()}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let job = service
        .store
        .execute(
            json!({"command":"job.create","project":project["id"],"title":"Export"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    let task = service
        .store
        .execute(
            json!({"command":"task.add","job":job["id"],"title":"Task"}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result;
    (dir, service, task["id"].as_str().unwrap().to_owned())
}

#[tokio::test]
async fn obsidian_export_does_not_generate_markdown_for_properties() {
    let (_dir, service, task) = export_fixture().await;
    FRONTMATTER_CALLS.set(0);
    let snapshot = service.obsidian_snapshot().await.unwrap();
    assert_eq!(
        FRONTMATTER_CALLS.get(),
        0,
        "property exports should not render and parse Markdown"
    );
    let point = service.obsidian_note(&task).await.unwrap();
    assert_eq!(snapshot["notes"][0], point);
}

#[tokio::test]
async fn obsidian_export_ignores_plan_and_lease_bodies() {
    let (_dir, service, task) = export_fixture().await;
    let expected = service.obsidian_snapshot().await.unwrap();
    sqlx::query("INSERT INTO plans(id,data) VALUES('plan_unrelated','{}')")
        .execute(&service.store.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO task_leases(id,data) VALUES(?,'{}')")
        .bind(&task)
        .execute(&service.store.pool)
        .await
        .unwrap();
    assert_eq!(service.obsidian_snapshot().await.unwrap(), expected);
}

#[tokio::test]
async fn full_projection_does_not_rescan_entity_vectors_for_each_document() {
    let (_dir, service, _) = export_fixture().await;
    let project = service.store.projects().await.unwrap()[0].id.clone();
    for index in 0..64 {
        let job = service
            .store
            .execute(
                json!({"command":"job.create","project":project,"title":format!("Job {index}")}),
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .result;
        service
            .store
            .execute(
                json!({"command":"task.add","job":job["id"],"title":format!("Task {index}")}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
    }
    let state = service.store.snapshot().await.unwrap();
    crate::model::RESOLVE_ITEMS.set(0);
    service
        .render_state_locked(&state, None, &BTreeMap::new())
        .await
        .unwrap();
    let visited = crate::model::RESOLVE_ITEMS.get();
    assert!(
        visited < state.tasks.len() * 20,
        "full projection scanned {visited} entity identifiers for {} tasks",
        state.tasks.len()
    );
}
