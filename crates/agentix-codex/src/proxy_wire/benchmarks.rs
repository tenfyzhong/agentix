//! Run each implementation in a fresh process for meaningful peak-RSS comparisons.
//! `OBSERVER_IMPL=legacy|stream OBSERVER_BYTES=... OBSERVER_CLIENTS=1|16`
//! `cargo test --release -p agentix-codex --lib observer_benchmark -- --ignored --nocapture`
use super::*;
use std::time::Instant;

#[test]
#[ignore = "manual release CPU/latency/throughput/RSS comparison"]
fn observer_benchmark() {
    let implementation = std::env::var("OBSERVER_IMPL").unwrap_or_else(|_| "stream".into());
    let size: usize = std::env::var("OBSERVER_BYTES")
        .unwrap_or_else(|_| "128".into())
        .parse()
        .unwrap();
    let clients: usize = std::env::var("OBSERVER_CLIENTS")
        .unwrap_or_else(|_| "1".into())
        .parse()
        .unwrap();
    let samples = if size >= 1024 * 1024 {
        20
    } else {
        10000 / clients
    };
    let registry = crate::ClientRegistry::default();
    let connections = (0..clients)
        .map(|_| registry.connect(None))
        .collect::<Vec<_>>();
    let mut old = (0..if implementation == "legacy" {
        clients
    } else {
        0
    })
        .map(|_| legacy_bench::Observer::new(Role::Client))
        .collect::<Vec<_>>();
    let mut new = (0..if implementation == "stream" {
        clients
    } else {
        0
    })
        .map(|_| Observer::new(Role::Client))
        .collect::<Vec<_>>();
    let prefix = br#"{"id":1,"result":{"thread":{"turns":[{"text":""#;
    let suffix = br#""}],"id":"live","source":"cli"}}}"#;
    let len = prefix.len() + size + suffix.len();
    let mut header = vec![0x81, 127];
    header.extend_from_slice(&(len as u64).to_be_bytes());
    // Input remains bounded too, so process peak RSS measures the observer rather
    // than a generated 65 MiB test fixture. No model, filesystem history or IM.
    let block = vec![b'x'; size.min(16 * 1024)];
    let cpu = cpu_time::ProcessTime::now();
    let elapsed = Instant::now();
    let mut timings = Vec::new();
    let mut errors = 0;
    for _ in 0..samples {
        for (i, connection) in connections.iter().copied().enumerate() {
            registry.client_frame(connection, r#"{"id":1,"method":"thread/start"}"#);
            let start = Instant::now();
            let mut feed = |bytes: &[u8]| {
                let result = if implementation == "legacy" {
                    old[i].feed(bytes, |s| registry.server_frame(connection, s))
                } else {
                    new[i].feed_with(bytes, |s, projected| {
                        if projected {
                            registry.projected_frame(connection, s, false);
                        } else {
                            registry.server_frame(connection, s);
                        }
                    })
                };
                if result.is_err() {
                    errors += 1;
                }
            };
            feed(&header);
            feed(prefix);
            for _ in 0..size / block.len() {
                feed(&block);
            }
            feed(&block[..size % block.len()]);
            feed(suffix);
            timings.push(start.elapsed().as_secs_f64() * 1e6);
        }
    }
    let seconds = elapsed.elapsed().as_secs_f64();
    let cpu_ms = cpu.elapsed().as_secs_f64() * 1000.0;
    timings.sort_by(f64::total_cmp);
    let tracked = registry.snapshot().iter().all(|c| c.sessions == ["live"]);
    let count = timings.len();
    println!(
        "OBSERVER_BENCH {}",
        serde_json::json!({"implementation":implementation,"payload_bytes":size,"clients":clients,"messages":count,"p50_us":timings[count/2],"p95_us":timings[count*95/100],"p99_us":timings[count*99/100],"elapsed_ms":seconds*1000.0,"cpu_ms":cpu_ms,"mib_per_second":f64::from(u32::try_from(len*count).unwrap())/1_048_576.0/seconds,"tracked":tracked,"errors":errors})
    );
    if implementation != "legacy" || size < 64 * 1024 * 1024 {
        assert!(tracked);
        assert_eq!(errors, 0);
    }
}
