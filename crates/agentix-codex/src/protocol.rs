use agentix_core::{
    AgentEvent, InteractionKind, InteractionRequest, ItemSummary, SessionStatus, TurnStatus,
};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IdRef {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TurnStartResult {
    pub turn: IdRef,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnSteerResult {
    pub turn_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TextInput {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct QueuedSubmission {
    pub id: String,
    pub input: Vec<TextInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueueAddResult {
    pub queued_submission: QueuedSubmission,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueueListResult {
    pub data: Vec<QueuedSubmission>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelDescriptor {
    pub id: Option<String>,
    pub model: Option<String>,
    pub display_name: Option<String>,
    #[serde(default)]
    pub supported_reasoning_efforts: Vec<ReasoningEffortDescriptor>,
    #[serde(default)]
    pub service_tiers: Vec<ModelServiceTier>,
}

impl ModelDescriptor {
    pub(crate) fn identifier(&self) -> Option<&str> {
        self.model.as_deref().or(self.id.as_deref())
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModelServiceTier {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReasoningEffortDescriptor {
    pub reasoning_effort: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelListResult {
    pub data: Vec<ModelDescriptor>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    Response {
        id: Value,
        result: Result<Value, RpcError>,
    },
    Event(AgentEvent),
    Interaction(InteractionRequest),
    Ignored,
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("message is missing a method or response result")]
    UnknownShape,
    #[error("{method} is missing required field {field}")]
    MissingField { method: String, field: &'static str },
    #[error("invalid RPC error object")]
    InvalidRpcError,
}

pub fn decode_server_frame(value: &Value) -> Result<ServerMessage, ProtocolError> {
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        if let Some(id) = value.get("id") {
            return decode_interaction(id.clone(), method, params);
        }
        return decode_notification(method, &params);
    }
    if let Some(id) = value.get("id") {
        if let Some(result) = value.get("result") {
            return Ok(ServerMessage::Response {
                id: id.clone(),
                result: Ok(result.clone()),
            });
        }
        if let Some(error) = value.get("error") {
            let code = error
                .get("code")
                .and_then(Value::as_i64)
                .ok_or(ProtocolError::InvalidRpcError)?;
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .ok_or(ProtocolError::InvalidRpcError)?;
            return Ok(ServerMessage::Response {
                id: id.clone(),
                result: Err(RpcError {
                    code,
                    message: message.to_owned(),
                    data: error.get("data").cloned(),
                }),
            });
        }
    }
    Err(ProtocolError::UnknownShape)
}

fn decode_interaction(
    id: Value,
    method: &str,
    params: Value,
) -> Result<ServerMessage, ProtocolError> {
    let kind = match method {
        "item/commandExecution/requestApproval" => InteractionKind::CommandApproval,
        "item/fileChange/requestApproval" => InteractionKind::FileApproval,
        "item/tool/requestUserInput" | "tool/requestUserInput" => InteractionKind::UserInput,
        _ => return Ok(ServerMessage::Ignored),
    };
    let session_id = required_string(&params, method, "threadId")?;
    let turn_id = required_string(&params, method, "turnId")?;
    let item_id = params
        .get("itemId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let available_decisions = params
        .get("availableDecisions")
        .and_then(Value::as_array)
        .map_or_else(
            || match kind {
                InteractionKind::CommandApproval | InteractionKind::FileApproval => {
                    vec!["accept".into(), "decline".into(), "cancel".into()]
                }
                InteractionKind::UserInput => Vec::new(),
            },
            |values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            },
        );
    let (title, detail) = interaction_copy(&kind, &params);
    Ok(ServerMessage::Interaction(InteractionRequest {
        rpc_id: id,
        method: method.to_owned(),
        session_id,
        turn_id,
        item_id,
        kind,
        title,
        detail,
        available_decisions,
        auto_resolution_ms: params.get("autoResolutionMs").and_then(Value::as_u64),
        payload: params,
    }))
}

fn interaction_copy(kind: &InteractionKind, params: &Value) -> (String, String) {
    match kind {
        InteractionKind::CommandApproval => {
            let command = params
                .get("command")
                .map_or_else(|| "Command execution".to_owned(), value_as_text);
            let cwd = params.get("cwd").and_then(Value::as_str).unwrap_or("");
            ("Command approval".into(), format!("{command}\n{cwd}"))
        }
        InteractionKind::FileApproval => (
            "File change approval".into(),
            params
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("Codex requests permission to modify files")
                .to_owned(),
        ),
        InteractionKind::UserInput => (
            "Codex needs input".into(),
            params.get("questions").map_or_else(
                || "Please answer the Codex prompt".to_owned(),
                value_as_text,
            ),
        ),
    }
}

fn decode_notification(method: &str, params: &Value) -> Result<ServerMessage, ProtocolError> {
    let event = match method {
        "item/agentMessage/delta" => AgentEvent::AgentMessageDelta {
            session_id: required_string(params, method, "threadId")?,
            turn_id: required_string(params, method, "turnId")?,
            item_id: required_string(params, method, "itemId")?,
            delta: required_string(params, method, "delta")?,
        },
        "thread/status/changed" => AgentEvent::SessionStatusChanged {
            session_id: required_string(params, method, "threadId")?,
            status: parse_session_status(params.get("status")),
        },
        "thread/queue/changed" => AgentEvent::QueueChanged {
            session_id: required_string(params, method, "threadId")?,
        },
        "turn/started" => AgentEvent::TurnStarted {
            session_id: required_string(params, method, "threadId")?,
            turn_id: nested_id(params, method, "turn")?,
        },
        "turn/completed" => {
            let turn = params
                .get("turn")
                .ok_or_else(|| ProtocolError::MissingField {
                    method: method.to_owned(),
                    field: "turn",
                })?;
            AgentEvent::TurnCompleted {
                session_id: required_string(params, method, "threadId")?,
                turn_id: required_string(turn, method, "id")?,
                status: parse_turn_status(turn.get("status").and_then(Value::as_str)),
                error: turn
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }
        }
        "item/started" => {
            let item = params
                .get("item")
                .ok_or_else(|| ProtocolError::MissingField {
                    method: method.to_owned(),
                    field: "item",
                })?;
            AgentEvent::ItemStarted {
                session_id: required_string(params, method, "threadId")?,
                turn_id: required_string(params, method, "turnId")?,
                item_id: required_string(item, method, "id")?,
                kind: item
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned(),
                label: item_label(item),
            }
        }
        "item/completed" => {
            let item = params
                .get("item")
                .ok_or_else(|| ProtocolError::MissingField {
                    method: method.to_owned(),
                    field: "item",
                })?;
            AgentEvent::ItemCompleted {
                session_id: required_string(params, method, "threadId")?,
                turn_id: required_string(params, method, "turnId")?,
                item: item_summary(item, method)?,
            }
        }
        "serverRequest/resolved" => AgentEvent::InteractionResolved {
            session_id: required_string(params, method, "threadId")?,
            request_id: params
                .get("requestId")
                .map(value_as_text)
                .unwrap_or_default(),
        },
        _ => return Ok(ServerMessage::Ignored),
    };
    Ok(ServerMessage::Event(event))
}

fn required_string(
    value: &Value,
    method: &str,
    field: &'static str,
) -> Result<String, ProtocolError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ProtocolError::MissingField {
            method: method.to_owned(),
            field,
        })
}

fn nested_id(value: &Value, method: &str, field: &'static str) -> Result<String, ProtocolError> {
    let nested = value
        .get(field)
        .ok_or_else(|| ProtocolError::MissingField {
            method: method.to_owned(),
            field,
        })?;
    required_string(nested, method, "id")
}

pub(crate) fn parse_session_status(value: Option<&Value>) -> SessionStatus {
    match value
        .and_then(|status| status.get("type"))
        .and_then(Value::as_str)
    {
        Some("notLoaded") => SessionStatus::NotLoaded,
        Some("idle") => SessionStatus::Idle,
        Some("active") => SessionStatus::Active,
        Some("systemError") => SessionStatus::SystemError,
        _ => SessionStatus::Unknown,
    }
}

pub(crate) fn parse_turn_status(value: Option<&str>) -> TurnStatus {
    match value {
        Some("inProgress") => TurnStatus::InProgress,
        Some("completed") => TurnStatus::Completed,
        Some("interrupted") => TurnStatus::Interrupted,
        Some("failed") => TurnStatus::Failed,
        _ => TurnStatus::Unknown,
    }
}

pub(crate) fn item_summary(item: &Value, method: &str) -> Result<ItemSummary, ProtocolError> {
    let kind = item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let text = match kind.as_str() {
        "agentMessage" | "plan" => item
            .get("text")
            .or_else(|| item.get("content"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        "userMessage" => item
            .get("content")
            .and_then(Value::as_array)
            .map(|content| {
                content
                    .iter()
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
        _ => None,
    };
    Ok(ItemSummary {
        id: required_string(item, method, "id")?,
        kind,
        text,
        status: item
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn item_label(item: &Value) -> String {
    item.get("command")
        .or_else(|| item.get("tool"))
        .or_else(|| item.get("type"))
        .map_or_else(|| "Codex item".to_owned(), value_as_text)
}

fn value_as_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(values) => values
            .iter()
            .map(value_as_text)
            .collect::<Vec<_>>()
            .join(" "),
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod typed_response_tests {
    use super::QueueListResult;

    #[test]
    fn queue_results_are_deserialized_into_typed_protocol_data() {
        let result: QueueListResult = serde_json::from_value(serde_json::json!({
            "data": [{
                "id": "queued-1",
                "input": [{"type": "text", "text": "follow up"}]
            }],
            "nextCursor": "next"
        }))
        .unwrap();

        assert_eq!(result.data[0].id, "queued-1");
        assert_eq!(result.next_cursor.as_deref(), Some("next"));
    }
}
