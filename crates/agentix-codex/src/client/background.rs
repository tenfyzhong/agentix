use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use agentix_core::{AgentEvent, SessionId, TurnStatus};
use serde_json::{Value, json};

use super::{ClientError, CodexClient};
use crate::protocol::{ServerMessage, decode_server_frame, parse_turn_status};

/// Observe stored turn metadata without resuming a thread or acquiring its writer.
pub(super) struct BackgroundTurns {
    latest_completed: HashMap<SessionId, Option<String>>,
    started_at: u64,
}

impl BackgroundTurns {
    pub(super) fn new() -> Self {
        Self {
            latest_completed: HashMap::new(),
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }

    pub(super) async fn poll(&mut self, client: &CodexClient, running: &HashSet<SessionId>) {
        // Read a disappearing session once more to catch a turn that finished
        // immediately before its terminal process exited.
        let candidates = running
            .iter()
            .chain(self.latest_completed.keys())
            .cloned()
            .collect::<HashSet<_>>();
        for session in candidates {
            match self.read_completions(client, &session).await {
                Ok((latest, events)) => {
                    if running.contains(&session) {
                        self.latest_completed.insert(session.clone(), latest);
                    } else {
                        self.latest_completed.remove(&session);
                    }
                    let subscribed = client.subscriptions.lock().await.contains(&session);
                    if subscribed {
                        continue;
                    }
                    let mut completed = client.completed_turns.lock().await;
                    // A live subscription may have delivered several of these turns
                    // since the last poll. Skip through its latest completed turn.
                    let already_delivered = events.iter().rposition(|event| {
                        matches!(event, AgentEvent::TurnCompleted { turn_id, .. }
                            if completed.get(&session) == Some(turn_id))
                    });
                    for event in events
                        .into_iter()
                        .skip(already_delivered.map_or(0, |index| index + 1))
                    {
                        if let AgentEvent::TurnCompleted { turn_id, .. } = &event {
                            completed.insert(session.clone(), turn_id.clone());
                            let _ = client.events.send(event);
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, session = %session, "failed to read background Codex turns");
                }
            }
        }
    }

    async fn read_completions(
        &self,
        client: &CodexClient,
        session: &SessionId,
    ) -> Result<(Option<String>, Vec<AgentEvent>), ClientError> {
        let previous = self.latest_completed.get(session);
        let mut latest = previous.cloned().flatten();
        let mut found_terminal = false;
        let mut events = Vec::new();
        let mut cursor = None;
        'pages: loop {
            let page = read_turn_page(client, session, cursor).await?;
            let turns = page["data"]
                .as_array()
                .ok_or(ClientError::InvalidResponse("background turn list"))?;
            for turn in turns {
                if !matches!(
                    parse_turn_status(turn["status"].as_str()),
                    TurnStatus::Completed | TurnStatus::Failed | TurnStatus::Interrupted
                ) {
                    continue;
                }
                let id = turn["id"]
                    .as_str()
                    .ok_or(ClientError::InvalidResponse("background turn id"))?;
                if !found_terminal {
                    latest = Some(id.to_owned());
                    found_terminal = true;
                }
                if previous.and_then(Option::as_deref) == Some(id)
                    || (previous.is_none()
                        && turn["completedAt"]
                            .as_u64()
                            .is_none_or(|time| time < self.started_at))
                {
                    break 'pages;
                }
                let ServerMessage::Event(event) = decode_server_frame(&json!({
                    "method": "turn/completed",
                    "params": {"threadId": session.as_str(), "turn": turn}
                }))?
                else {
                    return Err(ClientError::InvalidResponse("background completion"));
                };
                events.push(event);
            }
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        // The API returns newest first; notify in turn execution order.
        events.reverse();
        Ok((latest, events))
    }
}

async fn read_turn_page(
    client: &CodexClient,
    session: &SessionId,
    cursor: Option<String>,
) -> Result<Value, ClientError> {
    match client
        .request_after_reconnect(
            "thread/turns/list",
            json!({
                "threadId": session.as_str(),
                "cursor": cursor,
                "limit": 100,
                "sortDirection": "desc",
                "itemsView": "notLoaded"
            }),
        )
        .await
    {
        Err(ClientError::Rpc { code: -32601, .. }) => {
            let thread = client.read_thread(session, true).await?;
            let mut turns = thread["turns"]
                .as_array()
                .ok_or(ClientError::InvalidResponse("background thread turns"))?
                .clone();
            turns.reverse();
            Ok(json!({"data": turns, "nextCursor": null}))
        }
        result => result,
    }
}
