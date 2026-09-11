//! Proxy WebSocket authentication tests.
#![cfg(unix)]
use agentix_codex::{CodexProxy, ProxyOptions};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

fn options(value: Value) -> ProxyOptions {
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn non_loopback_requires_authentication() {
    let error = CodexProxy::bind("ws://0.0.0.0:0", "unix:///unused")
        .await
        .err()
        .expect("must reject unauthenticated public listener");
    assert!(error.to_string().contains("authentication"));
}

#[tokio::test]
#[allow(clippy::result_large_err)] // tungstenite requires HTTP responses in upgrade callbacks.
async fn capability_handshake_rejects_bad_credentials_before_upstream_connection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("token");
    std::fs::write(&path, "test-token-with-high-entropy-123456789\n").unwrap();
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = CodexProxy::bind_with_options(
        "ws://0.0.0.0:0",
        &format!("ws://{}", upstream.local_addr().unwrap()),
        &options(json!({"ws_auth":"capability-token","ws_token_file":path})),
    )
    .await
    .unwrap();
    let url = proxy.endpoint().replace("0.0.0.0", "127.0.0.1");
    for credential in [None, Some("Bearer wrong"), Some("Basic wrong")] {
        let mut request = url.as_str().into_client_request().unwrap();
        if let Some(value) = credential {
            request
                .headers_mut()
                .insert("Authorization", value.parse().unwrap());
        }
        let error = tokio_tungstenite::connect_async(request).await.unwrap_err();
        let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
            panic!("expected HTTP rejection")
        };
        assert_eq!(response.status(), 401);
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), upstream.accept())
            .await
            .is_err()
    );
    assert!(proxy.registry().snapshot().is_empty());
    let server = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_hdr_async(
            stream,
            |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                assert!(!request.headers().contains_key("authorization"));
                Ok(response)
            },
        )
        .await
        .unwrap();
        let Message::Text(text) = ws.next().await.unwrap().unwrap() else {
            panic!()
        };
        let request: Value = serde_json::from_str(&text).unwrap();
        ws.send(Message::text(
            json!({"id":request["id"],"result":{"thread":{"id":"remote"}}}).to_string(),
        ))
        .await
        .unwrap();
        while ws.next().await.is_some() {}
    });
    let mut request = url.as_str().into_client_request().unwrap();
    request.headers_mut().insert(
        "Authorization",
        "Bearer test-token-with-high-entropy-123456789"
            .parse()
            .unwrap(),
    );
    let (mut client, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    client
        .send(Message::text(r#"{"id":1,"method":"thread/start"}"#))
        .await
        .unwrap();
    client.next().await.unwrap().unwrap();
    assert_eq!(proxy.registry().snapshot()[0].sessions, ["remote"]);
    proxy.shutdown().await;
    server.await.unwrap();
}

#[tokio::test]
async fn invalid_auth_settings_fail_before_binding() {
    for value in [
        json!({"ws_auth":"capability-token"}),
        json!({"ws_token_sha256":"00"}),
        json!({"ws_auth":"capability-token","ws_token_sha256":"bad"}),
        json!({"ws_auth":"capability-token","ws_token_sha256":"00".repeat(32),"ws_token_file":"/unused"}),
        json!({"ws_auth":"signed-bearer-token","ws_shared_secret_file":"relative"}),
        json!({"ws_auth":"capability-token","ws_token_sha256":"00".repeat(32),"ws_issuer":"wrong-mode"}),
    ] {
        assert!(
            CodexProxy::bind_with_options("ws://127.0.0.1:0", "unix:///unused", &options(value))
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn signed_bearer_checks_signature_expiry_nbf_issuer_audience_and_algorithm() {
    let d = tempfile::tempdir().unwrap();
    let secret = "01234567890123456789012345678901ab";
    let path = d.path().join("secret");
    std::fs::write(&path, secret).unwrap();
    let proxy = CodexProxy::bind_with_options("ws://127.0.0.1:0", "unix:///unused", &options(json!({"ws_auth":"signed-bearer-token","ws_shared_secret_file":path,"ws_issuer":"issuer","ws_audience":"audience","ws_max_clock_skew_seconds":0}))).await.unwrap();
    let now = jsonwebtoken::get_current_timestamp();
    let valid = json!({"exp":now+300,"nbf":now-10,"iss":"issuer","aud":["another","audience"]});
    for (field, value) in [
        ("exp", json!(now - 10)),
        ("nbf", json!(now + 300)),
        ("iss", json!("wrong")),
        ("aud", json!("wrong")),
        ("exp", Value::Null),
    ] {
        let mut claims = valid.clone();
        claims[field] = value;
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();
        assert_eq!(handshake(proxy.endpoint(), &token).await, 401);
    }
    for (algorithm, key) in [
        (jsonwebtoken::Algorithm::HS256, "wrong-secret"),
        (jsonwebtoken::Algorithm::HS384, secret),
    ] {
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(algorithm),
            &valid,
            &jsonwebtoken::EncodingKey::from_secret(key.as_bytes()),
        )
        .unwrap();
        assert_eq!(handshake(proxy.endpoint(), &token).await, 401);
    }
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &valid,
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap();
    // Valid credentials reach the intentionally unavailable upstream.
    assert_eq!(handshake(proxy.endpoint(), &token).await, 502);
    proxy.shutdown().await;
}

async fn handshake(endpoint: &str, token: &str) -> u16 {
    let mut request = endpoint.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Authorization", format!("Bearer {token}").parse().unwrap());
    match tokio_tungstenite::connect_async(request).await {
        Ok((_, response)) => response.status().as_u16(),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => response.status().as_u16(),
        Err(error) => panic!("unexpected handshake error: {error}"),
    }
}

#[tokio::test]
async fn idle_connections_remain_registered_without_heartbeat() {
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = CodexProxy::bind_with_options(
        "ws://127.0.0.1:0",
        &format!("ws://{}", upstream.local_addr().unwrap()),
        &ProxyOptions::default(),
    )
    .await
    .unwrap();
    let server = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..2 {
            let (stream, _) = upstream.accept().await.unwrap();
            tasks.spawn(async move {
                let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                while let Some(Ok(frame)) = ws.next().await {
                    if let Message::Text(text) = frame {
                        let v: Value = serde_json::from_str(&text).unwrap();
                        ws.send(Message::text(json!({"id":v["id"],"result":{"thread":{"id":v["params"]["threadId"]}}}).to_string())).await.unwrap();
                    } else { let _ = ws.flush().await; }
                }
            });
        }
        while tasks.join_next().await.is_some() {}
    });
    let (mut silent, _) = tokio_tungstenite::connect_async(proxy.endpoint())
        .await
        .unwrap();
    silent
        .send(Message::text(
            r#"{"id":1,"method":"thread/resume","params":{"threadId":"silent"}}"#,
        ))
        .await
        .unwrap();
    silent.next().await.unwrap().unwrap();
    let (mut active, _) = tokio_tungstenite::connect_async(proxy.endpoint())
        .await
        .unwrap();
    active
        .send(Message::text(
            r#"{"id":1,"method":"thread/resume","params":{"threadId":"active"}}"#,
        ))
        .await
        .unwrap();
    active.next().await.unwrap().unwrap();
    let reader = tokio::spawn(async move {
        while active.next().await.is_some() {
            if active.flush().await.is_err() {
                break;
            }
        }
    });
    tokio::time::sleep(std::time::Duration::from_secs(45)).await;
    let sessions: Vec<_> = proxy
        .registry()
        .snapshot()
        .into_iter()
        .flat_map(|c| c.sessions)
        .collect();
    assert_eq!(sessions.len(), 2);
    assert!(sessions.contains(&"silent".to_owned()));
    assert!(sessions.contains(&"active".to_owned()));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), silent.next())
            .await
            .is_err()
    );
    silent
        .send(Message::Ping(vec![1, 2, 3].into()))
        .await
        .unwrap();
    assert_eq!(
        silent.next().await.unwrap().unwrap(),
        Message::Pong(vec![1, 2, 3].into())
    );
    drop(silent);
    proxy.shutdown().await;
    reader.await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn capability_digest_origin_and_duplicate_authorization_are_checked() {
    use sha2::{Digest, Sha256};
    let token = "another-test-token-01234567890123456789";
    let hash = format!("{:x}", Sha256::digest(token.as_bytes()));
    let proxy = CodexProxy::bind_with_options(
        "ws://127.0.0.1:0",
        "unix:///unused",
        &options(json!({"ws_auth":"capability-token","ws_token_sha256":hash})),
    )
    .await
    .unwrap();
    // Valid credentials reach the intentionally unavailable upstream.
    assert_eq!(handshake(proxy.endpoint(), token).await, 502);
    assert_eq!(handshake(proxy.endpoint(), "wrong").await, 401);
    for origin in [false, true] {
        let mut request = proxy.endpoint().into_client_request().unwrap();
        request
            .headers_mut()
            .insert("Authorization", format!("Bearer {token}").parse().unwrap());
        if origin {
            request
                .headers_mut()
                .insert("Origin", "https://example.com".parse().unwrap());
        } else {
            request
                .headers_mut()
                .append("Authorization", "Bearer wrong".parse().unwrap());
        }
        let error = tokio_tungstenite::connect_async(request).await.unwrap_err();
        let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
            panic!()
        };
        assert_eq!(response.status().as_u16(), if origin { 403 } else { 401 });
    }
    proxy.shutdown().await;
}
