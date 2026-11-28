use std::sync::Arc;

use agentix_memory::{
    AgentConfig, EmbeddingConfig, HttpEmbedding, HttpModel, HttpProvider, Message, Model,
    ModelRequest, ProviderConfig, ProviderProtocol, ToolDefinition,
};
use serde_json::json;

#[path = "support/http.rs"]
mod http;

#[test]
fn provider_protocol_replaces_the_legacy_pair() {
    for protocol in [
        "openai_responses",
        "openai_chat_completions",
        "anthropic",
        "google",
        "ollama",
    ] {
        let parsed: ProviderConfig = toml::from_str(&format!(
            "protocol = {protocol:?}\nbase_url = \"https://example.invalid\""
        ))
        .unwrap();
        let serialized = toml::Value::try_from(parsed).unwrap();
        assert_eq!(serialized["protocol"].as_str(), Some(protocol));
        assert!(serialized.get("api").is_none());
    }
    for obsolete in ["protocol = \"openai\"", "api = \"responses\""] {
        assert!(
            toml::from_str::<ProviderConfig>(&format!(
                "{obsolete}\nbase_url = \"https://example.invalid\""
            ))
            .is_err()
        );
    }
}

#[tokio::test]
async fn native_models_replay_parallel_tool_calls() {
    let anthropic = json!([
        {"type":"thinking","thinking":"private","signature":"opaque"},
        {"type":"tool_use","id":"a","name":"memory_search","input":{"query":"offline"}},
        {"type":"tool_use","id":"b","name":"memory_search","input":{}}
    ]);
    let google = json!({"role":"model","parts":[
        {"text":"private","thought":true},
        {"functionCall":{"id":"a","name":"memory_search","args":{"query":"offline"}},"thoughtSignature":"opaque"},
        {"functionCall":{"name":"memory_search"}}
    ]});
    for (protocol, first, final_reply) in [
        (
            "anthropic",
            json!({"stop_reason":"tool_use","content":anthropic,"usage":{"input_tokens":10,"output_tokens":5}}),
            json!({"stop_reason":"end_turn","content":[{"type":"text","text":"done"}]}),
        ),
        (
            "google",
            json!({"candidates":[{"finishReason":"STOP","content":google}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":2,"thoughtsTokenCount":3}}),
            json!({"candidates":[{"finishReason":"STOP","content":{"role":"model","parts":[{"text":"done"}]}}]}),
        ),
    ] {
        let server = http::MockHttp::start(vec![(200, first), (200, final_reply)]).await;
        let connection = toml::from_str(&format!(
            "protocol = {protocol:?}\nbase_url = {:?}",
            server.url
        ))
        .unwrap();
        let model = HttpModel::new(
            Arc::new(HttpProvider::new(connection).unwrap()),
            AgentConfig {
                model: "native-model".into(),
                ..AgentConfig::default()
            },
        )
        .unwrap();
        let mut input = request();
        let reply = model.complete(&input).await.unwrap();
        assert_eq!(reply.calls.len(), 2);
        assert_eq!(reply.calls[0].id, "a");
        assert_eq!(reply.calls[1].arguments, json!({}));
        assert!(reply.text.is_empty());
        assert_eq!(
            (reply.usage.input_tokens, reply.usage.output_tokens),
            (10, 5)
        );
        input.history.push(Message::Model(reply.continuation));
        input
            .history
            .extend(reply.calls.into_iter().map(|call| Message::Tool {
                id: call.id,
                output: "[]".into(),
            }));
        assert_eq!(model.complete(&input).await.unwrap().text, "done");
        let requests = server.requests.lock().unwrap();
        let (path, body) = &requests[1];
        if protocol == "anthropic" {
            assert_eq!(path, "POST /messages HTTP/1.1");
            assert_eq!(body["messages"][1]["content"], anthropic);
            let results = &body["messages"][2]["content"];
            assert_eq!(results[0]["tool_use_id"], "a");
            assert_eq!(results[1]["tool_use_id"], "b");
            assert_eq!(body["tools"][0]["input_schema"], input.tools[0].parameters);
        } else {
            assert_eq!(path, "POST /models/native-model:generateContent HTTP/1.1");
            assert_eq!(body["contents"][1], google);
            let results = &body["contents"][2]["parts"];
            assert_eq!(results[0]["functionResponse"]["id"], "a");
            assert_eq!(results[1]["functionResponse"]["name"], "memory_search");
            assert!(results[1]["functionResponse"].get("id").is_none());
            assert_eq!(
                body["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"],
                input.tools[0].parameters
            );
        }
    }
}

fn connection(url: &str, protocol: ProviderProtocol) -> Arc<HttpProvider> {
    Arc::new(
        HttpProvider::new(ProviderConfig {
            base_url: url.into(),
            protocol,
            api_key_env: None,
            api_key_pass: None,
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
        connection(&server.url, ProviderProtocol::OpenaiResponses),
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
        ..AgentConfig::default()
    };
    let model = HttpModel::new(
        connection(&server.url, ProviderProtocol::OpenaiChatCompletions),
        config,
    )
    .unwrap();
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
        connection(&openai.url, ProviderProtocol::OpenaiResponses),
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
        connection(&server.url, ProviderProtocol::OpenaiResponses),
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
    for protocol in [
        ProviderProtocol::OpenaiResponses,
        ProviderProtocol::OpenaiChatCompletions,
    ] {
        let responses = protocol == ProviderProtocol::OpenaiResponses;
        for effort in [None, Some("low"), Some("high")] {
            let mut value = json!({"model":"configured-model"});
            if let Some(effort) = effort {
                value["reasoning_effort"] = json!(effort);
            }
            let config: AgentConfig = serde_json::from_value(value).unwrap();
            let reply = if responses {
                json!({"status":"completed","output":[]})
            } else {
                json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"done"}}]})
            };
            let server = http::MockHttp::start(vec![(200, reply)]).await;
            HttpModel::new(connection(&server.url, protocol), config)
                .unwrap()
                .complete(&request())
                .await
                .unwrap();
            let seen = server.requests.lock().unwrap();
            let body = &seen[0].1;
            if responses {
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
        connection(&server.url, ProviderProtocol::OpenaiResponses),
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

#[tokio::test]
async fn omitted_provider_api_routes_to_chat_completions() {
    let server = http::MockHttp::start(vec![(200, json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"done"}}]}))]).await;
    let provider =
        serde_json::from_value::<ProviderConfig>(json!({"base_url": server.url})).unwrap();
    let model = HttpModel::new(
        Arc::new(HttpProvider::new(provider).unwrap()),
        AgentConfig {
            model: "configured-chat-model".into(),
            ..AgentConfig::default()
        },
    )
    .unwrap();
    assert_eq!(model.complete(&request()).await.unwrap().text, "done");
    assert_eq!(
        server.requests.lock().unwrap()[0].0,
        "POST /chat/completions HTTP/1.1"
    );
}

#[tokio::test]
async fn google_embeddings_preserve_batch_order_and_configured_dimensions() {
    let server = http::MockHttp::start(vec![(
        200,
        json!({"embeddings":[{"values":[1.0,0.0]},{"values":[0.0,1.0]}]}),
    )])
    .await;
    let embedding = HttpEmbedding::new(
        connection(&server.url, ProviderProtocol::Google),
        EmbeddingConfig {
            model: "models/native-embedding".into(),
            dimensions: Some(2),
            ..EmbeddingConfig::default()
        },
    )
    .unwrap();
    assert_eq!(
        embedding
            .embed(&["first".into(), "second".into()])
            .await
            .unwrap(),
        vec![vec![1.0, 0.0], vec![0.0, 1.0]]
    );
    let seen = server.requests.lock().unwrap();
    let (path, body) = &seen[0];
    assert_eq!(
        path,
        "POST /models/native-embedding:batchEmbedContents HTTP/1.1"
    );
    assert_eq!(body["requests"][0]["model"], "models/native-embedding");
    assert_eq!(body["requests"][0]["content"]["parts"][0]["text"], "first");
    assert_eq!(body["requests"][1]["outputDimensionality"], 2);
}

#[test]
fn generation_protocols_sharing_openai_embeddings_keep_the_same_fingerprint() {
    let config = EmbeddingConfig::default();
    let base_url = "http://127.0.0.1:1/v1";
    let responses = HttpEmbedding::new(
        connection(base_url, ProviderProtocol::OpenaiResponses),
        config.clone(),
    )
    .unwrap();
    let chat = HttpEmbedding::new(
        connection(base_url, ProviderProtocol::OpenaiChatCompletions),
        config,
    )
    .unwrap();
    assert_eq!(responses.fingerprint(), chat.fingerprint());
}

#[test]
fn memory_capabilities_are_checked_before_http_requests() {
    let base_url = "https://example.invalid";
    assert!(
        HttpModel::new(
            connection(base_url, ProviderProtocol::Ollama),
            AgentConfig::default()
        )
        .err()
        .unwrap()
        .to_string()
        .contains("only for embeddings")
    );
    assert!(
        HttpEmbedding::new(
            connection(base_url, ProviderProtocol::Anthropic),
            EmbeddingConfig::default()
        )
        .err()
        .unwrap()
        .to_string()
        .contains("does not provide an embedding API")
    );
    assert!(
        HttpModel::new(
            connection(base_url, ProviderProtocol::Google),
            AgentConfig {
                model: "models/../test".into(),
                ..AgentConfig::default()
            }
        )
        .err()
        .unwrap()
        .to_string()
        .contains("invalid: Gemini model name")
    );
}
