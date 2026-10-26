use agentix_metrics::{Event, Metrics, Operation, Service, TokenKind, TokenSource};

fn record(metrics: &Metrics) {
    metrics.event(Event::TelegramInbound, 1);
    metrics.tokens(TokenSource::Codex, TokenKind::Input, 2);
}

#[test]
fn warmed_counters_do_not_allocate() {
    let metrics = Metrics::new(Service::Agentix, "test");
    record(&metrics);
    let allocations = allocation_counter::measure(|| {
        for _ in 0..1000 {
            record(&metrics);
        }
    });
    assert_eq!(allocations.count_total, 0);
    let text = metrics.encode().unwrap();
    assert!(text.contains("agentix_events_total{component=\"telegram\",event=\"inbound\"} 1001"));
    assert!(text.contains("agentix_tokens_total{source=\"codex_app_server\",kind=\"input\"} 2002"));
    assert!(!text.contains("kind=\"output\""));
}

#[test]
fn disabled_recording_does_not_allocate_even_on_first_use() {
    let metrics = Metrics::default();
    let allocations = allocation_counter::measure(|| {
        record(&metrics);
        drop(metrics.start(Operation::CodexRpc));
    });
    assert_eq!(allocations.count_total, 0);
}

#[test]
fn concurrent_first_observations_share_counters() {
    let metrics = Metrics::new(Service::Agentix, "test");
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..1000 {
                    record(&metrics);
                }
            });
        }
    });
    let text = metrics.encode().unwrap();
    assert!(text.contains("agentix_events_total{component=\"telegram\",event=\"inbound\"} 8000"));
    assert!(
        text.contains("agentix_tokens_total{source=\"codex_app_server\",kind=\"input\"} 16000")
    );
    assert!(!text.contains("kind=\"output\""));
}

#[test]
fn established_usage_baselines_do_not_allocate() {
    use agentix_metrics::{TokenDeltas, Usage};
    let metrics = Metrics::new(Service::Agentix, "test");
    let mut deltas = TokenDeltas::default();
    let usage = |input| Usage {
        input: Some(input),
        ..Usage::default()
    };
    deltas.record(&metrics, "session", usage(10));
    deltas.record(&metrics, "session", usage(11));
    let allocations = allocation_counter::measure(|| {
        for input in 12..1012 {
            deltas.record(&metrics, "session", usage(input));
        }
    });
    assert_eq!(allocations.count_total, 0);
    assert!(
        metrics
            .encode()
            .unwrap()
            .contains("agentix_tokens_total{source=\"codex_app_server\",kind=\"input\"} 1001")
    );
}
