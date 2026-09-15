// Run identically on the baseline and candidate; timings are informational.
#![cfg(unix)]
#[allow(dead_code)]
mod support;

use agentix_codex::{ClientRegistry, CodexClient};
use agentix_core::{AgentAdapter, SessionId};
use serde_json::json;
use std::{
    hint::black_box,
    path::Path,
    time::{Duration, Instant},
};
use support::{MockCodexAppServer, MockThread, MockTurn};

#[test]
#[ignore = "manual cross-version performance comparison"]
fn notification_observation_cost() {
    for owned in [false, true] {
        let registry = ClientRegistry::default();
        let connection = registry.connect(None);
        if owned {
            registry.client_message(connection, &json!({"id":1,"method":"thread/start"}));
            registry.server_message(connection, &json!({"id":1,"result":{"thread":{"id":"s"}}}));
        }
        for bytes in [64, 16384] {
            let frame = format!(
                r#"{{"method":"item/agentMessage/delta","params":{{"threadId":"s","delta":"{}"}}}}"#,
                "x".repeat(bytes)
            );
            let mut samples = Vec::new();
            for _ in 0..5 {
                let start = Instant::now();
                for _ in 0..20000 {
                    registry.server_frame(connection, black_box(&frame));
                }
                samples.push(start.elapsed().as_nanos() / 20000);
            }
            samples.sort_unstable();
            eprintln!(
                "PERF frame owned={owned} bytes={bytes} median_ns={} samples={samples:?}",
                samples[2]
            );
        }
    }
}

#[tokio::test]
#[ignore = "manual cross-version request amplification comparison, about 45 seconds"]
async fn notification_driven_request_volume() {
    for sessions in [1, 10] {
        let server = MockCodexAppServer::start();
        let registry = ClientRegistry::default();
        let mut connections = Vec::new();
        for i in 0..sessions {
            let id = format!("s{i}");
            server
                .add_thread(
                    MockThread::new(&id, "Session", "/work")
                        .with_turn(MockTurn::in_progress_with_output("turn", "input", "answer")),
                )
                .await;
            server.set_active_writer(&id).await;
            let connection = registry.connect(None);
            registry.client_message(connection, &json!({"id":1,"method":"thread/start"}));
            registry.server_message(connection, &json!({"id":1,"result":{"thread":{"id":id}}}));
            connections.push(connection);
        }
        let client = CodexClient::connect_with_registry(
            server.endpoint(),
            Path::new("codex"),
            Path::new("/tmp"),
            true,
            registry.clone(),
        )
        .await
        .unwrap();
        for i in 0..sessions {
            client
                .attach(&SessionId::new(format!("s{i}")))
                .await
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let before = server.request_methods().await.len();
        tokio::time::sleep(Duration::from_secs(11)).await;
        let idle = server.request_methods().await.len() - before;
        let before = server.request_methods().await.len();
        let frame = r#"{"method":"item/agentMessage/delta","params":{"threadId":"s0","turnId":"turn","itemId":"answer","delta":"x"}}"#;
        let start = Instant::now();
        for _ in 0..220 {
            registry.server_frame(connections[0], frame);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let methods = server.request_methods().await;
        let mut counts = std::collections::BTreeMap::new();
        for method in &methods[before..] {
            *counts.entry(method).or_insert(0) += 1;
        }
        eprintln!(
            "PERF requests sessions={sessions} background=true idle_11s={idle} notifications=220 stream_ms={} stream_requests={} methods={counts:?}",
            start.elapsed().as_millis(),
            methods.len() - before
        );
        drop(client);
    }
}

#[tokio::test]
async fn content_notifications_only_read_the_target_and_share_attached_history() {
    let server = MockCodexAppServer::start();
    let registry = ClientRegistry::default();
    let mut connections = Vec::new();
    for id in ["a", "b", "background"] {
        server
            .add_thread(
                MockThread::new(id, id, "/work")
                    .with_turn(MockTurn::in_progress_with_output("turn", "input", "answer")),
            )
            .await;
        server.set_active_writer(id).await;
        let connection = registry.connect(None);
        registry.client_message(connection, &json!({"id":1,"method":"thread/start"}));
        registry.server_message(connection, &json!({"id":1,"result":{"thread":{"id":id}}}));
        connections.push(connection);
    }
    let client = CodexClient::connect_with_registry(
        server.endpoint(),
        Path::new("codex"),
        Path::new("/tmp"),
        true,
        registry.clone(),
    )
    .await
    .unwrap();
    client.attach(&SessionId::new("a")).await.unwrap();
    client.attach(&SessionId::new("b")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = server.request_methods().await.len();
    registry.server_frame(
        connections[0],
        r#"{"method":"item/agentMessage/delta","params":{"threadId":"a","delta":"x"}}"#,
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        server.request_methods().await.len() - before,
        1,
        "one attached change must have one shared read"
    );
    let before = server.request_methods().await.len();
    registry.server_frame(
        connections[2],
        r#"{"method":"item/agentMessage/delta","params":{"threadId":"background","delta":"x"}}"#,
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        server.request_methods().await.len(),
        before,
        "background deltas must not query history"
    );
    server.complete_turn("background", "turn", "done").await;
    let before = server.request_methods().await.len();
    registry.server_frame(
        connections[2],
        r#"{"method":"turn/completed","params":{"threadId":"background"}}"#,
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        server.request_methods().await.len() - before,
        1,
        "only the completed background session is read"
    );
    let before = server.request_methods().await.len();
    for _ in 0..20 {
        registry.server_frame(
            connections[0],
            r#"{"method":"item/agentMessage/delta","params":{"threadId":"a","delta":"x"}}"#,
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        server.request_methods().await.len() - before <= 5,
        "sustained updates must be coalesced"
    );
}

#[test]
#[ignore = "manual timing regression; compare minimum samples to reduce scheduler noise"]
fn owned_notification_cost_does_not_scale_with_trailing_payload() {
    let registry = ClientRegistry::default();
    let connection = registry.connect(None);
    registry.client_message(connection, &json!({"id":1,"method":"thread/start"}));
    registry.server_message(connection, &json!({"id":1,"result":{"thread":{"id":"s"}}}));
    let measure = |size| {
        let frame = format!(
            r#"{{"method":"item/agentMessage/delta","params":{{"threadId":"s","delta":"{}"}}}}"#,
            "x".repeat(size)
        );
        (0..3)
            .map(|_| {
                let start = Instant::now();
                for _ in 0..10000 {
                    registry.server_frame(connection, black_box(&frame));
                }
                start.elapsed()
            })
            .min()
            .unwrap()
    };
    let small = measure(64);
    let large = measure(16384);
    eprintln!("PERF payload-after-route small={small:?} large={large:?}");
    assert!(
        large < small * 3,
        "routing must not scan trailing payload: {small:?} versus {large:?}"
    );
}

#[tokio::test]
async fn legacy_idle_observation_keeps_the_original_polling_rate() {
    let server = MockCodexAppServer::start();
    server
        .add_thread(
            MockThread::new("legacy", "Legacy", "/work")
                .with_turn(MockTurn::in_progress_with_output("turn", "input", "answer")),
        )
        .await;
    server.set_active_writer("legacy").await;
    let client = CodexClient::connect_with_background_turn_notifications(
        server.endpoint(),
        Path::new("codex"),
        Path::new("/tmp"),
        false,
    )
    .await
    .unwrap();
    client.attach(&SessionId::new("legacy")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let before = server.request_methods().await.len();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        server.request_methods().await.len(),
        before,
        "legacy must not poll at 4 Hz"
    );
}

#[test]
#[ignore = "manual routing performance regression for reordered JSON fields"]
fn owned_notification_cost_handles_payload_before_route() {
    let registry = ClientRegistry::default();
    let connection = registry.connect(None);
    registry.client_message(connection, &json!({"id":1,"method":"thread/start"}));
    registry.server_message(connection, &json!({"id":1,"result":{"thread":{"id":"s"}}}));
    let measure = |size| {
        let frame = json!({"method":"item/agentMessage/delta","params":{"delta":"x".repeat(size),"threadId":"s"}}).to_string();
        (0..3)
            .map(|_| {
                let start = Instant::now();
                for _ in 0..10000 {
                    registry.server_frame(connection, black_box(&frame));
                }
                start.elapsed()
            })
            .min()
            .unwrap()
    };
    let small = measure(64);
    let large = measure(16384);
    eprintln!("PERF payload-before-route small={small:?} large={large:?}");
    assert!(
        large < small * 3,
        "skipping earlier payload must use a fast scan: {small:?} versus {large:?}"
    );
}
