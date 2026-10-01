use std::{collections::HashSet, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{AgentConfig, Message, Model, ModelRequest, TokenUsage, ToolDefinition};

/// A bounded, model-correctable argument error, never an operational failure.
#[derive(Debug)]
pub struct ToolInputError(pub String);

impl std::fmt::Display for ToolInputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ToolInputError {}

/// Tools are scoped by the caller. A loop cannot acquire new capabilities.
#[async_trait]
pub trait ToolSet: Send + Sync {
    fn definitions(&self) -> Vec<ToolDefinition>;
    async fn execute(&self, name: &str, arguments: Value) -> Result<Value>;
}

pub struct LoopResult {
    pub value: Value,
    pub usage: TokenUsage,
    pub steps: usize,
    pub tool_calls: usize,
}

pub struct AgentLoop {
    model: Arc<dyn Model>,
    config: AgentConfig,
}

impl AgentLoop {
    pub fn new(model: Arc<dyn Model>, config: AgentConfig) -> Self {
        Self { model, config }
    }

    /// All conversation state belongs to this invocation, including retries.
    pub async fn run(
        &self,
        instructions: &str,
        input: &str,
        tools: &dyn ToolSet,
        finish: ToolDefinition,
    ) -> Result<LoopResult> {
        self.run_validated(instructions, input, tools, finish, &|_| Ok(()))
            .await
    }

    /// Validate a proposal without side effects before accepting it. Invalid
    /// proposals receive feedback within the same time, step and token budgets.
    pub async fn run_validated(
        &self,
        instructions: &str,
        input: &str,
        tools: &dyn ToolSet,
        finish: ToolDefinition,
        validate: &(dyn Fn(&Value) -> Result<()> + Send + Sync),
    ) -> Result<LoopResult> {
        tokio::time::timeout(
            Duration::from_secs(self.config.task_timeout_seconds),
            self.run_inner(instructions, input, tools, finish, validate),
        )
        .await
        .context("memory Agent task timeout")?
    }

    async fn run_inner(
        &self,
        instructions: &str,
        input: &str,
        tools: &dyn ToolSet,
        finish: ToolDefinition,
        validate: &(dyn Fn(&Value) -> Result<()> + Send + Sync),
    ) -> Result<LoopResult> {
        let mut definitions = tools.definitions();
        definitions.push(finish.clone());
        let names: HashSet<_> = definitions.iter().map(|d| d.name.clone()).collect();
        ensure!(
            names.len() == definitions.len(),
            "duplicate Agent tool name"
        );
        let mut request = ModelRequest {
            instructions: instructions.into(),
            history: vec![Message::User(input.into())],
            tools: definitions,
        };
        let mut usage = TokenUsage::default();
        let mut tool_calls = 0;
        for step in 0..self.config.max_steps {
            self.check_context(&request)?;
            let reply = self.model.complete(&request).await?;
            usage.input_tokens = usage.input_tokens.saturating_add(reply.usage.input_tokens);
            usage.output_tokens = usage
                .output_tokens
                .saturating_add(reply.usage.output_tokens);
            if reply.calls.is_empty() {
                request.history.push(Message::Model(reply.continuation));
                request.history.push(Message::User(
                    "No tool call was returned. Continue with the supplied tools and finish with the submission tool; a submission must satisfy the supplied task and schema.".into(),
                ));
                continue;
            }
            tool_calls += reply.calls.len();
            ensure!(
                tool_calls <= self.config.max_tool_calls,
                "Agent tool budget exceeded"
            );
            let mut ids = HashSet::new();
            for call in &reply.calls {
                ensure!(names.contains(&call.name), "unknown Agent tool");
                ensure!(
                    !call.id.is_empty() && ids.insert(&call.id),
                    "duplicate or empty tool call ID"
                );
            }
            if let Some(call) = reply.calls.iter().find(|c| c.name == finish.name) {
                ensure!(
                    reply.calls.len() == 1,
                    "submission must be the only final tool call"
                );
                ensure!(
                    serde_json::to_vec(&call.arguments)?.len() <= self.config.max_context_bytes,
                    "Agent submission budget exceeded"
                );
                if let Err(error) = validate(&call.arguments) {
                    let mut message = error.to_string();
                    let mut end = message.len().min(2048);
                    while !message.is_char_boundary(end) {
                        end -= 1;
                    }
                    message.truncate(end);
                    request
                        .history
                        .push(Message::Model(reply.continuation.clone()));
                    request.history.push(Message::Tool {
                        id: call.id.clone(),
                        output: serde_json::to_string(&json!({"error": message}))?,
                    });
                    continue;
                }
                return Ok(LoopResult {
                    value: call.arguments.clone(),
                    usage,
                    steps: step + 1,
                    tool_calls,
                });
            }
            request.history.push(Message::Model(reply.continuation));
            self.check_context(&request)?;
            for call in reply.calls {
                let output = match tools.execute(&call.name, call.arguments).await {
                    Ok(output) => output,
                    Err(error) => match error.downcast_ref::<ToolInputError>() {
                        Some(input) => json!({"error": input.to_string()}),
                        None => return Err(error),
                    },
                };
                request.history.push(Message::Tool {
                    id: call.id,
                    output: serde_json::to_string(&output)?,
                });
                self.check_context(&request)?;
            }
        }
        bail!("Agent step budget exceeded")
    }

    fn check_context(&self, request: &ModelRequest) -> Result<()> {
        ensure!(
            serde_json::to_vec(request)?.len() <= self.config.max_context_bytes,
            "Agent context budget exceeded"
        );
        Ok(())
    }
}
