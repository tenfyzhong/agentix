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
