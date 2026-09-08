use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use super::{InteractionKey, PendingInteractionView, PendingSessionInput, TurnBuffer, UiAction};
use crate::{
    ActionButton, ActionRegistry, AgentAdapter, ConversationRef, MessageRef, SessionId,
    WorkspaceRuntimePort,
};

pub(super) struct BackgroundNotification {
    pub(super) turn_id: String,
    pub(super) recipients: HashSet<ConversationRef>,
}

pub(super) struct TurnCoordinator {
    pub(super) cold: super::cold_turns::ColdTurns,
    pub(super) active: Mutex<HashMap<SessionId, String>>,
    pub(super) buffers: Mutex<HashMap<(SessionId, String), TurnBuffer>>,
    pub(super) views: Mutex<HashMap<(SessionId, String), MessageRef>>,
    pub(super) last_renders: Mutex<HashMap<(SessionId, String), Instant>>,
    pub(super) stop_actions: Mutex<HashMap<(SessionId, String), ActionButton>>,
    pub(super) background_notifications: Mutex<HashMap<SessionId, BackgroundNotification>>,
}

impl Default for TurnCoordinator {
    fn default() -> Self {
        Self {
            cold: super::cold_turns::ColdTurns::default(),
            active: Mutex::new(HashMap::new()),
            buffers: Mutex::new(HashMap::new()),
            views: Mutex::new(HashMap::new()),
            last_renders: Mutex::new(HashMap::new()),
            stop_actions: Mutex::new(HashMap::new()),
            background_notifications: Mutex::new(HashMap::new()),
        }
    }
}

impl TurnCoordinator {
    pub(super) async fn background_notification_delivered(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
        turn: &str,
    ) -> bool {
        self.background_notifications
            .lock()
            .await
            .get(session)
            .is_some_and(|notice| {
                notice.turn_id == turn && notice.recipients.contains(conversation)
            })
    }

    pub(super) async fn record_background_notification(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
        turn: &str,
    ) {
        let mut notifications = self.background_notifications.lock().await;
        let notice =
            notifications
                .entry(session.clone())
                .or_insert_with(|| BackgroundNotification {
                    turn_id: turn.to_owned(),
                    recipients: HashSet::new(),
                });
        if notice.turn_id != turn {
            turn.clone_into(&mut notice.turn_id);
            notice.recipients.clear();
        }
        notice.recipients.insert(conversation.clone());
    }

    pub(super) async fn is_active(&self, session: &SessionId) -> bool {
        self.active.lock().await.contains_key(session)
    }

    pub(super) async fn active_turn(&self, session: &SessionId) -> Option<String> {
        self.active.lock().await.get(session).cloned()
    }

    pub(super) async fn set_active(&self, session: SessionId, turn_id: String) {
        self.active.lock().await.insert(session, turn_id);
    }

    pub(super) async fn remove_active(&self, session: &SessionId) -> Option<String> {
        self.active.lock().await.remove(session)
    }

    pub(super) async fn should_render(
        &self,
        key: &(SessionId, String),
        force: bool,
        interval: Duration,
    ) -> bool {
        let now = Instant::now();
        let mut renders = self.last_renders.lock().await;
        if !force
            && renders
                .get(key)
                .is_some_and(|last| now.duration_since(*last) < interval)
        {
            return false;
        }
        renders.insert(key.clone(), now);
        true
    }

    pub(super) async fn mark_elapsed_rendered(
        &self,
        key: &(SessionId, String),
        elapsed_seconds: Option<u64>,
    ) {
        if let Some(elapsed_seconds) = elapsed_seconds
            && let Some(buffer) = self.buffers.lock().await.get_mut(key)
        {
            buffer.rendered_elapsed_seconds = Some(elapsed_seconds);
        }
    }
}

pub(super) struct InteractionCoordinator {
    pub(super) actions: Mutex<ActionRegistry<UiAction>>,
    pub(super) pending: Mutex<HashMap<InteractionKey, PendingInteractionView>>,
    pub(super) turn_action_groups: Mutex<HashMap<(SessionId, String), String>>,
    pub(super) reply_modes: Mutex<HashMap<ConversationRef, InteractionKey>>,
    pub(super) session_inputs: Mutex<HashMap<ConversationRef, PendingSessionInput>>,
    pub(super) owners: Mutex<HashMap<ConversationRef, String>>,
}

impl Default for InteractionCoordinator {
    fn default() -> Self {
        Self {
            actions: Mutex::new(ActionRegistry::default()),
            pending: Mutex::new(HashMap::new()),
            turn_action_groups: Mutex::new(HashMap::new()),
            reply_modes: Mutex::new(HashMap::new()),
            session_inputs: Mutex::new(HashMap::new()),
            owners: Mutex::new(HashMap::new()),
        }
    }
}

impl InteractionCoordinator {
    /// Return owned state so a caller's `if let` cannot retain the shared mutex
    /// while it awaits an agent command or an IM response.
    pub(super) async fn take_session_input(
        &self,
        conversation: &ConversationRef,
    ) -> Option<PendingSessionInput> {
        self.session_inputs.lock().await.remove(conversation)
    }

    pub(super) async fn take_reply_mode(
        &self,
        conversation: &ConversationRef,
    ) -> Option<InteractionKey> {
        self.reply_modes.lock().await.remove(conversation)
    }
}

#[derive(Debug)]
pub(super) struct RmuxController {
    enabled: bool,
    pub(super) selected: Mutex<HashMap<ConversationRef, crate::AgentKind>>,
}

impl RmuxController {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            selected: Mutex::default(),
        }
    }

    pub(super) async fn runtime<'a>(
        &self,
        agent: &'a dyn AgentAdapter,
        conversation: &ConversationRef,
    ) -> Option<&'a dyn WorkspaceRuntimePort> {
        if self.enabled {
            agent.workspace_for(self.selected.lock().await.get(conversation).copied())
        } else {
            None
        }
    }

    pub(super) async fn default_directory(
        &self,
        agent: &dyn AgentAdapter,
        conversation: &ConversationRef,
    ) -> String {
        self.runtime(agent, conversation)
            .await
            .map_or_else(|| "~".into(), WorkspaceRuntimePort::default_directory)
    }
}

#[cfg(test)]
mod tests {
    use super::TurnCoordinator;
    use crate::{ChannelKind, ConversationRef, SessionId};

    #[tokio::test]
    async fn background_dedup_retains_one_turn_per_session_and_tracks_each_recipient() {
        let turns = TurnCoordinator::default();
        let first = ConversationRef::new(ChannelKind::Telegram, "first");
        let second = ConversationRef::new(ChannelKind::Telegram, "second");
        let session = SessionId::new("session");
        let other = SessionId::new("other");
        turns
            .record_background_notification(&first, &other, "independent")
            .await;
        for index in 0..100 {
            let turn = format!("turn_{index}");
            assert!(
                !turns
                    .background_notification_delivered(&first, &session, &turn)
                    .await
            );
            turns
                .record_background_notification(&first, &session, &turn)
                .await;
            assert!(
                turns
                    .background_notification_delivered(&first, &session, &turn)
                    .await
            );
            assert!(
                !turns
                    .background_notification_delivered(&second, &session, &turn)
                    .await
            );
            turns
                .record_background_notification(&second, &session, &turn)
                .await;
            assert!(
                turns
                    .background_notification_delivered(&first, &session, &turn)
                    .await
            );
            assert!(
                turns
                    .background_notification_delivered(&second, &session, &turn)
                    .await
            );
            assert_eq!(turns.background_notifications.lock().await.len(), 2);
        }
        assert!(
            !turns
                .background_notification_delivered(&first, &session, "turn_0")
                .await
        );
        assert!(
            turns
                .background_notification_delivered(&first, &other, "independent")
                .await
        );
    }
}
