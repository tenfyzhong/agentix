//! Explicit conversion from the public wire contract to Agentix domain models.
use crate::wire;
use agentix_domain as core;

impl From<wire::SessionStatus> for core::SessionStatus {
    fn from(value: wire::SessionStatus) -> Self {
        match value {
            wire::SessionStatus::NotLoaded => Self::NotLoaded,
            wire::SessionStatus::Idle => Self::Idle,
            wire::SessionStatus::Active => Self::Active,
            wire::SessionStatus::SystemError => Self::SystemError,
            wire::SessionStatus::Offline => Self::Offline,
            wire::SessionStatus::Unknown => Self::Unknown,
        }
    }
}
impl From<wire::TurnStatus> for core::TurnStatus {
    fn from(value: wire::TurnStatus) -> Self {
        match value {
            wire::TurnStatus::InProgress => Self::InProgress,
            wire::TurnStatus::Completed => Self::Completed,
            wire::TurnStatus::Interrupted => Self::Interrupted,
            wire::TurnStatus::Failed => Self::Failed,
            wire::TurnStatus::Unknown => Self::Unknown,
        }
    }
}
impl From<wire::Session> for core::SessionSummary {
    fn from(value: wire::Session) -> Self {
        Self {
            id: core::SessionId::new(value.id),
            name: value.name,
            preview: value.preview,
            cwd: value.cwd,
            updated_at: value.updated_at,
            status: value.status.into(),
            terminal: None,
        }
    }
}
impl From<wire::Item> for core::ItemSummary {
    fn from(value: wire::Item) -> Self {
        Self {
            id: value.id,
            kind: value.kind,
            text: value.text,
            status: value.status,
        }
    }
}
impl From<wire::Turn> for core::TurnSummary {
    fn from(value: wire::Turn) -> Self {
        Self {
            id: value.id,
            status: value.status.into(),
            user_text: value.user_text,
            agent_text: value.agent_text,
            tools: value
                .tools
                .into_iter()
                .map(|t| core::ToolSummary {
                    kind: t.kind,
                    label: t.label,
                    status: t.status,
                })
                .collect(),
            items: value.items.into_iter().map(Into::into).collect(),
        }
    }
}
impl From<wire::History> for core::HistoryPage {
    fn from(value: wire::History) -> Self {
        Self {
            turns: value.turns.into_iter().map(Into::into).collect(),
            older_cursor: value.older_cursor,
            newer_cursor: value.newer_cursor,
        }
    }
}
impl From<wire::QueueItem> for core::QueuedPrompt {
    fn from(value: wire::QueueItem) -> Self {
        Self {
            id: value.id,
            text: value.text,
        }
    }
}

impl From<wire::Event> for core::AgentEvent {
    fn from(value: wire::Event) -> Self {
        match value {
            wire::Event::SessionExited(v) => Self::SessionExited {
                session_id: v.session_id,
            },
            wire::Event::QueueChanged(v) => Self::QueueChanged {
                session_id: v.session_id,
            },
            wire::Event::TurnStarted(v) => Self::TurnStarted {
                session_id: v.session_id,
                turn_id: v.turn_id,
            },
            wire::Event::AgentMessageDelta(v) => Self::AgentMessageDelta {
                session_id: v.session_id,
                turn_id: v.turn_id,
                item_id: v.item_id,
                delta: v.delta,
            },
            wire::Event::ItemStarted(v) => Self::ItemStarted {
                session_id: v.session_id,
                turn_id: v.turn_id,
                item_id: v.item_id,
                kind: v.kind,
                label: v.label,
            },
            wire::Event::ItemCompleted(v) => Self::ItemCompleted {
                session_id: v.session_id,
                turn_id: v.turn_id,
                item: v.item.into(),
            },
            wire::Event::TurnCompleted(v) => Self::TurnCompleted {
                session_id: v.session_id,
                turn_id: v.turn_id,
                status: v.status.into(),
                error: v.error,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_snapshot_round_trip_and_domain_conversion() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../plugins/agentix-bridge/protocol/snapshot.json"
        ))
        .unwrap();
        let snapshot: wire::Snapshot = serde_json::from_value(fixture.clone()).unwrap();
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), fixture);
        let summary: core::SessionSummary = snapshot.session.into();
        assert_eq!(summary.updated_at, Some(1_788_880_000));
        assert_eq!(summary.id.as_str(), "native-id");
        let turn: core::TurnSummary = snapshot.turns.into_iter().next().unwrap().into();
        assert_eq!(turn.items[0].text.as_deref(), Some("ok"));
        assert_eq!(turn.status, core::TurnStatus::Completed);
    }
}
