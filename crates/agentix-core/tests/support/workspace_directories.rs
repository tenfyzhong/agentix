use super::*;

fn inbound(chat: &str, text: &str) -> InboundEnvelope {
    InboundEnvelope::text(
        uuid::Uuid::new_v4().to_string(),
        ConversationRef::new(ChannelKind::Telegram, chat),
        "owner",
        text,
    )
}

async fn fixture() -> (Engine, Arc<FakeAgent>, Arc<FakeChannel>) {
    let agent = Arc::new(FakeAgent::new());
    let channel = Arc::new(FakeChannel::default());
    let engine = Engine::new(
        agent.clone(),
        SqliteState::in_memory().await.unwrap(),
        vec![channel.clone()],
    );
    (engine, agent, channel)
}

pub(super) async fn click(engine: &Engine, channel: &FakeChannel, label: &str) {
    let view = channel.sent().last().unwrap().1.clone();
    let token = view
        .actions
        .iter()
        .find(|a| a.label == label)
        .unwrap_or_else(|| panic!("missing {label}: {view:?}"))
        .token
        .clone();
    engine
        .handle_inbound(InboundEnvelope::action(
            uuid::Uuid::new_v4().to_string(),
            ConversationRef::new(ChannelKind::Telegram, "chat-a"),
            "owner",
            token,
        ))
        .await
        .unwrap();
}

fn mutations(agent: &FakeAgent) -> Vec<String> {
    agent
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("mux-mutate"))
        .collect()
}

#[tokio::test]
async fn creation_previews_home_and_only_confirmation_creates() {
    let (engine, agent, channel) = fixture().await;
    engine
        .handle_inbound(inbound("chat-a", "/rmux"))
        .await
        .unwrap();
    click(&engine, &channel, "+ Session").await;
    assert!(
        mutations(&agent).is_empty(),
        "preview must not create a pane"
    );
    let view = channel.sent().last().unwrap().1.clone();
    assert!(view.body.contains("/work/multiplexer"));
    assert!(view.body.contains("HOME"));
    click(&engine, &channel, "Create").await;
    assert_eq!(mutations(&agent).len(), 1);
    assert!(mutations(&agent)[0].contains("/work/multiplexer"));
}

#[tokio::test]
async fn absolute_path_input_is_local_and_selection_is_one_shot() {
    let (engine, agent, channel) = fixture().await;
    engine
        .handle_inbound(inbound("chat-a", "/rmux"))
        .await
        .unwrap();
    click(&engine, &channel, "+ Session").await;
    click(&engine, &channel, "Choose directory").await;
    click(&engine, &channel, "Enter path").await;
    engine
        .handle_inbound(inbound("chat-a", "/custom/work tree"))
        .await
        .unwrap();
    assert!(
        channel
            .sent()
            .last()
            .unwrap()
            .1
            .body
            .contains("/custom/work tree")
    );
    assert!(mutations(&agent).is_empty());
    click(&engine, &channel, "Create").await;
    assert!(mutations(&agent)[0].contains("/custom/work tree"));
    engine
        .handle_inbound(inbound("chat-a", "/rmux"))
        .await
        .unwrap();
    click(&engine, &channel, "+ Session").await;
    assert!(
        channel
            .sent()
            .last()
            .unwrap()
            .1
            .body
            .contains("/work/multiplexer")
    );
    assert!(!agent.calls().iter().any(|c| c.contains("send:")));
}

#[tokio::test]
async fn attached_worktree_is_preserved_and_invalid_cwd_falls_back_home() {
    for cwd in [Some("/repo/.git/wtm/feature"), Some("/missing"), None] {
        let (engine, agent, channel) = fixture().await;
        agent.sessions.lock().unwrap()[0].cwd = cwd.map(str::to_owned);
        engine
            .handle_inbound(inbound("chat-a", "/attach thr_a"))
            .await
            .unwrap();
        engine
            .handle_inbound(inbound("chat-a", "/rmux"))
            .await
            .unwrap();
        click(&engine, &channel, "+ Session").await;
        assert!(channel.sent().last().unwrap().1.body.contains(
            if cwd == Some("/repo/.git/wtm/feature") {
                cwd.unwrap()
            } else {
                "/work/multiplexer"
            }
        ));
    }
}

#[tokio::test]
async fn cancellation_invalidates_buttons_without_creating() {
    let (engine, agent, channel) = fixture().await;
    engine
        .handle_inbound(inbound("chat-a", "/rmux"))
        .await
        .unwrap();
    click(&engine, &channel, "+ Session").await;
    let old = channel
        .sent()
        .last()
        .unwrap()
        .1
        .actions
        .iter()
        .find(|a| a.label == "Create")
        .unwrap()
        .token
        .clone();
    click(&engine, &channel, "Cancel").await;
    assert!(
        engine
            .handle_inbound(InboundEnvelope::action(
                "old",
                ConversationRef::new(ChannelKind::Telegram, "chat-a"),
                "owner",
                old
            ))
            .await
            .is_err()
    );
    assert!(mutations(&agent).is_empty());
}

#[tokio::test]
async fn invalid_manual_path_can_be_retried_and_cancelled() {
    let (engine, agent, channel) = fixture().await;
    engine
        .handle_inbound(inbound("chat-a", "/rmux"))
        .await
        .unwrap();
    click(&engine, &channel, "+ Session").await;
    click(&engine, &channel, "Choose directory").await;
    click(&engine, &channel, "Enter path").await;
    engine
        .handle_inbound(inbound("chat-a", "/missing"))
        .await
        .unwrap();
    assert!(channel.sent().last().unwrap().1.body.contains("missing"));
    engine
        .handle_inbound(inbound("chat-a", "/custom"))
        .await
        .unwrap();
    assert!(channel.sent().last().unwrap().1.body.contains("/custom"));
    engine
        .handle_inbound(inbound("chat-a", "/cancel"))
        .await
        .unwrap();
    assert!(mutations(&agent).is_empty());
}

#[tokio::test]
async fn new_window_offers_distinct_directories_and_prefers_attached_pane() {
    let (engine, agent, channel) = fixture().await;
    {
        let mut snapshot = agent.snapshot.lock().unwrap();
        let mut second = snapshot.sessions[0].windows[0].clone();
        second.id = "@2".into();
        for pane in &mut second.panes {
            pane.cwd = "/other".into();
            pane.agent_session = None;
        }
        snapshot.sessions[0].windows.push(second);
    }
    engine
        .handle_inbound(inbound("chat-a", "/rmux"))
        .await
        .unwrap();
    click(&engine, &channel, "agentix").await;
    click(&engine, &channel, "+ Window").await;
    let view = channel.sent().last().unwrap().1.clone();
    assert!(view.actions.iter().any(|a| a.label == "/other"));
    assert!(view.actions.iter().any(|a| a.label == "/work/parser"));
    click(&engine, &channel, "/other").await;
    click(&engine, &channel, "Create").await;
    assert!(mutations(&agent)[0].contains("/other"));
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "/rmux"))
        .await
        .unwrap();
    click(&engine, &channel, "agentix").await;
    click(&engine, &channel, "+ Window").await;
    let view = channel.sent().last().unwrap().1.clone();
    assert!(view.body.contains("/work/parser"));
    assert!(!view.actions.iter().any(|a| a.label == "/other"));
}

#[tokio::test]
async fn missing_target_at_confirmation_never_creates() {
    let (engine, agent, channel) = fixture().await;
    engine
        .handle_inbound(inbound("chat-a", "/rmux"))
        .await
        .unwrap();
    click(&engine, &channel, "agentix").await;
    click(&engine, &channel, "+ Window").await;
    agent.snapshot.lock().unwrap().sessions.clear();
    let token = channel
        .sent()
        .last()
        .unwrap()
        .1
        .actions
        .iter()
        .find(|a| a.label == "Create")
        .unwrap()
        .token
        .clone();
    assert!(
        engine
            .handle_inbound(InboundEnvelope::action(
                "missing-target",
                ConversationRef::new(ChannelKind::Telegram, "chat-a"),
                "owner",
                token
            ))
            .await
            .is_err()
    );
    assert!(mutations(&agent).is_empty());
}

#[tokio::test]
async fn browse_back_preserves_confirmed_selection() {
    let (engine, _, channel) = fixture().await;
    engine
        .handle_inbound(inbound("chat-a", "/rmux"))
        .await
        .unwrap();
    click(&engine, &channel, "+ Session").await;
    click(&engine, &channel, "Choose directory").await;
    click(&engine, &channel, "child").await;
    click(&engine, &channel, "Back").await;
    assert!(!channel.sent().last().unwrap().1.body.contains("/child"));
}

#[tokio::test]
async fn other_owner_cannot_supply_path_and_commands_cancel_input() {
    let (engine, agent, channel) = fixture().await;
    engine
        .handle_inbound(inbound("chat-a", "/rmux"))
        .await
        .unwrap();
    click(&engine, &channel, "+ Session").await;
    click(&engine, &channel, "Choose directory").await;
    click(&engine, &channel, "Enter path").await;
    assert!(
        engine
            .handle_inbound(inbound_as("chat-a", "intruder", "/other"))
            .await
            .is_err()
    );
    engine
        .handle_inbound(inbound("chat-a", "/help"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "/custom"))
        .await
        .unwrap();
    assert!(
        channel
            .sent()
            .last()
            .unwrap()
            .1
            .body
            .contains("unknown command")
    );
    assert!(mutations(&agent).is_empty());
}

#[tokio::test]
async fn disconnect_rejects_pending_directory_text() {
    let (engine, agent, channel) = fixture().await;
    engine
        .handle_inbound(inbound("chat-a", "/attach thr_a"))
        .await
        .unwrap();
    engine
        .handle_inbound(inbound("chat-a", "/rmux"))
        .await
        .unwrap();
    click(&engine, &channel, "+ Session").await;
    click(&engine, &channel, "Choose directory").await;
    click(&engine, &channel, "Enter path").await;
    engine
        .handle_agent_event(AgentEvent::Disconnected {
            generation: 1,
            reason: "lost".into(),
        })
        .await
        .unwrap();
    assert!(
        engine
            .handle_inbound(inbound("chat-a", "relative-path"))
            .await
            .is_err()
    );
    assert!(mutations(&agent).is_empty());
    assert!(!agent.calls().iter().any(|c| c.contains("relative-path")));
}

#[tokio::test]
async fn registry_disconnect_expires_only_selected_backend_drafts() {
    use agentix_core::{AgentKind, AgentRegistry};
    for disconnected in [AgentKind::Codex, AgentKind::Pi] {
        let codex = Arc::new(FakeAgent::new());
        let pi = Arc::new(FakeAgent::new());
        let registry = Arc::new(
            AgentRegistry::new(vec![
                (AgentKind::Codex, codex.clone()),
                (AgentKind::Pi, pi.clone()),
            ])
            .unwrap(),
        );
        registry.list_sessions(None, 100).await.unwrap();
        let mut events = registry.subscribe();
        let channel = Arc::new(FakeChannel::default());
        let engine = Engine::new(
            registry,
            SqliteState::in_memory().await.unwrap(),
            vec![channel.clone()],
        );
        engine
            .handle_inbound(inbound("chat-a", "/rmux codex"))
            .await
            .unwrap();
        click(&engine, &channel, "+ Session").await;
        let token = channel
            .sent()
            .last()
            .unwrap()
            .1
            .actions
            .iter()
            .find(|a| a.label == "Create")
            .unwrap()
            .token
            .clone();
        let source = if disconnected == AgentKind::Codex {
            &codex
        } else {
            &pi
        };
        source
            .events
            .send(AgentEvent::Disconnected {
                generation: 1,
                reason: "lost".into(),
            })
            .unwrap();
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        engine.handle_agent_event(event).await.unwrap();
        let result = engine
            .handle_inbound(InboundEnvelope::action(
                "stale-create",
                ConversationRef::new(ChannelKind::Telegram, "chat-a"),
                "owner",
                token,
            ))
            .await;
        assert_eq!(result.is_err(), disconnected == AgentKind::Codex);
        assert_eq!(
            mutations(&codex).is_empty(),
            disconnected == AgentKind::Codex
        );
    }
}
