use std::{collections::HashSet, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use serde_json::Value;

use crate::{AgentConfig, Message, Model, ModelRequest, TokenUsage, ToolDefinition};

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
        tokio::time::timeout(
            Duration::from_secs(self.config.task_timeout_seconds),
            self.run_inner(instructions, input, tools, finish),
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
            ensure!(
                !reply.calls.is_empty(),
                "Agent must finish with the submission tool"
            );
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
                let output = tools.execute(&call.name, call.arguments).await?;
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
