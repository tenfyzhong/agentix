//! Retry an unavailable backend without holding up other configured runtimes.
use crate::{
    AgentAdapter, AgentError, AgentEvent, HistoryPage, InteractionDecision, MultiplexerMutation,
    MultiplexerMutationResult, MultiplexerSnapshot, QueuedPrompt, QueuedPromptPort, SessionCommand,
    SessionCommandResult, SessionControlPort, SessionId, SessionPage, WorkspaceRuntimePort,
};
use async_trait::async_trait;
use std::future::Future;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::broadcast;

pub struct DeferredAgent {
    name: &'static str,
    directory: String,
    connected: Arc<RwLock<Option<Arc<dyn AgentAdapter>>>>,
    events: broadcast::Sender<AgentEvent>,
    task: tokio::task::JoinHandle<()>,
}
impl DeferredAgent {
    pub fn new<F, Fut>(name: &'static str, directory: String, factory: F) -> Self
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: Future<Output = Result<Arc<dyn AgentAdapter>, AgentError>> + Send,
    {
        let connected = Arc::new(RwLock::new(None));
        let (events, _) = broadcast::channel(1024);
        let target = connected.clone();
        let output = events.clone();
        let task = tokio::spawn(async move {
            loop {
                match factory().await {
                    Ok(agent) => {
                        let mut source = agent.subscribe();
                        *target.write().expect("backend lock") = Some(agent);
                        let _ = output.send(AgentEvent::Connected { generation: 1 });
                        loop {
                            match source.recv().await {
                                Ok(event) => {
                                    let _ = output.send(event);
                                }
                                Err(broadcast::error::RecvError::Lagged(_)) => {
                                    let _ = output.send(AgentEvent::Disconnected {
                                        generation: 1,
                                        reason: "backend event stream lagged".into(),
                                    });
                                }
                                Err(broadcast::error::RecvError::Closed) => break,
                            }
                        }
                        *target.write().expect("backend lock") = None;
                    }
                    Err(error) => {
                        tracing::warn!(%error, backend = name, "backend unavailable; retrying");
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
        Self {
            name,
            directory,
            connected,
            events,
            task,
        }
    }
    fn agent(&self) -> Result<Arc<dyn AgentAdapter>, AgentError> {
        self.connected
            .read()
            .expect("backend lock")
            .clone()
            .ok_or_else(|| AgentError::Unavailable(format!("{} is reconnecting", self.name)))
    }
}
impl Drop for DeferredAgent {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn unsupported() -> AgentError {
    AgentError::Rejected("backend capability is unavailable".into())
}
#[async_trait]
impl AgentAdapter for DeferredAgent {
    fn display_name(&self) -> &'static str {
        self.name
    }
    fn generation(&self) -> u64 {
        1
    }
    fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }
    fn queued_prompts(&self) -> Option<&dyn QueuedPromptPort> {
        Some(self)
    }
    fn session_control(&self) -> Option<&dyn SessionControlPort> {
        Some(self)
    }
    fn workspace_runtime(&self) -> Option<&dyn WorkspaceRuntimePort> {
        Some(self)
    }
    async fn session_capabilities(&self, session: &SessionId) -> crate::SessionCapabilities {
        match self.agent() {
            Ok(agent) => agent.session_capabilities(session).await,
            Err(_) => crate::SessionCapabilities::default(),
        }
    }
    async fn session_access(&self, session: &SessionId) -> crate::SessionAccess {
        match self.agent() {
            Ok(agent) => agent.session_access(session).await,
            Err(_) => crate::SessionAccess::Offline,
        }
    }
    async fn is_subagent(&self, session: &SessionId) -> Result<bool, AgentError> {
        self.agent()?.is_subagent(session).await
    }
    async fn refresh(&self) -> Result<(), AgentError> {
        self.agent()?.refresh().await
    }
    async fn is_read_only(&self, session: &SessionId) -> bool {
        match self.agent() {
            Ok(agent) => agent.is_read_only(session).await,
            Err(_) => true,
        }
    }
    async fn list_sessions(
        &self,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<SessionPage, AgentError> {
        self.agent()?.list_sessions(cursor, limit).await
    }
    async fn read_history(
        &self,
        session: &SessionId,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<HistoryPage, AgentError> {
        self.agent()?.read_history(session, cursor, limit).await
    }
    async fn attach(&self, session: &SessionId) -> Result<(), AgentError> {
        self.agent()?.attach(session).await
    }
    async fn unsubscribe(&self, session: &SessionId) -> Result<(), AgentError> {
        self.agent()?.unsubscribe(session).await
    }
    async fn start_turn(&self, session: &SessionId, text: &str) -> Result<String, AgentError> {
        self.agent()?.start_turn(session, text).await
    }
    async fn steer(
        &self,
        session: &SessionId,
        turn: &str,
        text: &str,
    ) -> Result<String, AgentError> {
        self.agent()?.steer(session, turn, text).await
    }
    async fn interrupt(&self, session: &SessionId, turn: &str) -> Result<(), AgentError> {
        self.agent()?.interrupt(session, turn).await
    }
    async fn resolve_interaction(&self, decision: InteractionDecision) -> Result<(), AgentError> {
        self.agent()?.resolve_interaction(decision).await
    }
}
#[async_trait]
impl QueuedPromptPort for DeferredAgent {
    async fn queue_prompt(
        &self,
        session: &SessionId,
        text: &str,
        id: &str,
    ) -> Result<QueuedPrompt, AgentError> {
        self.agent()?
            .queued_prompts()
            .ok_or_else(unsupported)?
            .queue_prompt(session, text, id)
            .await
    }
    async fn list_queued_prompts(
        &self,
        session: &SessionId,
    ) -> Result<Vec<QueuedPrompt>, AgentError> {
        self.agent()?
            .queued_prompts()
            .ok_or_else(unsupported)?
            .list_queued_prompts(session)
            .await
    }
    async fn queue_status(&self, session: &SessionId) -> Result<Option<String>, AgentError> {
        self.agent()?
            .queued_prompts()
            .ok_or_else(unsupported)?
            .queue_status(session)
            .await
    }
    async fn control_queue(&self, session: &SessionId, action: &str) -> Result<(), AgentError> {
        self.agent()?
            .queued_prompts()
            .ok_or_else(unsupported)?
            .control_queue(session, action)
            .await
    }
}
#[async_trait]
impl SessionControlPort for DeferredAgent {
    async fn run_session_command(
        &self,
        session: &SessionId,
        command: SessionCommand,
    ) -> Result<SessionCommandResult, AgentError> {
        self.agent()?
            .session_control()
            .ok_or_else(unsupported)?
            .run_session_command(session, command)
            .await
    }
}
#[async_trait]
impl WorkspaceRuntimePort for DeferredAgent {
    fn default_directory(&self) -> String {
        self.directory.clone()
    }
    async fn snapshot(&self) -> Result<Option<MultiplexerSnapshot>, AgentError> {
        self.agent()?
            .workspace_runtime()
            .ok_or_else(unsupported)?
            .snapshot()
            .await
    }
    async fn mutate(
        &self,
        mutation: MultiplexerMutation,
    ) -> Result<MultiplexerMutationResult, AgentError> {
        self.agent()?
            .workspace_runtime()
            .ok_or_else(unsupported)?
            .mutate(mutation)
            .await
    }
}
