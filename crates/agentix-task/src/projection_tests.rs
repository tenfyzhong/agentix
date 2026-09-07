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
