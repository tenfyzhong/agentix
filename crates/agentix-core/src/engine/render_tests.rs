use super::{DeliveryClass, Engine};
use crate::{
    AgentAdapter, AgentError, AgentEvent, ChannelAdapter, ChannelError, ChannelKind,
    ConversationRef, HistoryPage, InteractionDecision, MessageRef, OutboundView, SessionId,
    SessionPage, SqliteState,
};
use async_trait::async_trait;
use std::{
    future::{Future, poll_fn},
    sync::Arc,
    task::Poll,
    time::Duration,
};
use tokio::sync::broadcast;

// A throttled render must not call the agent or send a channel update.
struct UnusedAgent;

#[async_trait]
impl AgentAdapter for UnusedAgent {
    fn display_name(&self) -> &'static str {
        "Test"
    }
    async fn list_sessions(&self, _: Option<String>, _: u32) -> Result<SessionPage, AgentError> {
        unreachable!()
    }
    async fn read_history(
        &self,
        _: &SessionId,
        _: Option<String>,
        _: u32,
    ) -> Result<HistoryPage, AgentError> {
        unreachable!()
    }
    async fn attach(&self, _: &SessionId) -> Result<(), AgentError> {
        unreachable!()
    }
    async fn unsubscribe(&self, _: &SessionId) -> Result<(), AgentError> {
        unreachable!()
    }
    async fn start_turn(&self, _: &SessionId, _: &str) -> Result<String, AgentError> {
        unreachable!()
    }
    async fn steer(&self, _: &SessionId, _: &str, _: &str) -> Result<String, AgentError> {
        unreachable!()
    }
    async fn interrupt(&self, _: &SessionId, _: &str) -> Result<(), AgentError> {
        unreachable!()
    }
    async fn resolve_interaction(&self, _: InteractionDecision) -> Result<(), AgentError> {
        unreachable!()
    }
    fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        unreachable!()
    }
    fn generation(&self) -> u64 {
        0
    }
}

struct UnusedChannel;

#[derive(Default)]
struct CompletedTurnChannel {
    interval: Duration,
    sends: std::sync::atomic::AtomicUsize,
    updates: tokio::sync::Mutex<Vec<(MessageRef, OutboundView)>>,
}

#[async_trait]
impl ChannelAdapter for CompletedTurnChannel {
    fn streaming_update_interval(&self) -> Duration {
        self.interval
    }
    fn kind(&self) -> ChannelKind {
        ChannelKind::Telegram
    }
    async fn send(
        &self,
        conversation: &ConversationRef,
        _: &OutboundView,
    ) -> Result<MessageRef, ChannelError> {
        let id = self
            .sends
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(MessageRef::new(conversation.clone(), id.to_string()))
    }
    async fn update(
        &self,
        _: &ConversationRef,
        message: &MessageRef,
        view: &OutboundView,
    ) -> Result<(), ChannelError> {
        self.updates
            .lock()
            .await
            .push((message.clone(), view.clone()));
        Ok(())
    }
}

#[tokio::test]
async fn completed_turn_bodies_do_not_accumulate_for_a_connected_session() {
    let channel = Arc::new(CompletedTurnChannel::default());
    let engine = Engine::new(
        Arc::new(UnusedAgent),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    let session = SessionId::new("session");
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat");
    for number in 0..100 {
        let turn = format!("turn-{number}");
        engine
            .record_turn_started(session.clone(), turn.clone())
            .await
            .unwrap();
        engine
            .turns
            .buffers
            .lock()
            .await
            .get_mut(&(session.clone(), turn.clone()))
            .unwrap()
            .agent_text = "x".repeat(8192);
        engine
            .handle_turn_completed(
                &conversation,
                &session,
                turn,
                crate::TurnStatus::Completed,
                None,
                DeliveryClass::Live,
            )
            .await
            .unwrap();
    }
    assert_eq!(
        channel.sends.load(std::sync::atomic::Ordering::Relaxed),
        100
    );
    let buffers = engine.turns.buffers.lock().await;
    let bytes: usize = buffers
        .values()
        .map(|buffer| buffer.user_text.capacity() + buffer.agent_text.capacity())
        .sum();
    assert!(
        bytes <= 256 * 1024,
        "completed turns retain {bytes} bytes in {} buffers",
        buffers.len()
    );
    assert!(engine.turns.views.lock().await.is_empty());
    assert!(engine.turns.last_renders.lock().await.is_empty());
}

#[tokio::test]
async fn cold_turn_restore_updates_the_original_message_and_keeps_its_prompt() {
    let channel = Arc::new(CompletedTurnChannel::default());
    let engine = Engine::new(
        Arc::new(UnusedAgent),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    let session = SessionId::new("session");
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat");
    let key = (session.clone(), "turn".to_owned());
    engine
        .record_turn_started(session.clone(), key.1.clone())
        .await
        .unwrap();
    engine
        .turns
        .buffers
        .lock()
        .await
        .get_mut(&key)
        .unwrap()
        .user_text = "Original question".into();
    engine
        .handle_turn_completed(
            &conversation,
            &session,
            key.1.clone(),
            crate::TurnStatus::Completed,
            None,
            DeliveryClass::Live,
        )
        .await
        .unwrap();
    assert!(!engine.turns.buffers.lock().await.contains_key(&key));
    engine
        .handle_completed_item(
            &conversation,
            &session,
            &key.1,
            &crate::ItemSummary {
                id: "late".into(),
                kind: "agentMessage".into(),
                text: Some("Late answer".into()),
                status: Some("completed".into()),
            },
            DeliveryClass::Live,
        )
        .await
        .unwrap();
    let updates = channel.updates.lock().await;
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].0.message_id, "0");
    assert!(updates[0].1.body.contains("Original question"));
    assert!(updates[0].1.body.contains("Late answer"));
    assert!(updates[0].1.actions.is_empty());
    assert_eq!(channel.sends.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!(!engine.turns.buffers.lock().await.contains_key(&key));
    drop(updates);
    engine
        .cleanup_exited_turn(&conversation, &session, &key.1)
        .await;
    assert!(
        engine
            .turns
            .cold
            .session_turns(&session)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn throttled_late_items_return_to_cold_storage() {
    let channel = Arc::new(CompletedTurnChannel {
        interval: Duration::MAX,
        ..CompletedTurnChannel::default()
    });
    let engine = Engine::new(
        Arc::new(UnusedAgent),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    let session = SessionId::new("session");
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat");
    engine
        .handle_turn_completed(
            &conversation,
            &session,
            "turn".into(),
            crate::TurnStatus::Completed,
            None,
            DeliveryClass::Live,
        )
        .await
        .unwrap();
    engine
        .handle_completed_item(
            &conversation,
            &session,
            "turn",
            &crate::ItemSummary {
                id: "late".into(),
                kind: "agentMessage".into(),
                text: Some("Late answer".into()),
                status: Some("completed".into()),
            },
            DeliveryClass::Live,
        )
        .await
        .unwrap();
    assert!(channel.updates.lock().await.is_empty());
    assert!(engine.turns.buffers.lock().await.is_empty());
    assert_eq!(
        engine
            .turns
            .cold
            .load(&session, "turn")
            .await
            .unwrap()
            .unwrap()
            .buffer
            .agent_text,
        "Late answer"
    );
}

#[tokio::test]
async fn unattached_completion_releases_its_turn_buffer() {
    let engine = Engine::new(
        Arc::new(UnusedAgent),
        SqliteState::in_memory().await.unwrap(),
        vec![Arc::new(UnusedChannel)],
    );
    let session = SessionId::new("session");
    engine
        .record_turn_started(session.clone(), "turn".into())
        .await
        .unwrap();
    engine
        .notify_unattached_turn_completion(&session, "turn", &crate::TurnStatus::Completed, None)
        .await
        .unwrap();
    assert!(engine.turns.buffers.lock().await.is_empty());
    engine.handle_session_exit(&session).await.unwrap();
    assert!(
        engine
            .turns
            .cold
            .session_turns(&session)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn hydrating_a_new_running_turn_releases_the_previous_body() {
    let engine = Engine::new(
        Arc::new(UnusedAgent),
        SqliteState::in_memory().await.unwrap(),
        vec![Arc::new(CompletedTurnChannel::default())],
    );
    let session = SessionId::new("session");
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat");
    for id in ["first", "second"] {
        engine
            .hydrate_running_turn(
                &conversation,
                &session,
                &crate::TurnSummary {
                    id: id.into(),
                    status: crate::TurnStatus::InProgress,
                    user_text: Some("Question".into()),
                    agent_text: Some("Answer".into()),
                    tools: Vec::new(),
                    items: Vec::new(),
                },
            )
            .await
            .unwrap();
    }
    assert_eq!(engine.turns.buffers.lock().await.len(), 1);
    assert_eq!(
        engine.turns.cold.session_turns(&session).await.unwrap(),
        vec!["first"]
    );
}

#[tokio::test]
async fn failed_turn_archive_keeps_hot_state_for_retry() {
    let channel = Arc::new(CompletedTurnChannel::default());
    let engine = Engine::new(
        Arc::new(UnusedAgent),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    let session = SessionId::new("session");
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat");
    engine
        .handle_turn_completed(
            &conversation,
            &session,
            "first".into(),
            crate::TurnStatus::Completed,
            None,
            DeliveryClass::Live,
        )
        .await
        .unwrap();
    engine.turns.cold.reject_writes(true).await;
    let key = (session.clone(), "second".to_owned());
    engine
        .record_turn_started(session.clone(), key.1.clone())
        .await
        .unwrap();
    engine
        .turns
        .buffers
        .lock()
        .await
        .get_mut(&key)
        .unwrap()
        .agent_text = "Preserve this answer".into();
    assert!(
        engine
            .handle_turn_completed(
                &conversation,
                &session,
                key.1.clone(),
                crate::TurnStatus::Completed,
                None,
                DeliveryClass::Live
            )
            .await
            .is_err()
    );
    assert_eq!(
        engine.turns.buffers.lock().await[&key].agent_text,
        "Preserve this answer"
    );
    assert!(engine.turns.views.lock().await.contains_key(&key));
    engine.turns.cold.reject_writes(false).await;
    engine
        .handle_turn_completed(
            &conversation,
            &session,
            key.1.clone(),
            crate::TurnStatus::Completed,
            None,
            DeliveryClass::Live,
        )
        .await
        .unwrap();
    assert!(engine.turns.buffers.lock().await.is_empty());
    assert_eq!(channel.sends.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert_eq!(
        engine
            .turns
            .cold
            .load(&session, &key.1)
            .await
            .unwrap()
            .unwrap()
            .buffer
            .agent_text,
        "Preserve this answer"
    );
}

#[async_trait]
impl ChannelAdapter for UnusedChannel {
    fn streaming_update_interval(&self) -> Duration {
        Duration::MAX
    }
    fn kind(&self) -> ChannelKind {
        ChannelKind::Telegram
    }
    async fn send(
        &self,
        _: &ConversationRef,
        _: &OutboundView,
    ) -> Result<MessageRef, ChannelError> {
        unreachable!()
    }
    async fn update(
        &self,
        _: &ConversationRef,
        _: &MessageRef,
        _: &OutboundView,
    ) -> Result<(), ChannelError> {
        unreachable!()
    }
}

#[tokio::test]
async fn throttled_render_does_not_wait_for_the_session_cache() {
    let engine = Engine::new(
        Arc::new(UnusedAgent),
        SqliteState::in_memory().await.unwrap(),
        vec![Arc::new(UnusedChannel)],
    );
    let session = SessionId::new("session");
    let key = (session.clone(), "turn".to_owned());
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat");
    assert!(engine.turns.should_render(&key, false, Duration::MAX).await);
    let cache = engine.sessions.cache.lock().await;
    let mut throttled =
        Box::pin(engine.render_turn(&conversation, &session, "turn", DeliveryClass::Live, false));
    assert!(
        matches!(
            poll_fn(|cx| Poll::Ready(throttled.as_mut().poll(cx))).await,
            Poll::Ready(Ok(()))
        ),
        "throttled updates must skip the session cache"
    );
    let mut forced =
        Box::pin(engine.render_turn(&conversation, &session, "turn", DeliveryClass::Live, true));
    assert!(
        poll_fn(|cx| Poll::Ready(forced.as_mut().poll(cx)))
            .await
            .is_pending(),
        "forced updates must still read the label"
    );
    drop(cache);
}

#[tokio::test]
async fn configured_process_output_survives_final_answer_and_deduplicates_items() {
    for (reasoning, tools) in [(false, false), (true, false), (false, true), (true, true)] {
        let engine = Engine::new(
            Arc::new(UnusedAgent),
            SqliteState::in_memory().await.unwrap(),
            vec![],
        )
        .with_output(crate::OutputConfig {
            show_reasoning: reasoning,
            show_tool_calls: tools,
        });
        let session = SessionId::new("s");
        for (id, kind, text) in [
            ("r", "reasoning", "Consider options"),
            ("t", "commandExecution", "cargo test"),
            ("t", "commandExecution", "cargo test"),
            ("a", "agentMessage", "Final answer"),
        ] {
            engine
                .apply_completed_item(
                    &session,
                    "turn",
                    &crate::ItemSummary {
                        id: id.into(),
                        kind: kind.into(),
                        text: Some(text.into()),
                        status: Some("completed".into()),
                    },
                )
                .await;
        }
        let buffers = engine.turns.buffers.lock().await;
        let body = super::presentation::live_turn_body(
            "Test",
            &buffers[&(session, "turn".into())],
            DeliveryClass::Live,
        );
        let sections = buffers.values().next().unwrap().view_sections("Test");
        assert_eq!(
            sections.len(),
            1 + usize::from(reasoning) + usize::from(tools)
        );
        assert_eq!(sections.last().unwrap().body, "Final answer");
        assert_eq!(
            sections.iter().any(|s| s.title == "🧠 Reasoning"),
            reasoning
        );
        assert_eq!(sections.iter().any(|s| s.title == "🔨 Tool Call"), tools);
        assert_eq!(body.contains("Consider options"), reasoning);
        assert_eq!(body.contains("cargo test"), tools);
        assert!(body.contains("Final answer"));
        if reasoning {
            assert!(body.contains("**Reasoning**\n>\n> Consider options"));
        }
        if tools {
            assert!(body.contains("**Tool call**: commandExecution (completed)\n>\n> cargo test"));
        }
        if reasoning || tools {
            let last_process = if tools {
                "cargo test"
            } else {
                "Consider options"
            };
            assert!(body.contains(&format!(
                "{last_process}\n>\n>\n> **Output**\n>\n> Final answer"
            )));
        } else {
            assert!(!body.contains("**Output**"));
        }
        if reasoning && tools {
            assert!(body.contains("Consider options\n>\n>\n> **Tool call**"));
        }
        assert!(body.matches("cargo test").count() <= 1);
    }
}

#[tokio::test]
async fn interleaved_process_and_output_blocks_survive_updates_and_cold_storage() {
    let engine = Engine::new(
        Arc::new(UnusedAgent),
        SqliteState::in_memory().await.unwrap(),
        vec![],
    )
    .with_output(crate::OutputConfig {
        show_reasoning: true,
        show_tool_calls: true,
    });
    let session = SessionId::new("interleaved");
    for (id, kind, text) in [
        ("r1", "reasoning", "First thought\nMore thought"),
        ("t1", "commandExecution", "First command\nFirst result"),
        ("a1", "agentMessage", "Progress report\nStill working"),
        ("r2", "reasoning", "Second thought\nAnother thought"),
        ("t2", "commandExecution", "Second command\nPending"),
        ("a2", "agentMessage", "Final answer\nDetails"),
        ("t2", "commandExecution", "Second command\nSecond result"),
        ("a2", "agentMessage", "Final answer\nDetails"),
    ] {
        engine
            .apply_completed_item(
                &session,
                "turn",
                &crate::ItemSummary {
                    id: id.into(),
                    kind: kind.into(),
                    text: Some(text.into()),
                    status: Some("completed".into()),
                },
            )
            .await;
    }
    engine.archive_turn(&session, "turn").await.unwrap();
    engine.restore_cold_turn(&session, "turn").await.unwrap();
    let buffers = engine.turns.buffers.lock().await;
    let body = super::presentation::live_turn_body(
        "Test",
        &buffers[&(session, "turn".into())],
        DeliveryClass::Live,
    );
    let mut remaining = body.as_str();
    for block in [
        "**Reasoning**\n>\n> First thought\n> More thought",
        "**Tool call**: commandExecution (completed)\n>\n> First command\n> First result",
        "**Output**\n>\n> Progress report\n> Still working",
        "**Reasoning**\n>\n> Second thought\n> Another thought",
        "**Tool call**: commandExecution (completed)\n>\n> Second command\n> Second result",
        "**Output**\n>\n> Final answer\n> Details",
    ] {
        remaining = remaining
            .split_once(block)
            .unwrap_or_else(|| panic!("missing or reordered block {block}: {body}"))
            .1;
    }
    assert_eq!(body.matches("Final answer").count(), 1);
    assert_eq!(body.matches("Second command").count(), 1);
    assert!(!body.contains("Pending"));
}

#[tokio::test]
async fn streamed_output_updates_its_own_block_without_replacing_process_messages() {
    let channel = Arc::new(CompletedTurnChannel::default());
    let engine = Engine::new(
        Arc::new(UnusedAgent),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    )
    .with_output(crate::OutputConfig {
        show_reasoning: true,
        show_tool_calls: true,
    });
    let session = SessionId::new("streamed");
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat");
    for (id, kind, text) in [
        ("r", "reasoning", "Thinking\nCarefully"),
        ("a1", "delta", "Checking"),
        ("t", "commandExecution", "Run tests"),
        ("a1", "delta", " now"),
        ("a1", "agentMessage", "Checking now"),
        ("a2", "delta", "Done"),
        ("a2", "agentMessage", "Done!"),
    ] {
        if kind == "delta" {
            engine
                .handle_message_delta(
                    &conversation,
                    &session,
                    "turn",
                    id,
                    text,
                    DeliveryClass::Live,
                )
                .await
                .unwrap();
        } else {
            engine
                .handle_completed_item(
                    &conversation,
                    &session,
                    "turn",
                    &crate::ItemSummary {
                        id: id.into(),
                        kind: kind.into(),
                        text: Some(text.into()),
                        status: Some("completed".into()),
                    },
                    DeliveryClass::Live,
                )
                .await
                .unwrap();
        }
    }
    engine
        .handle_turn_completed(
            &conversation,
            &session,
            "turn".into(),
            crate::TurnStatus::Failed,
            Some("Example error".into()),
            DeliveryClass::Live,
        )
        .await
        .unwrap();
    let updates = channel.updates.lock().await;
    for (_, view) in updates.iter() {
        assert!(view.body.contains("Thinking\n> Carefully"));
    }
    let body = &updates.last().unwrap().1.body;
    let mut remaining = body.as_str();
    for text in [
        "Thinking",
        "Checking now",
        "Run tests",
        "Done!",
        "Error: Example error",
    ] {
        remaining = remaining.split_once(text).unwrap().1;
        assert_eq!(body.matches(text).count(), 1);
    }
}

#[tokio::test]
async fn commentary_is_reasoning_and_turn_view_has_ordered_sections() {
    let engine = Engine::new(
        Arc::new(UnusedAgent),
        SqliteState::in_memory().await.unwrap(),
        vec![],
    )
    .with_output(crate::OutputConfig {
        show_reasoning: true,
        show_tool_calls: true,
    });
    let session = SessionId::new("sections");
    for (id, kind, text) in [
        ("u", "userMessage", "Question"),
        ("c1", "commentary", "I will check the card implementation."),
        ("t1", "commandExecution", "Check source"),
        ("c2", "commentary", "I found the component."),
        ("t2", "commandExecution", "Run tests"),
        ("a", "agentMessage", "Final answer"),
    ] {
        engine
            .apply_completed_item(
                &session,
                "turn",
                &crate::ItemSummary {
                    id: id.into(),
                    kind: kind.into(),
                    text: Some(text.into()),
                    status: None,
                },
            )
            .await;
    }
    let buffers = engine.turns.buffers.lock().await;
    let view = super::presentation::live_turn_view(
        "Codex",
        "session",
        "turn",
        &buffers[&(session, "turn".into())],
        DeliveryClass::Live,
    );
    assert!(view.body.contains("**Reasoning**\n>\n> I will check"));
    let json = serde_json::to_value(view).unwrap();
    let sections = json["sections"].as_array().expect("structured sections");
    let titles: Vec<_> = sections
        .iter()
        .map(|s| s["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        [
            "👤 You",
            "🧠 Reasoning",
            "🔨 Tool Call",
            "🧠 Reasoning",
            "🔨 Tool Call",
            "🤖 Codex"
        ]
    );
    assert_eq!(sections[1]["body"], "I will check the card implementation.");
    assert_eq!(sections[5]["body"], "Final answer");
}

#[tokio::test]
async fn commentary_deltas_remain_in_reasoning_and_obey_visibility() {
    for visible in [false, true] {
        let channel = Arc::new(CompletedTurnChannel::default());
        let engine = Engine::new(
            Arc::new(UnusedAgent),
            SqliteState::in_memory().await.unwrap(),
            vec![channel],
        )
        .with_output(crate::OutputConfig {
            show_reasoning: visible,
            show_tool_calls: true,
        });
        let session = SessionId::new("commentary-stream");
        let conversation = ConversationRef::new(ChannelKind::Telegram, "chat");
        engine
            .handle_routed_event(
                conversation.clone(),
                session.clone(),
                AgentEvent::ItemStarted {
                    session_id: session.to_string(),
                    turn_id: "turn".into(),
                    item_id: "c".into(),
                    kind: "commentary".into(),
                    label: "agentMessage".into(),
                },
                DeliveryClass::Live,
            )
            .await
            .unwrap();
        for delta in ["I will ", "check."] {
            engine
                .handle_message_delta(
                    &conversation,
                    &session,
                    "turn",
                    "c",
                    delta,
                    DeliveryClass::Live,
                )
                .await
                .unwrap();
            let buffers = engine.turns.buffers.lock().await;
            let buffer = &buffers[&(session.clone(), "turn".into())];
            assert!(buffer.agent_text.is_empty());
            let sections = buffer.view_sections("Codex");
            assert_eq!(sections.len(), usize::from(visible));
            if visible {
                assert_eq!(sections[0].title, "🧠 Reasoning");
            }
        }
        for (id, kind, text) in [
            ("c", "commentary", "I will check."),
            ("a", "agentMessage", "Answer"),
        ] {
            engine
                .apply_completed_item(
                    &session,
                    "turn",
                    &crate::ItemSummary {
                        id: id.into(),
                        kind: kind.into(),
                        text: Some(text.into()),
                        status: None,
                    },
                )
                .await;
        }
        let buffers = engine.turns.buffers.lock().await;
        let buffer = &buffers[&(session, "turn".into())];
        let sections = buffer.view_sections("Codex");
        assert_eq!(sections.len(), 1 + usize::from(visible));
        assert_eq!(sections.last().unwrap().body, "Answer");
        if visible {
            assert_eq!(sections[0].body, "I will check.");
        }
    }
}

#[tokio::test]
async fn repeated_commentary_start_preserves_streamed_and_completed_content() {
    let engine = Engine::new(
        Arc::new(UnusedAgent),
        SqliteState::in_memory().await.unwrap(),
        vec![Arc::new(CompletedTurnChannel::default())],
    )
    .with_output(crate::OutputConfig {
        show_reasoning: true,
        show_tool_calls: true,
    });
    let session = SessionId::new("repeat");
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat");
    for completed in [false, true] {
        engine
            .handle_routed_event(
                conversation.clone(),
                session.clone(),
                AgentEvent::ItemStarted {
                    session_id: session.to_string(),
                    turn_id: "turn".into(),
                    item_id: "c".into(),
                    kind: "commentary".into(),
                    label: "agentMessage".into(),
                },
                DeliveryClass::Live,
            )
            .await
            .unwrap();
        if completed {
            engine
                .apply_completed_item(
                    &session,
                    "turn",
                    &crate::ItemSummary {
                        id: "c".into(),
                        kind: "commentary".into(),
                        text: Some("I will check.".into()),
                        status: None,
                    },
                )
                .await;
        } else {
            engine
                .handle_message_delta(
                    &conversation,
                    &session,
                    "turn",
                    "c",
                    "I will check.",
                    DeliveryClass::Live,
                )
                .await
                .unwrap();
        }
        engine
            .handle_routed_event(
                conversation.clone(),
                session.clone(),
                AgentEvent::ItemStarted {
                    session_id: session.to_string(),
                    turn_id: "turn".into(),
                    item_id: "c".into(),
                    kind: "commentary".into(),
                    label: "agentMessage".into(),
                },
                DeliveryClass::Live,
            )
            .await
            .unwrap();
        let buffers = engine.turns.buffers.lock().await;
        let sections = buffers[&(session.clone(), "turn".into())].view_sections("Codex");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "🧠 Reasoning");
        assert_eq!(sections[0].body, "I will check.");
    }
}

#[tokio::test]
async fn late_commentary_classification_replaces_output_and_survives_cold_restore() {
    for visible in [false, true] {
        let engine = Engine::new(
            Arc::new(UnusedAgent),
            SqliteState::in_memory().await.unwrap(),
            vec![Arc::new(CompletedTurnChannel::default())],
        )
        .with_output(crate::OutputConfig {
            show_reasoning: visible,
            show_tool_calls: true,
        });
        let session = SessionId::new("late-phase");
        let conversation = ConversationRef::new(ChannelKind::Telegram, "chat");
        engine
            .handle_message_delta(
                &conversation,
                &session,
                "turn",
                "c",
                "Checking",
                DeliveryClass::Live,
            )
            .await
            .unwrap();
        for (id, kind, text) in [
            ("c", "commentary", "Checking"),
            ("a", "agentMessage", "Answer"),
            ("c", "commentary", "Checking"),
        ] {
            engine
                .apply_completed_item(
                    &session,
                    "turn",
                    &crate::ItemSummary {
                        id: id.into(),
                        kind: kind.into(),
                        text: Some(text.into()),
                        status: None,
                    },
                )
                .await;
        }
        let before = engine.turns.buffers.lock().await[&(session.clone(), "turn".into())]
            .view_sections("Codex");
        assert_eq!(before.len(), 1 + usize::from(visible));
        assert_eq!(before.last().unwrap().body, "Answer");
        if visible {
            assert_eq!(before[0].body, "Checking");
        }
        engine.archive_turn(&session, "turn").await.unwrap();
        engine.restore_cold_turn(&session, "turn").await.unwrap();
        assert_eq!(
            engine.turns.buffers.lock().await[&(session, "turn".into())].view_sections("Codex"),
            before
        );
    }
}
