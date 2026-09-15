//! Coalesce content received while the initial attachment snapshot is pending.
use std::collections::HashSet;

use crate::{AgentEvent, HistoryPage, ItemSummary, TurnStatus, TurnSummary};

#[derive(Default)]
pub(super) struct AttachmentHistory {
    turns: Vec<ObservedTurn>,
}

struct ObservedTurn {
    summary: TurnSummary,
    snapshots: HashSet<String>,
}

impl AttachmentHistory {
    pub(super) fn observe(&mut self, event: &AgentEvent) {
        let (AgentEvent::TurnStarted { turn_id, .. }
        | AgentEvent::TurnCompleted { turn_id, .. }
        | AgentEvent::UserMessage { turn_id, .. }
        | AgentEvent::AgentMessageDelta { turn_id, .. }
        | AgentEvent::ItemStarted { turn_id, .. }
        | AgentEvent::ItemCompleted { turn_id, .. }) = event
        else {
            return;
        };
        let index = self
            .turns
            .iter()
            .position(|turn| &turn.summary.id == turn_id)
            .unwrap_or_else(|| {
                // Attachment presents recent content, not an unbounded event backlog.
                if self.turns.len() == 32 {
                    self.turns.remove(0);
                }
                self.turns.push(ObservedTurn {
                    summary: TurnSummary {
                        id: turn_id.clone(),
                        status: TurnStatus::InProgress,
                        user_text: None,
                        agent_text: None,
                        tools: Vec::new(),
                        items: Vec::new(),
                    },
                    snapshots: HashSet::new(),
                });
                self.turns.len() - 1
            });
        let turn = &mut self.turns[index];
        let (item, append, snapshot) = match event {
            AgentEvent::TurnCompleted { status, .. } => {
                turn.summary.status = status.clone();
                return;
            }
            AgentEvent::UserMessage { item_id, text, .. } => (
                ItemSummary {
                    id: item_id.clone(),
                    kind: "userMessage".into(),
                    text: Some(text.clone()),
                    status: None,
                },
                false,
                true,
            ),
            AgentEvent::ItemCompleted { item, .. } => (item.clone(), false, true),
            AgentEvent::ItemStarted { item_id, kind, .. } => (
                ItemSummary {
                    id: item_id.clone(),
                    kind: kind.clone(),
                    text: None,
                    status: Some("inProgress".into()),
                },
                false,
                false,
            ),
            AgentEvent::AgentMessageDelta { item_id, delta, .. } => (
                ItemSummary {
                    id: item_id.clone(),
                    kind: "agentMessage".into(),
                    text: Some(delta.clone()),
                    status: None,
                },
                true,
                false,
            ),
            _ => return,
        };
        if snapshot {
            turn.snapshots.insert(item.id.clone());
        }
        if let Some(existing) = turn
            .summary
            .items
            .iter_mut()
            .find(|existing| existing.id == item.id)
        {
            if append {
                existing
                    .text
                    .get_or_insert_default()
                    .push_str(item.text.as_deref().unwrap_or_default());
            } else if snapshot {
                *existing = item;
            }
        } else {
            turn.summary.items.push(item);
        }
    }

    pub(super) fn merge(self, history: &mut HistoryPage) {
        for observed in self.turns {
            if let Some(turn) = history
                .turns
                .iter_mut()
                .find(|turn| turn.id == observed.summary.id)
            {
                if !matches!(
                    observed.summary.status,
                    TurnStatus::InProgress | TurnStatus::Unknown
                ) {
                    turn.status = observed.summary.status.clone();
                }
                seed_summary_items(turn, &observed.summary);
                for item in observed.summary.items {
                    if let Some(existing) = turn
                        .items
                        .iter_mut()
                        .find(|existing| existing.id == item.id)
                    {
                        if observed.snapshots.contains(&item.id) {
                            *existing = item;
                        } else if let Some(text) = item.text {
                            existing.text = Some(merge_delta(
                                existing.text.as_deref().unwrap_or_default(),
                                &text,
                            ));
                        }
                    } else {
                        turn.items.push(item);
                    }
                }
                if turn.items.iter().any(|item| item.kind == "userMessage") {
                    turn.user_text = None;
                }
            } else {
                history.turns.push(observed.summary);
            }
        }
    }
}

// Older hosts have cumulative text without item IDs. Give it an identity before
// merging events so a partial stream cannot hide the earlier snapshot text.
fn seed_summary_items(turn: &mut TurnSummary, observed: &TurnSummary) {
    for (kind, text) in [
        ("agentMessage", &turn.agent_text),
        ("userMessage", &turn.user_text),
    ] {
        if turn.items.iter().any(|item| item.kind == kind) {
            continue;
        }
        let Some(text) = text.as_deref().filter(|text| !text.is_empty()) else {
            continue;
        };
        let matching = observed.items.iter().find(|item| {
            item.kind == kind && (kind == "agentMessage" || item.text.as_deref() == Some(text))
        });
        if kind == "agentMessage" && matching.is_none() {
            continue;
        }
        turn.items.push(ItemSummary {
            id: matching.map_or_else(
                || format!("attachment-history-user:{}", turn.id),
                |item| item.id.clone(),
            ),
            kind: kind.into(),
            text: Some(text.into()),
            status: None,
        });
    }
}

// The notification stream can start before or after the history snapshot.
// Preserve their shared text once, including UTF-8 boundaries.
fn merge_delta(history: &str, observed: &str) -> String {
    if history.contains(observed) {
        return history.to_owned();
    }
    if observed.starts_with(history) {
        return observed.to_owned();
    }
    // KMP keeps long, repetitive output linear instead of comparing every suffix.
    let pattern = observed.as_bytes();
    let mut prefix = vec![0; pattern.len()];
    for index in 1..pattern.len() {
        let mut length = prefix[index - 1];
        while length > 0 && pattern[index] != pattern[length] {
            length = prefix[length - 1];
        }
        if pattern[index] == pattern[length] {
            length += 1;
        }
        prefix[index] = length;
    }
    let mut overlap = 0;
    for byte in history.bytes() {
        while overlap > 0 && (overlap == pattern.len() || byte != pattern[overlap]) {
            overlap = prefix[overlap - 1];
        }
        if byte == pattern[overlap] {
            overlap += 1;
        }
    }
    format!("{history}{}", &observed[overlap..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_history_merges_partial_summary_and_later_input() {
        let mut observed = AttachmentHistory::default();
        observed.observe(&AgentEvent::AgentMessageDelta {
            session_id: "s".into(),
            turn_id: "t".into(),
            item_id: "answer".into(),
            delta: " world".into(),
        });
        observed.observe(&AgentEvent::UserMessage {
            session_id: "s".into(),
            turn_id: "t".into(),
            item_id: "user2".into(),
            text: "second question".into(),
        });
        let mut page = HistoryPage {
            turns: vec![TurnSummary {
                id: "t".into(),
                status: TurnStatus::InProgress,
                user_text: Some("first question".into()),
                agent_text: Some("hello".into()),
                tools: vec![],
                items: vec![],
            }],
            older_cursor: None,
            newer_cursor: None,
        };
        observed.merge(&mut page);
        let buffer =
            super::super::TurnBuffer::from_summary(&page.turns[0], crate::OutputConfig::default());
        assert_eq!(buffer.agent_text, "hello world");
        assert_eq!(buffer.user_text, "first question\n\nsecond question");
    }

    #[test]
    fn attachment_history_merges_overlapping_utf8_deltas_once() {
        for (snapshot, delta, expected) in [
            ("你好", "好世界", "你好世界"),
            ("hello world", " world", "hello world"),
            ("hello", "hello world", "hello world"),
            ("hello wor", " world", "hello world"),
        ] {
            assert_eq!(merge_delta(snapshot, delta), expected);
        }
    }
}
