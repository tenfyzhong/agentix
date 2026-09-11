//! JSONL direct fixture versus JSONL -> production adapter -> Unix WebSocket fixture.
#![cfg(unix)]
use agentix_codex::{ClientRegistry, proxy_stdio};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio_tungstenite::tungstenite::Message;

async fn sample(proxied: bool, size: usize, streaming: bool) -> serde_json::Value {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("up.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let payload =
        json!({"method":"item/agentMessage/delta","params":{"delta":"x".repeat(size)}}).to_string();
    let count = if streaming {
        (8 * 1024 * 1024 / payload.len()).max(32)
    } else {
        300
    };
    let (client, service) = UnixStream::pair().unwrap();
    let mut service = Some(service);
    let registry = ClientRegistry::default();
    let adapter = if proxied {
        let (service_read, service_write) = service.take().unwrap().into_split();
        let endpoint = format!("unix://{}", path.display());
        Some(tokio::spawn(async move {
            proxy_stdio(service_read, service_write, &endpoint, registry, None).await
        }))
    } else {
        None
    };
    // In direct mode the JSONL server owns the service socket; in proxy mode it uses WS.
    let server_payload = payload.clone();
    let server = if proxied {
        tokio::spawn(async move {
            let mut ws = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
                .await
                .unwrap();
            if streaming {
                ws.next().await.unwrap().unwrap();
                for _ in 0..count {
                    ws.send(Message::text(server_payload.clone()))
                        .await
                        .unwrap();
                }
            } else {
                for _ in 0..count {
                    let msg = ws.next().await.unwrap().unwrap();
                    ws.send(msg).await.unwrap();
                }
            }
            ws.next().await.unwrap().unwrap();
        })
    } else {
        let (read, mut write) = service.take().unwrap().into_split();
        tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            if streaming {
                lines.next_line().await.unwrap().unwrap();
                for _ in 0..count {
                    write.write_all(server_payload.as_bytes()).await.unwrap();
                    write.write_all(b"\n").await.unwrap();
                }
            } else {
                for _ in 0..count {
                    let line = lines.next_line().await.unwrap().unwrap();
                    write.write_all(line.as_bytes()).await.unwrap();
                    write.write_all(b"\n").await.unwrap();
                }
            }
            lines.next_line().await.unwrap().unwrap();
        })
    };
    let (read, mut write) = client.into_split();
    let mut lines = BufReader::new(read).lines();
    let start = Instant::now();
    let mut timings = Vec::new();
    if streaming {
        write.write_all(b"{}\n").await.unwrap();
    }
    for _ in 0..count {
        let t = Instant::now();
        if !streaming {
            write.write_all(payload.as_bytes()).await.unwrap();
            write.write_all(b"\n").await.unwrap();
        }
        assert_eq!(lines.next_line().await.unwrap().unwrap(), payload);
        timings.push(u64::try_from(t.elapsed().as_micros()).unwrap());
    }
    write.write_all(b"{}\n").await.unwrap();
    let elapsed = start.elapsed();
    server.await.unwrap();
    drop(write);
    drop(lines);
    if let Some(adapter) = adapter {
        let _ = adapter.await.unwrap();
    }
    timings.sort_unstable();
    json!({"proxied":proxied,"payload_bytes":payload.len(),"streaming":streaming,"messages":count,"elapsed_ms":elapsed.as_secs_f64()*1000.0,"mib_per_second":f64::from(u32::try_from(payload.len()*count).unwrap())/1_048_576.0/elapsed.as_secs_f64(),"p50_us":timings[timings.len()/2],"p99_us":timings[timings.len()*99/100]})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual release stdio performance comparison"]
async fn compare_stdio() {
    for round in 0..3 {
        for (size, streaming) in [
            (128, false),
            (1024, true),
            (16_384, true),
            (1_048_576, true),
        ] {
            for proxied in [false, true] {
                let mut result =
                    tokio::time::timeout(Duration::from_mins(1), sample(proxied, size, streaming))
                        .await
                        .unwrap();
                result["round"] = json!(round);
                println!("STDIO_BENCH {result}");
            }
        }
    }
}
