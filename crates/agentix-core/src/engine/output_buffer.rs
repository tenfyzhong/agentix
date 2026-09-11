//! Preserve first-seen order while updating each output item independently.
use super::{TurnBuffer, TurnOutputItem};

impl TurnBuffer {
    pub(super) fn record_output(
        &mut self,
        id: Option<&str>,
        text: &str,
        process: bool,
        append: bool,
    ) {
        // Hydrated history has text but no item IDs yet.
        if self.output_items.is_empty() && !self.agent_text.is_empty() {
            self.output_items.push(TurnOutputItem {
                id: if process { None } else { id.map(str::to_owned) },
                text: self.agent_text.clone(),
                process: false,
            });
        }
        if let Some(existing) = self
            .output_items
            .iter_mut()
            .find(|item| id.is_some() && item.id.as_deref() == id && item.process == process)
        {
            if append {
                existing.text.push_str(text);
            } else {
                text.clone_into(&mut existing.text);
            }
        } else {
            self.output_items.push(TurnOutputItem {
                id: id.map(str::to_owned),
                text: text.to_owned(),
                process,
            });
        }
        self.agent_text = self
            .output_items
            .iter()
            .filter(|item| !item.process && !item.text.is_empty())
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
    }

    pub(super) fn render_output(&self) -> String {
        if self.output_items.is_empty() {
            return self.agent_text.clone();
        }
        let has_process = self.output_items.iter().any(|item| item.process);
        self.output_items
            .iter()
            .filter(|item| !item.text.is_empty())
            .map(|item| {
                if has_process && !item.process {
                    format!("**Output**\n\n{}", item.text)
                } else {
                    item.text.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n\n")
    }
}
