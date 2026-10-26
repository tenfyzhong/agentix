//! Run separately in release mode; no allocation-counting allocator is linked here.
use agentix_metrics::{Event, Metrics, Service, TokenKind, TokenSource};
use std::{hint::black_box, time::Instant};

#[test]
#[ignore = "manual release performance measurement"]
fn counter_recording_benchmark() {
    for enabled in [false, true] {
        let metrics = if enabled {
            Metrics::new(Service::Agentix, "benchmark")
        } else {
            Metrics::default()
        };
        metrics.event(Event::TelegramInbound, 1);
        metrics.tokens(TokenSource::Codex, TokenKind::Input, 1);
        for sample in 0..7 {
            let start = Instant::now();
            for _ in 0..1_000_000 {
                black_box(&metrics).event(black_box(Event::TelegramInbound), black_box(1));
                black_box(&metrics).tokens(
                    black_box(TokenSource::Codex),
                    black_box(TokenKind::Input),
                    black_box(1),
                );
            }
            println!(
                "{{\"enabled\":{enabled},\"sample\":{sample},\"pairs\":1000000,\"elapsed_ns\":{}}}",
                start.elapsed().as_nanos()
            );
        }
        black_box(metrics.encode().unwrap());
    }
}
