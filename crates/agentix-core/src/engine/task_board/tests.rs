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
                archive_directory: "Archived Projects".into(),
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
            .record_job_message(
                &crate::AgentEvent::ItemCompleted {
                    session_id: session.into(),
                    turn_id: "turn".into(),
                    item: crate::ItemSummary {
                        id: "message".into(),
                        kind: "agentMessage".into(),
                        text: Some(text.into()),
                        status: None,
                    },
                },
                None,
            )
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

#[tokio::test]
async fn task_service_records_configured_process_items_without_an_im_connection() {
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
                archive_directory: "Archived Projects".into(),
            },
        })
        .await
        .unwrap(),
    );
    let mut tasks = TaskBoardService::new(
        Some(backend),
        crate::SqliteState::in_memory().await.unwrap(),
    );
    tasks.output = crate::OutputConfig {
        show_reasoning: true,
        show_tool_calls: true,
    };
    for (session, text) in [
        ("pi:same", "old"),
        ("omp:same", "other"),
        ("pi:same", "updated"),
    ] {
        tasks
            .record_job_message(
                &crate::AgentEvent::ItemCompleted {
                    session_id: session.into(),
                    turn_id: "turn".into(),
                    item: crate::ItemSummary {
                        id: "message".into(),
                        kind: "reasoning".into(),
                        text: Some(text.into()),
                        status: None,
                    },
                },
                None,
            )
            .await;
    }
    let messages = tasks.conversations.lock().await;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[&("pi:same".into(), "turn".into())].len(), 1);
    assert_eq!(
        messages[&("pi:same".into(), "turn".into())][0]["text"],
        "**Reasoning**\n\nupdated"
    );
    assert_eq!(
        messages[&("omp:same".into(), "turn".into())][0]["text"],
        "**Reasoning**\n\nother"
    );
}

#[tokio::test]
async fn task_service_stages_discussion_before_a_job_exists() {
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
                archive_directory: "Archived Projects".into(),
            },
        })
        .await
        .unwrap(),
    );
    let tasks = TaskBoardService::new(
        Some(backend.clone()),
        crate::SqliteState::in_memory().await.unwrap(),
    );
    tasks
        .record_job_message(
            &crate::AgentEvent::ItemCompleted {
                session_id: "discussion".into(),
                turn_id: "turn".into(),
                item: crate::ItemSummary {
                    id: "u".into(),
                    kind: "userMessage".into(),
                    text: Some("Discuss capacity".into()),
                    status: None,
                },
            },
            None,
        )
        .await;
    let draft = backend
        .store()
        .discussion_list("discussion", 0, 10)
        .await
        .unwrap();
    assert_eq!(draft["turns"][0]["messages"][0]["text"], "Discuss capacity");
    assert!(backend.store().snapshot().await.unwrap().jobs.is_empty());
}

#[tokio::test]
async fn task_service_exports_unbound_discussion_for_the_session_project() {
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
                archive_directory: "Archived Projects".into(),
            },
        })
        .await
        .unwrap(),
    );
    let project = backend
        .store()
        .execute(
            json!({"command":"project.register","name":"Memory","root":root.path()}),
            WriteOptions::default(),
        )
        .await
        .unwrap()
        .result["id"]
        .clone();
    let tasks = TaskBoardService::new(
        Some(backend.clone()),
        crate::SqliteState::in_memory().await.unwrap(),
    );
    tasks
        .record_job_message(
            &crate::AgentEvent::ItemCompleted {
                session_id: "discussion".into(),
                turn_id: "turn".into(),
                item: crate::ItemSummary {
                    id: "u".into(),
                    kind: "userMessage".into(),
                    text: Some("Discuss capacity".into()),
                    status: None,
                },
            },
            Some(root.path().to_str().unwrap()),
        )
        .await;
    let draft = backend
        .store()
        .discussion_list("discussion", 0, 10)
        .await
        .unwrap();
    assert_eq!(draft["turns"][0]["messages"][0]["text"], "Discuss capacity");
    assert!(backend.store().snapshot().await.unwrap().jobs.is_empty());
    let sources = backend.store().memory_sources(0, 100).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].project_id, project.as_str().unwrap());
    assert_eq!(sources[0].messages[0]["text"], "Discuss capacity");
}
