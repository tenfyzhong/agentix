// Exercise the non-Unix implementation on every CI host to catch API drift.
// The compatibility methods must retain the Unix client's async signatures.
#[allow(clippy::unused_async)]
#[path = "../src/client_unsupported.rs"]
mod unsupported;

use std::path::Path;

use agentix_codex::CodexEndpoint;
use serde_json::json;
use unsupported::CodexClient;

#[tokio::test]
async fn unsupported_client_constructors_report_transport_unavailability() {
    let endpoint =
        CodexEndpoint::from_socket_path(&std::env::temp_dir().join("agentix-codex-test.sock"))
            .unwrap();
    let command = Path::new("codex");
    let directory = Path::new("~");
    let mut errors = vec![
        CodexClient::connect(endpoint.clone()).await.unwrap_err(),
        CodexClient::connect_with_command(endpoint.clone(), command)
            .await
            .unwrap_err(),
        CodexClient::connect_with_command_and_rmux_directory(endpoint.clone(), command, directory)
            .await
            .unwrap_err(),
    ];
    for enabled in [false, true] {
        errors.push(
            CodexClient::connect_with_background_turn_notifications(
                endpoint.clone(),
                command,
                directory,
                enabled,
            )
            .await
            .unwrap_err(),
        );
    }
    errors.push(
        CodexClient
            .request("thread/list", json!({}))
            .await
            .unwrap_err(),
    );
    for error in errors {
        assert_eq!(
            error.to_string(),
            "the Codex app-server Unix socket transport is unavailable on this platform"
        );
    }
}
