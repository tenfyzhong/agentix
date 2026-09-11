//! Opt-in local transport comparison; no external daemon or model is used.
#![cfg(unix)]
use agentix_codex::CodexProxy;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}
type Ws = WebSocketStream<Box<dyn Io>>;

enum Listener {
    Unix(UnixListener),
    Tcp(TcpListener),
}
impl Listener {
    async fn accept(&self) -> Box<dyn Io> {
        match self {
            Self::Unix(l) => Box::new(l.accept().await.unwrap().0),
            Self::Tcp(l) => Box::new(l.accept().await.unwrap().0),
        }
    }
}
async fn connect(endpoint: &str) -> Ws {
    let stream: Box<dyn Io> = if let Some(path) = endpoint.strip_prefix("unix://") {
        Box::new(UnixStream::connect(path).await.unwrap())
    } else {
        Box::new(
            TcpStream::connect(endpoint.strip_prefix("ws://").unwrap())
                .await
                .unwrap(),
        )
    };
    tokio_tungstenite::client_async("ws://localhost/", stream)
        .await
        .unwrap()
        .0
}

fn message_count(streaming: bool, payload_len: usize) -> usize {
    if streaming {
        (8 * 1024 * 1024 / payload_len).max(32)
    } else {
        300
    }
}

// Independent binary axes of the diagnostic transport matrix.
#[allow(clippy::fn_params_excessive_bools)]
async fn sample(
    front_tcp: bool,
    back_tcp: bool,
    proxied: bool,
    clients: usize,
    size: usize,
    streaming: bool,
) -> serde_json::Value {
    let dir = tempfile::tempdir().unwrap();
    let (listener, upstream) = if back_tcp {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", l.local_addr().unwrap());
        (Listener::Tcp(l), endpoint)
    } else {
        let path = dir.path().join("up.sock");
        (
            Listener::Unix(UnixListener::bind(&path).unwrap()),
            format!("unix://{}", path.display()),
        )
    };
    let payload = json!({"method":"item/agentMessage/delta","params":{"threadId":"bench","delta":"x".repeat(size)}}).to_string();
    let count = message_count(streaming, payload.len());
    let server_payload = payload.clone();
    let server = tokio::spawn(async move {
        let mut jobs = tokio::task::JoinSet::new();
        for _ in 0..clients {
            let io = listener.accept().await;
            let payload = server_payload.clone();
            jobs.spawn(async move {
                let mut ws = tokio_tungstenite::accept_async(io).await.unwrap();
                if streaming {
                    assert!(ws.next().await.unwrap().unwrap().is_text());
                    for _ in 0..count {
                        ws.send(Message::text(payload.clone())).await.unwrap();
                    }
                    // Wait for receiver completion before dropping the transport.
                    assert!(ws.next().await.unwrap().unwrap().is_text());
                } else {
                    for _ in 0..count {
                        let msg = ws.next().await.unwrap().unwrap();
                        ws.send(msg).await.unwrap();
                    }
                }
            });
        }
        while let Some(result) = jobs.join_next().await {
            result.unwrap();
        }
    });
    let frontend = if front_tcp {
        "ws://127.0.0.1:0".into()
    } else {
        format!("unix://{}", dir.path().join("proxy.sock").display())
    };
    let proxy = if proxied {
        Some(CodexProxy::bind(&frontend, &upstream).await.unwrap())
    } else {
        None
    };
    let endpoint = proxy
        .as_ref()
        .map_or(upstream.as_str(), CodexProxy::endpoint);
    let mut sockets = Vec::new();
    let handshake = Instant::now();
    for _ in 0..clients {
        sockets.push(connect(endpoint).await);
    }
    let handshake_us = handshake.elapsed().as_secs_f64() * 1_000_000.0
        / f64::from(u32::try_from(clients).unwrap());
    let start = Instant::now();
    let mut jobs = tokio::task::JoinSet::new();
    for mut ws in sockets {
        let payload = payload.clone();
        jobs.spawn(async move {
            let mut timings = Vec::new();
            if streaming {
                ws.send(Message::text("start")).await.unwrap();
            }
            for _ in 0..count {
                let t = Instant::now();
                if !streaming {
                    ws.send(Message::text(payload.clone())).await.unwrap();
                }
                let msg = ws.next().await.unwrap().unwrap();
                assert_eq!(msg.to_text().unwrap(), payload);
                timings.push(u64::try_from(t.elapsed().as_micros()).unwrap());
            }
            if streaming {
                ws.send(Message::text("done")).await.unwrap();
            }
            timings
        });
    }
    let mut timings = Vec::new();
    while let Some(result) = jobs.join_next().await {
        timings.extend(result.unwrap());
    }
    let elapsed = start.elapsed();
    server.await.unwrap();
    if let Some(proxy) = proxy {
        proxy.shutdown().await;
    }
    timings.sort_unstable();
    json!({"front":if front_tcp {"ws"} else {"unix"},"upstream":if back_tcp {"ws"} else {"unix"},"proxied":proxied,"clients":clients,"payload_bytes":payload.len(),"streaming":streaming,"messages":count*clients,"elapsed_ms":elapsed.as_secs_f64()*1000.0,"mib_per_second":f64::from(u32::try_from(payload.len()*count*clients).unwrap())/1_048_576.0/elapsed.as_secs_f64(),"p50_us":timings[timings.len()/2],"p99_us":timings[timings.len()*99/100],"handshake_us":handshake_us})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual release performance comparison"]
async fn compare_direct_and_proxy() {
    let rounds: usize = std::env::var("PROXY_BENCH_ROUNDS")
        .unwrap_or_else(|_| "3".into())
        .parse()
        .unwrap();
    for round in 0..rounds {
        for (front, back) in [(false, false), (true, false), (true, true), (false, true)] {
            for (clients, size, streaming) in [
                (1, 128, false),
                (16, 128, false),
                (1, 1024, true),
                (1, 16_384, true),
                (1, 1_048_576, true),
                (16, 1024, true),
            ] {
                for proxied in if round % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    let mut value = tokio::time::timeout(
                        Duration::from_mins(1),
                        sample(front, back, proxied, clients, size, streaming),
                    )
                    .await
                    .expect("benchmark sample stalled");
                    value["round"] = json!(round);
                    println!("PROXY_BENCH {value}");
                }
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual release regression for an artificial accept timer floor"]
async fn accept_has_no_timer_floor() {
    let result = sample(false, false, true, 16, 128, false).await;
    assert!(
        result["handshake_us"].as_f64().unwrap() < 750.0,
        "artificial accept delay: {result}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual release regression for large-message relay overhead"]
async fn large_stream_avoids_excessive_copy_overhead() {
    let mut ratios = Vec::new();
    for _ in 0..3 {
        let direct = sample(true, true, false, 1, 1_048_576, true).await;
        let proxy = sample(true, true, true, 1, 1_048_576, true).await;
        ratios.push(proxy["elapsed_ms"].as_f64().unwrap() / direct["elapsed_ms"].as_f64().unwrap());
    }
    ratios.sort_by(f64::total_cmp);
    assert!(ratios[1] < 2.5, "large-message relay overhead: {ratios:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual release comparison of paced interactive RPC latency"]
async fn compare_paced_rpc() {
    for tcp in [false, true] {
        for proxied in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let (listener, upstream) = if tcp {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let endpoint = format!("ws://{}", listener.local_addr().unwrap());
                (Listener::Tcp(listener), endpoint)
            } else {
                let path = dir.path().join("up.sock");
                (
                    Listener::Unix(UnixListener::bind(&path).unwrap()),
                    format!("unix://{}", path.display()),
                )
            };
            let server = tokio::spawn(async move {
                let mut ws = tokio_tungstenite::accept_async(listener.accept().await)
                    .await
                    .unwrap();
                for _ in 0..100 {
                    let msg = ws.next().await.unwrap().unwrap();
                    ws.send(msg).await.unwrap();
                }
            });
            let frontend = if tcp {
                "ws://127.0.0.1:0".into()
            } else {
                format!("unix://{}", dir.path().join("proxy.sock").display())
            };
            let proxy = if proxied {
                Some(CodexProxy::bind(&frontend, &upstream).await.unwrap())
            } else {
                None
            };
            let mut ws = connect(
                proxy
                    .as_ref()
                    .map_or(upstream.as_str(), CodexProxy::endpoint),
            )
            .await;
            let payload =
                json!({"method":"item/agentMessage/delta","params":{"delta":"x".repeat(1024)}})
                    .to_string();
            let mut times = Vec::new();
            for _ in 0..100 {
                tokio::time::sleep(Duration::from_millis(10)).await;
                let started = Instant::now();
                ws.send(Message::text(payload.clone())).await.unwrap();
                assert_eq!(
                    ws.next().await.unwrap().unwrap().to_text().unwrap(),
                    payload
                );
                times.push(started.elapsed().as_micros());
            }
            times.sort_unstable();
            println!(
                "PACED_BENCH {}",
                json!({"tcp":tcp,"proxied":proxied,"messages":100,"p50_us":times[50],"p99_us":times[99]})
            );
            drop(ws);
            server.await.unwrap();
            if let Some(proxy) = proxy {
                proxy.shutdown().await;
            }
        }
    }
}
