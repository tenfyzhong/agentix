//! Preserve first-seen order while updating each output item independently.
use super::{TurnBuffer, TurnOutputItem};

impl TurnBuffer {
    pub(super) fn append_commentary(&mut self, id: &str, delta: &str) {
        if self
            .output_items
            .iter()
            .any(|item| item.id.as_deref() == Some(id) && item.text.is_empty())
        {
            self.record_output(Some(id), "**Reasoning**\n\n", true, false);
        }
        self.record_output(Some(id), delta, true, true);
    }

    pub(super) fn view_sections(&self, agent_name: &str) -> Vec<agentix_domain::ViewSection> {
        use agentix_domain::ViewSection;
        let mut sections = Vec::new();
        if !self.user_text.trim().is_empty() {
            sections.push(ViewSection {
                title: "👤 You".into(),
                body: self.user_text.clone(),
                collapsible: false,
            });
        }
        for item in self
            .output_items
            .iter()
            .filter(|item| item.process && !item.text.trim().is_empty())
        {
            let (title, body) = if let Some(text) = item.text.strip_prefix("**Reasoning**\n\n") {
                ("🧠 Reasoning", text.to_owned())
            } else {
                (
                    "🔨 Tool Call",
                    item.text
                        .strip_prefix("**Tool call**: ")
                        .unwrap_or(&item.text)
                        .to_owned(),
                )
            };
            if let Some(previous) = sections.last_mut()
                && previous.collapsible
                && previous.title == title
            {
                previous.body.push_str("\n\n");
                previous.body.push_str(&body);
                continue;
            }
            sections.push(ViewSection {
                title: title.into(),
                body,
                collapsible: true,
            });
        }
        if !self.agent_text.trim().is_empty() {
            sections.push(ViewSection {
                title: format!("🤖 {agent_name}"),
                body: self.agent_text.clone(),
                collapsible: false,
            });
        }
        sections
    }

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
            .find(|item| id.is_some() && item.id.as_deref() == id)
        {
            existing.process = process;
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
        let mut output = String::new();
        let mut previous_process = None;
        for item in self
            .output_items
            .iter()
            .filter(|item| !item.text.is_empty())
        {
            let process = item
                .process
                .then(|| item.text.starts_with("**Reasoning**\n\n"));
            if process.is_some() && process == previous_process {
                output.push_str("\n\n");
                output.push_str(
                    item.text
                        .strip_prefix("**Reasoning**\n\n")
                        .or_else(|| item.text.strip_prefix("**Tool call**: "))
                        .unwrap_or(&item.text),
                );
            } else {
                if !output.is_empty() {
                    output.push_str("\n\n\n");
                }
                if has_process && !item.process {
                    output.push_str("**Output**\n\n");
                }
                output.push_str(&item.text);
            }
            previous_process = process;
        }
        output
    }
}
