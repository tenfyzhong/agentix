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
        let reasoning = matches!(item.kind.as_str(), "reasoning" | "thinking");
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
            return (!text.is_empty()).then(|| format!("Reasoning\n\n{text}"));
        }
        let status = item.status.as_deref().unwrap_or("completed");
        Some(
            format!("Tool call: {} ({status})\n\n{text}", item.kind)
                .trim_end()
                .to_owned(),
        )
    }
}
