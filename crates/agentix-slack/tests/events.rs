use agentix_domain::{ChannelKind, InboundPayload};
use agentix_slack::normalize_event;
use serde_json::{Value, json};

#[allow(clippy::needless_pass_by_value)]
fn event(message: Value) -> Value {
    json!({"type":"events_api", "envelope_id":"env", "payload": {
        "type":"event_callback", "event_id":"Ev1", "team_id":"T1", "event":message
    }})
}

#[test]
fn accepts_owner_dms_and_mentions_with_thread_isolation() {
    let dm = event(
        json!({"type":"message", "channel":"D1", "channel_type":"im", "user":"U1", "text":"/sessions", "ts":"1.000001"}),
    );
    let input = normalize_event(&dm, "T1", "BOT", &["U1".into()]).unwrap();
    assert_eq!(input.owner_id, "U1");
    assert_eq!(input.conversation.channel.to_string(), "slack");
    assert_eq!(input.conversation.conversation_id, "T1:D1");
    assert_eq!(input.event_id, "T1:1.000001:D1");
    assert_eq!(input.payload, InboundPayload::Text("/sessions".into()));
    let mention = event(
        json!({"type":"app_mention", "channel":"C1", "user":"U1", "text":"<@BOT> /sessions", "ts":"2.000001", "thread_ts":"1.000001"}),
    );
    let input = normalize_event(&mention, "T1", "BOT", &["U1".into()]).unwrap();
    assert_eq!(input.conversation.conversation_id, "T1:C1:1.000001");
    assert_eq!(input.payload, InboundPayload::Text("/sessions".into()));
}

#[test]
fn rejects_foreign_owners_teams_bots_and_unmentioned_channels() {
    for message in [
        json!({"type":"message","channel":"D1","channel_type":"im","user":"OTHER","text":"hello","ts":"1.1"}),
        json!({"type":"message","channel":"D1","channel_type":"im","user":"U1","bot_id":"B1","text":"hello","ts":"1.1"}),
        json!({"type":"message","channel":"C1","user":"U1","text":"hello","ts":"1.1"}),
        json!({"type":"message","subtype":"message_deleted","channel":"D1","user":"U1","text":"hello","ts":"1.1"}),
    ] {
        assert!(normalize_event(&event(message), "T1", "BOT", &["U1".into()]).is_none());
    }
    let mut foreign = event(
        json!({"type":"message","channel":"D1","channel_type":"im","user":"U1","text":"hello","ts":"1.1"}),
    );
    foreign["payload"]["team_id"] = json!("OTHER");
    assert!(normalize_event(&foreign, "T1", "BOT", &["U1".into()]).is_none());
}

#[test]
fn edited_inbox_uses_original_message_identity_and_microsecond_version() {
    let edited = event(
        json!({"type":"message","subtype":"message_changed","channel":"D1","channel_type":"im","message":{"user":"U1","text":"/inbox edited &amp; kept","ts":"1.000001","edited":{"ts":"2.000002"}}}),
    );
    let input = normalize_event(&edited, "T1", "BOT", &["U1".into()]).unwrap();
    assert_eq!(
        input.payload,
        InboundPayload::TextEdited {
            original_event_id: "T1:1.000001:D1".into(),
            version: 2_000_002,
            text: "/inbox edited & kept".into()
        }
    );
}

#[test]
fn button_preserves_action_token_and_message_thread() {
    let envelope = json!({"type":"interactive","envelope_id":"env","payload":{
        "type":"block_actions","team":{"id":"T1"},"user":{"id":"U1"},"channel":{"id":"C1"},
        "message":{"ts":"3.000001","thread_ts":"1.000001"},
        "actions":[{"action_id":"agentix_0","value":"opaque-token","action_ts":"4.1"}]
    }});
    let input = normalize_event(&envelope, "T1", "BOT", &["U1".into()]).unwrap();
    let InboundPayload::Action {
        token,
        message: Some(message),
    } = input.payload
    else {
        panic!("action")
    };
    assert_eq!(token, "opaque-token");
    assert_eq!(message.message_id, "3.000001");
    assert_eq!(message.conversation.conversation_id, "T1:C1:1.000001");
    assert_eq!(
        message.conversation.channel,
        "slack".parse::<ChannelKind>().unwrap()
    );
}

#[test]
fn slash_command_gateway_preserves_prompts_and_routes_commands() {
    for (text, expected) in [
        ("/sessions", "/sessions"),
        ("Please explain /sessions", "Please explain /sessions"),
    ] {
        let envelope = json!({"type":"slash_commands","envelope_id":"slash-1","payload":{"team_id":"T1","user_id":"U1","channel_id":"D1","command":"/agentix","text":text,"trigger_id":"trigger"}});
        let input = normalize_event(&envelope, "T1", "BOT", &["U1".into()]).unwrap();
        assert_eq!(input.payload, InboundPayload::Text(expected.into()));
        assert_eq!(input.conversation.conversation_id, "T1:D1");
        assert!(normalize_event(&envelope, "T1", "BOT", &["OTHER".into()]).is_none());
    }
}

#[test]
fn mention_removal_preserves_quoted_bot_mentions_and_editing_away_the_prefix() {
    let message = event(
        json!({"type":"app_mention","channel":"C1","user":"U1","text":"<@BOT> /inbox keep <@BOT> in the requirement","ts":"1.000001"}),
    );
    assert_eq!(
        normalize_event(&message, "T1", "BOT", &["U1".into()])
            .unwrap()
            .payload,
        InboundPayload::Text("/inbox keep <@BOT> in the requirement".into())
    );
    let edited = event(
        json!({"type":"message","subtype":"message_changed","channel":"C1","previous_message":{"text":"<@BOT> /inbox old"},"message":{"user":"U1","text":"/inbox new","ts":"1.000001","edited":{"ts":"2.000001"}}}),
    );
    assert!(normalize_event(&edited, "T1", "BOT", &["U1".into()]).is_some());
}

#[test]
fn native_slash_commands_translate_to_unified_commands_without_arguments() {
    for (command, text, expected) in [
        ("/agentix-sessions", "", "/sessions"),
        ("/sessions", "", "/sessions"),
        ("/rename", "  My session  ", "/rename My session"),
        ("/agentix-rename", "  My session  ", "/rename My session"),
    ] {
        let envelope = json!({"type":"slash_commands","envelope_id":"native","payload":{
            "team_id":"T1","user_id":"U1","channel_id":"D1","command":command,"text":text}});
        assert_eq!(
            normalize_event(&envelope, "T1", "BOT", &["U1".into()])
                .unwrap()
                .payload,
            InboundPayload::Text(expected.into())
        );
        assert!(normalize_event(&envelope, "T2", "BOT", &["U1".into()]).is_none());
        assert!(normalize_event(&envelope, "T1", "BOT", &["U2".into()]).is_none());
    }
}

#[test]
fn slash_affixes_are_removed_without_changing_arguments_or_dm_text() {
    use agentix_slack::CommandAffixes;
    let affixes = CommandAffixes::new("ax-", "-dev").unwrap();
    for (command, arguments, expected) in [
        ("/ax-sessions-dev", "pi", "/sessions pi"),
        ("/ax-agentix-dev", "/status", "/status"),
    ] {
        let input = json!({"type":"slash_commands","envelope_id":"env","payload":{"team_id":"T1","user_id":"U1","channel_id":"D1","command":command,"text":arguments}});
        let normalized = affixes.normalize(&input).unwrap();
        assert_eq!(
            normalize_event(&normalized, "T1", "BOT", &["U1".into()])
                .unwrap()
                .payload,
            InboundPayload::Text(expected.into())
        );
    }
    let claim =
        json!({"type":"slash_commands","payload":{"command":"/ax-claim-dev","text":"code"}});
    assert_eq!(
        affixes.normalize(&claim).unwrap()["payload"]["command"],
        "/claim"
    );
    let wrong = json!({"type":"slash_commands","payload":{"command":"/sessions"}});
    assert!(affixes.normalize(&wrong).is_none());
    let dm = event(json!({"type":"message","text":"/sessions"}));
    assert_eq!(affixes.normalize(&dm).unwrap(), dm);
}
