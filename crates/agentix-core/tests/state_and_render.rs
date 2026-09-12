use agentix_core::{
    AgentCommand, ChannelKind, ConversationRef, GoalCommand, HistoryWatermark, ParsedInput,
    RenderKey, SessionCommand, SessionId, SqliteState, chunk_text, parse_input,
};

#[tokio::test]
async fn uncertain_event_count_excludes_completed_and_retryable_inputs_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.sqlite3");
    let state = SqliteState::open(&path).await.unwrap();
    assert_eq!(state.uncertain_event_count().await.unwrap(), 0);
    state
        .fence_event(ChannelKind::Telegram, "interrupted")
        .await
        .unwrap();
    state
        .fence_event(ChannelKind::Telegram, "interrupted")
        .await
        .unwrap();
    state
        .claim_event(ChannelKind::Telegram, "done")
        .await
        .unwrap();
    state
        .complete_event(ChannelKind::Telegram, "done")
        .await
        .unwrap();
    state
        .claim_event(ChannelKind::Telegram, "retry")
        .await
        .unwrap();
    state
        .release_event(ChannelKind::Telegram, "retry")
        .await
        .unwrap();
    assert_eq!(state.uncertain_event_count().await.unwrap(), 1);
    drop(state);
    let state = SqliteState::open(&path).await.unwrap();
    assert_eq!(state.uncertain_event_count().await.unwrap(), 1);
}

#[tokio::test]
async fn runtime_state_rejects_a_task_database_without_adding_tables() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks.sqlite3");
    let tasks = agentix_task::Store::open(&path).await.unwrap();
    assert!(SqliteState::open(&path).await.is_err());
    assert!(tasks.snapshot().await.unwrap().tasks.is_empty());
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_schema WHERE name = 'bindings'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[test]
#[allow(clippy::too_many_lines)]
fn commands_are_distinct_from_prompts() {
    assert_eq!(
        parse_input("/sessions").unwrap(),
        ParsedInput::Command(AgentCommand::Sessions)
    );
    assert_eq!(
        parse_input("/rmux").unwrap(),
        ParsedInput::Command(AgentCommand::Multiplexer {
            kind: agentix_core::MultiplexerKind::default(),
            backend: None
        })
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
    let original = "Agentix \u{2603} \u{1f680} streaming works.";
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

#[tokio::test]
async fn cancelled_inbound_events_remain_fenced_after_restart_without_losing_completed_state() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let channel = ChannelKind::Telegram;
    let state = SqliteState::open(&path).await.unwrap();
    assert!(state.claim_event(channel, "uncertain").await.unwrap());
    state.fence_event(channel, "uncertain").await.unwrap();
    state.release_event(channel, "uncertain").await.unwrap();
    // Cancellation can interrupt the first SQLite await before the caller sees
    // its result. The fence must also work when no claim row is visible yet.
    state
        .fence_event(channel, "claim-interrupted")
        .await
        .unwrap();
    assert!(state.claim_event(channel, "completed").await.unwrap());
    state.complete_event(channel, "completed").await.unwrap();
    state.fence_event(channel, "completed").await.unwrap();
    assert!(state.claim_event(channel, "failed").await.unwrap());
    state.release_event(channel, "failed").await.unwrap();
    drop(state);
    let state = SqliteState::open(&path).await.unwrap();
    for id in ["uncertain", "claim-interrupted", "completed"] {
        assert!(
            !state.claim_event(channel, id).await.unwrap(),
            "must not automatically replay {id}"
        );
    }
    assert!(state.claim_event(channel, "failed").await.unwrap());
    assert!(state.claim_event(channel, "not-started").await.unwrap());
}

#[tokio::test]
async fn checkpoint_does_not_wait_for_an_active_reader_and_preserves_bindings() {
    use sqlx::Connection;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("checkpoint.sqlite3");
    let state = SqliteState::open(&path).await.unwrap();
    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(&path);
    let mut reader = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let mut snapshot = reader.begin().await.unwrap();
    sqlx::query("SELECT * FROM bindings")
        .fetch_all(&mut *snapshot)
        .await
        .unwrap();
    let conversation = ConversationRef::new(ChannelKind::Telegram, "checkpoint");
    state
        .attach(&conversation, &SessionId::new("saved"))
        .await
        .unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), state.checkpoint()).await;
    snapshot.rollback().await.unwrap();
    result
        .expect("checkpoint must not wait for readers")
        .unwrap();
    drop(reader);
    drop(state);
    let restored = SqliteState::open(&path).await.unwrap();
    assert_eq!(
        restored.current_session(&conversation).await.unwrap(),
        Some(SessionId::new("saved"))
    );
}

#[tokio::test]
async fn slack_thread_bindings_and_event_deduplication_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slack.sqlite3");
    let chat = ConversationRef::new(ChannelKind::Slack, "T1:C1:1.000001");
    let sibling = ConversationRef::new(ChannelKind::Slack, "T1:C1:2.000001");
    let state = SqliteState::open(&path).await.unwrap();
    state
        .attach(&chat, &SessionId::new("thr_slack"))
        .await
        .unwrap();
    state
        .attach(&sibling, &SessionId::new("thr_other"))
        .await
        .unwrap();
    assert!(
        state
            .record_event(ChannelKind::Slack, "T1:1.000001:C1")
            .await
            .unwrap()
    );
    drop(state);
    let state = SqliteState::open(&path).await.unwrap();
    assert_eq!(
        state.current_session(&chat).await.unwrap(),
        Some(SessionId::new("thr_slack"))
    );
    assert_eq!(
        state.current_session(&sibling).await.unwrap(),
        Some(SessionId::new("thr_other"))
    );
    assert!(
        !state
            .record_event(ChannelKind::Slack, "T1:1.000001:C1")
            .await
            .unwrap()
    );
    assert!(
        state
            .record_event(ChannelKind::Telegram, "T1:1.000001:C1")
            .await
            .unwrap()
    );
    assert_eq!(state.list_bindings().await.unwrap().len(), 2);
}

#[test]
fn tmux_command_is_a_workspace_command() {
    assert!(parse_input("/tmux").is_ok());
    assert!(parse_input("/tmux claude").is_ok());
}
