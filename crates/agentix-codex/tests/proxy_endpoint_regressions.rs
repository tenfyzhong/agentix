//! Proxy endpoint ownership regression tests.
#![cfg(unix)]
// Endpoint regressions shared by the proxy and its internal upstream client.
use agentix_codex::{CodexClient, CodexEndpoint, CodexProxy, UpstreamServer};
use futures_util::{SinkExt, StreamExt};
use std::{path::Path, time::Duration};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn internal_client_connects_to_ipv6_upstream() {
    let listener = TcpListener::bind("[::1]:0").await.unwrap();
    let endpoint =
        CodexEndpoint::parse(&format!("ws://{}", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move {
        let mut ws = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
            .await
            .unwrap();
        let request = ws.next().await.unwrap().unwrap();
        let request: serde_json::Value = serde_json::from_str(request.to_text().unwrap()).unwrap();
        ws.send(Message::text(
            serde_json::json!({"id":request["id"],"result":{"userAgent":"test"}}).to_string(),
        ))
        .await
        .unwrap();
        let _ = ws.next().await;
    });
    let result = tokio::time::timeout(Duration::from_secs(2), CodexClient::connect(endpoint)).await;
    server.abort();
    let result = result.expect("internal connection timed out");
    assert!(
        result.is_ok(),
        "IPv6 internal client failed: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn ipv6_loopback_upstream_attempts_local_start() {
    let listener = TcpListener::bind("[::1]:0").await.unwrap();
    let endpoint =
        CodexEndpoint::parse(&format!("ws://{}", listener.local_addr().unwrap())).unwrap();
    drop(listener);
    let d = tempfile::tempdir().unwrap();
    let error = UpstreamServer::ensure(&endpoint, &d.path().join("absent-codex"))
        .await
        .err()
        .expect("nonexistent executable must fail");
    assert!(
        error
            .to_string()
            .contains("start Codex upstream app-server"),
        "{error:#}"
    );
}

#[tokio::test]
async fn explicit_default_ws_port_is_not_reported_missing() {
    // No upstream is contacted. Binding failure (in use/permission) is legitimate;
    // rejecting the explicitly provided port as missing is not.
    let result = CodexProxy::bind("ws://127.0.0.1:80", "unix:///unused-upstream.sock").await;
    match result {
        Ok(proxy) => proxy.shutdown().await,
        Err(error) => assert!(!error.to_string().contains("missing port"), "{error:#}"),
    }
}

#[tokio::test]
async fn proxy_refuses_same_socket_through_parent_symlink() {
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real");
    std::fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, d.path().join("alias")).unwrap();
    let listen = format!("unix://{}", real.join("proxy.sock").display());
    let upstream = format!("unix://{}", d.path().join("alias/proxy.sock").display());
    let result = CodexProxy::bind(&listen, &upstream).await;
    let accepted = result.is_ok();
    if let Ok(proxy) = result {
        proxy.shutdown().await;
    }
    assert!(
        !accepted,
        "proxy accepted an upstream resolving to its own socket"
    );
    assert!(!Path::new(&real.join("proxy.sock")).exists());
}

#[tokio::test]
async fn dangling_upstream_symlink_to_proxy_is_rejected_and_cleaned() {
    let d = tempfile::tempdir().unwrap();
    let socket = d.path().join("proxy.sock");
    let alias = d.path().join("upstream.sock");
    std::os::unix::fs::symlink(&socket, &alias).unwrap();
    let result = CodexProxy::bind(
        &format!("unix://{}", socket.display()),
        &format!("unix://{}", alias.display()),
    )
    .await;
    assert!(
        result
            .err()
            .unwrap()
            .is::<agentix_codex::ProxyConfigError>()
    );
    assert!(!socket.exists());
    assert!(alias.symlink_metadata().unwrap().file_type().is_symlink());
}

#[tokio::test]
async fn runtime_rejects_alias_in_missing_directory_before_launch() {
    let d = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(d.path(), d.path().join("alias")).unwrap();
    let socket = d.path().join("new/proxy.sock");
    let upstream = CodexEndpoint::from_socket_path(&d.path().join("alias/new/proxy.sock")).unwrap();
    let result = CodexClient::connect_with_proxy(
        &format!("unix://{}", socket.display()),
        upstream,
        &d.path().join("must-not-launch"),
        d.path(),
        false,
    )
    .await;
    assert!(
        result
            .err()
            .unwrap()
            .is::<agentix_codex::ProxyConfigError>()
    );
    assert!(!socket.exists());
}

#[tokio::test]
async fn different_sockets_in_aliased_directory_are_allowed() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("real")).unwrap();
    std::os::unix::fs::symlink(d.path().join("real"), d.path().join("alias")).unwrap();
    let proxy = CodexProxy::bind(
        &format!("unix://{}", d.path().join("real/proxy.sock").display()),
        &format!("unix://{}", d.path().join("alias/upstream.sock").display()),
    )
    .await
    .unwrap();
    proxy.shutdown().await;
}
