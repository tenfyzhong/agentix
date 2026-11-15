use super::*;

#[tokio::test]
async fn task_service_collects_job_messages_without_an_engine_or_im_connection() {
    use agentix_task::{Config, DocumentConfig, StorageConfig};
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".obsidian")).unwrap();
    let backend = Arc::new(
        Service::open(Config {
            tracking: agentix_task::TrackingConfig::default(),
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
            tracking: agentix_task::TrackingConfig::default(),
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
            tracking: agentix_task::TrackingConfig::default(),
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
    assert!(
        draft["turns"][0]["messages"][0]["completed_at_ms"]
            .as_i64()
            .is_some()
    );
    tasks
        .record_job_message(
            &crate::AgentEvent::ItemCompleted {
                session_id: "discussion".into(),
                turn_id: "turn".into(),
                item: crate::ItemSummary {
                    id: "a".into(),
                    kind: "agentMessage".into(),
                    text: Some("Done".into()),
                    status: None,
                },
            },
            None,
        )
        .await;
    assert!(tasks.conversations.lock().await[&("discussion".into(), "turn".into())][1]["completed_at_ms"].is_null());
    tasks
        .record_job_message(
            &crate::AgentEvent::TurnCompleted {
                session_id: "discussion".into(),
                turn_id: "turn".into(),
                status: crate::TurnStatus::Completed,
                error: None,
            },
            None,
        )
        .await;
    let complete = backend
        .store()
        .discussion_list("discussion", 0, 10)
        .await
        .unwrap();
    assert!(
        complete["turns"][0]["messages"][1]["completed_at_ms"]
            .as_i64()
            .unwrap()
            >= draft["turns"][0]["messages"][0]["completed_at_ms"]
                .as_i64()
                .unwrap()
    );
    assert!(backend.store().snapshot().await.unwrap().jobs.is_empty());
}

#[tokio::test]
async fn task_service_exports_unbound_discussion_for_the_session_project() {
    use agentix_task::{Config, DocumentConfig, StorageConfig};
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".obsidian")).unwrap();
    let backend = Arc::new(
        Service::open(Config {
            tracking: agentix_task::TrackingConfig::default(),
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
    assert!(
        draft["turns"][0]["messages"][0]["completed_at_ms"]
            .as_i64()
            .is_some()
    );
    assert!(backend.store().snapshot().await.unwrap().jobs.is_empty());
    let sources = backend.store().memory_sources(0, 100).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].project_id, project.as_str().unwrap());
    assert_eq!(sources[0].messages[0]["text"], "Discuss capacity");
}

#[tokio::test]
async fn antigravity_bridge_and_hooks_merge_native_messages_with_timing() {
    use agentix_task::{Config, DocumentConfig, StorageConfig};
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".obsidian")).unwrap();
    let backend = Arc::new(
        Service::open(Config {
            tracking: agentix_task::TrackingConfig::default(),
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

    let options = WriteOptions {
        session_ref: Some("native".into()),
        ..WriteOptions::default()
    };
    for start in [0, 3] {
        let turn = format!("native:step:{start}");
        let messages = json!([
            {"id":turn,"role":"user","text":"same prompt","completed_at_ms":1000},
            {"id":format!("native:step:{}",start+1),"role":"assistant","text":"progress"},
            {"id":format!("native:step:{}",start+2),"role":"assistant","text":"done","completed_at_ms":62333}
        ]);
        // Exercise both arrival orders; the first observed timestamp must survive.
        for pass in 0..2 {
            if (start == 0) == (pass == 0) {
                backend
                    .execute(
                        json!({"command":"session.record","session":"native",
                    "turn_id":turn,"source":"transcript","messages":messages}),
                        options.clone(),
                    )
                    .await
                    .unwrap();
            } else {
                record_antigravity_bridge_turn(&tasks, start, &turn).await;
            }
        }
    }
    let drafts = backend
        .store()
        .discussion_list("native", 0, 10)
        .await
        .unwrap();
    let turns = drafts["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 2);
    for turn in turns {
        let messages = turn["messages"].as_array().unwrap();
        assert_eq!(
            messages.len(),
            3,
            "bridge and hooks must share message identity"
        );
        assert_eq!(messages.iter().filter(|m| m["role"] == "user").count(), 1);
        assert!(messages[0]["completed_at_ms"].as_i64().is_some());
        assert!(messages[1]["completed_at_ms"].is_null());
        assert!(messages[2]["completed_at_ms"].as_i64().is_some());
    }
    assert_eq!(turns[0]["messages"][0]["completed_at_ms"], 1000);
    assert_eq!(turns[0]["messages"][2]["completed_at_ms"], 62333);
    let project = backend
        .execute(
            json!({"command":"project.register","name":"Test","root":root.path()}),
            options.clone(),
        )
        .await
        .unwrap()
        .result;
    let job = backend
        .execute(
            json!({"command":"job.create","project":project["id"],"title":"Native capture",
        "conversation_turns":["native:step:0","native:step:3"]}),
            options.clone(),
        )
        .await
        .unwrap()
        .result;
    assert_eq!(job["conversation"].as_array().unwrap().len(), 6);
    let document = std::fs::read_to_string(
        root.path()
            .join("docs")
            .join(job["document_path"].as_str().unwrap()),
    )
    .unwrap();
    assert_eq!(document.matches("### Turn ").count(), 2);
    assert_eq!(document.matches("#### User input").count(), 2);
    assert_eq!(document.matches("#### Agent output").count(), 2);
    assert!(document.contains("Elapsed: 61.333 s"));
}

async fn record_antigravity_bridge_turn(tasks: &TaskBoardService, start: i32, turn: &str) {
    for (offset, kind, text) in [
        (0, "userMessage", "same prompt"),
        (1, "agentMessage", "progress"),
        (2, "agentMessage", "done"),
    ] {
        tasks
            .record_job_message(
                &crate::AgentEvent::ItemCompleted {
                    session_id: "antigravity:native".into(),
                    turn_id: turn.into(),
                    item: crate::ItemSummary {
                        id: format!("native:step:{}", start + offset),
                        kind: kind.into(),
                        text: Some(text.into()),
                        status: None,
                    },
                },
                None,
            )
            .await;
    }
    tasks
        .record_job_message(
            &crate::AgentEvent::TurnCompleted {
                session_id: "antigravity:native".into(),
                turn_id: turn.into(),
                status: crate::TurnStatus::Completed,
                error: None,
            },
            None,
        )
        .await;
}

#[tokio::test]
async fn excluded_directory_never_buffers_or_records_messages_without_later_cwd() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".obsidian")).unwrap();
    let config: agentix_task::Config = serde_json::from_value(json!({
        "schema_version":1,
        "tracking":{"exclude_paths":[root.path()]},
        "storage":{"path":root.path().join("tasks.sqlite3")},
        "documents":{"root":root.path(),"directory":"docs"}
    }))
    .unwrap();
    let backend = Arc::new(Service::open(config).await.unwrap());
    let tasks = TaskBoardService::new(
        Some(backend.clone()),
        crate::SqliteState::in_memory().await.unwrap(),
    );
    for (kind, cwd) in [
        ("userMessage", root.path().to_str()),
        ("agentMessage", None),
    ] {
        tasks
            .record_job_message(
                &crate::AgentEvent::ItemCompleted {
                    session_id: "codex:excluded".into(),
                    turn_id: "turn".into(),
                    item: crate::ItemSummary {
                        id: kind.into(),
                        kind: kind.into(),
                        text: Some("Do not capture".into()),
                        status: None,
                    },
                },
                cwd,
            )
            .await;
    }
    assert!(tasks.conversations.lock().await.is_empty());
    assert!(
        backend
            .store()
            .memory_sources(0, 10)
            .await
            .unwrap()
            .is_empty()
    );
}
