use std::sync::{Arc, Mutex};

use agentix_memory::{
    AgentConfig, AgentLoop, Message, Model, ModelReply, ModelRequest, TokenUsage, ToolCall,
    ToolDefinition, ToolSet,
};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};

struct Scripted {
    requests: Mutex<Vec<ModelRequest>>,
    endless: bool,
}

#[async_trait]
impl Model for Scripted {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelReply> {
        self.requests.lock().unwrap().push(request.clone());
        let initial = request.history.len() == 1 || self.endless;
        let call = if initial {
            ToolCall {
                id: "read1".into(),
                name: "lookup".into(),
                arguments: json!({"query":"decision"}),
            }
        } else {
            ToolCall {
                id: "finish1".into(),
                name: "submit".into(),
                arguments: json!({"items":[]}),
            }
        };
        Ok(ModelReply {
            continuation: json!([{"type":"function_call","call_id":call.id,"name":call.name,"arguments":call.arguments.to_string()}]),
            calls: vec![call],
            text: String::new(),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        })
    }
}

struct ReadTools {
    calls: Mutex<usize>,
    large: bool,
}
#[async_trait]
impl ToolSet for ReadTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "lookup".into(),
            description: "Read scoped data".into(),
            parameters: json!({"type":"object"}),
        }]
    }
    async fn execute(&self, name: &str, _arguments: Value) -> Result<Value> {
        assert_eq!(name, "lookup");
        *self.calls.lock().unwrap() += 1;
        Ok(if self.large {
            json!("x".repeat(8192))
        } else {
            json!({"evidence":"read"})
        })
    }
}
fn submit() -> ToolDefinition {
    ToolDefinition {
        name: "submit".into(),
        description: "Return proposals".into(),
        parameters: json!({"type":"object"}),
    }
}

#[tokio::test]
async fn each_loop_has_fresh_context_and_preserves_only_its_own_tool_results() {
    let model = Arc::new(Scripted {
        requests: Mutex::new(Vec::new()),
        endless: false,
    });
    let tools = ReadTools {
        calls: Mutex::new(0),
        large: false,
    };
    let agent = AgentLoop::new(model.clone(), AgentConfig::default());
    let a = agent
        .run("Extract decisions", "project-a", &tools, submit())
        .await
        .unwrap();
    let b = agent
        .run("Extract decisions", "project-b", &tools, submit())
        .await
        .unwrap();
    assert_eq!(a.value, json!({"items":[]}));
    assert_eq!(b.value, a.value);
    assert_eq!(a.usage.input_tokens, 20);
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert!(matches!(&requests[2].history[0],Message::User(s) if s=="project-b"));
    assert_eq!(requests[2].history.len(), 1);
    assert_eq!(requests[1].history.len(), 3);
    assert_eq!(*tools.calls.lock().unwrap(), 2);
}

#[tokio::test]
async fn runaway_tools_and_oversized_context_fail_without_unbounded_calls() {
    let model = Arc::new(Scripted {
        requests: Mutex::new(Vec::new()),
        endless: true,
    });
    let tools = ReadTools {
        calls: Mutex::new(0),
        large: false,
    };
    let config = AgentConfig {
        max_tool_calls: 2,
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(model.clone(), config);
    assert!(
        agent
            .run("Extract", "source", &tools, submit())
            .await
            .is_err()
    );
    assert_eq!(*tools.calls.lock().unwrap(), 2);
    let tools = ReadTools {
        calls: Mutex::new(0),
        large: true,
    };
    let config = AgentConfig {
        max_context_bytes: 4096,
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(model, config);
    assert!(
        agent
            .run("Extract", "source", &tools, submit())
            .await
            .is_err()
    );
    assert_eq!(*tools.calls.lock().unwrap(), 1);
}

struct NeverCompletes;
#[async_trait]
impl Model for NeverCompletes {
    async fn complete(&self, _request: &ModelRequest) -> Result<ModelReply> {
        std::future::pending().await
    }
}
#[tokio::test]
async fn stalled_provider_is_cancelled_by_the_whole_task_deadline() {
    let tools = ReadTools {
        calls: Mutex::new(0),
        large: false,
    };
    let agent = AgentLoop::new(
        Arc::new(NeverCompletes),
        AgentConfig {
            task_timeout_seconds: 1,
            ..AgentConfig::default()
        },
    );
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        agent.run("Extract", "source", &tools, submit()),
    )
    .await
    .unwrap()
    .err()
    .unwrap();
    assert!(error.to_string().contains("timeout"));
    assert_eq!(*tools.calls.lock().unwrap(), 0);
}

struct FailingTools {
    recoverable: bool,
    calls: Mutex<usize>,
}
#[async_trait]
impl ToolSet for FailingTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        ReadTools {
            calls: Mutex::new(0),
            large: false,
        }
        .definitions()
    }
    async fn execute(&self, _name: &str, _arguments: Value) -> Result<Value> {
        *self.calls.lock().unwrap() += 1;
        if self.recoverable {
            Err(agentix_memory::ToolInputError("query must be nonempty".into()).into())
        } else {
            anyhow::bail!("storage unavailable")
        }
    }
}

#[tokio::test]
async fn invalid_tool_input_is_returned_to_model_without_hiding_storage_failure() {
    let model = Arc::new(Scripted {
        requests: Mutex::new(Vec::new()),
        endless: false,
    });
    let tools = FailingTools {
        recoverable: true,
        calls: Mutex::new(0),
    };
    let result = AgentLoop::new(model.clone(), AgentConfig::default())
        .run("Extract", "source", &tools, submit())
        .await
        .unwrap();
    assert_eq!(result.steps, 2);
    {
        let requests = model.requests.lock().unwrap();
        let Message::Tool { id, output } = &requests[1].history[2] else {
            panic!("missing tool error feedback")
        };
        assert_eq!(id, "read1");
        assert_eq!(
            serde_json::from_str::<Value>(output).unwrap()["error"],
            "query must be nonempty"
        );
    }
    let tools = FailingTools {
        recoverable: false,
        calls: Mutex::new(0),
    };
    let error = AgentLoop::new(model, AgentConfig::default())
        .run("Extract", "source", &tools, submit())
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("storage unavailable"));
    assert_eq!(*tools.calls.lock().unwrap(), 1);
}

#[tokio::test]
async fn repeated_invalid_tool_input_still_exhausts_the_call_budget() {
    let model = Arc::new(Scripted {
        requests: Mutex::new(Vec::new()),
        endless: true,
    });
    let tools = FailingTools {
        recoverable: true,
        calls: Mutex::new(0),
    };
    let config = AgentConfig {
        max_tool_calls: 2,
        ..AgentConfig::default()
    };
    let error = AgentLoop::new(model, config)
        .run("Extract", "source", &tools, submit())
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("tool budget"));
    assert_eq!(*tools.calls.lock().unwrap(), 2);
}

struct CorrectingSubmission {
    requests: Mutex<Vec<ModelRequest>>,
    always_empty: bool,
}
#[async_trait]
impl Model for CorrectingSubmission {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelReply> {
        let mut requests = self.requests.lock().unwrap();
        requests.push(request.clone());
        let n = requests.len();
        Ok(ModelReply {
            continuation: json!([]),
            calls: if n == 1 || self.always_empty {
                vec![]
            } else {
                vec![ToolCall {
                    id: format!("finish-{n}"),
                    name: "submit".into(),
                    arguments: json!({"valid":n >= 3}),
                }]
            },
            text: String::new(),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 2,
            },
        })
    }
}

#[tokio::test]
async fn empty_reply_and_invalid_submission_are_corrected_within_original_budget() {
    let model = Arc::new(CorrectingSubmission {
        requests: Mutex::new(vec![]),
        always_empty: false,
    });
    let tools = ReadTools {
        calls: Mutex::new(0),
        large: false,
    };
    let result = AgentLoop::new(model.clone(), AgentConfig::default())
        .run_validated("Extract", "source", &tools, submit(), &|value| {
            anyhow::ensure!(value["valid"] == true, "current source evidence required");
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(result.steps, 3);
    assert_eq!(result.tool_calls, 2);
    assert_eq!(result.usage.input_tokens, 30);
    let requests = model.requests.lock().unwrap();
    assert!(
        matches!(requests[1].history.last(), Some(Message::User(text)) if text.contains("submission"))
    );
    assert!(
        matches!(requests[2].history.last(), Some(Message::Tool { output, .. }) if output.contains("current source evidence required"))
    );
}

#[tokio::test]
async fn repeated_empty_replies_exhaust_the_step_budget() {
    let model = Arc::new(CorrectingSubmission {
        requests: Mutex::new(vec![]),
        always_empty: true,
    });
    let tools = ReadTools {
        calls: Mutex::new(0),
        large: false,
    };
    let config = AgentConfig {
        max_steps: 3,
        ..AgentConfig::default()
    };
    let error = AgentLoop::new(model.clone(), config)
        .run("Extract", "source", &tools, submit())
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("step budget"));
    assert_eq!(model.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn repeated_invalid_submissions_do_not_reset_the_step_budget() {
    let model = Arc::new(CorrectingSubmission {
        requests: Mutex::new(vec![]),
        always_empty: false,
    });
    let tools = ReadTools {
        calls: Mutex::new(0),
        large: false,
    };
    let config = AgentConfig {
        max_steps: 3,
        ..AgentConfig::default()
    };
    let error = AgentLoop::new(model.clone(), config)
        .run_validated("Extract", "source", &tools, submit(), &|_| {
            anyhow::bail!("unsupported submission")
        })
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("step budget"));
    assert_eq!(model.requests.lock().unwrap().len(), 3);
}
