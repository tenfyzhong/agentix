use std::sync::Arc;

use agentix_memory::{
    AgentConfig, EmbeddingConfig, HttpEmbedding, HttpModel, HttpProvider, Message, Model, ModelApi,
    ModelRequest, ProviderConfig, ProviderProtocol, ToolDefinition,
};
use serde_json::json;

#[path = "support/http.rs"]
mod http;

fn connection(url: &str, protocol: ProviderProtocol) -> Arc<HttpProvider> {
    Arc::new(
        HttpProvider::new(ProviderConfig {
            base_url: url.into(),
            protocol,
            api_key_env: None,
            max_in_flight: 4,
        })
        .unwrap(),
    )
}

fn request() -> ModelRequest {
    ModelRequest {
        instructions: "Use scoped tools".into(),
        history: vec![Message::User("Decide what to retain".into())],
        tools: vec![ToolDefinition {
            name: "memory_search".into(),
            description: "Search this project".into(),
            parameters: json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}),
        }],
    }
}

#[tokio::test]
async fn responses_preserves_reasoning_and_tool_outputs_without_hidden_session_state() {
    let reasoning =
        json!({"type":"reasoning","id":"rs1","summary":[],"encrypted_content":"opaque"});
    let call = json!({"type":"function_call","call_id":"call1","name":"memory_search","arguments":"{\"query\":\"offline\"}"});
    let server=http::MockHttp::start(vec![(200,json!({"status":"completed","output":[reasoning,call],"usage":{"input_tokens":10,"output_tokens":5}})),
        (200,json!({"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}]})),
        (200,json!({"status":"completed","output":[]}))]).await;
    let model = HttpModel::new(
        connection(&server.url, ProviderProtocol::Openai),
        AgentConfig::default(),
    )
    .unwrap();
    let mut input = request();
    let reply = model.complete(&input).await.unwrap();
    assert_eq!(reply.calls[0].id, "call1");
    assert_eq!(reply.usage.input_tokens, 10);
    input.history.push(Message::Model(reply.continuation));
    input.history.push(Message::Tool {
        id: "call1".into(),
        output: "[]".into(),
    });
    assert_eq!(model.complete(&input).await.unwrap().text, "done");
    model.complete(&request()).await.unwrap();
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests[0].0, "POST /responses HTTP/1.1");
    assert_eq!(requests[0].1["model"], "gpt-6-astra");
    assert_eq!(requests[0].1["store"], false);
    assert_eq!(requests[0].1["tool_choice"], "required");
    assert_eq!(requests[1].1["input"][1], reasoning);
    assert_eq!(requests[1].1["input"][3]["call_id"], "call1");
    assert_eq!(requests[2].1["input"].as_array().unwrap().len(), 1);
    assert!(requests[2].1.get("previous_response_id").is_none());
}

#[tokio::test]
async fn chat_completions_maps_calls_without_changing_the_model() {
    let server=http::MockHttp::start(vec![(200,json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"memory_search","arguments":"{\"query\":\"offline\"}"}}]}}]}))]).await;
    let config = AgentConfig {
        model: "configured-chat-model".into(),
        api: ModelApi::ChatCompletions,
        ..AgentConfig::default()
    };
    let model = HttpModel::new(connection(&server.url, ProviderProtocol::Openai), config).unwrap();
    let reply = model.complete(&request()).await.unwrap();
    assert_eq!(reply.calls[0].name, "memory_search");
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests[0].0, "POST /chat/completions HTTP/1.1");
    assert_eq!(requests[0].1["model"], "configured-chat-model");
    assert_eq!(
        requests[0].1["tools"][0]["function"]["name"],
        "memory_search"
    );
}

#[tokio::test]
async fn embeddings_preserve_batch_order_and_ollama_does_not_silently_truncate() {
    let openai = http::MockHttp::start(vec![(
        200,
        json!({"data":[{"index":1,"embedding":[0.0,1.0]},{"index":0,"embedding":[1.0,0.0]}]}),
    )])
    .await;
    let settings = EmbeddingConfig {
        enabled: true,
        model: "configured-embedding".into(),
        dimensions: Some(2),
        ..EmbeddingConfig::default()
    };
    let client = HttpEmbedding::new(
        connection(&openai.url, ProviderProtocol::Openai),
        settings.clone(),
    )
    .unwrap();
    let vectors = client.embed(&["one".into(), "two".into()]).await.unwrap();
    assert_eq!(vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
    assert_eq!(openai.requests.lock().unwrap()[0].1["dimensions"], 2);
    let ollama = http::MockHttp::start(vec![(200, json!({"embeddings":[[1.0,0.0]]}))]).await;
    let client =
        HttpEmbedding::new(connection(&ollama.url, ProviderProtocol::Ollama), settings).unwrap();
    client.embed(&["one".into()]).await.unwrap();
    let requests = ollama.requests.lock().unwrap();
    assert_eq!(requests[0].0, "POST /api/embed HTTP/1.1");
    assert_eq!(requests[0].1["truncate"], false);
}

#[tokio::test]
async fn provider_errors_and_partial_generation_do_not_become_success() {
    let server = http::MockHttp::start(vec![
        (
            429,
            json!({"error":{"message":"sensitive-provider-detail"}}),
        ),
        (200, json!({"status":"incomplete","output":[]})),
    ])
    .await;
    let model = HttpModel::new(
        connection(&server.url, ProviderProtocol::Openai),
        AgentConfig::default(),
    )
    .unwrap();
    let error = model.complete(&request()).await.unwrap_err().to_string();
    assert!(error.contains("429"));
    assert!(!error.contains("sensitive-provider-detail"));
    assert!(model.complete(&request()).await.is_err());
}

#[tokio::test]
async fn reasoning_effort_is_optional_and_mapped_to_each_api() {
    for api in ["responses", "chat_completions"] {
        for effort in [None, Some("low"), Some("high")] {
            let mut value = json!({"api":api,"model":"configured-model"});
            if let Some(effort) = effort {
                value["reasoning_effort"] = json!(effort);
            }
            let config: AgentConfig = serde_json::from_value(value).unwrap();
            let reply = if api == "responses" {
                json!({"status":"completed","output":[]})
            } else {
                json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"done"}}]})
            };
            let server = http::MockHttp::start(vec![(200, reply)]).await;
            HttpModel::new(connection(&server.url, ProviderProtocol::Openai), config)
                .unwrap()
                .complete(&request())
                .await
                .unwrap();
            let seen = server.requests.lock().unwrap();
            let body = &seen[0].1;
            if api == "responses" {
                assert_eq!(body["reasoning"]["effort"], json!(effort));
                assert!(body.get("reasoning_effort").is_none());
                if effort.is_none() {
                    assert!(body.get("reasoning").is_none());
                }
            } else {
                assert_eq!(body["reasoning_effort"], json!(effort));
                assert!(body.get("reasoning").is_none());
                if effort.is_none() {
                    assert!(body.get("reasoning_effort").is_none());
                }
            }
        }
    }
}

#[test]
fn reasoning_effort_rejects_invalid_configuration() {
    for value in [json!(""), json!("loow"), json!(42)] {
        assert!(serde_json::from_value::<AgentConfig>(json!({"reasoning_effort":value})).is_err());
    }
}

#[tokio::test]
async fn provider_location_error_is_actionable_without_exposing_arbitrary_response_content() {
    let server = http::MockHttp::start(vec![
        (400, json!({"error":{"code":400,"status":"FAILED_PRECONDITION","message":"User location is not supported for the API use."}})),
        (400, json!({"error":{"type":"invalid_request_error","param":"tools[0].function.parameters","message":"private conversation and secret"}})),
    ]).await;
    let model = HttpModel::new(
        connection(&server.url, ProviderProtocol::Openai),
        AgentConfig::default(),
    )
    .unwrap();
    let error = model.complete(&request()).await.unwrap_err().to_string();
    assert!(error.contains("400"));
    assert!(error.contains("FAILED_PRECONDITION"));
    assert!(error.contains("location"));
    let error = model.complete(&request()).await.unwrap_err().to_string();
    assert!(error.contains("invalid_request_error"));
    assert!(error.contains("tools[0].function.parameters"));
    assert!(!error.contains("private conversation"));
}
