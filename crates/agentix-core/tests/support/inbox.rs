use super::*;

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
