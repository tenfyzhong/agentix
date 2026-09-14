//! Preserve first-seen order while updating each output item independently.
use super::{TurnBuffer, TurnOutputItem};

impl TurnBuffer {
    pub(super) fn from_summary(turn: &crate::TurnSummary, output: crate::OutputConfig) -> Self {
        let mut buffer = Self {
            user_text: turn.user_text.clone().unwrap_or_default(),
            status: turn.status.clone(),
            ..Self::default()
        };
        for item in &turn.items {
            if item.kind != "userMessage" {
                buffer.apply_item(item, output);
            }
        }
        if buffer.user_text.trim().is_empty() {
            let mut users: Vec<&crate::ItemSummary> = Vec::new();
            for item in turn.items.iter().filter(|item| item.kind == "userMessage") {
                if let Some(previous) = users.iter_mut().find(|previous| previous.id == item.id) {
                    *previous = item;
                } else {
                    users.push(item);
                }
            }
            buffer.user_text = users
                .iter()
                .filter_map(|item| item.text.as_deref())
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
        }
        // Older hosts may expose summary text without corresponding message items.
        if buffer.agent_text.trim().is_empty()
            && let Some(text) = turn
                .agent_text
                .as_deref()
                .filter(|text| !text.trim().is_empty())
        {
            buffer.record_summary(text);
        }
        buffer
    }

    pub(super) fn merge_summary(&mut self, turn: &crate::TurnSummary, output: crate::OutputConfig) {
        let restored = Self::from_summary(turn, output);
        if !restored.user_text.trim().is_empty()
            && (turn
                .user_text
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
                || self.user_text.trim().is_empty())
        {
            self.user_text = restored.user_text;
        }
        for item in restored.output_items {
            if item.summary_fallback {
                self.record_summary(&item.text);
            } else {
                self.record_output(item.id.as_deref(), &item.text, item.process, false);
            }
        }
    }

    fn record_summary(&mut self, text: &str) {
        // Bridges can expose one cumulative answer without including its item ID
        // in history. Update that answer, preserving its ID if already known.
        let mut answers = self.output_items.iter_mut().filter(|item| !item.process);
        if let Some(item) = answers.next() {
            if answers.next().is_some() {
                // A summary cannot identify which of several distinct answers changed.
                return;
            }
            if item.text == text {
                return;
            }
            item.text = text.into();
        } else {
            self.output_items.push(TurnOutputItem {
                active: false,
                id: None,
                text: text.into(),
                process: false,
                summary_fallback: true,
            });
        }
        if let Some(index) = self.output_items.iter().position(|item| !item.process) {
            self.activate_output(index);
        }
        self.refresh_agent_text();
    }

    pub(super) fn apply_item(
        &mut self,
        item: &crate::ItemSummary,
        output: crate::OutputConfig,
    ) -> bool {
        let process = output.process_text(item);
        if !matches!(
            item.kind.as_str(),
            "agentMessage" | "userMessage" | "commentary"
        ) && process.is_none()
        {
            return false;
        }
        self.ensure_started();
        match item.kind.as_str() {
            "agentMessage" => self.record_output(
                Some(&item.id),
                item.text.as_deref().unwrap_or_default(),
                false,
                false,
            ),
            "commentary" => self.record_output(
                Some(&item.id),
                process.as_deref().unwrap_or_default(),
                true,
                false,
            ),
            "userMessage" => self.user_text = item.text.clone().unwrap_or_default(),
            _ => {
                if let Some(text) = process {
                    self.record_output(Some(&item.id), &text, true, false);
                }
            }
        }
        true
    }

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
                expanded: None,
            });
        }
        for item in self
            .output_items
            .iter()
            .filter(|item| !item.text.trim().is_empty())
        {
            let agent_title = format!("🤖 {agent_name}");
            let (title, body) = if !item.process {
                (agent_title.as_str(), item.text.clone())
            } else if let Some(text) = item.text.strip_prefix("**Reasoning**\n\n") {
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
                && previous.title == title
            {
                previous.body.push_str("\n\n");
                previous.body.push_str(&body);
                previous.expanded = Some(previous.expanded == Some(true) || item.active);
                continue;
            }
            sections.push(ViewSection {
                title: title.into(),
                body,
                collapsible: item.process,
                expanded: Some(item.active),
            });
        }
        if self.output_items.is_empty() && !self.agent_text.trim().is_empty() {
            sections.push(ViewSection {
                title: format!("🤖 {agent_name}"),
                body: self.agent_text.clone(),
                collapsible: false,
                expanded: None,
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
                active: false,
                summary_fallback: process || id.is_none(),
                id: if process { None } else { id.map(str::to_owned) },
                text: self.agent_text.clone(),
                process: false,
            });
        }
        let existing_index = self
            .output_items
            .iter()
            .position(|item| id.is_some() && item.id.as_deref() == id)
            .or_else(|| {
                (!process && id.is_some())
                    .then(|| {
                        self.output_items
                            .iter()
                            .position(|item| item.summary_fallback)
                    })
                    .flatten()
            });
        let activate = !text.trim().is_empty()
            && (append
                || existing_index.is_none_or(|index| {
                    let existing = &self.output_items[index];
                    existing.text.trim().is_empty()
                        || (!process && (existing.process || existing.text != text))
                }));
        if let Some(index) = existing_index {
            let existing = &mut self.output_items[index];
            if existing.summary_fallback {
                existing.id = id.map(str::to_owned);
                existing.summary_fallback = false;
            }
            existing.process = process;
            if append {
                existing.text.push_str(text);
            } else {
                text.clone_into(&mut existing.text);
            }
        } else {
            self.output_items.push(TurnOutputItem {
                active: false,
                summary_fallback: false,
                id: id.map(str::to_owned),
                text: text.to_owned(),
                process,
            });
        }
        if activate {
            self.activate_output(existing_index.unwrap_or(self.output_items.len() - 1));
        }
        self.refresh_agent_text();
    }

    fn activate_output(&mut self, index: usize) {
        for (position, item) in self.output_items.iter_mut().enumerate() {
            item.active = position == index;
        }
    }

    fn refresh_agent_text(&mut self) {
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
        let has_process = self
            .output_items
            .iter()
            .any(|item| item.process && !item.text.trim().is_empty());
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
