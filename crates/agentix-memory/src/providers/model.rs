use std::{collections::HashSet, sync::Arc};

use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::HttpProvider;
use crate::{AgentConfig, ModelApi, ProviderProtocol};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Message {
    User(String),
    Model(Value),
    Tool { id: String, output: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
    pub instructions: String,
    pub history: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelReply {
    pub continuation: Value,
    pub calls: Vec<ToolCall>,
    pub text: String,
    pub usage: TokenUsage,
}

#[async_trait]
pub trait Model: Send + Sync {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelReply>;
}

pub struct HttpModel {
    provider: Arc<HttpProvider>,
    config: AgentConfig,
}

impl HttpModel {
    pub fn new(provider: Arc<HttpProvider>, config: AgentConfig) -> Result<Self> {
        ensure!(
            provider.protocol == ProviderProtocol::Openai,
            "Agent requires an OpenAI-compatible provider"
        );
        ensure!(!config.model.is_empty(), "missing Agent model");
        ensure!(
            !(config.model == "gpt-6-astra" || config.model.starts_with("gpt-6-astra-"))
                || config.api == ModelApi::Responses,
            "GPT-6 Astra tool calling requires Responses"
        );
        Ok(Self { provider, config })
    }

    fn request_body(&self, request: &ModelRequest) -> Result<Value> {
        let mut input = Vec::new();
        if self.config.api == ModelApi::ChatCompletions {
            input.push(json!({"role":"system","content":request.instructions}));
        }
        for item in &request.history {
            match item {
                Message::User(text) => input.push(json!({"role":"user","content":text})),
                Message::Model(output) => {
                    if self.config.api == ModelApi::Responses {
                        input.extend(
                            output
                                .as_array()
                                .context("invalid Responses continuation")?
                                .iter()
                                .cloned(),
                        );
                    } else {
                        input.push(output.clone());
                    }
                }
                Message::Tool { id, output } => {
                    input.push(if self.config.api == ModelApi::Responses {
                        json!({"type":"function_call_output","call_id":id,"output":output})
                    } else {
                        json!({"role":"tool","tool_call_id":id,"content":output})
                    });
                }
            }
        }
        let tools:Vec<_>=request.tools.iter().map(|tool| {
            let function=json!({"name":tool.name,"description":tool.description,"parameters":tool.parameters,"strict":true});
            if self.config.api==ModelApi::Responses {
                let mut function=function;function["type"]=json!("function");function
            } else {json!({"type":"function","function":function})}
        }).collect();
        let mut body = if self.config.api == ModelApi::Responses {
            json!({"model":self.config.model,"instructions":request.instructions,"input":input,"tools":tools,
                "store":false,"include":["reasoning.encrypted_content"],"max_output_tokens":self.config.max_output_tokens})
        } else {
            json!({"model":self.config.model,"messages":input,"tools":tools,"max_completion_tokens":self.config.max_output_tokens})
        };
        if !request.tools.is_empty() {
            body["tool_choice"] = json!("required");
        }
        if let Some(effort) = self.config.reasoning_effort {
            if self.config.api == ModelApi::Responses {
                body["reasoning"] = json!({"effort": effort});
            } else {
                body["reasoning_effort"] = json!(effort);
            }
        }
        Ok(body)
    }
}

#[async_trait]
impl Model for HttpModel {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelReply> {
        let body = self.request_body(request)?;
        let endpoint = if self.config.api == ModelApi::Responses {
            "responses"
        } else {
            "chat/completions"
        };
        let value = self
            .provider
            .post(endpoint, body, self.config.request_timeout_seconds)
            .await?;
        if self.config.api == ModelApi::Responses {
            responses_reply(&value)
        } else {
            chat_reply(&value)
        }
    }
}

fn call(id: &Value, name: &Value, arguments: &Value) -> Result<ToolCall> {
    let id = id
        .as_str()
        .filter(|s| !s.is_empty())
        .context("missing model tool call ID")?;
    let name = name
        .as_str()
        .filter(|s| !s.is_empty())
        .context("missing model tool name")?;
    let arguments =
        serde_json::from_str(arguments.as_str().context("invalid model tool arguments")?)?;
    Ok(ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
    })
}

fn responses_reply(value: &Value) -> Result<ModelReply> {
    ensure!(
        value.get("status").is_none_or(|v| v == "completed"),
        "model response is incomplete or failed"
    );
    let output = value["output"].as_array().context("missing model output")?;
    let mut calls = Vec::new();
    let mut text = String::new();
    let mut ids = HashSet::new();
    for item in output {
        if item["type"] == "function_call" {
            let call = call(&item["call_id"], &item["name"], &item["arguments"])?;
            ensure!(ids.insert(call.id.clone()), "duplicate model tool call ID");
            calls.push(call);
        } else if item["type"] == "message" {
            for part in item["content"]
                .as_array()
                .context("invalid model message content")?
            {
                ensure!(part["type"] != "refusal", "model refused request");
                if part["type"] == "output_text" {
                    text.push_str(part["text"].as_str().context("invalid model text")?);
                }
            }
        }
    }
    Ok(ModelReply {
        continuation: json!(output),
        calls,
        text,
        usage: TokenUsage {
            input_tokens: value["usage"]["input_tokens"].as_u64().unwrap_or(0),
            output_tokens: value["usage"]["output_tokens"].as_u64().unwrap_or(0),
        },
    })
}

fn chat_reply(value: &Value) -> Result<ModelReply> {
    let choice = value["choices"]
        .as_array()
        .and_then(|v| v.first())
        .context("missing model choice")?;
    ensure!(
        matches!(
            choice["finish_reason"].as_str(),
            Some("stop" | "tool_calls")
        ),
        "model response is incomplete or failed"
    );
    let message = &choice["message"];
    ensure!(message["refusal"].is_null(), "model refused request");
    let mut calls = Vec::new();
    let mut ids = HashSet::new();
    if let Some(items) = message["tool_calls"].as_array() {
        for item in items {
            let call = call(
                &item["id"],
                &item["function"]["name"],
                &item["function"]["arguments"],
            )?;
            ensure!(ids.insert(call.id.clone()), "duplicate model tool call ID");
            calls.push(call);
        }
    }
    Ok(ModelReply {
        continuation: message.clone(),
        calls,
        text: message["content"].as_str().unwrap_or_default().into(),
        usage: TokenUsage {
            input_tokens: value["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: value["usage"]["completion_tokens"].as_u64().unwrap_or(0),
        },
    })
}
