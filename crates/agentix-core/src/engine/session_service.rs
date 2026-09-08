//! Session discovery, metadata and durable binding transitions.
use super::EngineError;
use crate::{
    AgentAdapter, AgentError, AttachOutcome, BindingTable, ConversationRef, DeliveryClass,
    EventImportance, HistoryPage, SessionId, SessionSummary, SqliteState,
};
use std::collections::{HashMap, HashSet};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Default)]
pub(super) struct HistoryCursors {
    pub(super) older: Option<String>,
    pub(super) newer: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RestoredBindingStatus {
    Attached,
    Offline,
    Detached,
}

pub(super) struct SessionService {
    state: SqliteState,
    transitions: Mutex<()>,
    pub(super) bindings: Mutex<BindingTable>,
    pub(super) cache: Mutex<HashMap<SessionId, SessionSummary>>,
    pub(super) history_cursors: Mutex<HashMap<ConversationRef, HistoryCursors>>,
}

impl SessionService {
    pub(super) fn new(state: SqliteState) -> Self {
        Self {
            state,
            transitions: Mutex::new(()),
            bindings: Mutex::new(BindingTable::default()),
            cache: Mutex::new(HashMap::new()),
            history_cursors: Mutex::new(HashMap::new()),
        }
    }
}

impl SessionService {
    pub(super) async fn current(&self, conversation: &ConversationRef) -> Option<SessionId> {
        self.bindings
            .lock()
            .await
            .current_session(conversation)
            .cloned()
    }

    pub(super) async fn bound_conversation(&self, session: &SessionId) -> Option<ConversationRef> {
        self.bindings
            .lock()
            .await
            .bound_conversation(session)
            .cloned()
    }

    pub(super) async fn attach_at_epoch(
        &self,
        conversation: ConversationRef,
        session: SessionId,
        previous_session_active: bool,
        epoch: u64,
    ) -> AttachOutcome {
        self.bindings.lock().await.attach_at_epoch(
            conversation,
            session,
            previous_session_active,
            epoch,
        )
    }

    pub(super) async fn detach(
        &self,
        conversation: &ConversationRef,
        keep_draining: bool,
    ) -> Option<SessionId> {
        self.bindings
            .lock()
            .await
            .detach(conversation, keep_draining)
    }

    pub(super) async fn epoch(&self, conversation: &ConversationRef) -> u64 {
        self.bindings.lock().await.epoch(conversation)
    }

    pub(super) async fn route(
        &self,
        session: &SessionId,
        importance: EventImportance,
    ) -> Option<(ConversationRef, DeliveryClass)> {
        self.bindings.lock().await.route(session, importance)
    }

    pub(super) async fn finish_draining(&self, session: &SessionId) {
        self.bindings.lock().await.finish_draining(session);
    }
}

pub(super) struct BindingTransition {
    pub(super) outcome: AttachOutcome,
    pub(super) persisted_previous: Option<SessionId>,
}

impl SessionService {
    /// Serializes only local state changes; subscription and IM I/O run outside this gate.
    pub(super) async fn commit_binding(
        &self,
        conversation: &ConversationRef,
        session: &SessionId,
        previous_active: bool,
    ) -> Result<BindingTransition, EngineError> {
        let _guard = self.transitions.lock().await;
        let persistent = self.state.attach(conversation, session).await?;
        let outcome = self
            .attach_at_epoch(
                conversation.clone(),
                session.clone(),
                previous_active,
                persistent.epoch,
            )
            .await;
        debug_assert_eq!(persistent.epoch, outcome.epoch);
        Ok(BindingTransition {
            outcome,
            persisted_previous: persistent.previous_session,
        })
    }

    pub(super) async fn commit_detach(
        &self,
        conversation: &ConversationRef,
        active: bool,
    ) -> Result<SessionId, EngineError> {
        let _guard = self.transitions.lock().await;
        self.state.detach(conversation).await?;
        self.detach(conversation, active)
            .await
            .ok_or(EngineError::NoCurrentSession)
    }

    pub(super) async fn restore_binding(
        &self,
        agent: &dyn AgentAdapter,
        conversation: &ConversationRef,
        session: &SessionId,
    ) -> Result<RestoredBindingStatus, EngineError> {
        let status = match agent.attach(session).await {
            Ok(()) => RestoredBindingStatus::Attached,
            Err(AgentError::Unavailable(reason)) => {
                tracing::warn!(%reason, ?conversation, %session, "saved agent session is offline; retaining binding until reconnect");
                RestoredBindingStatus::Offline
            }
            Err(AgentError::Rejected(reason)) => {
                tracing::warn!(%reason, ?conversation, %session, "saved agent session is no longer attachable");
                self.state.detach(conversation).await?;
                return Ok(RestoredBindingStatus::Detached);
            }
            Err(error) => return Err(error.into()),
        };
        let epoch = self.state.binding_epoch(conversation).await?;
        self.attach_at_epoch(conversation.clone(), session.clone(), false, epoch)
            .await;
        Ok(status)
    }

    pub(super) async fn filtered_sessions(
        &self,
        agent: &dyn AgentAdapter,
        kind: Option<crate::AgentKind>,
    ) -> Result<Vec<SessionSummary>, EngineError> {
        let mut result = Vec::new();
        let mut cursor = None;
        let mut seen = HashSet::new();
        loop {
            let page = agent.list_sessions(cursor, 25).await?;
            result.extend(page.sessions.into_iter().filter(|session| {
                kind.is_none_or(|kind| {
                    crate::SessionKey::decode(&session.id).is_some_and(|key| key.agent == kind)
                })
            }));
            if result.len() >= 25 {
                result.truncate(25);
                break;
            }
            let Some(next) = page.next_cursor else {
                break;
            };
            if !seen.insert(next.clone()) {
                break;
            }
            cursor = Some(next);
        }
        Ok(result)
    }

    pub(super) async fn remember_history_cursors(
        &self,
        conversation: &ConversationRef,
        history: &HistoryPage,
    ) {
        self.history_cursors.lock().await.insert(
            conversation.clone(),
            HistoryCursors {
                older: history.older_cursor.clone(),
                newer: history.newer_cursor.clone(),
            },
        );
    }

    pub(super) async fn cache_session_summary(
        &self,
        agent: &dyn AgentAdapter,
        session_id: &SessionId,
    ) {
        if self.cache.lock().await.contains_key(session_id) {
            return;
        }
        match agent.list_sessions(None, 100).await {
            Ok(page) => {
                let mut sessions = self.cache.lock().await;
                sessions.extend(
                    page.sessions
                        .into_iter()
                        .map(|session| (session.id.clone(), session)),
                );
            }
            Err(error) => {
                tracing::debug!(%error, session = %session_id, "failed to load session title");
            }
        }
    }
}
