use agentix_core::{
    AgentCommand, ChannelKind, ConversationRef, GoalCommand, HistoryWatermark, ParsedInput,
    RenderKey, SessionCommand, SessionId, SqliteState, chunk_text, parse_input,
};

#[test]
#[allow(clippy::too_many_lines)]
fn commands_are_distinct_from_prompts() {
    assert_eq!(
        parse_input("/sessions").unwrap(),
        ParsedInput::Command(AgentCommand::Sessions)
    );
    assert_eq!(
        parse_input("/rmux").unwrap(),
        ParsedInput::Command(AgentCommand::Multiplexer)
    );
    assert!(parse_input("/mux").is_err());
    assert_eq!(
        parse_input("/attach 9f31c2ab").unwrap(),
        ParsedInput::Command(AgentCommand::Attach("9f31c2ab".into()))
    );
    assert_eq!(
        parse_input("/history older").unwrap(),
        ParsedInput::Command(AgentCommand::HistoryOlder)
    );
    assert_eq!(
        parse_input("/queue").unwrap(),
        ParsedInput::Command(AgentCommand::Queue)
    );
    assert!(parse_input("/new").is_err());
    assert_eq!(
        parse_input("/compact").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Compact))
    );
    assert_eq!(
        parse_input("/fork").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Fork))
    );
    assert_eq!(
        parse_input("/model gpt-5.6").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Model(Some(
            "gpt-5.6".into()
        ))))
    );
    assert_eq!(
        parse_input("/reasoning xhigh").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Reasoning(Some(
            "xhigh".into()
        ))))
    );
    assert!(parse_input("/thinking detailed").is_err());
    assert_eq!(
        parse_input("/skills").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Skills))
    );
    assert_eq!(
        parse_input("/plan").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Plan {
            enabled: true,
            prompt: None,
        }))
    );
    assert_eq!(
        parse_input("/plan off").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Plan {
            enabled: false,
            prompt: None,
        }))
    );
    assert_eq!(
        parse_input("/plan design a safe migration").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Plan {
            enabled: true,
            prompt: Some("design a safe migration".into()),
        }))
    );
    assert_eq!(
        parse_input("/goal ship the release").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Goal(
            GoalCommand::Set("ship the release".into())
        )))
    );
    assert_eq!(
        parse_input("/goal pause").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Goal(
            GoalCommand::Pause
        )))
    );
    assert_eq!(
        parse_input("/review").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Review))
    );
    assert_eq!(
        parse_input("/status").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Status))
    );
    assert_eq!(
        parse_input("/mcp").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Mcp))
    );
    assert_eq!(
        parse_input("/fast").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Fast(None)))
    );
    assert_eq!(
        parse_input("/fast on").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Fast(Some(true))))
    );
    assert_eq!(
        parse_input("/clear release follow-up").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Clear(Some(
            "release follow-up".into()
        ))))
    );
    assert_eq!(
        parse_input("/exit").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Exit))
    );
    assert_eq!(
        parse_input("/diff").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Diff))
    );
    assert_eq!(
        parse_input("/rename parser cleanup").unwrap(),
        ParsedInput::Command(AgentCommand::Session(SessionCommand::Rename(Some(
            "parser cleanup".into()
        ))))
    );
    assert_eq!(
        parse_input("fix the failing tests").unwrap(),
        ParsedInput::Prompt("fix the failing tests".into())
    );
    assert!(parse_input("/attach").is_err());
}

#[test]
fn chunks_respect_utf8_boundaries_and_reassemble_exactly() {
    let original = "Agentix 你好，世界。Codex streaming works.";
    let chunks = chunk_text(original, 13);

    assert!(chunks.iter().all(|chunk| chunk.len() <= 13));
    assert_eq!(chunks.concat(), original);
}

#[test]
fn hydrated_completed_items_suppress_duplicate_live_events() {
    let completed = RenderKey::new("thr_a", "turn_a", "item_a");
    let active = RenderKey::new("thr_a", "turn_b", "item_b");
    let watermark = HistoryWatermark::from_completed([completed.clone()]);

    assert!(!watermark.should_apply(&completed));
    assert!(watermark.should_apply(&active));
}

#[tokio::test]
async fn sqlite_state_persists_exclusive_bindings_and_event_deduplication() {
    let state = SqliteState::in_memory().await.unwrap();
    let telegram = ConversationRef::new(ChannelKind::Telegram, "chat-a");
    let feishu = ConversationRef::new(ChannelKind::Feishu, "chat-b");
    let first = SessionId::new("thr_first");
    let second = SessionId::new("thr_second");

    let first_result = state.attach(&telegram, &first).await.unwrap();
    assert_eq!(first_result.epoch, 1);

    let switch_result = state.attach(&telegram, &second).await.unwrap();
    assert_eq!(switch_result.previous_session, Some(first));
    assert_eq!(switch_result.epoch, 2);

    let displaced = state.attach(&feishu, &second).await.unwrap();
    assert_eq!(displaced.displaced_conversation, Some(telegram.clone()));
    assert_eq!(state.current_session(&telegram).await.unwrap(), None);
    assert_eq!(state.current_session(&feishu).await.unwrap(), Some(second));

    assert!(
        state
            .record_event(ChannelKind::Telegram, "update-1")
            .await
            .unwrap()
    );
    assert!(
        !state
            .record_event(ChannelKind::Telegram, "update-1")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn sqlite_state_lists_bindings_for_restart_recovery() {
    let state = SqliteState::in_memory().await.unwrap();
    let telegram = ConversationRef::new(ChannelKind::Telegram, "chat-a");
    let feishu = ConversationRef::new(ChannelKind::Feishu, "chat-b");
    state
        .attach(&telegram, &SessionId::new("thr-a"))
        .await
        .unwrap();
    state
        .attach(&feishu, &SessionId::new("thr-b"))
        .await
        .unwrap();

    let mut bindings = state.list_bindings().await.unwrap();
    bindings.sort_by(|left, right| left.0.conversation_id.cmp(&right.0.conversation_id));

    assert_eq!(bindings.len(), 2);
    assert_eq!(bindings[0], (telegram, SessionId::new("thr-a")));
    assert_eq!(bindings[1], (feishu, SessionId::new("thr-b")));
}

#[tokio::test]
async fn sqlite_state_suspends_a_binding_without_forgetting_its_session() {
    let state = SqliteState::in_memory().await.unwrap();
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat-a");
    let attached = state
        .attach(&conversation, &SessionId::new("thr-a"))
        .await
        .unwrap();

    let new_epoch = state.suspend(&conversation).await.unwrap();

    assert_eq!(new_epoch, attached.epoch + 1);
    assert_eq!(
        state.current_session(&conversation).await.unwrap(),
        Some(SessionId::new("thr-a"))
    );
}
