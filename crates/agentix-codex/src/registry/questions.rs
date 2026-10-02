//! Observe CLI questions without changing the proxied wire stream.
use super::{AgentEvent, BTreeSet, ClientRegistry, State, Value};
use crate::{ServerMessage, decode_server_frame};

pub(super) struct PendingQuestion {
    request: agentix_domain::InteractionRequest,
    connections: BTreeSet<u64>,
    answered: BTreeSet<usize>,
}

pub(super) struct SubmittedAnswers {
    key: String,
    indices: BTreeSet<usize>,
}

impl ClientRegistry {
    pub(crate) fn complete_question(&self, id: &Value) {
        let mut state = self.state.lock().unwrap();
        let key = id.to_string();
        remember_resolution(&mut state, key.clone());
        if let Some(question) = state.questions.remove(&key) {
            publish(&mut state, resolved(&question.request));
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
        for connection in state.connections.values_mut() {
            connection.question_inputs.clear();
        }
        state.lifecycle.retain(|(_, event)| {
            !matches!(
                event,
                AgentEvent::InteractionRequested(_) | AgentEvent::InteractionResolved { .. }
            )
        });
    }

    pub(super) fn observe_async_question_input(&self, connection: u64, value: &Value) {
        let Some(id) = value
            .get("id")
            .filter(|id| id.is_string() || id.is_number())
        else {
            return;
        };
        let Some(thread) = value["params"]["threadId"].as_str() else {
            return;
        };
        let Some(input) = value["params"]["input"].as_array() else {
            return;
        };
        let text = input
            .iter()
            .filter(|part| part["type"] == "text")
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let mut state = self.state.lock().unwrap();
        let questions = state
            .questions
            .iter()
            .filter(|(_, question)| {
                question.request.session_id == thread && question.connections.contains(&connection)
            })
            .filter_map(|(key, question)| {
                let indices = async_question_answers(&question.request, &text);
                (!indices.is_empty()).then(|| SubmittedAnswers {
                    key: key.clone(),
                    indices,
                })
            })
            .collect::<Vec<_>>();
        if let Some(client) = state.connections.get_mut(&connection) {
            // Snapshot only questions observed before this submission. A late
            // response must not clear new questions emitted while it was pending.
            if questions.is_empty() {
                client.question_inputs.remove(&id.to_string());
            } else {
                client.question_inputs.insert(id.to_string(), questions);
            }
        }
    }

    pub(super) fn observe_async_question_acceptance(&self, connection: u64, value: &Value) {
        let Some(id) = value.get("id") else { return };
        let mut state = self.state.lock().unwrap();
        let Some(questions) = state
            .connections
            .get_mut(&connection)
            .and_then(|client| client.question_inputs.remove(&id.to_string()))
        else {
            return;
        };
        if value.get("result").is_none() || value.get("error").is_some() {
            return;
        }
        let mut changed = false;
        for SubmittedAnswers { key, indices } in questions {
            let Some(question) = state.questions.get_mut(&key) else {
                continue;
            };
            question.answered.extend(indices);
            let count = question.request.rpc_id["agentixAsyncQuestion"]["questions"]
                .as_array()
                .map_or(0, Vec::len);
            if count > 0 && question.answered.len() == count {
                let question = state.questions.remove(&key).unwrap();
                remember_resolution(&mut state, key);
                publish(&mut state, resolved(&question.request));
                changed = true;
            }
        }
        drop(state);
        if changed {
            self.changed
                .send_modify(|version| *version = version.wrapping_add(1));
        }
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
                if let Some(question) = state.questions.get_mut(&key) {
                    question.connections.insert(connection);
                    return;
                }
                state.questions.insert(
                    key,
                    PendingQuestion {
                        request: request.clone(),
                        connections: BTreeSet::from([connection]),
                        answered: BTreeSet::new(),
                    },
                );
                AgentEvent::InteractionRequested(request)
            }
            Ok(ServerMessage::Event(AgentEvent::InteractionResolved { .. })) => {
                let Some(id) = value["params"].get("requestId") else {
                    return;
                };
                let key = id.to_string();
                let Some(question) = state.questions.remove(&key) else {
                    return;
                };
                remember_resolution(&mut state, key);
                resolved(&question.request)
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
            .is_some_and(|question| question.connections.contains(&connection))
        {
            return;
        }
        let question = state.questions.remove(&key).unwrap();
        remember_resolution(&mut state, key);
        publish(&mut state, resolved(&question.request));
        drop(state);
        self.changed
            .send_modify(|version| *version = version.wrapping_add(1));
    }
}

fn async_question_answers(
    request: &agentix_domain::InteractionRequest,
    text: &str,
) -> BTreeSet<usize> {
    let Some(questions) = request.rpc_id["agentixAsyncQuestion"]["questions"].as_array() else {
        return BTreeSet::new();
    };
    let text = text.trim();
    if let Some(body) = text.strip_prefix("<send_user_message_question_reply>") {
        let Some(body) = body.strip_suffix("</send_user_message_question_reply>") else {
            return BTreeSet::new();
        };
        let Ok(Value::Array(answers)) = serde_json::from_str::<Value>(body) else {
            return BTreeSet::new();
        };
        return questions
            .iter()
            .enumerate()
            .filter(|(_, question)| {
                answers.iter().any(|answer| {
                    answer["question"] == question["title"]
                        && answer["answer"]
                            .as_str()
                            .is_some_and(|text| !text.trim().is_empty())
                })
            })
            .map(|(index, _)| index)
            .collect();
    }
    // The CLI and Agentix also submit readable title/answer blocks as input.
    questions
        .iter()
        .enumerate()
        .filter(|(_, question)| {
            question["title"].as_str().is_some_and(|title| {
                text.split("\n\n").any(|block| {
                    block
                        .strip_prefix(title)
                        .and_then(|answer| answer.strip_prefix('\n'))
                        .is_some_and(|answer| !answer.trim().is_empty())
                })
            })
        })
        .map(|(index, _)| index)
        .collect()
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
