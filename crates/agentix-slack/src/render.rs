use agentix_domain::{ActionStyle, ChannelError, OutboundView};
use serde_json::{Value, json};

fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn escaped_char(character: char, buffer: &mut [u8; 4]) -> &str {
    match character {
        '&' => "&amp;",
        '<' => "&lt;",
        '>' => "&gt;",
        _ => character.encode_utf8(buffer),
    }
}

fn fallback(title: &str, body: &str) -> String {
    let mut result = String::new();
    let mut length = 0;
    for character in title
        .chars()
        .chain(std::iter::once('\n'))
        .chain(body.chars())
    {
        let mut buffer = [0; 4];
        let escaped = escaped_char(character, &mut buffer);
        let count = if character.is_ascii() {
            escaped.len()
        } else {
            1
        };
        if length + count > 4000 {
            break;
        }
        result.push_str(escaped);
        length += count;
    }
    result
}

// Work and temporary allocations stop at the display budget, regardless of the
// input size. Escapes and code delimiters are indivisible across section splits.
fn sections(mut input: &str, budget: usize) -> Vec<String> {
    let mut output = Vec::new();
    let mut fenced = false;
    let mut inline = false;
    while !input.is_empty() && output.len() < budget {
        let mut text = if fenced {
            "```\n".to_owned()
        } else {
            String::new()
        };
        let mut length = text.len();
        while !input.is_empty() && length < 2880 {
            if input.starts_with("```") {
                text.push_str("```");
                length += 3;
                input = &input[3..];
                fenced = !fenced;
            } else if !fenced && !inline && input.starts_with("**") {
                text.push('*');
                length += 1;
                input = &input[2..];
            } else {
                let character = input.chars().next().expect("nonempty input");
                let mut buffer = [0; 4];
                let escaped = escaped_char(character, &mut buffer);
                length += if character.is_ascii() {
                    escaped.len()
                } else {
                    1
                };
                text.push_str(escaped);
                input = &input[character.len_utf8()..];
                if character == '`' && !fenced {
                    inline = !inline;
                }
            }
        }
        if fenced {
            text.push_str("\n```");
        }
        if output.len() + 1 == budget && !input.is_empty() {
            text.push_str("\n… (truncated)");
        }
        output.push(text);
    }
    output
}

/// Render within Slack's 50-block, 3000-character section and button limits.
/// User/model content cannot introduce Slack mention or link control sequences.
pub fn render_view(view: &OutboundView) -> Result<Value, ChannelError> {
    if view.actions.len() > 25
        || view
            .actions
            .iter()
            .any(|button| button.token.chars().count() > 2000 || button.token.is_empty())
    {
        return Err(ChannelError::InvalidPayload(
            "Slack supports up to 25 actions with nonempty values of at most 2000 characters"
                .into(),
        ));
    }
    let title = if view.title.is_empty() {
        "Agentix"
    } else {
        &view.title
    };
    let mut blocks =
        vec![json!({"type":"header","text":{"type":"plain_text","text":truncate(title,150)}})];
    if let Some(subtitle) = view.subtitle.as_deref().filter(|text| !text.is_empty()) {
        blocks.push(json!({"type":"context","elements":[{"type":"plain_text","text":truncate(subtitle,2000)}]}));
    }
    let budget = 50 - blocks.len() - usize::from(!view.actions.is_empty());
    for text in sections(&view.body, budget) {
        blocks.push(json!({"type":"section","text":{"type":"mrkdwn","text":text,"verbatim":true}}));
    }
    if !view.actions.is_empty() {
        let elements: Vec<_> = view.actions.iter().enumerate().map(|(index,button)| {
            let mut element = json!({"type":"button","action_id":format!("agentix_{index}"),"text":{"type":"plain_text","text":truncate(&button.label,75)},"value":button.token});
            match button.style {
                ActionStyle::Primary => element["style"] = json!("primary"),
                ActionStyle::Danger => element["style"] = json!("danger"),
                ActionStyle::Default => {}
            }
            element
        }).collect();
        blocks.push(json!({"type":"actions","elements":elements}));
    }
    Ok(
        json!({"text":fallback(title, &view.body),"blocks":blocks,"unfurl_links":false,"unfurl_media":false,"parse":"none"}),
    )
}
