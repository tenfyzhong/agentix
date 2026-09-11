//! One selection and formatting policy for IM output and Job conversations.
use crate::ItemSummary;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutputConfig {
    pub show_reasoning: bool,
    pub show_tool_calls: bool,
}

impl OutputConfig {
    pub(crate) fn project_event(self, event: crate::AgentEvent) -> crate::AgentEvent {
        if let crate::AgentEvent::ItemStarted {
            session_id,
            turn_id,
            item_id,
            kind,
            label,
        } = &event
        {
            // Start labels describe the item type, not reasoning content.
            if matches!(kind.as_str(), "reasoning" | "thinking" | "commentary") {
                return event;
            }
            let item = ItemSummary {
                id: item_id.clone(),
                kind: kind.clone(),
                text: Some(label.clone()),
                status: Some("inProgress".into()),
            };
            if self.process_text(&item).is_some() {
                return crate::AgentEvent::ItemCompleted {
                    session_id: session_id.clone(),
                    turn_id: turn_id.clone(),
                    item,
                };
            }
        }
        event
    }

    pub(crate) fn process_text(self, item: &ItemSummary) -> Option<String> {
        let reasoning = matches!(item.kind.as_str(), "reasoning" | "thinking" | "commentary");
        if matches!(
            item.kind.as_str(),
            "agentMessage" | "userMessage" | "plan" | "unknown"
        ) || (reasoning && !self.show_reasoning)
            || (!reasoning && !self.show_tool_calls)
        {
            return None;
        }
        let text = item.text.as_deref().unwrap_or_default().trim();
        if reasoning {
            return (!text.is_empty()).then(|| format!("**Reasoning**\n\n{text}"));
        }
        let status = item.status.as_deref().unwrap_or("completed");
        Some(
            format!("**Tool call**: {} ({status})\n\n{text}", item.kind)
                .trim_end()
                .to_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_started_labels_are_not_projected_as_content() {
        let config = OutputConfig {
            show_reasoning: true,
            show_tool_calls: true,
        };
        for kind in ["reasoning", "thinking"] {
            let event = crate::AgentEvent::ItemStarted {
                session_id: "s".into(),
                turn_id: "t".into(),
                item_id: "r".into(),
                kind: kind.into(),
                label: kind.into(),
            };
            assert!(matches!(
                config.project_event(event),
                crate::AgentEvent::ItemStarted { .. }
            ));
        }
    }
}
