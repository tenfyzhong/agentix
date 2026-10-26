//! Opt-in real Unix RPC comparison; no external provider or developer session.
#![cfg(unix)]
#[allow(dead_code)]
mod support;
use agentix_codex::CodexClient;
use agentix_metrics::{Metrics, Service};
use std::{
    process::Command,
    time::{Duration, Instant},
};

#[tokio::test]
#[ignore = "isolated child of application_metrics_rpc_benchmark"]
async fn application_metrics_rpc_sample() {
    let mode = std::env::var("AGENTIX_METRICS_BENCH_MODE").expect("run the parent benchmark");
    let metrics = if mode == "enabled" {
        Metrics::new(Service::Agentix, "benchmark")
    } else {
        Metrics::default()
    };
    agentix_metrics::install(metrics.clone()).unwrap();
    let exporter = if metrics.enabled() {
        Some(metrics.serve("127.0.0.1:0".parse().unwrap()).await.unwrap())
    } else {
        None
    };
    let completed_scrapes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let scraper = exporter.as_ref().map(|exporter| {
        let completed_scrapes = completed_scrapes.clone();
        let address = exporter.address();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
                stream
                    .write_all(
                        b"GET /metrics HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
                let mut response = Vec::new();
                stream.read_to_end(&mut response).await.unwrap();
                assert!(response.starts_with(b"HTTP/1.1 200"));
                completed_scrapes.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
    });
    let server = support::MockCodexAppServer::start();
    let client = CodexClient::connect(server.endpoint()).await.unwrap();
    for _ in 0..1000 {
        client.list_sessions(None, 10).await.unwrap();
    }
    let mut latencies = Vec::with_capacity(100_000);
    let total = Instant::now();
    for _ in 0..100_000 {
        let start = Instant::now();
        client.list_sessions(None, 10).await.unwrap();
        latencies.push(start.elapsed().as_nanos());
    }
    let elapsed = total.elapsed().as_secs_f64();
    latencies.sort_unstable();
    println!(
        "METRICS_SAMPLE {}",
        serde_json::json!({"mode": mode, "operations": 100_000, "scrapes": completed_scrapes.load(std::sync::atomic::Ordering::Relaxed), "seconds": elapsed, "p95_ns": latencies[95_000], "requests_per_second": 100_000.0 / elapsed})
    );
    if let Some(scraper) = scraper {
        scraper.abort();
        assert!(scraper.await.unwrap_err().is_cancelled());
        assert!(completed_scrapes.load(std::sync::atomic::Ordering::Relaxed) >= 2);
    }
    client.shutdown().await.unwrap();
}

#[test]
#[ignore = "manual paired application RPC benchmark; use --release --nocapture"]
fn application_metrics_rpc_benchmark() {
    let mut samples = Vec::new();
    for round in 0..7 {
        let modes = if round % 2 == 0 {
            ["disabled", "enabled"]
        } else {
            ["enabled", "disabled"]
        };
        for mode in modes {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "application_metrics_rpc_sample",
                    "--ignored",
                    "--nocapture",
                ])
                .env("AGENTIX_METRICS_BENCH_MODE", mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8(output.stdout).unwrap();
            let line = text
                .lines()
                .find_map(|line| line.strip_prefix("METRICS_SAMPLE "))
                .unwrap();
            samples.push(serde_json::from_str::<serde_json::Value>(line).unwrap());
        }
    }
    println!(
        "METRICS_BENCHMARK {}",
        serde_json::json!({"profile": if cfg!(debug_assertions) { "debug" } else { "release" }, "samples": samples, "scope": "local Codex list_sessions RPC with exporter scraped at 10 Hz; no model, IM or Cloud"})
    );
}
