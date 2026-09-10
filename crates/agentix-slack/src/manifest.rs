//! Merge Agentix commands and their required scope into a server manifest.
use agentix_domain::{ChannelCommand, ChannelError};
use serde_json::{Value, json};
use std::collections::HashSet;

pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 23 // Slack's 32-character limit includes /agentix-.
        && name.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

fn invalid() -> ChannelError {
    ChannelError::InvalidPayload(
        "invalid Slack manifest or command catalog; Socket Mode must be enabled".into(),
    )
}

/// Preserve unrelated configuration and migrate the legacy prefixed menu.
/// Commands in the supplied catalog are managed by Agentix.
pub fn merge_command_manifest(
    remote: &Value,
    commands: &[ChannelCommand],
) -> Result<Value, ChannelError> {
    if !remote.is_object() || remote["settings"]["socket_mode_enabled"] != true {
        return Err(invalid());
    }
    let mut merged = remote.clone();
    let features = merged
        .as_object_mut()
        .ok_or_else(invalid)?
        .entry("features")
        .or_insert_with(|| json!({}));
    let features = features.as_object_mut().ok_or_else(invalid)?;
    let slash = features
        .entry("slash_commands")
        .or_insert_with(|| json!([]));
    let slash = slash.as_array_mut().ok_or_else(invalid)?;
    if slash.iter().any(|c| c["command"].as_str().is_none()) {
        return Err(invalid());
    }
    slash.retain(|c| {
        c["command"].as_str().is_some_and(|name| {
            name != "/claim"
                && name != "/agentix"
                && !name.starts_with("/agentix-")
                && !commands
                    .iter()
                    .any(|command| name.strip_prefix('/') == Some(command.name.as_str()))
        })
    });
    slash.push(json!({"command":"/agentix","description":"Send a command or prompt to Agentix","usage_hint":"/sessions or your prompt","should_escape":false}));
    let mut names = HashSet::new();
    for command in commands {
        if !valid_name(&command.name)
            || command.description.is_empty()
            || command.description.chars().count() > 100
            || !names.insert(&command.name)
        {
            return Err(invalid());
        }
        // Slack rejects these built-in names during manifest validation.
        let prefix = if matches!(command.name.as_str(), "rename" | "status") {
            "/agentix-"
        } else {
            "/"
        };
        slash.push(json!({"command":format!("{prefix}{}",command.name),"description":command.description,"should_escape":false}));
    }
    if slash.len() > 100 {
        return Err(invalid());
    }
    let oauth = merged
        .as_object_mut()
        .ok_or_else(invalid)?
        .entry("oauth_config")
        .or_insert_with(|| json!({}));
    let scopes = oauth
        .as_object_mut()
        .ok_or_else(invalid)?
        .entry("scopes")
        .or_insert_with(|| json!({}));
    let bot = scopes
        .as_object_mut()
        .ok_or_else(invalid)?
        .entry("bot")
        .or_insert_with(|| json!([]));
    let bot = bot.as_array_mut().ok_or_else(invalid)?;
    if !bot.iter().any(|scope| scope == "commands") {
        bot.push(json!("commands"));
    }
    Ok(merged)
}

/// Before enrollment, this dedicated app exposes only the claim command.
pub(crate) fn merge_for_owner(
    remote: &Value,
    commands: &[ChannelCommand],
    has_owner: bool,
) -> Result<Value, ChannelError> {
    let mut merged = merge_command_manifest(remote, commands)?;
    if !has_owner {
        merged["features"]["slash_commands"] = json!([{
            "command":"/claim", "description":"Link your Slack account to Agentix",
            "usage_hint":"<code>", "should_escape":false
        }]);
    }
    Ok(merged)
}

// Slack can reorder commands and omit false/empty defaults on export.
pub(crate) fn commands_match(left: &Value, right: &Value) -> bool {
    fn normalized(value: &Value) -> Vec<(String, String, String, String, bool)> {
        let mut commands = value["features"]["slash_commands"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| {
                (
                    c["command"].as_str().unwrap_or_default().to_owned(),
                    c["description"].as_str().unwrap_or_default().to_owned(),
                    c["usage_hint"].as_str().unwrap_or_default().to_owned(),
                    c["url"].as_str().unwrap_or_default().to_owned(),
                    c["should_escape"].as_bool().unwrap_or(false),
                )
            })
            .collect::<Vec<_>>();
        commands.sort();
        commands
    }
    normalized(left) == normalized(right)
        && left["oauth_config"]["scopes"]["bot"]
            .as_array()
            .is_some_and(|scopes| scopes.iter().any(|s| s == "commands"))
}
