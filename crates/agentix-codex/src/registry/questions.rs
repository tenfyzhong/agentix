//! Observe CLI questions without changing the proxied wire stream.
use super::{AgentEvent, BTreeSet, ClientRegistry, State, Value};
use crate::{ServerMessage, decode_server_frame};

impl ClientRegistry {
    pub(crate) fn complete_question(&self, id: &Value) {
        let mut state = self.state.lock().unwrap();
        let key = id.to_string();
        remember_resolution(&mut state, key.clone());
        if let Some((request, _)) = state.questions.remove(&key) {
            publish(&mut state, resolved(&request));
        }
        drop(state);
        self.changed
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    pub(crate) fn question_is_pending(&self, id: &Value) -> bool {
        self.state
            .lock()
            .unwrap()
            .questions
            .contains_key(&id.to_string())
    }

    pub(crate) fn question_was_resolved(&self, id: &Value) -> bool {
        self.state
            .lock()
            .unwrap()
            .resolved_questions
            .contains(&id.to_string())
    }

    pub(crate) fn reset_questions(&self) {
        let mut state = self.state.lock().unwrap();
        state.questions.clear();
        state.resolved_questions.clear();
        state.lifecycle.retain(|(_, event)| {
            !matches!(
                event,
                AgentEvent::InteractionRequested(_) | AgentEvent::InteractionResolved { .. }
            )
        });
    }

    pub(super) fn observe_question_frame(&self, connection: u64, method: &str, text: &str) {
        if !matches!(
            method,
            "item/tool/requestUserInput"
                | "tool/requestUserInput"
                | "serverRequest/resolved"
                | "item/completed"
        ) {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        if !state.connections.contains_key(&connection) {
            return;
        }
        let decoded = crate::protocol::async_question(&value)
            .map(ServerMessage::Interaction)
            .map_or_else(|| decode_server_frame(&value), Ok);
        let event = match decoded {
            Ok(ServerMessage::Interaction(request)) => {
                let key = request.rpc_id.to_string();
                if state.resolved_questions.contains(&key) {
                    return;
                }
                if let Some((_, connections)) = state.questions.get_mut(&key) {
                    connections.insert(connection);
                    return;
                }
                state
                    .questions
                    .insert(key, (request.clone(), BTreeSet::from([connection])));
                AgentEvent::InteractionRequested(request)
            }
            Ok(ServerMessage::Event(AgentEvent::InteractionResolved { .. })) => {
                let Some(id) = value["params"].get("requestId") else {
                    return;
                };
                let key = id.to_string();
                let Some((request, _)) = state.questions.remove(&key) else {
                    return;
                };
                remember_resolution(&mut state, key);
                resolved(&request)
            }
            _ => return,
        };
        publish(&mut state, event);
        drop(state);
        self.changed
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    pub(super) fn observe_question_answer(&self, connection: u64, text: &str) {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return;
        };
        if value.get("result").is_none() && value.get("error").is_none() {
            return;
        }
        let Some(id) = value.get("id") else { return };
        let mut state = self.state.lock().unwrap();
        let key = id.to_string();
        if !state
            .questions
            .get(&key)
            .is_some_and(|(_, connections)| connections.contains(&connection))
        {
            return;
        }
        let (request, _) = state.questions.remove(&key).unwrap();
        remember_resolution(&mut state, key);
        publish(&mut state, resolved(&request));
        drop(state);
        self.changed
            .send_modify(|version| *version = version.wrapping_add(1));
    }
}

fn remember_resolution(state: &mut State, key: String) {
    if !state.resolved_questions.contains(&key) {
        state.resolved_questions.push_back(key);
        while state.resolved_questions.len() > 1024 {
            state.resolved_questions.pop_front();
        }
    }
}

fn resolved(request: &agentix_domain::InteractionRequest) -> AgentEvent {
    AgentEvent::InteractionResolved {
        session_id: request.session_id.clone(),
        request_id: request
            .rpc_id
            .as_str()
            .map_or_else(|| request.rpc_id.to_string(), str::to_owned),
    }
}

fn publish(state: &mut State, event: AgentEvent) {
    state.sequence += 1;
    state.lifecycle.push_back((state.sequence, event));
    while state.lifecycle.len() > 1024 {
        state.lifecycle.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_control_reply_prevents_late_proxy_question_replay() {
        let registry = ClientRegistry::default();
        let cli = registry.connect(None);
        let frame = json!({"id":91,"method":"item/tool/requestUserInput","params":{"threadId":"t","turnId":"turn","itemId":"item","questions":[]}}).to_string();
        registry.server_frame(cli, &frame);
        registry.complete_question(&json!(91));
        assert!(!registry.question_is_pending(&json!(91)));
        registry.server_frame(cli, &frame);
        assert_eq!(registry.lifecycle_since(0).len(), 2);
        assert!(registry.question_was_resolved(&json!(91)));
    }
}
