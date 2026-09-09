//! Reusable in-process transport benchmark; no native CLI or provider is launched.
use agentix_bridge::{BridgeAdapter, BridgeHub, BridgeKind, wire};
use agentix_domain::AgentAdapter;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn main() {
    tokio::runtime::Runtime::new().unwrap().block_on(run());
}
async fn run() {
    let hub = Arc::new(BridgeHub::new());
    let root = std::env::temp_dir();
    let adapter = BridgeAdapter::new(BridgeKind::Pi, hub.clone(), &root);
    let count = 32;
    let samples = 5;
    let delay_ms = 10;
    let mut hosts = Vec::new();
    for index in 0..count {
        let mut snapshot: Value = serde_json::from_str(include_str!(
            "../../../plugins/agentix-bridge/protocol/snapshot.json"
        ))
        .unwrap();
        let id = format!("session-{index}");
        snapshot["session"]["id"] = json!(id);
        let (server, host) = tokio::io::duplex(65536);
        let response = snapshot.clone();
        hosts.push(tokio::spawn(async move {
            let mut host = BufReader::new(host);
            let mut line = String::new();
            while host.read_line(&mut line).await.unwrap_or_default() > 0 {
                let frame: Value = serde_json::from_str(&line).unwrap();
                line.clear();
                if frame["method"].is_string() {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    let response = json!({"id":frame["id"],"ok":true,"result":response});
                    if host
                        .get_mut()
                        .write_all(format!("{response}\n").as_bytes())
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }));
        hub.accept(json!({"id":"register","method":"register","params":{
            "version":wire::PROTOCOL_VERSION,"agent":"pi","instance":snapshot["instance"],"pid":std::process::id(),
            "session_id":id,"session_file":root.join(format!("{id}.jsonl")),"cwd":root,"snapshot":snapshot,
        }}), server).await.unwrap();
    }
    assert_eq!(
        adapter
            .list_sessions(None, 100)
            .await
            .unwrap()
            .sessions
            .len(),
        count
    );
    let mut timings = Vec::new();
    for _ in 0..samples {
        let begin = Instant::now();
        assert_eq!(
            adapter
                .list_sessions(None, 100)
                .await
                .unwrap()
                .sessions
                .len(),
            count
        );
        timings.push(begin.elapsed().as_secs_f64() * 1000.0);
    }
    let mut sorted = timings.clone();
    sorted.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "profile":"release", "platform":std::env::consts::OS, "arch":std::env::consts::ARCH,
            "connections":count,"response_delay_ms":delay_ms,"samples":samples,
            "ms":timings,"median_ms":sorted[sorted.len()/2],
        }))
        .unwrap()
    );
    hub.shutdown().await;
    for host in hosts {
        host.await.unwrap();
    }
}
