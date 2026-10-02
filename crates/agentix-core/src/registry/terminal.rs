//! One observer and reply lifecycle for every native backend.
use super::{
    AgentAdapter, AgentError, AgentEvent, AgentKind, Arc, BTreeMap, HashMap, HashSet, Mutex,
    Routes, SessionId, SessionRef, Value, broadcast, json,
};
use crate::{
    InteractionKind, InteractionRequest, TerminalInteraction, TerminalInteractionKind,
    TerminalInteractionPort, TerminalInteractionResponse, TerminalInteractionTarget,
};
use futures_util::{StreamExt, stream::FuturesUnordered};
use std::time::Duration;

struct Seen {
    target: TerminalInteractionTarget,
    prompt: TerminalInteraction,
    observations: usize,
    token: Option<String>,
    answered: bool,
}
struct Pending {
    session: SessionRef,
    target: TerminalInteractionTarget,
    prompt: TerminalInteraction,
    switching: bool,
}
#[derive(Default)]
struct State {
    observed: HashSet<SessionRef>,
    seen: HashMap<SessionRef, Seen>,
    pending: HashMap<String, Pending>,
    switches: HashMap<SessionRef, String>,
}
#[derive(Default)]
pub(super) struct TerminalObserver {
    state: Mutex<State>,
}
impl TerminalObserver {
    pub(super) fn begin_switch(&self, session: SessionRef, client_id: String) {
        self.state
            .lock()
            .unwrap()
            .switches
            .insert(session, client_id);
    }
    pub(super) fn fail_switch(&self, session: &SessionRef) {
        self.state.lock().unwrap().switches.remove(session);
    }
    pub(super) fn observe(&self, session: SessionRef) {
        self.state.lock().unwrap().observed.insert(session);
    }
    pub(super) fn unobserve(&self, session: &SessionRef, events: &broadcast::Sender<AgentEvent>) {
        let mut state = self.state.lock().unwrap();
        state.observed.remove(session);
        state.switches.remove(session);
        Self::clear(&mut state, session, events);
    }
    fn clear(state: &mut State, session: &SessionRef, events: &broadcast::Sender<AgentEvent>) {
        if let Some(seen) = state.seen.remove(session)
            && let Some(token) = seen.token
        {
            state.pending.remove(&token);
            let _ = events.send(AgentEvent::InteractionResolved {
                session_id: session.encode().to_string(),
                request_id: token,
            });
        }
    }
    pub(super) fn backend_event(
        &self,
        kind: AgentKind,
        event: &AgentEvent,
        events: &broadcast::Sender<AgentEvent>,
    ) {
        let Some(id) = event.session_id() else {
            return;
        };
        let session = SessionRef::new(kind, SessionId::new(id));
        let mut state = self.state.lock().unwrap();
        match event {
            AgentEvent::SessionSwitchStarted { client_id, .. } => {
                state.switches.insert(session, client_id.clone());
            }
            AgentEvent::SessionSwitchFailed { .. } | AgentEvent::SessionExited { .. } => {
                state.switches.remove(&session);
                Self::clear(&mut state, &session, events);
            }
            AgentEvent::InteractionRequested(_) => Self::clear(&mut state, &session, events),
            _ => {}
        }
    }
    pub(super) fn spawn(
        self: &Arc<Self>,
        agents: BTreeMap<AgentKind, Arc<dyn AgentAdapter>>,
        port: Arc<dyn TerminalInteractionPort>,
        routes: Routes,
        events: broadcast::Sender<AgentEvent>,
    ) -> tokio::task::JoinHandle<()> {
        let observer = self.clone();
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(Duration::from_millis(750));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut inspections = FuturesUnordered::new();
            let mut active = HashSet::new();
            let mut next_session = 0;
            loop {
                tokio::select! {
                    _ = ticks.tick() => {
                        let mut sessions = observer.state.lock().unwrap().observed.iter().cloned().collect::<Vec<_>>();
                        sessions.sort_by_key(|session| session.encode().to_string());
                        if !sessions.is_empty() {
                            let count = sessions.len();
                            sessions.rotate_left(next_session % count);
                        }
                        for session in sessions {
                            if active.len() >= 4 { break; }
                            next_session = next_session.wrapping_add(1);
                            if active.contains(&session) { continue; }
                            if routes.lock().unwrap().values().any(|route| route.session == session) {
                                Self::clear(&mut observer.state.lock().unwrap(), &session, &events);
                                continue;
                            }
                            let agent = agents[&session.agent].clone();
                            let port = port.clone();
                            active.insert(session.clone());
                            inspections.push(async move {
                                let inspection = tokio::time::timeout(Duration::from_secs(3), async {
                                    let target = agent.terminal_interaction_target(session.native_id.adapter_id()).await?;
                                    let prompt = port.inspect(session.agent, &target).await.ok()??;
                                    Some((target, prompt))
                                }).await.ok().flatten();
                                (session, inspection)
                            });
                        }
                    }
                    Some((session, inspection)) = inspections.next(), if !inspections.is_empty() => {
                        active.remove(&session);
                        // A protocol request may arrive during the terminal read.
                        if routes.lock().unwrap().values().any(|route| route.session == session) {
                            Self::clear(&mut observer.state.lock().unwrap(), &session, &events);
                        } else { observer.record(&session, inspection, &events); }
                    }
                }
            }
        })
    }
    fn record(
        &self,
        session: &SessionRef,
        inspection: Option<(TerminalInteractionTarget, TerminalInteraction)>,
        events: &broadcast::Sender<AgentEvent>,
    ) {
        let mut state = self.state.lock().unwrap();
        if !state.observed.contains(session) {
            return;
        }
        let Some((target, prompt)) = inspection else {
            Self::clear(&mut state, session, events);
            return;
        };
        if !state.seen.get(session).is_some_and(|seen| {
            seen.target == target
                && seen.prompt.fingerprint == prompt.fingerprint
                && seen.prompt.pane_id == prompt.pane_id
        }) {
            Self::clear(&mut state, session, events);
            state.seen.insert(
                session.clone(),
                Seen {
                    target,
                    prompt,
                    observations: 1,
                    token: None,
                    answered: false,
                },
            );
            return;
        }
        let seen = state.seen.get_mut(session).unwrap();
        seen.observations += 1;
        if seen.token.is_some() || seen.answered {
            return;
        }
        let token = format!("terminal:{}", uuid::Uuid::new_v4());
        seen.token = Some(token.clone());
        let target = seen.target.clone();
        let prompt = seen.prompt.clone();
        let switching = state.switches.get(session) == Some(&target.client_id);
        let mut decisions = prompt.choices.clone();
        let detail = if prompt.kind == TerminalInteractionKind::Unknown {
            decisions.push("Handled in terminal".into());
            format!(
                "This dialog cannot be answered reliably from IM. Handle it in the original terminal (pane {}). Use /rmux or /tmux to find the pane.\n\n{}",
                prompt.pane_id, prompt.detail
            )
        } else {
            decisions.push("cancel".into());
            format!(
                "{}\n\nChoose an option, or use /cancel. No default is submitted.",
                prompt.detail
            )
        };
        state.pending.insert(
            token.clone(),
            Pending {
                session: session.clone(),
                target: target.clone(),
                prompt: prompt.clone(),
                switching,
            },
        );
        let _ = events.send(AgentEvent::InteractionRequested(InteractionRequest {
            rpc_id: json!(token),
            method: "agentix/terminal/interaction".into(),
            session_id: session.encode().to_string(),
            turn_id: "terminal".into(),
            item_id: None,
            kind: InteractionKind::CommandApproval,
            title: prompt.title,
            detail,
            available_decisions: decisions,
            payload: json!({"clientId":target.client_id,"sessionSwitch":switching,"terminal":true,"cancelable":prompt.kind != TerminalInteractionKind::Unknown}),
            auto_resolution_ms: None,
        }));
    }
    pub(super) async fn respond(
        &self,
        token: &str,
        response: &Value,
        agents: &BTreeMap<AgentKind, Arc<dyn AgentAdapter>>,
        port: &dyn TerminalInteractionPort,
        events: &broadcast::Sender<AgentEvent>,
    ) -> Result<(), AgentError> {
        let invalid = |message: &str| AgentError::Rejected(message.into());
        let pending = {
            let mut state = self.state.lock().unwrap();
            let pending = state
                .pending
                .remove(token)
                .ok_or_else(|| invalid("Terminal interaction expired or was already answered"))?;
            if let Some(seen) = state.seen.get_mut(&pending.session) {
                seen.answered = true;
                seen.token = None;
            }
            pending
        };
        let session = &pending.session;
        let agent = &agents[&session.agent];
        let result = async {
            if !self.state.lock().unwrap().observed.contains(session)
                || agent
                    .terminal_interaction_target(session.native_id.adapter_id())
                    .await
                    .as_ref()
                    != Some(&pending.target)
                || agent.is_read_only(session.native_id.adapter_id()).await
            {
                return Err(invalid(
                    "The original agent session changed or is no longer writable",
                ));
            }
            let decision = response["decision"]
                .as_str()
                .ok_or_else(|| invalid("Select a displayed terminal choice"))?;
            if pending.prompt.kind == TerminalInteractionKind::Unknown {
                return if decision == "Handled in terminal" {
                    Ok(())
                } else {
                    Err(invalid("Handle this dialog in the original terminal"))
                };
            }
            let selection = if decision == "cancel" {
                TerminalInteractionResponse::Cancel
            } else {
                TerminalInteractionResponse::Choice(
                    pending
                        .prompt
                        .choices
                        .iter()
                        .position(|choice| choice == decision)
                        .ok_or_else(|| invalid("Select a displayed terminal choice"))?,
                )
            };
            if pending.switching {
                agent
                    .set_native_session_switch(
                        session.native_id.adapter_id(),
                        &pending.target.client_id,
                        selection != TerminalInteractionResponse::Cancel,
                    )
                    .await?;
            }
            port.respond(session.agent, &pending.target, &pending.prompt, selection)
                .await
        }
        .await;
        let _ = events.send(AgentEvent::InteractionResolved {
            session_id: session.encode().to_string(),
            request_id: token.into(),
        });
        if pending.switching && (result.is_err() || response["decision"] == "cancel") {
            let _ = agent
                .set_native_session_switch(
                    session.native_id.adapter_id(),
                    &pending.target.client_id,
                    false,
                )
                .await;
            let _ = events.send(AgentEvent::SessionSwitchFailed {
                session_id: session.encode().to_string(),
                client_id: pending.target.client_id,
                reason: result
                    .as_ref()
                    .err()
                    .map_or_else(|| "Session switch cancelled".into(), ToString::to_string),
            });
        }
        result
    }
}
