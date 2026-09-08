use super::*;

#[tokio::test]
async fn task_service_collects_job_messages_without_an_engine_or_im_connection() {
    use agentix_task::{Config, DocumentConfig, StorageConfig};
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".obsidian")).unwrap();
    let backend = Arc::new(
        Service::open(Config {
            schema_version: 1,
            storage: StorageConfig {
                path: root.path().join("tasks.sqlite3"),
            },
            documents: DocumentConfig {
                root: root.path().into(),
                directory: "docs".into(),
            },
        })
        .await
        .unwrap(),
    );
    let tasks = TaskBoardService::new(
        Some(backend),
        crate::SqliteState::in_memory().await.unwrap(),
    );
    for (session, text) in [
        ("pi:same", "old"),
        ("omp:same", "other"),
        ("pi:same", "updated"),
    ] {
        tasks
            .record_job_message(&crate::AgentEvent::ItemCompleted {
                session_id: session.into(),
                turn_id: "turn".into(),
                item: crate::ItemSummary {
                    id: "message".into(),
                    kind: "agentMessage".into(),
                    text: Some(text.into()),
                    status: None,
                },
            })
            .await;
    }
    let messages = tasks.conversations.lock().await;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[&("pi:same".into(), "turn".into())].len(), 1);
    assert_eq!(
        messages[&("pi:same".into(), "turn".into())][0]["text"],
        "updated"
    );
    assert_eq!(
        messages[&("omp:same".into(), "turn".into())][0]["text"],
        "other"
    );
}
