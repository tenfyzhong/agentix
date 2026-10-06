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

// Correctable feedback is separate from an operational failure such as a failed DB read.
#[async_trait]
pub(crate) trait ProposalValidator: Send + Sync {
    async fn validate(&self, value: &Value) -> Result<Option<String>>;
}

struct ImmediateValidator<'a>(&'a (dyn Fn(&Value) -> Result<()> + Send + Sync));
#[async_trait]
impl ProposalValidator for ImmediateValidator<'_> {
    async fn validate(&self, value: &Value) -> Result<Option<String>> {
        Ok((self.0)(value).err().map(|error| error.to_string()))
    }
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
        self.run_async_validated(
            instructions,
            input,
            tools,
            finish,
            &ImmediateValidator(validate),
        )
        .await
    }

    pub(crate) async fn run_async_validated(
        &self,
        instructions: &str,
        input: &str,
        tools: &dyn ToolSet,
        finish: ToolDefinition,
        validate: &dyn ProposalValidator,
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
        validate: &dyn ProposalValidator,
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
            if step + 1 == self.config.max_steps {
                request.tools = vec![finish.clone()];
                request.history.push(Message::User("This is the final step within the original budget. Submit a validated result using the evidence already inspected. Report insufficient evidence when answering a question; do not invent facts or mutations to finish.".into()));
            }
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
                ensure!(
                    request.tools.iter().any(|tool| tool.name == call.name),
                    "unknown or unavailable Agent tool"
                );
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
                if let Some(mut message) = validate.validate(&call.arguments).await? {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModelReply, ToolCall};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct SubmissionModel(AtomicUsize);
    #[async_trait]
    impl Model for SubmissionModel {
        async fn complete(&self, _: &ModelRequest) -> Result<ModelReply> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ModelReply {
                continuation: json!([]),
                calls: vec![ToolCall {
                    id: "submission".into(),
                    name: "submit".into(),
                    arguments: json!({}),
                }],
                text: String::new(),
                usage: TokenUsage::default(),
            })
        }
    }
    struct NoTools;
    #[async_trait]
    impl ToolSet for NoTools {
        fn definitions(&self) -> Vec<ToolDefinition> {
            vec![]
        }
        async fn execute(&self, _: &str, _: Value) -> Result<Value> {
            bail!("no tools available")
        }
    }
    struct FailedIndexedRead;
    #[async_trait]
    impl ProposalValidator for FailedIndexedRead {
        async fn validate(&self, _: &Value) -> Result<Option<String>> {
            tokio::task::yield_now().await;
            bail!("indexed database read unavailable")
        }
    }

    #[tokio::test]
    async fn operational_validation_failure_stops_without_model_correction() {
        let model = Arc::new(SubmissionModel(AtomicUsize::new(0)));
        let agent = AgentLoop::new(model.clone(), AgentConfig::default());
        let finish = ToolDefinition {
            name: "submit".into(),
            description: "Return a proposal".into(),
            parameters: json!({"type":"object","properties":{},"required":[],"additionalProperties":false}),
        };
        let error = agent
            .run_async_validated(
                "Submit a proposal",
                "{}",
                &NoTools,
                finish,
                &FailedIndexedRead,
            )
            .await
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("indexed database read unavailable")
        );
        assert_eq!(model.0.load(Ordering::SeqCst), 1);
    }
}
