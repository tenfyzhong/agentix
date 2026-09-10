use agentix_domain::{ChannelKind, ConversationRef, InboundEnvelope, InboundPayload, MessageRef};
use serde_json::Value;

fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|value| !value.is_empty())
}

pub(crate) fn conversation(team: &str, channel: &str, thread: Option<&str>) -> ConversationRef {
    ConversationRef::new(
        ChannelKind::Slack,
        thread.map_or_else(
            || format!("{team}:{channel}"),
            |thread| format!("{team}:{channel}:{thread}"),
        ),
    )
}

fn timestamp_version(ts: &str) -> Option<i64> {
    let (seconds, fraction) = ts.split_once('.')?;
    if fraction.len() > 6 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let micros =
        fraction.parse::<i64>().ok()? * 10_i64.pow(6 - u32::try_from(fraction.len()).ok()?);
    seconds
        .parse::<i64>()
        .ok()?
        .checked_mul(1_000_000)?
        .checked_add(micros)
}

/// Normalize authenticated Socket Mode payloads. Owners and workspace are checked
/// before any user input reaches the core, including interactive callbacks.
#[must_use]
pub fn normalize_event(
    envelope: &Value,
    team: &str,
    bot: &str,
    owners: &[String],
) -> Option<InboundEnvelope> {
    let payload = envelope.get("payload")?;
    if envelope["type"] == "slash_commands" {
        let owner = string(payload, "user_id")?;
        if payload["team_id"] != team || !owners.iter().any(|allowed| allowed == owner) {
            return None;
        }
        let command = string(payload, "command")?;
        let arguments = payload["text"].as_str()?.trim();
        let text = if command == "/agentix" {
            arguments.to_owned()
        } else {
            let name = command
                .strip_prefix("/agentix-")
                .or_else(|| command.strip_prefix('/'))?;
            if !crate::manifest::valid_name(name) {
                return None;
            }
            if arguments.is_empty() {
                format!("/{name}")
            } else {
                format!("/{name} {arguments}")
            }
        };
        if text.is_empty() {
            return None;
        }
        return Some(InboundEnvelope::text(
            format!("{team}:slash:{}", string(envelope, "envelope_id")?),
            conversation(team, string(payload, "channel_id")?, None),
            owner,
            text,
        ));
    }
    if envelope["type"] == "interactive" {
        if payload["type"] != "block_actions" || payload["team"]["id"] != team {
            return None;
        }
        let owner = string(&payload["user"], "id")?;
        if !owners.iter().any(|allowed| allowed == owner) {
            return None;
        }
        let channel = string(&payload["channel"], "id")?;
        let message = payload.get("message")?;
        let ts = string(message, "ts")?;
        let action = payload["actions"].as_array()?.first()?;
        if !string(action, "action_id")?.starts_with("agentix_") {
            return None;
        }
        let conversation = conversation(team, channel, string(message, "thread_ts"));
        return Some(InboundEnvelope::action_from_message(
            format!("{team}:action:{}", string(envelope, "envelope_id")?),
            conversation.clone(),
            owner,
            string(action, "value")?,
            MessageRef::new(conversation, ts),
        ));
    }
    normalize_message(envelope, team, bot, owners)
}

fn normalize_message(
    envelope: &Value,
    team: &str,
    bot: &str,
    owners: &[String],
) -> Option<InboundEnvelope> {
    let payload = envelope.get("payload")?;
    if envelope["type"] != "events_api" || payload["team_id"] != team {
        return None;
    }
    let event = payload.get("event")?;
    if !matches!(string(event, "type")?, "message" | "app_mention") {
        return None;
    }
    let edited = event["subtype"] == "message_changed";
    let message = if edited { event.get("message")? } else { event };
    if string(message, "bot_id").is_some()
        || (string(message, "subtype").is_some_and(|kind| kind != "file_share"))
        || (!edited && string(event, "subtype").is_some_and(|kind| kind != "file_share"))
    {
        return None;
    }
    let owner = string(message, "user")?;
    if owner == bot || !owners.iter().any(|allowed| allowed == owner) {
        return None;
    }
    let text = string(message, "text")?;
    let mention = format!("<@{bot}>");
    let channel = string(event, "channel")?;
    let private = event["channel_type"] == "im" || channel.starts_with('D');
    let previously_mentioned = edited
        && event["previous_message"]["text"]
            .as_str()
            .is_some_and(|text| text.contains(&mention));
    if !private && !text.contains(&mention) && !previously_mentioned {
        return None;
    }
    let text = text.trim_start();
    let text = text
        .strip_prefix(&mention)
        .unwrap_or(text)
        .trim()
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    if text.is_empty() {
        return None;
    }
    let ts = string(message, "ts")?;
    let conversation = conversation(team, channel, string(message, "thread_ts"));
    let original = format!("{team}:{ts}:{channel}");
    if edited {
        let version = timestamp_version(string(&message["edited"], "ts")?)?;
        return Some(InboundEnvelope {
            event_id: format!("{original}:edit:{version}"),
            conversation,
            owner_id: owner.into(),
            payload: InboundPayload::TextEdited {
                original_event_id: original,
                version,
                text,
            },
        });
    }
    Some(InboundEnvelope::text(original, conversation, owner, text))
}
