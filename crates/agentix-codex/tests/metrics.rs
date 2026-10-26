// Exercise metrics through the Unix RPC transport.
#![cfg(unix)]
#[allow(dead_code)]
mod support;
use agentix_codex::CodexClient;
use agentix_metrics::{Metrics, Service};

#[tokio::test]
async fn real_rpc_path_records_application_metrics_without_request_content() {
    let metrics = Metrics::new(Service::Agentix, "test");
    agentix_metrics::install(metrics.clone()).unwrap();
    let server = support::MockCodexAppServer::start();
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    client.list_sessions(None, 10).await.unwrap();
    let text = metrics.encode().unwrap();
    assert!(
        text.contains("component=\"codex\",operation=\"rpc\",outcome=\"success\"} 1"),
        "{text}"
    );
    assert!(!text.contains("thread/list"));
    client.shutdown().await.unwrap();
}
