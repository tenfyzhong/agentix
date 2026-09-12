//! Local routing reservations for the bounded Engine runtime.
//!
//! Admission never calls an agent or channel. Pending work is resolved again
//! after completions, so a queued attachment cannot leave stale session keys.
use std::collections::HashMap;

use super::{Engine, EngineError, MultiplexerUiAction, UiAction};
use crate::{
    AgentCommand, AgentEvent, AgentKind, BindingTable, ConversationRef, DispatchScope,
    EventImportance, InboundEnvelope, InboundPayload, ParsedInput, SessionCommand, SessionId,
    SessionKey, parse_input,
};

#[derive(Debug)]
pub enum EngineWork {
    Inbound(InboundEnvelope),
    Event(AgentEvent),
    Working { session: SessionId, turn: String },
    Recover,
    TaskBoard,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EngineResource {
    Conversation(ConversationRef),
    Session(SessionId),
    Backend(Option<AgentKind>),
    TaskBoard,
}

#[derive(Clone)]
enum WorkspaceMutation {
    Backend(AgentKind),
    AllBackends,
}

#[derive(Clone, Default)]
struct ActionRoute {
    session: Option<SessionId>,
    replace_session: bool,
    workspace_mutation: Option<WorkspaceMutation>,
}

impl From<&UiAction> for ActionRoute {
    fn from(action: &UiAction) -> Self {
        let mut route = Self::default();
        match action {
            UiAction::Attach(session)
            | UiAction::Stop {
                session_id: session,
                ..
            }
            | UiAction::QueueControl {
                session_id: session,
                ..
            } => {
                route.session = Some(session.clone());
            }
            UiAction::SessionCommand {
                session_id,
                command,
            } => {
                route.session = Some(session_id.clone());
                route.replace_session = replaces_session(command);
            }
            UiAction::Resolve { interaction, .. }
            | UiAction::BeginInput(interaction)
            | UiAction::SelectInput { interaction, .. }
            | UiAction::BeginCustomInput(interaction) => {
                route.session = Some(interaction.session_id.clone());
            }
            UiAction::Task(action) => route.session = Some(action.session_id.clone()),
            UiAction::Multiplexer(
                backend,
                MultiplexerUiAction::Directory {
                    action: super::DirectoryAction::Confirm,
                    ..
                },
            )
            | UiAction::MultiplexerLaunch(backend, _) => {
                route.workspace_mutation = Some(
                    backend.map_or(WorkspaceMutation::AllBackends, WorkspaceMutation::Backend),
                );
            }
            UiAction::Multiplexer(..) | UiAction::TaskBrowse(_) => {}
        }
        route
    }
}

fn replaces_session(command: &SessionCommand) -> bool {
    matches!(command, SessionCommand::Fork | SessionCommand::Clear(_))
}

/// A short-lived local view. Do not cache across worker completions.
pub struct EngineDispatchSnapshot {
    bindings: BindingTable,
    backends: Vec<AgentKind>,
    actions: HashMap<String, ActionRoute>,
    replies: HashMap<ConversationRef, SessionId>,
}

impl EngineDispatchSnapshot {
    #[must_use]
    pub fn scope(&self, work: &EngineWork) -> DispatchScope<EngineResource> {
        let mut shared = Vec::new();
        let mut exclusive = Vec::new();
        match work {
            EngineWork::Recover => return DispatchScope::Global,
            EngineWork::TaskBoard => exclusive.push(EngineResource::TaskBoard),
            EngineWork::Event(event) => {
                let Some(session) = event.session_id() else {
                    return DispatchScope::Global;
                };
                self.reserve_session(&SessionId::new(session), &mut shared, &mut exclusive);
            }
            EngineWork::Working { session, .. } => {
                self.reserve_session(session, &mut shared, &mut exclusive);
            }
            EngineWork::Inbound(envelope) => {
                exclusive.push(EngineResource::Conversation(envelope.conversation.clone()));
                let current = self.bindings.current_session(&envelope.conversation);
                if let Some(session) = current {
                    self.reserve_session(session, &mut shared, &mut exclusive);
                }
                // A reply may target a draining interaction after the user
                // switches sessions. Reserve that target as well as the binding.
                if let Some(session) = self.replies.get(&envelope.conversation) {
                    self.reserve_session(session, &mut shared, &mut exclusive);
                }
                let route = match &envelope.payload {
                    InboundPayload::Text(text) => match parse_input(text) {
                        Ok(ParsedInput::Command(AgentCommand::Attach(session))) => ActionRoute {
                            session: Some(SessionId::new(session)),
                            ..ActionRoute::default()
                        },
                        Ok(ParsedInput::Command(AgentCommand::Session(command))) => ActionRoute {
                            session: current.cloned(),
                            replace_session: replaces_session(&command),
                            ..ActionRoute::default()
                        },
                        _ => ActionRoute::default(),
                    },
                    InboundPayload::Action { token, .. } => {
                        // Tokens issued after this snapshot require a new view.
                        // A global reservation is conservative for that rare
                        // race; execution still performs full token validation.
                        let Some(route) = self.actions.get(token) else {
                            return DispatchScope::Global;
                        };
                        route.clone()
                    }
                    InboundPayload::TextEdited { .. } => ActionRoute::default(),
                };
                if let Some(session) = &route.session {
                    self.reserve_session(session, &mut shared, &mut exclusive);
                }
                if route.replace_session {
                    // The replacement ID is returned by the host. Fence its
                    // backend until binding commits, including early events for
                    // IDs which admission could not know in advance.
                    self.reserve_backend(route.session.as_ref(), &mut exclusive);
                }
                if let Some(backend) = route.workspace_mutation {
                    match backend {
                        WorkspaceMutation::Backend(kind) => {
                            exclusive.push(EngineResource::Backend(Some(kind)));
                        }
                        WorkspaceMutation::AllBackends => {
                            self.reserve_backend(None, &mut exclusive);
                        }
                    }
                }
            }
        }
        DispatchScope::Access { shared, exclusive }
    }

    fn reserve_backend(&self, session: Option<&SessionId>, resources: &mut Vec<EngineResource>) {
        if let Some(key) = session.and_then(SessionKey::decode) {
            resources.push(EngineResource::Backend(Some(key.agent)));
        } else if self.backends.is_empty() {
            resources.push(EngineResource::Backend(None));
        } else {
            resources.extend(
                self.backends
                    .iter()
                    .map(|kind| EngineResource::Backend(Some(*kind))),
            );
        }
    }

    fn reserve_session(
        &self,
        session: &SessionId,
        shared: &mut Vec<EngineResource>,
        exclusive: &mut Vec<EngineResource>,
    ) {
        self.reserve_backend(Some(session), shared);
        let aliases = if let Some(key) = SessionKey::decode(session) {
            vec![session.clone(), key.encode()]
        } else {
            std::iter::once(session.clone())
                .chain(
                    self.backends
                        .iter()
                        .map(|kind| SessionKey::new(*kind, session.clone()).encode()),
                )
                .collect()
        };
        for alias in aliases {
            if let Some((conversation, _)) = self.bindings.route(&alias, EventImportance::Critical)
            {
                exclusive.push(EngineResource::Conversation(conversation));
            }
            exclusive.push(EngineResource::Session(alias));
        }
    }
}

impl Engine {
    pub async fn fence_inbound(
        &self,
        channel: crate::ChannelKind,
        event_id: &str,
    ) -> Result<(), EngineError> {
        self.state.fence_event(channel, event_id).await?;
        Ok(())
    }

    pub async fn dispatch_snapshot(&self) -> EngineDispatchSnapshot {
        let bindings = self.sessions.bindings.lock().await.clone();
        let actions = self
            .interactions
            .actions
            .lock()
            .await
            .iter()
            .map(|(token, action)| (token.to_owned(), ActionRoute::from(action)))
            .collect();
        let replies = self
            .interactions
            .reply_modes
            .lock()
            .await
            .iter()
            .map(|(conversation, interaction)| {
                (conversation.clone(), interaction.session_id.clone())
            })
            .collect();
        EngineDispatchSnapshot {
            bindings,
            backends: self.agent.session_backends(),
            actions,
            replies,
        }
    }

    pub async fn observe_delivery_state(&self) -> Result<(), EngineError> {
        let uncertain = self.state.uncertain_event_count().await?;
        let rejected = self.state.rejected_event_count().await?;
        tracing::info!(target: "agentix::telemetry", uncertain_inputs = uncertain, rejected_inputs = rejected, "delivery state");
        Ok(())
    }

    pub async fn execute_work(&self, work: EngineWork) -> Result<(), EngineError> {
        match work {
            EngineWork::Inbound(envelope) => self.handle_inbound(envelope).await,
            EngineWork::Event(event) => self.handle_agent_event(event).await,
            EngineWork::Working { session, turn } => {
                self.refresh_working_turn(&session, &turn).await;
                Ok(())
            }
            EngineWork::Recover => self.recover_event_gap().await,
            EngineWork::TaskBoard => self.refresh_task_board().await,
        }
    }

    pub async fn working_turns(&self) -> Vec<(SessionId, String)> {
        self.turns
            .active
            .lock()
            .await
            .iter()
            .map(|(session, turn)| (session.clone(), turn.clone()))
            .collect()
    }

    pub async fn poll_inbox_sources(&self) -> Result<Vec<InboundEnvelope>, EngineError> {
        self.tasks.view(self).poll_inbox_sources().await
    }
}
