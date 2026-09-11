//! Installed Codex TUI -> optional production proxy -> local mock app-server.
#![cfg(unix)]
#[path = "support/mod.rs"]
#[allow(dead_code)]
mod support;

use agentix_codex::CodexProxy;
use serde_json::{Value, json};
use std::time::Duration;
use support::MockCodexAppServer;

const THREAD: &str = "01900000-0000-7000-8000-000000000001";

#[allow(clippy::too_many_lines)] // Keep one measured fixture lifetime together.
async fn sample(proxied: bool, paced: bool) -> Value {
    let server = MockCodexAppServer::start();
    server.enable_native_thread_ids().await;
    let directory = tempfile::tempdir().unwrap();
    let upstream = server.endpoint().address();
    let proxy = if proxied {
        Some(
            CodexProxy::bind(
                &format!("unix://{}", directory.path().join("proxy.sock").display()),
                &upstream,
            )
            .await
            .unwrap(),
        )
    } else {
        None
    };
    let endpoint = proxy
        .as_ref()
        .map_or(upstream.as_str(), CodexProxy::endpoint);
    let mut command = tokio::process::Command::new("python3");
    command
        .args([
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/support/native_cli_driver.py"
            ),
            endpoint,
            directory.path().to_str().unwrap(),
        ])
        .kill_on_drop(true);
    let driver = tokio::spawn(async move { command.output().await.unwrap() });
    let completion = tokio::time::timeout(Duration::from_secs(15), async {
        let mut previous = None;
        for warmup in [true, false] {
            let turn = loop {
                if let Some(turn) = server.latest_turn_id(THREAD).await
                    && previous.as_ref() != Some(&turn)
                {
                    break turn;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            };
            if warmup {
                server.complete_turn(THREAD, &turn, "BENCHMARK_READY").await;
            } else {
                let mut answer = String::from("BENCHMARK_FIRST\n");
                server.send_notification(json!({
                    "method": "item/started",
                    "params": {"threadId": THREAD, "turnId": turn,
                        "item": {"id": format!("{turn}_agent"), "type": "agentMessage", "text": ""}}
                })).await;
                server
                    .send_notification(json!({
                        "method": "item/agentMessage/delta",
                        "params": {"threadId": THREAD, "turnId": turn,
                            "itemId": format!("{turn}_agent"), "delta": answer}
                    }))
                    .await;
                if paced {
                    for _ in 0..32 {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        let chunk = format!("{}\n", "benchmark output ".repeat(4));
                        answer.push_str(&chunk);
                        server
                            .send_notification(json!({
                                "method": "item/agentMessage/delta",
                                "params": {"threadId": THREAD, "turnId": turn,
                                    "itemId": format!("{turn}_agent"), "delta": chunk}
                            }))
                            .await;
                    }
                }
                answer.push_str(" BENCHMARK_COMPLETE");
                server
                    .finish_turn(THREAD, &turn, &answer, " BENCHMARK_COMPLETE")
                    .await;
            }
            previous = Some(turn);
        }
    })
    .await;
    let output = driver.await.unwrap();
    assert!(
        completion.is_ok(),
        "methods={:?}\n{}",
        server.request_methods().await,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut result: Value = serde_json::from_slice(&output.stdout).unwrap();
    result["proxied"] = json!(proxied);
    result["paced"] = json!(paced);
    if let Some(proxy) = proxy {
        proxy.shutdown().await;
    }
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires installed Codex CLI and Python 3; local mock only"]
async fn native_cli_mock_round_trip() {
    let rounds: usize = std::env::var("PROXY_NATIVE_BENCH_ROUNDS")
        .unwrap_or_else(|_| "10".into())
        .parse()
        .unwrap();
    assert!(rounds > 0);
    for round in 0..rounds {
        for paced in [false, true] {
            for proxied in if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let mut result = sample(proxied, paced).await;
                result["round"] = json!(round);
                println!("NATIVE_CLI_BENCH {result}");
            }
        }
    }
}
