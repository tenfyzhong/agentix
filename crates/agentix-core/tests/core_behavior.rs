use agentix_core::{
    ActionRegistry, ActionScope, AgentCapabilities, BindingTable, ChannelCommand, ChannelKind,
    CommandMenu, ConversationRef, DeliveryClass, EventImportance, SessionId,
};

fn telegram(chat_id: &str) -> ConversationRef {
    ConversationRef::new(ChannelKind::Telegram, chat_id)
}

#[test]
fn switching_sessions_keeps_only_one_current_binding() {
    let mut bindings = BindingTable::default();
    let chat = telegram("42");
    let first = SessionId::new("thr_first");
    let second = SessionId::new("thr_second");

    let initial = bindings.attach(chat.clone(), first.clone(), false);
    assert_eq!(initial.epoch, 1);
    assert_eq!(bindings.current_session(&chat), Some(&first));

    let switched = bindings.attach(chat.clone(), second.clone(), true);
    assert_eq!(switched.previous_session, Some(first.clone()));
    assert_eq!(switched.epoch, 2);
    assert_eq!(bindings.current_session(&chat), Some(&second));
    assert_eq!(bindings.bound_conversation(&first), None);
    assert_eq!(
        bindings.route(&first, EventImportance::Stream),
        None,
        "ordinary deltas from the old session must be suppressed"
    );
    assert_eq!(
        bindings.route(&first, EventImportance::Critical),
        Some((chat, DeliveryClass::Draining)),
        "approvals and terminal events must still reach the old conversation"
    );
}

#[test]
fn attaching_a_session_elsewhere_displaces_the_old_conversation() {
    let mut bindings = BindingTable::default();
    let first_chat = telegram("1");
    let second_chat = telegram("2");
    let session = SessionId::new("thr_shared");

    bindings.attach(first_chat.clone(), session.clone(), false);
    let outcome = bindings.attach(second_chat.clone(), session.clone(), false);

    assert_eq!(outcome.displaced_conversation, Some(first_chat.clone()));
    assert_eq!(bindings.current_session(&first_chat), None);
    assert_eq!(bindings.bound_conversation(&session), Some(&second_chat));
}

#[test]
fn action_tokens_are_single_use_and_bound_to_the_exact_route() {
    let mut tokens = ActionRegistry::default();
    let conversation = telegram("42");
    let scope = ActionScope::new(conversation.clone(), "owner-1", 7, 3, "turn-actions");
    let token = tokens.issue(scope, "approve");

    assert!(
        tokens
            .consume(&token, &conversation, "owner-2", 7, 3)
            .is_err()
    );
    assert!(
        tokens
            .consume(&token, &conversation, "owner-1", 8, 3)
            .is_err()
    );
    assert_eq!(
        tokens
            .consume(&token, &conversation, "owner-1", 7, 3)
            .unwrap(),
        "approve"
    );
    assert!(
        tokens
            .consume(&token, &conversation, "owner-1", 7, 3)
            .is_err()
    );
}

#[test]
fn agent_capabilities_are_explicit_and_session_only_by_default() {
    let capabilities = AgentCapabilities::default();

    assert!(!capabilities.queued_prompts);
    assert!(!capabilities.session_control);
    assert!(!capabilities.workspace_runtime);
}

#[test]
fn command_menus_are_channel_neutral_data() {
    let menu = CommandMenu::new(vec![ChannelCommand::new("sessions", "Browse sessions")]);

    assert_eq!(menu.commands[0].name, "sessions");
    assert_eq!(menu.commands[0].description, "Browse sessions");
}

#[test]
fn finishing_a_draining_session_removes_its_route() {
    let mut bindings = BindingTable::default();
    let chat = telegram("42");
    let old = SessionId::new("thr_old");
    let new = SessionId::new("thr_new");
    bindings.attach(chat.clone(), old.clone(), false);
    bindings.attach(chat, new, true);

    bindings.finish_draining(&old);

    assert_eq!(bindings.route(&old, EventImportance::Critical), None);
}
