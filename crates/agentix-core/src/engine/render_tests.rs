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
async fn consecutive_process_blocks_merge_and_survive_updates_and_cold_storage() {
    for structured in [false, true] {
        let engine = Engine::new(
            Arc::new(UnusedAgent),
            SqliteState::in_memory().await.unwrap(),
            vec![],
        )
        .with_output(crate::OutputConfig {
            show_reasoning: true,
            show_tool_calls: true,
        });
        let session = SessionId::new("consecutive");
        for (id, kind, text) in [
            ("r1", "reasoning", "First thought"),
            ("r2", "commentary", "Next thought"),
            ("t1", "commandExecution", "First command"),
            ("t2", "fileChange", "Pending change"),
            ("r3", "thinking", "Last thought"),
            ("t3", "commandExecution", "Last command"),
            ("a", "agentMessage", "Final answer"),
            ("t2", "fileChange", "Updated change"),
            ("t2", "fileChange", "Updated change"),
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
        for restored in [false, true] {
            if restored {
                engine.archive_turn(&session, "turn").await.unwrap();
                engine.restore_cold_turn(&session, "turn").await.unwrap();
            }
            let buffers = engine.turns.buffers.lock().await;
            let buffer = &buffers[&(session.clone(), "turn".into())];
            if structured {
                let sections = buffer.view_sections("Test");
                assert_eq!(
                    sections
                        .iter()
                        .map(|s| s.title.as_str())
                        .collect::<Vec<_>>(),
                    [
                        "🧠 Reasoning",
                        "🔨 Tool Call",
                        "🧠 Reasoning",
                        "🔨 Tool Call",
                        "🤖 Test"
                    ]
                );
                assert_eq!(sections[0].body, "First thought\n\nNext thought");
                assert_eq!(
                    sections[1].body,
                    "commandExecution (completed)\n\nFirst command\n\nfileChange (completed)\n\nUpdated change"
                );
                assert_eq!(sections[4].body, "Final answer");
            } else {
                let output = buffer.render_output();
                assert_eq!(output.matches("**Reasoning**").count(), 2, "{output}");
                assert_eq!(output.matches("**Tool call**").count(), 2, "{output}");
                assert!(output.contains("First thought\n\nNext thought"));
                assert!(
                    output.contains("First command\n\nfileChange (completed)\n\nUpdated change")
                );
                assert_eq!(output.matches("Updated change").count(), 1);
                assert!(!output.contains("Pending change"));
                assert!(output.ends_with("**Output**\n\nFinal answer"));
            }
        }
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

#[tokio::test]
async fn reconfigured_engine_preserves_live_state_and_changes_output_policy() {
    let state = SqliteState::in_memory().await.unwrap();
    let previous = Engine::new(Arc::new(UnusedAgent), state.clone(), vec![]);
    previous
        .turns
        .active
        .lock()
        .await
        .insert(SessionId::new("live"), "turn".into());
    let mut next = Engine::new(Arc::new(UnusedAgent), state, vec![])
        .with_output(crate::OutputConfig {
            show_reasoning: true,
            show_tool_calls: true,
        })
        .with_background_turn_notifications(false);
    next.inherit_runtime(&previous);
    assert_eq!(
        next.turns.active.lock().await.get(&SessionId::new("live")),
        Some(&"turn".into())
    );
    next.turns
        .active
        .lock()
        .await
        .insert(SessionId::new("later"), "turn2".into());
    assert!(
        previous
            .turns
            .active
            .lock()
            .await
            .contains_key(&SessionId::new("later"))
    );
    assert!(next.output.show_reasoning);
    assert!(!next.background_turn_notifications);
    assert!(!previous.output.show_reasoning);
}

#[test]
fn structured_output_preserves_process_blocks_after_agent_output() {
    let mut buffer = super::TurnBuffer::default();
    for (id, text, process) in [
        ("r1", "**Reasoning**\n\nFirst", true),
        ("a1", "Progress", false),
        ("r2", "**Reasoning**\n\nNext", true),
        ("t1", "**Tool call**: commandExecution", true),
        ("a2", "Done", false),
    ] {
        buffer.record_output(Some(id), text, process, false);
    }
    let sections = buffer.view_sections("Test");
    assert_eq!(
        sections
            .iter()
            .map(|s| s.title.as_str())
            .collect::<Vec<_>>(),
        [
            "🧠 Reasoning",
            "🤖 Test",
            "🧠 Reasoning",
            "🔨 Tool Call",
            "🤖 Test"
        ]
    );
    assert_eq!(sections[1].body, "Progress");
    assert_eq!(sections[4].body, "Done");
}

#[test]
fn summary_only_history_adopts_the_first_completed_output_id_without_duplication() {
    let mut buffer = super::TurnBuffer::from_summary(
        &crate::TurnSummary {
            id: "legacy".into(),
            status: crate::TurnStatus::InProgress,
            user_text: Some("Question".into()),
            agent_text: Some("Partial answer".into()),
            tools: vec![],
            items: vec![],
        },
        crate::OutputConfig::default(),
    );
    buffer.record_output(Some("answer"), "Complete answer", false, false);
    assert_eq!(buffer.agent_text, "Complete answer");
    assert_eq!(buffer.view_sections("Test")[1].body, "Complete answer");
}

#[tokio::test]
async fn history_process_summary_adopts_real_answer_id_after_cold_restore() {
    for append in [false, true] {
        let engine = Engine::new(
            Arc::new(UnusedAgent),
            SqliteState::in_memory().await.unwrap(),
            vec![],
        );
        let session = SessionId::new("pi-summary");
        let turn = crate::TurnSummary {
            id: "turn".into(),
            status: crate::TurnStatus::InProgress,
            user_text: Some("Question".into()),
            agent_text: Some("Partial".into()),
            tools: vec![],
            items: vec![crate::ItemSummary {
                id: "tool".into(),
                kind: "commandExecution".into(),
                text: Some("Checking".into()),
                status: None,
            }],
        };
        engine.turns.buffers.lock().await.insert(
            (session.clone(), turn.id.clone()),
            super::TurnBuffer::from_summary(
                &turn,
                crate::OutputConfig {
                    show_reasoning: true,
                    show_tool_calls: true,
                },
            ),
        );
        engine.archive_turn(&session, "turn").await.unwrap();
        engine.restore_cold_turn(&session, "turn").await.unwrap();
        let mut buffers = engine.turns.buffers.lock().await;
        let buffer = buffers.get_mut(&(session.clone(), "turn".into())).unwrap();
        if append {
            buffer.record_output(Some("answer"), " answer", false, true);
        }
        for _ in 0..2 {
            buffer.record_output(Some("answer"), "Complete answer", false, false);
        }
        assert_eq!(buffer.agent_text, "Complete answer");
        assert_eq!(buffer.output_items.len(), 2);
        assert!(buffer.view_sections("Pi")[0].body.contains("Question"));
        buffer.record_output(Some("next"), "Another answer", false, false);
        assert_eq!(buffer.agent_text, "Complete answer\n\nAnother answer");
    }
}

#[test]
fn history_preserves_aggregate_user_input_and_aggregates_items_when_missing() {
    for summary in [Some("First\n\nSecond"), None] {
        let turn = crate::TurnSummary {
            id: "turn".into(),
            status: crate::TurnStatus::Completed,
            user_text: summary.map(str::to_owned),
            agent_text: Some("Answer".into()),
            tools: vec![],
            items: [("u1", "First"), ("u2", "Second")]
                .into_iter()
                .map(|(id, text)| crate::ItemSummary {
                    id: id.into(),
                    kind: "userMessage".into(),
                    text: Some(text.into()),
                    status: None,
                })
                .collect(),
        };
        let buffer = super::TurnBuffer::from_summary(&turn, crate::OutputConfig::default());
        assert_eq!(buffer.user_text, "First\n\nSecond");
    }
}

#[test]
fn history_merge_is_idempotent_and_retains_existing_item_order() {
    let mut buffer = super::TurnBuffer::default();
    buffer.record_output(Some("early"), "**Reasoning**\n\nEarly", true, false);
    buffer.record_output(Some("overlap"), "**Tool call**: Pending", true, false);
    let turn = crate::TurnSummary {
        id: "turn".into(),
        status: crate::TurnStatus::Completed,
        user_text: None,
        agent_text: Some("Done".into()),
        tools: vec![],
        items: vec![crate::ItemSummary {
            id: "overlap".into(),
            kind: "commandExecution".into(),
            text: Some("Updated".into()),
            status: None,
        }],
    };
    for _ in 0..2 {
        buffer.merge_summary(
            &turn,
            crate::OutputConfig {
                show_reasoning: true,
                show_tool_calls: true,
            },
        );
    }
    assert_eq!(buffer.output_items.len(), 3);
    assert_eq!(buffer.output_items[0].id.as_deref(), Some("early"));
    assert_eq!(buffer.output_items[1].id.as_deref(), Some("overlap"));
    assert!(buffer.output_items[1].text.contains("Updated"));
    assert_eq!(buffer.agent_text, "Done");
}

#[tokio::test]
async fn cumulative_answer_closes_process_panel_without_reordering_and_survives_cold_restore() {
    let engine = Engine::new(
        Arc::new(UnusedAgent),
        SqliteState::in_memory().await.unwrap(),
        vec![],
    );
    let session = SessionId::new("cumulative");
    let mut buffer = super::TurnBuffer::default();
    buffer.record_output(Some("assistant"), "Checking", false, false);
    buffer.record_output(Some("tool"), "**Tool call**: Running", true, false);
    let sections = serde_json::to_value(buffer.view_sections("Pi")).unwrap();
    assert_eq!(sections[1]["expanded"], true);
    buffer.record_output(Some("assistant"), "Checking. Done", false, false);
    engine
        .turns
        .buffers
        .lock()
        .await
        .insert((session.clone(), "t".into()), buffer);
    engine.archive_turn(&session, "t").await.unwrap();
    engine.restore_cold_turn(&session, "t").await.unwrap();
    let mut buffers = engine.turns.buffers.lock().await;
    let buffer = buffers.get_mut(&(session, "t".into())).unwrap();
    buffer.record_output(Some("tool"), "**Tool call**: Completed", true, false);
    let sections = serde_json::to_value(buffer.view_sections("Pi")).unwrap();
    assert_eq!(sections[0]["body"], "Checking. Done");
    assert_eq!(sections[1]["expanded"], false);
    buffer.record_output(
        Some("reasoning"),
        "**Reasoning**\n\nNext thought",
        true,
        false,
    );
    let sections = serde_json::to_value(buffer.view_sections("Pi")).unwrap();
    assert_eq!(sections[1]["expanded"], false);
    assert_eq!(sections[2]["expanded"], true);
}

#[test]
fn repeated_answer_completion_does_not_close_current_tool() {
    let mut buffer = super::TurnBuffer::default();
    buffer.record_output(Some("a"), "Progress", false, false);
    buffer.record_output(Some("t"), "**Tool call**: Running", true, false);
    buffer.record_output(Some("a"), "Progress", false, false);
    assert_eq!(buffer.view_sections("Pi")[1].expanded, Some(true));
}

#[test]
fn resumed_commentary_stream_reopens_its_panel() {
    let mut buffer = super::TurnBuffer::default();
    buffer.record_output(Some("r"), "**Reasoning**\n\nFirst", true, false);
    buffer.record_output(Some("t"), "**Tool call**: Running", true, false);
    buffer.append_commentary("r", " then more");
    let sections = buffer.view_sections("Codex");
    assert_eq!(sections[0].expanded, Some(true));
    assert_eq!(sections[1].expanded, Some(false));
}

#[test]
fn merging_unchanged_history_preserves_active_tool() {
    let output = crate::OutputConfig {
        show_reasoning: true,
        show_tool_calls: true,
    };
    let turn = crate::TurnSummary {
        id: "t".into(),
        status: crate::TurnStatus::InProgress,
        user_text: None,
        agent_text: Some("Progress".into()),
        tools: vec![],
        items: vec![
            crate::ItemSummary {
                id: "a".into(),
                kind: "agentMessage".into(),
                text: Some("Progress".into()),
                status: None,
            },
            crate::ItemSummary {
                id: "tool".into(),
                kind: "commandExecution".into(),
                text: Some("Running".into()),
                status: None,
            },
        ],
    };
    let mut buffer = super::TurnBuffer::from_summary(&turn, output);
    let before = buffer.view_sections("Codex");
    buffer.merge_summary(&turn, output);
    assert_eq!(buffer.view_sections("Codex"), before);
}

#[test]
fn unchanged_summary_fallback_merge_keeps_the_active_tool() {
    let mut buffer = super::TurnBuffer::default();
    buffer.record_output(Some("a"), "Progress", false, false);
    buffer.record_output(Some("tool"), "**Tool call**: Running", true, false);
    let turn = crate::TurnSummary {
        id: "t".into(),
        status: crate::TurnStatus::InProgress,
        user_text: None,
        agent_text: Some("Progress".into()),
        tools: vec![],
        items: vec![],
    };
    buffer.merge_summary(&turn, crate::OutputConfig::default());
    assert_eq!(buffer.view_sections("Pi")[1].expanded, Some(true));
}

#[test]
fn hidden_commentary_placeholder_does_not_change_plain_output_format() {
    let mut buffer = super::TurnBuffer::default();
    buffer.record_output(Some("hidden"), "", true, false);
    buffer.record_output(Some("answer"), "Done", false, false);
    assert_eq!(buffer.render_output(), "Done");
}

#[derive(Default)]
struct SlowInputAgent {
    reads: std::sync::atomic::AtomicUsize,
    release: tokio::sync::Notify,
}

#[async_trait]
impl AgentAdapter for SlowInputAgent {
    fn display_name(&self) -> &'static str {
        "Test"
    }
    async fn list_sessions(&self, _: Option<String>, _: u32) -> Result<SessionPage, AgentError> {
        Ok(SessionPage {
            sessions: vec![],
            next_cursor: None,
        })
    }
    async fn read_history(
        &self,
        _: &SessionId,
        _: Option<String>,
        _: u32,
    ) -> Result<HistoryPage, AgentError> {
        self.reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.release.notified().await;
        Ok(HistoryPage {
            turns: vec![],
            older_cursor: None,
            newer_cursor: None,
        })
    }
    async fn attach(&self, _: &SessionId) -> Result<(), AgentError> {
        unreachable!()
    }
    async fn unsubscribe(&self, _: &SessionId) -> Result<(), AgentError> {
        Ok(())
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
    async fn read_turn_input(&self, _: &SessionId, _: &str) -> Result<Option<String>, AgentError> {
        self.reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.release.notified().await;
        Ok(Some("Recovered input".into()))
    }
    fn generation(&self) -> u64 {
        0
    }
}

async fn render_with_slow_input(existing_input: &str) {
    let agent = Arc::new(SlowInputAgent::default());
    let channel = Arc::new(CompletedTurnChannel::default());
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    let session = SessionId::new("slow-input-session");
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat");
    engine
        .record_turn_started(session.clone(), "turn".into())
        .await
        .unwrap();
    {
        let mut buffers = engine.turns.buffers.lock().await;
        let buffer = buffers.get_mut(&(session.clone(), "turn".into())).unwrap();
        buffer.user_text = existing_input.into();
        buffer.agent_text = "Already received answer".into();
        buffer.status = crate::TurnStatus::Completed;
    }
    // Keep the history read pending: neither displaying the answer nor releasing
    // the session dispatch lane should depend on the remote read finishing.
    let result = tokio::time::timeout(
        Duration::from_millis(100),
        engine.render_turn(&conversation, &session, "turn", DeliveryClass::Live, true),
    )
    .await;
    assert!(
        result.is_ok(),
        "slow input recovery blocked render completion"
    );
    result.unwrap().unwrap();
    assert_eq!(channel.sends.load(std::sync::atomic::Ordering::Relaxed), 1);
    if !existing_input.is_empty() {
        assert_eq!(agent.reads.load(std::sync::atomic::Ordering::Relaxed), 0);
    }
}

#[tokio::test]
async fn slow_input_recovery_does_not_block_received_output() {
    render_with_slow_input("").await;
}

#[tokio::test]
async fn existing_input_skips_slow_input_recovery() {
    render_with_slow_input("Native user input").await;
}

async fn slow_input_fixture() -> (
    Engine,
    Arc<SlowInputAgent>,
    Arc<CompletedTurnChannel>,
    SessionId,
    ConversationRef,
) {
    let agent = Arc::new(SlowInputAgent::default());
    let channel = Arc::new(CompletedTurnChannel::default());
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    let session = SessionId::new("recover-input");
    let conversation = ConversationRef::new(ChannelKind::Telegram, "chat");
    engine
        .record_turn_started(session.clone(), "turn".into())
        .await
        .unwrap();
    {
        let mut buffers = engine.turns.buffers.lock().await;
        let buffer = buffers.get_mut(&(session.clone(), "turn".into())).unwrap();
        buffer.status = crate::TurnStatus::Completed;
        buffer.agent_text = "Already received answer".into();
    }
    engine
        .render_turn(&conversation, &session, "turn", DeliveryClass::Live, true)
        .await
        .unwrap();
    (engine, agent, channel, session, conversation)
}

async fn complete_input_read(engine: &Engine, agent: &SlowInputAgent) -> super::EngineWork {
    agent.release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), engine.next_input_recovery())
        .await
        .unwrap()
}

#[tokio::test]
async fn slow_input_recovery_updates_the_original_completed_card() {
    let (engine, agent, channel, session, _) = slow_input_fixture().await;
    let work = complete_input_read(&engine, &agent).await;
    engine.execute_work(work).await.unwrap();
    assert_eq!(channel.sends.load(std::sync::atomic::Ordering::Relaxed), 1);
    let updates = channel.updates.lock().await;
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].0.message_id, "0");
    let view = &updates[0].1;
    assert!(format!("{view:?}").contains("Recovered input"));
    assert!(format!("{view:?}").contains("Already received answer"));
    assert!(view.actions.is_empty(), "completion must not regain Stop");
    let cold = engine
        .turns
        .cold
        .load(&session, "turn")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cold.buffer.user_text, "Recovered input");
    assert_eq!(cold.buffer.status, crate::TurnStatus::Completed);
    assert!(engine.turns.buffers.lock().await.is_empty());
}

#[tokio::test]
async fn slow_input_recovery_preserves_later_native_input() {
    let (engine, agent, channel, session, _) = slow_input_fixture().await;
    engine.restore_cold_turn(&session, "turn").await.unwrap();
    engine
        .turns
        .buffers
        .lock()
        .await
        .get_mut(&(session.clone(), "turn".into()))
        .unwrap()
        .user_text = "Native input arrived".into();
    engine.archive_turn(&session, "turn").await.unwrap();
    let work = complete_input_read(&engine, &agent).await;
    engine.execute_work(work).await.unwrap();
    assert!(channel.updates.lock().await.is_empty());
    let cold = engine
        .turns
        .cold
        .load(&session, "turn")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cold.buffer.user_text, "Native input arrived");
    assert!(engine.turns.buffers.lock().await.is_empty());
}

#[tokio::test]
async fn slow_input_recovery_queued_before_exit_cannot_restore_a_card() {
    let (engine, agent, channel, session, _) = slow_input_fixture().await;
    let work = complete_input_read(&engine, &agent).await;
    engine.handle_session_exit(&session).await.unwrap();
    engine.execute_work(work).await.unwrap();
    assert!(channel.updates.lock().await.is_empty());
    assert!(engine.turns.buffers.lock().await.is_empty());
    assert!(
        engine
            .turns
            .cold
            .load(&session, "turn")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn slow_input_recovery_deduplicates_repeated_renders() {
    let (engine, agent, channel, session, conversation) = slow_input_fixture().await;
    for _ in 0..20 {
        engine.restore_cold_turn(&session, "turn").await.unwrap();
        engine
            .render_turn(&conversation, &session, "turn", DeliveryClass::Live, true)
            .await
            .unwrap();
    }
    assert_eq!(agent.reads.load(std::sync::atomic::Ordering::Relaxed), 1);
    let work = complete_input_read(&engine, &agent).await;
    engine.execute_work(work).await.unwrap();
    assert_eq!(agent.reads.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(channel.sends.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[tokio::test]
async fn slow_input_recovery_shutdown_rejects_late_render_requests() {
    let (engine, agent, _, session, conversation) = slow_input_fixture().await;
    engine.cancel_input_recovery();
    engine.restore_cold_turn(&session, "turn").await.unwrap();
    engine
        .render_turn(&conversation, &session, "turn", DeliveryClass::Live, true)
        .await
        .unwrap();
    assert_eq!(
        agent.reads.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "draining render work must not start reads after shutdown"
    );
}

#[tokio::test]
async fn slow_input_recovery_limits_reads_and_drains_waiting_turns() {
    let (engine, agent, channel, session, conversation) = slow_input_fixture().await;
    for index in 1..20 {
        let turn = format!("turn-{index}");
        engine
            .record_turn_started(session.clone(), turn.clone())
            .await
            .unwrap();
        {
            let mut buffers = engine.turns.buffers.lock().await;
            let buffer = buffers.get_mut(&(session.clone(), turn.clone())).unwrap();
            buffer.agent_text = "Answer".into();
            buffer.status = crate::TurnStatus::Completed;
        }
        engine
            .render_turn(&conversation, &session, &turn, DeliveryClass::Live, true)
            .await
            .unwrap();
    }
    assert_eq!(agent.reads.load(std::sync::atomic::Ordering::Relaxed), 8);
    for _ in 0..20 {
        let work = complete_input_read(&engine, &agent).await;
        engine.execute_work(work).await.unwrap();
    }
    assert_eq!(agent.reads.load(std::sync::atomic::Ordering::Relaxed), 20);
    assert_eq!(channel.sends.load(std::sync::atomic::Ordering::Relaxed), 20);
    assert_eq!(channel.updates.lock().await.len(), 20);
}

#[tokio::test]
async fn slow_input_recovery_discards_results_after_binding_switch() {
    let (engine, agent, channel, session, conversation) = slow_input_fixture().await;
    let work = complete_input_read(&engine, &agent).await;
    engine.sessions.bindings.lock().await.attach(
        conversation,
        SessionId::new("replacement"),
        false,
    );
    engine.execute_work(work).await.unwrap();
    assert!(channel.updates.lock().await.is_empty());
    assert!(
        engine
            .turns
            .cold
            .load(&session, "turn")
            .await
            .unwrap()
            .unwrap()
            .buffer
            .user_text
            .is_empty()
    );
}

#[tokio::test]
async fn slow_input_recovery_is_cancelled_when_engine_is_dropped() {
    let (engine, agent, _, _, _) = slow_input_fixture().await;
    drop(engine);
    tokio::time::timeout(Duration::from_secs(1), async {
        while Arc::strong_count(&agent) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("input reader retained the adapter after Engine drop");
}

#[tokio::test]
async fn slow_input_recovery_exit_cleanup_does_not_restart_cancelled_reads() {
    let (engine, agent, _, session, conversation) = slow_input_fixture().await;
    engine.turns.input_recovery.cancel_session(&session);
    engine
        .cleanup_exited_turn(&conversation, &session, "turn")
        .await;
    assert_eq!(
        agent.reads.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "exit finalization must not start input history reads"
    );
}

#[tokio::test(start_paused = true)]
async fn terminal_elapsed_survives_cold_restore_and_repeated_completion() {
    // SQLite uses a worker thread; prevent automatic clock jumps while it replies.
    let clock_guard = tokio::spawn(async {
        loop {
            tokio::task::yield_now().await;
        }
    });
    for status in [
        crate::TurnStatus::Completed,
        crate::TurnStatus::Interrupted,
        crate::TurnStatus::Failed,
    ] {
        let channel = Arc::new(CompletedTurnChannel::default());
        let engine = Engine::new(
            Arc::new(UnusedAgent),
            SqliteState::in_memory().await.unwrap(),
            vec![channel.clone()],
        );
        let session = SessionId::new("elapsed-session");
        let conversation = ConversationRef::new(ChannelKind::Telegram, "chat");
        engine
            .record_turn_started(session.clone(), "turn".into())
            .await
            .unwrap();
        {
            let mut buffers = engine.turns.buffers.lock().await;
            let buffer = buffers.get_mut(&(session.clone(), "turn".into())).unwrap();
            buffer.user_text = "Question".into();
            buffer.agent_text = "Answer".into();
        }
        tokio::time::advance(Duration::from_secs(5)).await;
        engine
            .handle_turn_completed(
                &conversation,
                &session,
                "turn".into(),
                status.clone(),
                None,
                DeliveryClass::Live,
            )
            .await
            .unwrap();
        let first = engine
            .turns
            .cold
            .load(&session, "turn")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.buffer.elapsed_seconds(), Some(5));
        tokio::time::advance(Duration::from_secs(57)).await;
        let restored = engine
            .turns
            .cold
            .load(&session, "turn")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            restored.buffer.elapsed_seconds(),
            Some(5),
            "terminal duration grew while archived: {status:?}"
        );
        engine
            .handle_turn_completed(
                &conversation,
                &session,
                "turn".into(),
                status.clone(),
                None,
                DeliveryClass::Live,
            )
            .await
            .unwrap();
        let repeated = engine
            .turns
            .cold
            .load(&session, "turn")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(repeated.buffer.elapsed_seconds(), Some(5));
        engine
            .cleanup_exited_turn(&conversation, &session, "turn")
            .await;
        let updates = channel.updates.lock().await;
        let subtitle = updates.last().unwrap().1.subtitle.as_deref().unwrap();
        let expected = match status {
            crate::TurnStatus::Completed => "Completed in 5s",
            crate::TurnStatus::Interrupted => "Interrupted after 5s",
            _ => "Failed after 5s",
        };
        assert!(subtitle.ends_with(expected), "{subtitle}");
    }
    clock_guard.abort();
}

#[tokio::test]
async fn unattached_completion_finishes_cached_turn_without_removing_newer_active_turn() {
    for cold in [false, true] {
        let engine = Engine::new(
            Arc::new(UnusedAgent),
            SqliteState::in_memory().await.unwrap(),
            vec![],
        );
        let session = SessionId::new("background");
        let key = (session.clone(), "old".to_owned());
        engine.turns.buffers.lock().await.insert(
            key.clone(),
            super::TurnBuffer {
                status: crate::TurnStatus::InProgress,
                started_at: Some(tokio::time::Instant::now()),
                ..Default::default()
            },
        );
        engine
            .replace_stop_action(
                &key,
                &ConversationRef::new(ChannelKind::Telegram, "chat"),
                Some("owner"),
                true,
            )
            .await;
        if cold {
            engine.archive_turn(&session, "old").await.unwrap();
        }
        engine
            .turns
            .active
            .lock()
            .await
            .insert(session.clone(), "newer".into());
        engine
            .notify_unattached_turn_completion(&session, "old", &crate::TurnStatus::Completed, None)
            .await
            .unwrap();
        assert_eq!(
            engine.turns.active_turn(&session).await.as_deref(),
            Some("newer")
        );
        let archived = engine
            .turns
            .cold
            .load(&session, "old")
            .await
            .unwrap()
            .unwrap();
        assert!(!engine.turns.stop_actions.lock().await.contains_key(&key));
        assert!(
            !engine
                .interactions
                .turn_action_groups
                .lock()
                .await
                .contains_key(&key)
        );
        assert_eq!(archived.buffer.status, crate::TurnStatus::Completed);
        assert!(archived.buffer.terminal_elapsed.is_some());
        assert!(!engine.turns.buffers.lock().await.contains_key(&key));
    }
}

#[derive(Default)]
struct ReorderingChannel {
    reject_sends: std::sync::atomic::AtomicBool,
    reject_updates: std::sync::atomic::AtomicBool,
    stall_sends: std::sync::atomic::AtomicBool,
    send_calls: std::sync::atomic::AtomicUsize,
    entered: tokio_util::sync::CancellationToken,
    release: tokio_util::sync::CancellationToken,
    applied: tokio_util::sync::CancellationToken,
    messages: Arc<tokio::sync::Mutex<std::collections::HashMap<String, String>>>,
    writes: tokio::sync::Mutex<Vec<String>>,
    views: tokio::sync::Mutex<Vec<OutboundView>>,
}

#[async_trait]
impl ChannelAdapter for ReorderingChannel {
    fn kind(&self) -> ChannelKind {
        ChannelKind::Telegram
    }
    async fn send(
        &self,
        conversation: &ConversationRef,
        view: &OutboundView,
    ) -> Result<MessageRef, ChannelError> {
        self.send_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if self.reject_sends.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(ChannelError::Rejected("injected definite rejection".into()));
        }
        if self.stall_sends.load(std::sync::atomic::Ordering::Relaxed) {
            std::future::pending::<()>().await;
        }
        self.views.lock().await.push(view.clone());
        let mut messages = self.messages.lock().await;
        let id = format!("new-{}", messages.len());
        messages.insert(id.clone(), view.body.clone());
        Ok(MessageRef::new(conversation.clone(), id))
    }
    async fn update(
        &self,
        _: &ConversationRef,
        message: &MessageRef,
        view: &OutboundView,
    ) -> Result<(), ChannelError> {
        if self
            .reject_updates
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err(ChannelError::Rejected("injected edit rejection".into()));
        }
        self.writes.lock().await.push(view.body.clone());
        self.views.lock().await.push(view.clone());
        if view.body == "A" {
            let messages = self.messages.clone();
            let release = self.release.clone();
            let applied = self.applied.clone();
            let id = message.message_id.clone();
            // Model a server that commits even after the client drops its request.
            let remote = tokio::spawn(async move {
                release.cancelled().await;
                messages.lock().await.insert(id, "A".into());
                applied.cancel();
            });
            self.entered.cancel();
            remote.await.unwrap();
        } else {
            self.messages
                .lock()
                .await
                .insert(message.message_id.clone(), view.body.clone());
        }
        Ok(())
    }
}

async fn card_write_fixture() -> (Engine, Arc<ReorderingChannel>, ConversationRef, MessageRef) {
    let raw = Arc::new(ReorderingChannel::default());
    let engine = Engine::new(
        Arc::new(UnusedAgent),
        SqliteState::in_memory().await.unwrap(),
        vec![raw.clone()],
    );
    let conversation = ConversationRef::new(ChannelKind::Telegram, "ordering");
    let message = MessageRef::new(conversation.clone(), "original");
    (engine, raw, conversation, message)
}

#[tokio::test]
async fn card_writes_serialize_and_coalesce_pending_updates() {
    let (engine, raw, conversation, message) = card_write_fixture().await;
    let channel = engine.channel(ChannelKind::Telegram).unwrap();
    let a = OutboundView::text("Card", "A");
    let b = OutboundView::text("Card", "B");
    let c = OutboundView::text("Card", "C");
    let mut first = Box::pin(channel.update(&conversation, &message, &a));
    assert!(poll_fn(|cx| Poll::Ready(first.as_mut().poll(cx).is_pending())).await);
    raw.entered.cancelled().await;
    let mut second = Box::pin(channel.update(&conversation, &message, &b));
    let mut third = Box::pin(channel.update(&conversation, &message, &c));
    assert!(
        poll_fn(|cx| Poll::Ready(second.as_mut().poll(cx).is_pending())).await,
        "B must wait while A is in flight"
    );
    assert!(poll_fn(|cx| Poll::Ready(third.as_mut().poll(cx).is_pending())).await);
    raw.release.cancel();
    first.await.unwrap();
    second.await.unwrap();
    third.await.unwrap();
    assert_eq!(*raw.writes.lock().await, vec!["A", "C"]);
    assert_eq!(raw.messages.lock().await.get("original").unwrap(), "C");
}

#[tokio::test]
async fn card_writes_cancelled_request_cannot_overwrite_replacement() {
    let (engine, raw, conversation, message) = card_write_fixture().await;
    let channel = engine.channel(ChannelKind::Telegram).unwrap();
    let a = OutboundView::text("Card", "A");
    let mut first = Box::pin(channel.update(&conversation, &message, &a));
    assert!(poll_fn(|cx| Poll::Ready(first.as_mut().poll(cx).is_pending())).await);
    raw.entered.cancelled().await;
    drop(first);
    channel
        .update(&conversation, &message, &OutboundView::text("Card", "B"))
        .await
        .unwrap();
    raw.release.cancel();
    raw.applied.cancelled().await;
    let messages = raw.messages.lock().await;
    assert!(
        messages
            .iter()
            .any(|(id, body)| id != "original" && body == "B"),
        "B must use a fresh card after the result of A becomes unknown"
    );
    assert_eq!(messages.get("original").unwrap(), "A");
}

#[tokio::test]
async fn card_writes_background_query_keeps_its_original_revision() {
    let agent = Arc::new(SlowInputAgent::default());
    let raw = Arc::new(ReorderingChannel::default());
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![raw.clone()],
    );
    let conversation = ConversationRef::new(ChannelKind::Telegram, "query-revision");
    let message = MessageRef::new(conversation.clone(), "original");
    let session = SessionId::new("query-revision");
    engine
        .interactions
        .owners
        .lock()
        .await
        .insert(conversation.clone(), "owner".into());
    engine
        .turns
        .views
        .lock()
        .await
        .insert((session.clone(), "turn".into()), message.clone());
    engine
        .queue_background_completion(
            &session,
            "turn",
            &crate::TurnStatus::Completed,
            None,
            Some(&conversation),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while agent.reads.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    engine
        .channel(ChannelKind::Telegram)
        .unwrap()
        .update(&conversation, &message, &OutboundView::text("Card", "B"))
        .await
        .unwrap();
    agent.release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), async {
        while engine.background_completion_statistics().active != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(raw.messages.lock().await.get("original").unwrap(), "B");
    assert_eq!(*raw.writes.lock().await, vec!["B"]);
}

#[tokio::test]
async fn card_writes_timeout_replacement_survives_late_remote_commit_and_reload() {
    let (engine, raw, conversation, message) = card_write_fixture().await;
    tokio::time::pause();
    let channel = engine.channel(ChannelKind::Telegram).unwrap();
    let a = OutboundView::text("Card", "A");
    let mut first = Box::pin(channel.update(&conversation, &message, &a));
    assert!(poll_fn(|cx| Poll::Ready(first.as_mut().poll(cx).is_pending())).await);
    tokio::time::advance(Duration::from_secs(5)).await;
    first.await.unwrap();
    let mut reloaded = Engine::new(
        engine.agent.clone(),
        engine.state.clone(),
        vec![raw.clone()],
    );
    reloaded.inherit_runtime(&engine);
    reloaded
        .channel(ChannelKind::Telegram)
        .unwrap()
        .update(&conversation, &message, &OutboundView::text("Card", "B"))
        .await
        .unwrap();
    raw.release.cancel();
    raw.applied.cancelled().await;
    let replacement = {
        let messages = raw.messages.lock().await;
        assert_eq!(messages.get("original").unwrap(), "A");
        assert_eq!(messages.len(), 2);
        messages
            .iter()
            .find(|(id, body)| id.as_str() != "original" && body.as_str() == "B")
            .unwrap()
            .0
            .clone()
    };
    // A callback carries the physical replacement ID and must join the same queue.
    let replacement = MessageRef::new(conversation.clone(), replacement);
    let old_query = reloaded.card_writes.reserve(&message);
    reloaded
        .channel(ChannelKind::Telegram)
        .unwrap()
        .update(
            &conversation,
            &replacement,
            &OutboundView::text("Card", "C"),
        )
        .await
        .unwrap();
    assert!(!old_query.current());
    assert_eq!(
        raw.messages
            .lock()
            .await
            .get(&replacement.message_id)
            .unwrap(),
        "C"
    );
}

#[tokio::test]
async fn card_writes_other_cards_progress_while_one_is_blocked() {
    let (engine, raw, conversation, message) = card_write_fixture().await;
    let channel = engine.channel(ChannelKind::Telegram).unwrap();
    let a = OutboundView::text("Card", "A");
    let mut first = Box::pin(channel.update(&conversation, &message, &a));
    assert!(poll_fn(|cx| Poll::Ready(first.as_mut().poll(cx).is_pending())).await);
    let other = MessageRef::new(conversation.clone(), "other");
    channel
        .update(&conversation, &other, &OutboundView::text("Card", "B"))
        .await
        .unwrap();
    assert_eq!(raw.messages.lock().await.get("other").unwrap(), "B");
    raw.release.cancel();
    first.await.unwrap();
}

#[tokio::test]
async fn card_writes_duplicate_background_query_does_not_invalidate_first() {
    let agent = Arc::new(SlowInputAgent::default());
    let raw = Arc::new(ReorderingChannel::default());
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![raw.clone()],
    );
    let conversation = ConversationRef::new(ChannelKind::Telegram, "query-revision");
    let message = MessageRef::new(conversation.clone(), "original");
    let session = SessionId::new("query-revision");
    engine
        .interactions
        .owners
        .lock()
        .await
        .insert(conversation.clone(), "owner".into());
    engine
        .turns
        .views
        .lock()
        .await
        .insert((session.clone(), "turn".into()), message.clone());
    engine
        .queue_background_completion(
            &session,
            "turn",
            &crate::TurnStatus::Completed,
            None,
            Some(&conversation),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while agent.reads.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    engine
        .queue_background_completion(
            &session,
            "turn",
            &crate::TurnStatus::Completed,
            None,
            Some(&conversation),
        )
        .await
        .unwrap();
    agent.release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), async {
        while engine.background_completion_statistics().active != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        raw.messages
            .lock()
            .await
            .get("original")
            .is_some_and(|body| body.contains("Turn content is unavailable"))
    );
}

#[tokio::test]
async fn card_writes_cancelling_a_waiter_does_not_retire_the_active_card() {
    let (engine, raw, conversation, message) = card_write_fixture().await;
    let channel = engine.channel(ChannelKind::Telegram).unwrap();
    let a = OutboundView::text("Card", "A");
    let b = OutboundView::text("Card", "B");
    let mut first = Box::pin(channel.update(&conversation, &message, &a));
    assert!(poll_fn(|cx| Poll::Ready(first.as_mut().poll(cx).is_pending())).await);
    let mut waiting = Box::pin(channel.update(&conversation, &message, &b));
    assert!(poll_fn(|cx| Poll::Ready(waiting.as_mut().poll(cx).is_pending())).await);
    drop(waiting);
    raw.release.cancel();
    first.await.unwrap();
    channel
        .update(&conversation, &message, &OutboundView::text("Card", "C"))
        .await
        .unwrap();
    assert_eq!(raw.messages.lock().await.len(), 1);
    assert_eq!(raw.messages.lock().await.get("original").unwrap(), "C");
}

#[tokio::test]
async fn card_writes_disable_actions_waits_for_inflight_update() {
    let (engine, raw, conversation, message) = card_write_fixture().await;
    let channel = engine.channel(ChannelKind::Telegram).unwrap();
    let a = OutboundView::text("Card", "A");
    let mut first = Box::pin(channel.update(&conversation, &message, &a));
    assert!(poll_fn(|cx| Poll::Ready(first.as_mut().poll(cx).is_pending())).await);
    let mut disable = Box::pin(channel.disable_actions(&message));
    assert!(poll_fn(|cx| Poll::Ready(disable.as_mut().poll(cx).is_pending())).await);
    raw.release.cancel();
    first.await.unwrap();
    disable.await.unwrap();
}

#[tokio::test]
async fn card_writes_uncertain_replacement_send_is_not_repeated() {
    use std::sync::atomic::Ordering;
    let (engine, raw, conversation, message) = card_write_fixture().await;
    tokio::time::pause();
    let channel = engine.channel(ChannelKind::Telegram).unwrap();
    let a = OutboundView::text("Card", "A");
    let mut first = Box::pin(channel.update(&conversation, &message, &a));
    assert!(poll_fn(|cx| Poll::Ready(first.as_mut().poll(cx).is_pending())).await);
    drop(first);
    raw.stall_sends.store(true, Ordering::Relaxed);
    let b = OutboundView::text("Card", "B");
    let mut replacement = Box::pin(channel.update(&conversation, &message, &b));
    assert!(poll_fn(|cx| Poll::Ready(replacement.as_mut().poll(cx).is_pending())).await);
    tokio::time::advance(Duration::from_secs(5)).await;
    assert!(replacement.await.is_err());
    raw.stall_sends.store(false, Ordering::Relaxed);
    assert!(
        channel
            .update(&conversation, &message, &OutboundView::text("Card", "C"))
            .await
            .is_err()
    );
    assert_eq!(raw.send_calls.load(Ordering::Relaxed), 1);
    raw.release.cancel();
    raw.applied.cancelled().await;
}

#[tokio::test]
async fn card_writes_recheck_binding_after_waiting_for_active_writer() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (engine, raw, conversation, message) = card_write_fixture().await;
    let channel = engine.channel(ChannelKind::Telegram).unwrap();
    let a = OutboundView::text("Card", "A");
    let b = OutboundView::text("Card", "B");
    let mut first = Box::pin(channel.update(&conversation, &message, &a));
    assert!(poll_fn(|cx| Poll::Ready(first.as_mut().poll(cx).is_pending())).await);
    let revision = engine.card_writes.reserve(&message);
    let valid = AtomicBool::new(true);
    let mut second = Box::pin(engine.card_writes.update_if(
        raw.as_ref(),
        &revision,
        &conversation,
        &b,
        || async { valid.load(Ordering::SeqCst) },
    ));
    assert!(poll_fn(|cx| Poll::Ready(second.as_mut().poll(cx).is_pending())).await);
    valid.store(false, Ordering::SeqCst);
    raw.release.cancel();
    first.await.unwrap();
    assert!(!second.await.unwrap());
    assert_eq!(*raw.writes.lock().await, vec!["A"]);
}

#[tokio::test]
async fn card_review_disable_must_preserve_pending_final_content() {
    let (engine, raw, conversation, message) = card_write_fixture().await;
    let channel = engine.channel(ChannelKind::Telegram).unwrap();
    let a = OutboundView::text("Card", "A");
    let mut b = OutboundView::text("Card", "B");
    b.actions.push(crate::ActionButton {
        label: "Stop".into(),
        token: "stop".into(),
        style: crate::ActionStyle::Primary,
        disabled: false,
    });
    let mut first = Box::pin(channel.update(&conversation, &message, &a));
    assert!(poll_fn(|cx| Poll::Ready(first.as_mut().poll(cx).is_pending())).await);
    let mut final_content = Box::pin(channel.update(&conversation, &message, &b));
    assert!(poll_fn(|cx| Poll::Ready(final_content.as_mut().poll(cx).is_pending())).await);
    let mut disable = Box::pin(channel.disable_actions(&message));
    assert!(poll_fn(|cx| Poll::Ready(disable.as_mut().poll(cx).is_pending())).await);
    raw.release.cancel();
    first.await.unwrap();
    final_content.await.unwrap();
    disable.await.unwrap();
    assert_eq!(
        raw.messages.lock().await.get("original").unwrap(),
        "B",
        "disabling actions must not discard the latest answer body"
    );
    assert!(raw.views.lock().await.last().unwrap().actions[0].disabled);
    channel.update(&conversation, &message, &b).await.unwrap();
    assert!(!raw.views.lock().await.last().unwrap().actions[0].disabled);
}

#[tokio::test]
async fn card_review_definite_replacement_rejection_must_allow_recovery() {
    use std::sync::atomic::Ordering;
    let (engine, raw, conversation, message) = card_write_fixture().await;
    let channel = engine.channel(ChannelKind::Telegram).unwrap();
    let a = OutboundView::text("Card", "A");
    let mut first = Box::pin(channel.update(&conversation, &message, &a));
    assert!(poll_fn(|cx| Poll::Ready(first.as_mut().poll(cx).is_pending())).await);
    drop(first);
    raw.reject_sends.store(true, Ordering::Relaxed);
    assert!(
        channel
            .update(&conversation, &message, &OutboundView::text("Card", "B"))
            .await
            .is_err()
    );
    raw.reject_sends.store(false, Ordering::Relaxed);
    let recovered = channel
        .update(&conversation, &message, &OutboundView::text("Card", "C"))
        .await;
    raw.release.cancel();
    raw.applied.cancelled().await;
    assert!(
        recovered.is_ok(),
        "a definite rejection is not an unknown remote commit: {recovered:?}"
    );
}

struct LocalCooldownChannel {
    ready: tokio::time::Instant,
    wire_calls: std::sync::atomic::AtomicUsize,
    sends: std::sync::atomic::AtomicUsize,
}
#[async_trait]
impl ChannelAdapter for LocalCooldownChannel {
    fn kind(&self) -> ChannelKind {
        ChannelKind::Telegram
    }
    async fn send(
        &self,
        conversation: &ConversationRef,
        _: &OutboundView,
    ) -> Result<MessageRef, ChannelError> {
        agentix_domain::DeliveryAttempt::waiting();
        tokio::time::sleep_until(self.ready).await;
        agentix_domain::DeliveryAttempt::dispatched();
        self.wire_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.sends
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(MessageRef::new(conversation.clone(), "replacement"))
    }
    async fn update(
        &self,
        _: &ConversationRef,
        _: &MessageRef,
        _: &OutboundView,
    ) -> Result<(), ChannelError> {
        agentix_domain::DeliveryAttempt::waiting();
        tokio::time::sleep_until(self.ready).await;
        agentix_domain::DeliveryAttempt::dispatched();
        self.wire_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}

#[tokio::test]
async fn card_review_local_cooldown_must_not_permanently_disable_card() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let state = SqliteState::in_memory().await.unwrap();
    tokio::time::pause();
    let raw = Arc::new(LocalCooldownChannel {
        ready: tokio::time::Instant::now() + Duration::from_secs(90),
        wire_calls: AtomicUsize::new(0),
        sends: AtomicUsize::new(0),
    });
    let engine = Engine::new(Arc::new(UnusedAgent), state, vec![raw.clone()]);
    let conversation = ConversationRef::new(ChannelKind::Telegram, "cooldown");
    let message = MessageRef::new(conversation.clone(), "original");
    let channel = engine.channel(ChannelKind::Telegram).unwrap();
    let _ = channel
        .update(&conversation, &message, &OutboundView::text("Card", "A"))
        .await;
    assert_eq!(
        raw.wire_calls.load(Ordering::Relaxed),
        0,
        "no request reached the provider"
    );
    tokio::time::advance(Duration::from_secs(90)).await;
    let recovered = channel
        .update(&conversation, &message, &OutboundView::text("Card", "B"))
        .await;
    assert!(
        recovered.is_ok(),
        "expired local cooldown must allow delivery: {recovered:?}"
    );
}

#[tokio::test]
async fn card_writes_cancellation_during_local_wait_preserves_original_target() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let state = SqliteState::in_memory().await.unwrap();
    tokio::time::pause();
    let raw = Arc::new(LocalCooldownChannel {
        ready: tokio::time::Instant::now() + Duration::from_secs(30),
        wire_calls: AtomicUsize::new(0),
        sends: AtomicUsize::new(0),
    });
    let engine = Engine::new(Arc::new(UnusedAgent), state, vec![raw.clone()]);
    let conversation = ConversationRef::new(ChannelKind::Telegram, "cooldown");
    let message = MessageRef::new(conversation.clone(), "original");
    let channel = engine.channel(ChannelKind::Telegram).unwrap();
    let view = OutboundView::text("Card", "A");
    let mut pending = Box::pin(channel.update(&conversation, &message, &view));
    assert!(poll_fn(|cx| Poll::Ready(pending.as_mut().poll(cx).is_pending())).await);
    drop(pending);
    // A fresh attempt can wait through a cooldown longer than the wire budget.
    channel
        .update(&conversation, &message, &view)
        .await
        .unwrap();
    assert_eq!(raw.wire_calls.load(Ordering::Relaxed), 1);
    assert_eq!(raw.sends.load(Ordering::Relaxed), 0);
    // The original handle remains writable.
    let revision = engine.card_writes.reserve(&message);
    assert!(
        engine
            .card_writes
            .update(raw.as_ref(), &revision, &conversation, &view)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn card_writes_definite_edit_rejection_does_not_replace_card() {
    use std::sync::atomic::Ordering;
    let (engine, raw, conversation, message) = card_write_fixture().await;
    let channel = engine.channel(ChannelKind::Telegram).unwrap();
    raw.reject_updates.store(true, Ordering::Relaxed);
    assert!(matches!(
        channel
            .update(&conversation, &message, &OutboundView::text("Card", "B"))
            .await,
        Err(ChannelError::Rejected(_))
    ));
    raw.reject_updates.store(false, Ordering::Relaxed);
    channel
        .update(&conversation, &message, &OutboundView::text("Card", "C"))
        .await
        .unwrap();
    assert_eq!(raw.send_calls.load(Ordering::Relaxed), 0);
    assert_eq!(raw.messages.lock().await.get("original").unwrap(), "C");
}

async fn review_background_fixture() -> (
    Engine,
    Arc<SlowInputAgent>,
    Arc<ReorderingChannel>,
    ConversationRef,
    SessionId,
) {
    let agent = Arc::new(SlowInputAgent::default());
    let raw = Arc::new(ReorderingChannel::default());
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![raw.clone()],
    );
    let conversation = ConversationRef::new(ChannelKind::Telegram, "review");
    let session = SessionId::new("review-session");
    engine
        .interactions
        .owners
        .lock()
        .await
        .insert(conversation.clone(), "owner".into());
    engine.turns.views.lock().await.insert(
        (session.clone(), "turn".into()),
        MessageRef::new(conversation.clone(), "original"),
    );
    engine.turns.buffers.lock().await.insert(
        (session.clone(), "turn".into()),
        super::TurnBuffer {
            user_text: "Question".into(),
            ..super::TurnBuffer::default()
        },
    );
    (engine, agent, raw, conversation, session)
}

async fn wait_review_loading(raw: &ReorderingChannel) -> OutboundView {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(view) = raw
                .views
                .lock()
                .await
                .iter()
                .find(|view| view.body.contains("Loading turn content"))
                .cloned()
            {
                return view;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

async fn wait_review_background(engine: &Engine) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while engine.background_completion_statistics().active != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn review_rejected_final_update_preserves_visible_attach_token() {
    let (engine, agent, raw, conversation, session) = review_background_fixture().await;
    engine
        .queue_background_completion(
            &session,
            "turn",
            &crate::TurnStatus::Completed,
            None,
            Some(&conversation),
        )
        .await
        .unwrap();
    let loading = wait_review_loading(&raw).await;
    raw.reject_updates
        .store(true, std::sync::atomic::Ordering::Relaxed);
    agent.release.notify_one();
    wait_review_background(&engine).await;
    let epoch = engine.sessions.epoch(&conversation).await;
    let consumed = engine.interactions.actions.lock().await.consume(
        &loading.actions[0].token,
        &conversation,
        "owner",
        agent.generation(),
        epoch,
    );
    assert!(
        consumed.is_ok(),
        "the still-visible card must retain a usable Attach button: {consumed:?}"
    );
}

#[tokio::test]
async fn review_missing_content_notice_reaches_rich_card_sections() {
    let (engine, agent, raw, conversation, session) = review_background_fixture().await;
    engine
        .queue_background_completion(
            &session,
            "turn",
            &crate::TurnStatus::Completed,
            None,
            Some(&conversation),
        )
        .await
        .unwrap();
    wait_review_loading(&raw).await;
    agent.release.notify_one();
    wait_review_background(&engine).await;
    let views = raw.views.lock().await;
    let final_view = views.last().unwrap();
    assert!(
        final_view
            .body
            .contains("Additional turn content is unavailable")
    );
    assert!(!final_view.sections.is_empty());
    assert!(
        final_view.sections.iter().any(|section| section
            .body
            .contains("Additional turn content is unavailable")),
        "Feishu renders sections instead of the fallback body"
    );
}

#[tokio::test]
async fn review_disabled_channel_owner_does_not_prevent_completion_archive() {
    let (engine, agent, raw, conversation, session) = review_background_fixture().await;
    engine.interactions.owners.lock().await.insert(
        ConversationRef::new(ChannelKind::Feishu, "disabled-chat"),
        "owner".into(),
    );
    let mut reloaded = Engine::new(agent, engine.state.clone(), vec![raw]);
    reloaded.inherit_runtime(&engine);
    let result = reloaded
        .notify_unattached_turn_completion(&session, "turn", &crate::TurnStatus::Completed, None)
        .await;
    let archived = reloaded.turns.cold.load(&session, "turn").await.unwrap();
    assert!(
        archived.is_some(),
        "optional disabled-channel recipient must not prevent local archival: {result:?}"
    );
    assert!(result.is_ok());
    assert!(
        reloaded
            .interactions
            .owners
            .lock()
            .await
            .contains_key(&conversation)
    );
}

#[tokio::test]
async fn review_final_update_reuses_visible_attach_token() {
    let (engine, agent, raw, conversation, session) = review_background_fixture().await;
    engine
        .queue_background_completion(
            &session,
            "turn",
            &crate::TurnStatus::Completed,
            None,
            Some(&conversation),
        )
        .await
        .unwrap();
    let loading = wait_review_loading(&raw).await;
    assert!(
        loading
            .sections
            .iter()
            .any(|section| section.body.contains("Loading turn content"))
    );
    agent.release.notify_one();
    wait_review_background(&engine).await;
    let views = raw.views.lock().await;
    let final_view = views.last().unwrap();
    assert_eq!(loading.actions[0].token, final_view.actions[0].token);
    assert!(
        !final_view
            .sections
            .iter()
            .any(|section| section.body.contains("Loading turn content"))
    );
}

#[tokio::test]
async fn review_already_attached_completion_has_disabled_button() {
    let (engine, agent, raw, conversation, session) = review_background_fixture().await;
    engine
        .sessions
        .commit_binding(&conversation, &session, false)
        .await
        .unwrap();
    engine
        .queue_background_completion(
            &session,
            "turn",
            &crate::TurnStatus::Completed,
            None,
            Some(&conversation),
        )
        .await
        .unwrap();
    let loading = wait_review_loading(&raw).await;
    assert!(loading.actions[0].disabled);
    assert_eq!(loading.actions[0].label, "Attached");
    agent.release.notify_one();
    wait_review_background(&engine).await;
}

#[tokio::test]
async fn review_cancel_preserves_visible_attach_token() {
    let (engine, agent, raw, conversation, session) = review_background_fixture().await;
    engine
        .queue_background_completion(
            &session,
            "turn",
            &crate::TurnStatus::Completed,
            None,
            Some(&conversation),
        )
        .await
        .unwrap();
    let loading = wait_review_loading(&raw).await;
    engine.cancel_background_completions();
    tokio::task::yield_now().await;
    let epoch = engine.sessions.epoch(&conversation).await;
    assert!(
        engine
            .interactions
            .actions
            .lock()
            .await
            .consume(
                &loading.actions[0].token,
                &conversation,
                "owner",
                agent.generation(),
                epoch
            )
            .is_ok()
    );
}

#[tokio::test]
async fn review_cleanup_gate_preserves_revision_before_optional_io() {
    let (engine, agent, raw, conversation, session) = review_background_fixture().await;
    let permit = engine
        .prepare_background_completion(
            &session,
            "turn",
            &crate::TurnStatus::Completed,
            None,
            Some(&conversation),
        )
        .await
        .unwrap();
    tokio::task::yield_now().await;
    assert_eq!(agent.reads.load(std::sync::atomic::Ordering::Relaxed), 0);
    engine
        .channel(ChannelKind::Telegram)
        .unwrap()
        .update(
            &conversation,
            &MessageRef::new(conversation.clone(), "original"),
            &OutboundView::text("Newer", "B"),
        )
        .await
        .unwrap();
    permit.send(()).unwrap();
    agent.release.notify_one();
    wait_review_background(&engine).await;
    assert_eq!(*raw.writes.lock().await, vec!["B"]);
}

#[tokio::test]
async fn review_cancelled_cleanup_does_not_start_optional_io() {
    let (engine, agent, raw, conversation, session) = review_background_fixture().await;
    let permit = engine
        .prepare_background_completion(
            &session,
            "turn",
            &crate::TurnStatus::Completed,
            None,
            Some(&conversation),
        )
        .await
        .unwrap();
    drop(permit);
    wait_review_background(&engine).await;
    assert_eq!(agent.reads.load(std::sync::atomic::Ordering::Relaxed), 0);
    assert!(raw.views.lock().await.is_empty());
}

#[tokio::test]
async fn review_disabled_draining_channel_still_finishes_cleanup() {
    let (engine, agent, _, conversation, session) = review_background_fixture().await;
    engine
        .sessions
        .commit_binding(&conversation, &session, false)
        .await
        .unwrap();
    engine
        .sessions
        .commit_binding(&conversation, &SessionId::new("next"), true)
        .await
        .unwrap();
    let mut reloaded = Engine::new(agent, engine.state.clone(), vec![]);
    reloaded.inherit_runtime(&engine);
    reloaded
        .handle_turn_completed(
            &conversation,
            &session,
            "turn".into(),
            crate::TurnStatus::Completed,
            None,
            DeliveryClass::Draining,
        )
        .await
        .unwrap();
    assert!(
        reloaded
            .turns
            .cold
            .load(&session, "turn")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        reloaded
            .sessions
            .bindings
            .lock()
            .await
            .route(&session, crate::EventImportance::Critical)
            .is_none()
    );
    assert_eq!(reloaded.background_completion_statistics().active, 0);
}

#[tokio::test]
async fn review_consumed_attach_token_is_not_reissued() {
    let (engine, agent, raw, conversation, session) = review_background_fixture().await;
    engine
        .queue_background_completion(
            &session,
            "turn",
            &crate::TurnStatus::Completed,
            None,
            Some(&conversation),
        )
        .await
        .unwrap();
    let loading = wait_review_loading(&raw).await;
    let epoch = engine.sessions.epoch(&conversation).await;
    engine
        .interactions
        .actions
        .lock()
        .await
        .consume(
            &loading.actions[0].token,
            &conversation,
            "owner",
            agent.generation(),
            epoch,
        )
        .unwrap();
    agent.release.notify_one();
    wait_review_background(&engine).await;
    let views = raw.views.lock().await;
    assert!(views.last().unwrap().actions[0].disabled);
    assert!(
        engine
            .interactions
            .actions
            .lock()
            .await
            .iter()
            .next()
            .is_none()
    );
}

#[tokio::test]
async fn review_binding_change_refreshes_background_attach_scope() {
    let (engine, agent, raw, conversation, session) = review_background_fixture().await;
    engine
        .queue_background_completion(&session, "turn", &crate::TurnStatus::Completed, None, None)
        .await
        .unwrap();
    let loading = wait_review_loading(&raw).await;
    engine
        .sessions
        .commit_binding(&conversation, &SessionId::new("other"), false)
        .await
        .unwrap();
    agent.release.notify_one();
    wait_review_background(&engine).await;
    let views = raw.views.lock().await;
    let final_token = &views.last().unwrap().actions[0].token;
    assert_ne!(&loading.actions[0].token, final_token);
    let epoch = engine.sessions.epoch(&conversation).await;
    let mut actions = engine.interactions.actions.lock().await;
    assert!(matches!(
        actions.consume(
            &loading.actions[0].token,
            &conversation,
            "owner",
            agent.generation(),
            epoch
        ),
        Err(crate::ActionTokenError::StaleBinding)
    ));
    assert!(
        actions
            .consume(
                final_token,
                &conversation,
                "owner",
                agent.generation(),
                epoch
            )
            .is_ok()
    );
}

#[tokio::test]
async fn review_attach_during_history_read_shows_attached() {
    let (engine, agent, raw, conversation, session) = review_background_fixture().await;
    engine
        .queue_background_completion(&session, "turn", &crate::TurnStatus::Completed, None, None)
        .await
        .unwrap();
    wait_review_loading(&raw).await;
    engine
        .sessions
        .commit_binding(&conversation, &session, false)
        .await
        .unwrap();
    agent.release.notify_one();
    wait_review_background(&engine).await;
    let views = raw.views.lock().await;
    let button = &views.last().unwrap().actions[0];
    assert!(button.disabled);
    assert_eq!(button.label, "Attached");
}
