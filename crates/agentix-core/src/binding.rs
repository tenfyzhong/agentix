use std::collections::HashMap;

use crate::{ConversationRef, SessionId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventImportance {
    Stream,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryClass {
    Live,
    Draining,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachOutcome {
    pub previous_session: Option<SessionId>,
    pub displaced_conversation: Option<ConversationRef>,
    pub epoch: u64,
}

/// Maintains the one-current-session-per-conversation and
/// one-conversation-per-session invariants.
#[derive(Debug, Default)]
pub struct BindingTable {
    by_conversation: HashMap<ConversationRef, SessionId>,
    by_session: HashMap<SessionId, ConversationRef>,
    draining: HashMap<SessionId, ConversationRef>,
    epochs: HashMap<ConversationRef, u64>,
}

impl BindingTable {
    pub fn attach(
        &mut self,
        conversation: ConversationRef,
        session: SessionId,
        previous_session_active: bool,
    ) -> AttachOutcome {
        if self.by_conversation.get(&conversation) == Some(&session) {
            return AttachOutcome {
                previous_session: None,
                displaced_conversation: None,
                epoch: self.epoch(&conversation),
            };
        }

        let previous_session = self.by_conversation.remove(&conversation);
        if let Some(previous) = &previous_session {
            self.by_session.remove(previous);
            if previous_session_active {
                self.draining.insert(previous.clone(), conversation.clone());
            } else {
                self.draining.remove(previous);
            }
        }

        let displaced_conversation = self.by_session.remove(&session);
        if let Some(displaced) = &displaced_conversation {
            self.by_conversation.remove(displaced);
        }

        self.draining.remove(&session);
        self.by_conversation
            .insert(conversation.clone(), session.clone());
        self.by_session.insert(session, conversation.clone());
        let epoch = self.epochs.entry(conversation).or_default();
        *epoch = epoch.saturating_add(1);

        AttachOutcome {
            previous_session,
            displaced_conversation,
            epoch: *epoch,
        }
    }

    pub fn attach_at_epoch(
        &mut self,
        conversation: ConversationRef,
        session: SessionId,
        previous_session_active: bool,
        epoch: u64,
    ) -> AttachOutcome {
        let mut outcome = self.attach(conversation.clone(), session, previous_session_active);
        self.epochs.insert(conversation, epoch);
        outcome.epoch = epoch;
        outcome
    }

    #[must_use]
    pub fn current_session(&self, conversation: &ConversationRef) -> Option<&SessionId> {
        self.by_conversation.get(conversation)
    }

    #[must_use]
    pub fn bound_conversation(&self, session: &SessionId) -> Option<&ConversationRef> {
        self.by_session.get(session)
    }

    #[must_use]
    pub fn epoch(&self, conversation: &ConversationRef) -> u64 {
        self.epochs.get(conversation).copied().unwrap_or_default()
    }

    #[must_use]
    pub fn route(
        &self,
        session: &SessionId,
        importance: EventImportance,
    ) -> Option<(ConversationRef, DeliveryClass)> {
        if let Some(conversation) = self.by_session.get(session) {
            return Some((conversation.clone(), DeliveryClass::Live));
        }
        if importance == EventImportance::Critical {
            return self
                .draining
                .get(session)
                .cloned()
                .map(|conversation| (conversation, DeliveryClass::Draining));
        }
        None
    }

    pub fn finish_draining(&mut self, session: &SessionId) {
        self.draining.remove(session);
    }

    pub fn detach(
        &mut self,
        conversation: &ConversationRef,
        keep_draining: bool,
    ) -> Option<SessionId> {
        let session = self.by_conversation.remove(conversation)?;
        self.by_session.remove(&session);
        if keep_draining {
            self.draining.insert(session.clone(), conversation.clone());
        }
        let epoch = self.epochs.entry(conversation.clone()).or_default();
        *epoch = epoch.saturating_add(1);
        Some(session)
    }
}
