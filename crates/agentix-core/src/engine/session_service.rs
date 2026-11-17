//! Session discovery, metadata and durable binding transitions.
use super::EngineError;
use crate::{
    AgentAdapter, AgentError, AttachOutcome, BindingTable, ConversationRef, DeliveryClass,
    EventImportance, HistoryPage, SessionId, SessionSummary, SqliteState,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    sync::Arc,
    task::Poll,
};
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
    pub(super) cleanup: super::subscription_cleanup::SubscriptionCleanup,
    transitions: Mutex<()>,
    pub(super) bindings: Mutex<BindingTable>,
    pub(super) cache: Arc<Mutex<HashMap<SessionId, SessionSummary>>>,
    title_read: Mutex<Option<TitleRead>>,
    title_retry_after: Mutex<HashMap<SessionId, tokio::time::Instant>>,
    title_queue: Arc<Mutex<(VecDeque<SessionId>, bool)>>,
    title_pending: Arc<Mutex<HashSet<SessionId>>>,
    pub(super) history_cursors: Mutex<HashMap<ConversationRef, HistoryCursors>>,
}

struct TitleRead(tokio::task::JoinHandle<()>);

impl Drop for TitleRead {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl SessionService {
    pub(super) fn new(state: SqliteState) -> Self {
        Self {
            state,
            cleanup: super::subscription_cleanup::SubscriptionCleanup::default(),
            transitions: Mutex::new(()),
            bindings: Mutex::new(BindingTable::default()),
            cache: Arc::new(Mutex::new(HashMap::new())),
            title_read: Mutex::new(None),
            title_retry_after: Mutex::new(HashMap::new()),
            title_queue: Arc::new(Mutex::new((VecDeque::new(), false))),
            title_pending: Arc::new(Mutex::new(HashSet::new())),
            history_cursors: Mutex::new(HashMap::new()),
        }
    }
}

impl SessionService {
    /// Give a first background card a bounded chance to use uncached metadata.
    /// Timing out preserves the shared reader for subsequent notifications.
    pub(super) async fn await_background_title(&self, session_id: &SessionId) {
        let _ = tokio::time::timeout(std::time::Duration::from_millis(50), async {
            if self.has_title(session_id).await {
                return;
            }
            let mut reader = self.title_read.lock().await;
            if let Some(read) = reader.as_mut() {
                let _ = (&mut read.0).await;
                reader.take();
            }
        })
        .await;
    }

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
        if let Some(switch) = self.state.session_switch(conversation).await?
            && switch.target.is_none()
            && switch.paused.is_none()
        {
            self.attach_at_epoch(conversation.clone(), session.clone(), false, switch.epoch)
                .await;
            return Ok(RestoredBindingStatus::Offline);
        }
        if let Some(pending) = self.cleanup.pending(session) {
            super::subscription_cleanup::wait(pending).await;
        }
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

    async fn has_title(&self, session_id: &SessionId) -> bool {
        self.cache
            .lock()
            .await
            .get(session_id)
            .is_some_and(|summary| super::presentation::session_title(summary).is_some())
    }

    /// Bound retries triggered by streaming output; explicit attachment and
    /// resumption may still request metadata immediately after clearing the cache.
    pub(super) async fn refresh_session_title(
        &self,
        agent: Arc<dyn AgentAdapter>,
        session_id: &SessionId,
    ) {
        if self.has_title(session_id).await {
            return;
        }
        let now = tokio::time::Instant::now();
        let mut retry_after = self.title_retry_after.lock().await;
        if retry_after
            .get(session_id)
            .is_some_and(|deadline| *deadline > now)
        {
            return;
        }
        retry_after.retain(|_, deadline| *deadline > now);
        if retry_after.len() >= 1024 {
            return;
        }
        retry_after.insert(session_id.clone(), now + std::time::Duration::from_secs(5));
        drop(retry_after);
        self.cache_session_summary(agent, session_id).await;
    }

    /// Poll cached metadata once, then let one owned reader finish off the
    /// foreground path. All callers share one metadata reader and cache.
    pub(super) async fn cache_session_summary(
        &self,
        agent: Arc<dyn AgentAdapter>,
        session_id: &SessionId,
    ) {
        if self.has_title(session_id).await {
            return;
        }
        let mut reader = self.title_read.lock().await;
        if self.has_title(session_id).await {
            return;
        }
        let mut pending = self.title_pending.lock().await;
        if pending.len() >= 1024 || !pending.insert(session_id.clone()) {
            return;
        }
        let mut scheduling = self.title_queue.lock().await;
        scheduling.0.push_back(session_id.clone());
        drop(pending);
        if scheduling.1 {
            return;
        }
        scheduling.1 = true;
        drop(scheduling);
        let cache = self.cache.clone();
        let queue = self.title_queue.clone();
        let pending = self.title_pending.clone();
        let mut fetch =
            Box::pin(async move {
                loop {
                    let session = {
                        let mut scheduling = queue.lock().await;
                        if let Some(session) = scheduling.0.pop_front() {
                            session
                        } else {
                            scheduling.1 = false;
                            break;
                        }
                    };
                    if cache.lock().await.get(&session).is_some_and(|summary| {
                        super::presentation::session_title(summary).is_some()
                    }) {
                        pending.lock().await.remove(&session);
                        continue;
                    }
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(1),
                        agent.session_metadata(&session),
                    )
                    .await
                    {
                        Ok(Ok(summaries)) => {
                            let mut sessions = cache.lock().await;
                            for summary in summaries {
                                // Fill missing titles without overwriting newer named metadata
                                // or the live status of a session already in the cache.
                                sessions
                                    .entry(summary.id.clone())
                                    .and_modify(|cached| {
                                        if super::presentation::session_title(cached).is_none() {
                                            cached.name.clone_from(&summary.name);
                                            cached.preview.clone_from(&summary.preview);
                                        }
                                    })
                                    .or_insert(summary);
                            }
                        }
                        Ok(Err(error)) => {
                            tracing::debug!(%error, %session, "failed to load session title");
                        }
                        Err(_) => {
                            tracing::debug!(%session, "session title lookup exceeded its deadline");
                        }
                    }
                    pending.lock().await.remove(&session);
                }
            });
        let ready =
            std::future::poll_fn(|cx| Poll::Ready(fetch.as_mut().poll(cx).is_ready())).await;
        if !ready {
            *reader = Some(TitleRead(tokio::spawn(fetch)));
        }
    }
}
