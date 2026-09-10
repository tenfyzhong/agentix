use super::*;

#[tokio::test]
async fn inbox_views_edits_and_polling_ignore_unrelated_entity_bodies() {
    use sqlx::Connection;
    let (_dir, service, _) = task_fixture().await;
    let (engine, channel) = engine(service.clone()).await;
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    let original = input("/inbox Original");
    engine.handle_inbound(original.clone()).await.unwrap();
    let detail = button(&last(&channel), "View inbox entry");
    let mut db = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&service.config().storage.path),
    )
    .await
    .unwrap();
    for table in ["jobs", "tasks", "plans"] {
        sqlx::query(&format!(
            "INSERT INTO {table}(id,data) VALUES('unrelated','{{}}')"
        ))
        .execute(&mut db)
        .await
        .unwrap();
    }
    engine.refresh_inbox_sources().await.unwrap();
    click(&engine, detail).await;
    assert!(last(&channel).body.contains("Original"));
    engine.handle_inbound(input("/inboxes")).await.unwrap();
    assert!(last(&channel).body.contains("Original"));
    let mut edit = original.clone();
    edit.event_id = "scoped-edit".into();
    edit.payload = serde_json::from_value(json!({"TextEdited":{"original_event_id":original.event_id,"version":10,"text":"/inbox Scoped edit"}})).unwrap();
    *channel.inbox_source.lock().unwrap() = Some(edit);
    engine.refresh_inbox_sources().await.unwrap();
    engine.handle_inbound(input("/inboxes")).await.unwrap();
    assert!(last(&channel).body.contains("Scoped edit"));
}

#[tokio::test]
async fn inbox_submission_remains_available_on_read_only_attachments() {
    let (_dir, service, _) = task_fixture().await;
    let mut agent = FakeAgent::new();
    agent.read_only = true;
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        Arc::new(agent),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone());
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    engine
        .handle_inbound(input("/inbox Read-only submission"))
        .await
        .unwrap();
    assert_eq!(service.store().snapshot().await.unwrap().inboxes.len(), 1);
    engine.handle_inbound(input("/inboxes")).await.unwrap();
    assert!(last(&channel).body.contains("Read\\-only submission"));
    let menus = channel.menus.lock().unwrap();
    for name in ["inbox", "inboxes"] {
        assert!(
            menus
                .last()
                .unwrap()
                .commands
                .iter()
                .any(|c| c.name == name && c.contextual)
        );
    }
}

#[tokio::test]
async fn inbox_requires_attachment_and_preserves_multiline_submission_without_a_turn() {
    let (_dir, service, _) = task_fixture().await;
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_task_board(service.clone());
    for command in ["/inboxes", "/inbox New requirement"] {
        engine.handle_inbound(input(command)).await.unwrap();
        assert!(last(&channel).body.contains("/attach"));
    }
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    engine.handle_inbound(input("/inbox   ")).await.unwrap();
    assert!(last(&channel).body.contains("/inbox <content>"));
    let content = "New  requirement\n\n- **Keep** nested Markdown\n  - [ ] acceptance check\n\n```rust\nlet x =  1;\n```";
    let event = input(&format!("/inbox@agentix_bot {content}"));
    engine.handle_inbound(event.clone()).await.unwrap();
    let response = last(&channel);
    assert!(response.body.contains("demo"));
    assert!(response.body.contains("TODO"));
    engine.handle_inbound(event).await.unwrap();
    let state = service.store().snapshot().await.unwrap();
    assert_eq!(state.inboxes.len(), 1);
    assert_eq!(state.inboxes[0].content, content);
    assert!(response.body.contains(&state.inboxes[0].id));
    click(&engine, button(&response, "View inbox entry")).await;
    assert!(last(&channel).body.contains("New  requirement"));
    assert!(
        !agent
            .calls()
            .iter()
            .any(|c| c.starts_with("start:") || c.starts_with("steer:"))
    );
    let menus = channel.menus.lock().unwrap();
    for name in ["inbox", "inboxes"] {
        assert!(
            menus
                .last()
                .unwrap()
                .commands
                .iter()
                .any(|c| c.name == name && c.contextual)
        );
    }
}

#[tokio::test]
async fn inbox_project_list_pages_in_document_order_and_scopes_old_buttons() {
    let (_dir, service, _) = task_fixture().await;
    let project = service.store().snapshot().await.unwrap().projects[0]
        .id
        .clone();
    for n in 0..8 {
        let entry = write(
            &service,
            json!({"command":"inbox.add","project":project,"content":format!("Requirement {n}")}),
        )
        .await;
        if n == 1 {
            write(
                &service,
                json!({"command":"inbox.cancel","inbox":entry["id"]}),
            )
            .await;
        }
    }
    let (engine, channel) = engine(service).await;
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    engine.handle_inbound(input("/inboxes")).await.unwrap();
    let first = last(&channel);
    assert_eq!(first.title, "Project inbox");
    assert_eq!(first.subtitle.as_deref(), Some("Page 1 / 2"));
    assert!(first.body.contains("CANCELLED"));
    assert!(first.body.contains("Requirement 5"));
    assert!(!first.body.contains("Requirement 6"));
    click(&engine, button(&first, "Next")).await;
    assert!(last(&channel).body.contains("Requirement 7"));
    engine.handle_inbound(input("/attach thr_b")).await.unwrap();
    assert!(matches!(
        engine
            .handle_inbound(InboundEnvelope::action(
                uuid::Uuid::new_v4().to_string(),
                ConversationRef::new(ChannelKind::Telegram, "chat-a"),
                "owner",
                button(&first, "Requirement 0"),
            ))
            .await,
        Err(EngineError::InvalidAction)
    ));
    engine.handle_inbound(input("/inboxes")).await.unwrap();
    assert!(last(&channel).body.contains("registered project"));
}

#[tokio::test]
async fn inbox_response_retry_after_restart_does_not_append_again() {
    let (_dir, service, _) = task_fixture().await;
    let state = SqliteState::in_memory().await.unwrap();
    let channel = Arc::new(FakeChannel::default());
    let create = || {
        Engine::new(
            Arc::new(FakeAgent::new()),
            state.clone(),
            vec![channel.clone()],
        )
        .with_task_board(service.clone())
    };
    let engine = create();
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    *channel.inbox_send_failures.lock().unwrap() = 1;
    let event = input("/inbox Retry me");
    assert!(engine.handle_inbound(event.clone()).await.is_err());
    assert_eq!(service.store().snapshot().await.unwrap().inboxes.len(), 1);
    drop(engine);
    let engine = create();
    engine.restore_bindings().await.unwrap();
    engine.handle_inbound(event).await.unwrap();
    assert_eq!(service.store().snapshot().await.unwrap().inboxes.len(), 1);
    assert!(last(&channel).body.contains("TODO"));
}

#[tokio::test]
async fn inbox_message_edits_update_original_entry_after_detach_and_restart() {
    let (_dir, service, _) = task_fixture().await;
    let state = SqliteState::in_memory().await.unwrap();
    let channel = Arc::new(FakeChannel::default());
    let create = || {
        Engine::new(
            Arc::new(FakeAgent::new()),
            state.clone(),
            vec![channel.clone()],
        )
        .with_task_board(service.clone())
    };
    let engine = create();
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    let original = input("/inbox Original");
    engine.handle_inbound(original.clone()).await.unwrap();
    engine.handle_inbound(input("/detach")).await.unwrap();
    drop(engine);
    let engine = create();
    let edit = |event: &str, owner: &str, version: i64, text: &str| {
        let mut envelope = original.clone();
        envelope.event_id = event.into();
        envelope.owner_id = owner.into();
        envelope.payload = serde_json::from_value(json!({"TextEdited":{"original_event_id":original.event_id,"version":version,"text":text}})).unwrap();
        envelope
    };
    engine
        .handle_inbound(edit("edit-2", "owner", 2, "/inbox Edited\nDetails"))
        .await
        .unwrap();
    engine
        .handle_inbound(edit("edit-2-replay", "owner", 2, "/inbox Edited\nDetails"))
        .await
        .unwrap();
    engine
        .handle_inbound(edit("edit-old", "owner", 1, "/inbox Old"))
        .await
        .unwrap();
    assert!(
        engine
            .handle_inbound(edit("edit-forged", "stranger", 3, "/inbox Forged"))
            .await
            .is_err()
    );
    engine
        .handle_inbound(edit("edit-command", "owner", 4, "/stop"))
        .await
        .unwrap();
    let entries = service.store().snapshot().await.unwrap().inboxes;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].content, "Edited\nDetails");
    assert_eq!(entries[0].status.to_string(), "TODO");
    let doc = std::fs::read_to_string(service.config().output_dir().join("Projects/demo/Inbox.md"))
        .unwrap();
    assert!(doc.contains("- [ ] Edited"));
    assert!(doc.contains("  Details"));
    assert!(!doc.contains("Original"));
}

#[tokio::test]
async fn inbox_polling_repairs_source_edits_without_an_attachment() {
    let (_dir, service, _) = task_fixture().await;
    let (engine, channel) = engine(service.clone()).await;
    engine.handle_inbound(input("/attach thr_a")).await.unwrap();
    let original = input("/inbox Original");
    engine.handle_inbound(original.clone()).await.unwrap();
    engine.handle_inbound(input("/detach")).await.unwrap();
    let mut edit = original.clone();
    edit.event_id = "poll-edit".into();
    edit.payload = serde_json::from_value(json!({"TextEdited":{"original_event_id":original.event_id,"version":10,"text":"/inbox Polled"}})).unwrap();
    *channel.inbox_source.lock().unwrap() = Some(edit);
    engine.refresh_inbox_sources().await.unwrap();
    assert_eq!(
        service.store().snapshot().await.unwrap().inboxes[0].content,
        "Polled"
    );
    let revision = service.store().snapshot().await.unwrap().inboxes[0].revision;
    engine.refresh_inbox_sources().await.unwrap();
    assert_eq!(
        service.store().snapshot().await.unwrap().inboxes[0].revision,
        revision
    );
}

#[tokio::test]
async fn job_conversation_captures_completed_messages_even_without_im_binding() {
    let (_dir, service, _) = task_fixture().await;
    let (engine, _) = engine(service.clone()).await;
    for (id, kind, text) in [
        ("user", "userMessage", "Request"),
        ("tool", "commandExecution", "secret tool result"),
        ("reason", "reasoning", "private reasoning"),
        ("assistant", "agentMessage", "Visible answer"),
    ] {
        engine
            .handle_agent_event(AgentEvent::ItemCompleted {
                session_id: "thr_a".into(),
                turn_id: "turn_capture".into(),
                item: ItemSummary {
                    id: id.into(),
                    kind: kind.into(),
                    text: Some(text.into()),
                    status: None,
                },
            })
            .await
            .unwrap();
    }
    // No user prompt is assigned until the turn has finished creating its Job.
    assert!(serde_json::to_value(&service.store().snapshot().await.unwrap().jobs[0]).unwrap()["conversation"].as_array().unwrap().is_empty());
    engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_a".into(),
            turn_id: "turn_capture".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await
        .unwrap();
    let job = serde_json::to_value(&service.store().snapshot().await.unwrap().jobs[0]).unwrap();
    assert_eq!(job["conversation"].as_array().unwrap().len(), 2);
    assert_eq!(job["conversation"][1]["text"], "Visible answer");
    assert!(!job.to_string().contains("secret tool result"));
}

#[tokio::test]
async fn job_conversation_records_enabled_process_output_in_agent_quote() {
    let (_dir, service, _) = task_fixture().await;
    let (engine, _) = engine(service.clone()).await;
    let engine = engine.with_output(agentix_core::OutputConfig {
        show_reasoning: true,
        show_tool_calls: true,
    });
    for (id, kind, text) in [
        ("user", "userMessage", "Request"),
        ("tool", "commandExecution", "secret tool result"),
        ("reason", "reasoning", "private reasoning"),
        ("assistant", "agentMessage", "Visible answer"),
    ] {
        engine
            .handle_agent_event(AgentEvent::ItemCompleted {
                session_id: "thr_a".into(),
                turn_id: "turn_capture".into(),
                item: ItemSummary {
                    id: id.into(),
                    kind: kind.into(),
                    text: Some(text.into()),
                    status: None,
                },
            })
            .await
            .unwrap();
    }
    // No user prompt is assigned until the turn has finished creating its Job.
    assert!(serde_json::to_value(&service.store().snapshot().await.unwrap().jobs[0]).unwrap()["conversation"].as_array().unwrap().is_empty());
    engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_a".into(),
            turn_id: "turn_capture".into(),
            status: TurnStatus::Completed,
            error: None,
        })
        .await
        .unwrap();
    let job = serde_json::to_value(&service.store().snapshot().await.unwrap().jobs[0]).unwrap();
    assert_eq!(job["conversation"].as_array().unwrap().len(), 4);
    assert_eq!(job["conversation"][3]["text"], "Visible answer");
    let document = std::fs::read_to_string(
        service
            .config()
            .output_dir()
            .join(job["document_path"].as_str().unwrap()),
    )
    .unwrap();
    assert!(document.contains("> Tool call: commandExecution"));
    assert!(document.contains("> secret tool result"));
    assert!(document.contains("> Reasoning"));
    assert!(document.contains("> private reasoning"));
    assert!(document.contains("> Visible answer"));
}

#[tokio::test]
async fn job_process_output_keeps_tool_start_when_turn_is_interrupted() {
    let (_dir, service, _) = task_fixture().await;
    let (engine, _) = engine(service.clone()).await;
    let engine = engine.with_output(agentix_core::OutputConfig {
        show_reasoning: false,
        show_tool_calls: true,
    });
    engine
        .handle_agent_event(AgentEvent::ItemStarted {
            session_id: "thr_a".into(),
            turn_id: "t".into(),
            item_id: "tool".into(),
            kind: "commandExecution".into(),
            label: "cargo test".into(),
        })
        .await
        .unwrap();
    engine
        .handle_agent_event(AgentEvent::TurnCompleted {
            session_id: "thr_a".into(),
            turn_id: "t".into(),
            status: TurnStatus::Interrupted,
            error: None,
        })
        .await
        .unwrap();
    let job = serde_json::to_value(&service.store().snapshot().await.unwrap().jobs[0]).unwrap();
    assert!(job["conversation"].to_string().contains("cargo test"));
}
