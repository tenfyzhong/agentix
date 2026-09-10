use agentix_domain::ChannelCommand;
use agentix_slack::merge_command_manifest;
use serde_json::{Value, json};

fn commands() -> Vec<ChannelCommand> {
    vec![ChannelCommand::new("sessions", "Browse running sessions")]
}

#[test]
fn merge_preserves_server_settings_and_unrelated_commands_and_is_idempotent() {
    let remote = json!({"display_information":{"name":"Agentix"},
        "settings":{"socket_mode_enabled":true,"event_subscriptions":{"bot_events":["message.im"]}},
        "future_field":{"preserved":true},
        "features":{"shortcuts":[{"name":"Keep me"}],"slash_commands":[
            {"command":"/other","description":"Unrelated","url":"https://example.com"},
            {"command":"/agentix","description":"Old","url":"https://old.example.com"},
            {"command":"/agentix-obsolete","description":"Removed"},
            {"command":"/sessions","description":"Stale"},
            {"command":"/claim","description":"Bootstrap"}]}});
    let merged = merge_command_manifest(&remote, &commands()).unwrap();
    assert_eq!(merged["settings"], remote["settings"]);
    assert_eq!(merged["future_field"], remote["future_field"]);
    assert_eq!(
        merged["features"]["shortcuts"],
        remote["features"]["shortcuts"]
    );
    let slash = merged["features"]["slash_commands"].as_array().unwrap();
    assert_eq!(slash.len(), 3);
    assert_eq!(slash[0], remote["features"]["slash_commands"][0]);
    assert!(slash.iter().any(|c| c["command"] == "/sessions"));
    assert!(slash.iter().skip(1).all(|c| c.get("url").is_none()));
    assert_eq!(
        merge_command_manifest(&merged, &commands()).unwrap(),
        merged
    );
}

#[test]
fn rejects_malformed_manifests_and_command_definitions_before_update() {
    for remote in [
        Value::Null,
        json!({"features":[]}),
        json!({"features":{"slash_commands":{}}}),
        json!({"settings":{"socket_mode_enabled":false}}),
    ] {
        assert!(merge_command_manifest(&remote, &commands()).is_err());
    }
    let remote = json!({"settings":{"socket_mode_enabled":true}});
    for name in [
        "",
        "bad name",
        "UPPER",
        "this-command-name-is-far-too-long-for-slack",
    ] {
        assert!(merge_command_manifest(&remote, &[ChannelCommand::new(name, "test")]).is_err());
    }
}

#[test]
fn adds_commands_scope_without_removing_existing_oauth_configuration() {
    let remote = json!({"settings":{"socket_mode_enabled":true},
        "oauth_config":{"redirect_urls":["https://example.com/oauth"],
            "scopes":{"bot":["chat:write"],"user":["search:read"]}}});
    let merged = merge_command_manifest(&remote, &commands()).unwrap();
    assert_eq!(
        merged["oauth_config"]["scopes"]["bot"],
        json!(["chat:write", "commands"])
    );
    assert_eq!(
        merged["oauth_config"]["scopes"]["user"],
        remote["oauth_config"]["scopes"]["user"]
    );
    assert_eq!(
        merged["oauth_config"]["redirect_urls"],
        remote["oauth_config"]["redirect_urls"]
    );
    assert_eq!(
        merge_command_manifest(&merged, &commands()).unwrap(),
        merged
    );
}

#[test]
fn reserves_prefix_only_for_names_rejected_by_slack() {
    let remote = json!({"settings":{"socket_mode_enabled":true}});
    let commands = ["sessions", "rename", "status"].map(|name| ChannelCommand::new(name, "test"));
    let merged = merge_command_manifest(&remote, &commands).unwrap();
    let names = merged["features"]["slash_commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["command"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "/agentix",
            "/sessions",
            "/agentix-rename",
            "/agentix-status"
        ]
    );
    assert_eq!(merge_command_manifest(&merged, &commands).unwrap(), merged);
}
