use agentix_memory::{
    AgentConfig, HttpModel, HttpProvider, Model, ModelApi, ModelRequest, ProviderConfig,
    ProviderProtocol,
};
use agentix_metrics::{Metrics, Service};
use serde_json::json;
use std::sync::Arc;
#[allow(dead_code)]
#[path = "support/http.rs"]
mod http;
#[tokio::test]
async fn provider_metrics_count_real_usage_without_inventing_missing_tokens() {
    let metrics = Metrics::new(Service::Taskix, "test");
    agentix_metrics::install(metrics.clone()).unwrap();
    let server = http::MockHttp::start(vec![
        (
            200,
            json!({"status":"completed","output":[],"usage":{"input_tokens":2,"output_tokens":1}}),
        ),
        (200, json!({"status":"completed","output":[]})),
    ])
    .await;
    let provider = Arc::new(
        HttpProvider::new(ProviderConfig {
            protocol: ProviderProtocol::Openai,
            api: ModelApi::Responses,
            base_url: server.url.clone(),
            api_key_env: None,
            max_in_flight: 2,
        })
        .unwrap(),
    );
    let model = HttpModel::new(provider, AgentConfig::default()).unwrap();
    let request = ModelRequest {
        instructions: "private".into(),
        history: vec![],
        tools: vec![],
    };
    model.complete(&request).await.unwrap();
    model.complete(&request).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = agentix_memory::MemoryStore::open(&dir.path().join("memory.db"))
        .await
        .unwrap();
    let projection =
        agentix_memory::MemoryProjection::new(store, dir.path(), std::path::Path::new("Memory"))
            .unwrap();
    assert!(projection.sync_pending("private", "", 0).await.is_err());
    let text = metrics.encode().unwrap();
    assert!(text.contains("component=\"memory\",operation=\"projection\",outcome=\"error\"} 1"));
    assert!(
        text.contains("taskix_tokens_total{source=\"memory_provider\",kind=\"input\"} 2"),
        "{text}"
    );
    assert!(text.contains("taskix_tokens_total{source=\"memory_provider\",kind=\"output\"} 1"));
    assert!(text.contains("component=\"model\",operation=\"request\",outcome=\"success\"} 2"));
    assert!(!text.contains("cached_input"));
    assert!(!text.contains("private"));
}
