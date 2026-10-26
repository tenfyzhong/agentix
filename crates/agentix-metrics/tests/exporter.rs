use agentix_metrics::{Config, Metrics, Operation, Outcome, Service, Snapshot};
use std::time::Duration;

#[test]
fn disabled_metrics_allocate_no_series_and_config_defaults_off() {
    let config: Config = toml::from_str("").unwrap();
    assert!(!config.enabled);
    let metrics = Metrics::default();
    metrics.observe(
        Operation::Engine,
        Outcome::Success,
        Duration::from_millis(10),
    );
    assert!(metrics.encode().unwrap().is_empty());
}

#[test]
fn operation_drop_records_cancellation_and_releases_in_flight() {
    let metrics = Metrics::new(Service::Agentix, "test");
    let guard = metrics.start(Operation::Engine);
    assert!(
        metrics
            .encode()
            .unwrap()
            .contains("operations_in_flight{component=\"engine\",operation=\"dispatch\"} 1")
    );
    drop(guard);
    let text = metrics.encode().unwrap();
    assert!(text.contains("outcome=\"cancelled\"} 1"));
    assert!(text.contains("operations_in_flight{component=\"engine\",operation=\"dispatch\"} 0"));
    assert!(!text.contains("process_"));
    assert!(!text.contains("scrape_"));
}

#[tokio::test(start_paused = true)]
async fn stale_snapshot_is_omitted_instead_of_becoming_zero() {
    let metrics = Metrics::new(Service::Taskix, "test");
    metrics.publish(
        Snapshot::Tasks,
        "taskix_entities{kind=\"task\",state=\"blocked\"} 3\n".into(),
    );
    assert!(metrics.encode().unwrap().contains("taskix_entities"));
    tokio::time::advance(Duration::from_secs(121)).await;
    assert!(!metrics.encode().unwrap().contains("taskix_entities"));
}

#[tokio::test]
async fn exporter_serves_only_metrics_and_stops_on_drop() {
    let metrics = Metrics::new(Service::Agentix, "test");
    let server = metrics.serve("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let address = server.address();
    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://{address}/metrics"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.text().await.unwrap();
    assert!(body.contains("agentix_build_info"));
    assert_eq!(body.matches("# EOF").count(), 1);
    assert_eq!(
        client
            .get(format!("http://{address}/"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    drop(server);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        client
            .get(format!("http://{address}/metrics"))
            .send()
            .await
            .is_err()
    );
}

#[tokio::test]
async fn tracked_failure_is_not_a_success_or_cancellation() {
    let metrics = Metrics::new(Service::Agentix, "test");
    let result: Result<(), &str> = metrics
        .track(Operation::CodexRpc, async { Err("private error") })
        .await;
    assert!(result.is_err());
    let text = metrics.encode().unwrap();
    assert!(text.contains("outcome=\"error\"} 1"));
    assert!(!text.contains("private error"));
}

#[test]
fn turns_are_deduplicated_and_first_output_observed_once() {
    let metrics = Metrics::new(Service::Agentix, "test");
    let mut turns = agentix_metrics::Turns::new(metrics.clone());
    turns.started("codex:s", "turn");
    turns.started("codex:s", "turn");
    turns.output("codex:s", "turn");
    turns.output("codex:s", "turn");
    turns.completed("codex:s", "turn", Outcome::Success);
    turns.completed("codex:s", "turn", Outcome::Success);
    turns.started("codex:s", "turn");
    let text = metrics.encode().unwrap();
    assert!(text.contains("operation=\"turn\",outcome=\"success\"} 1"));
    assert!(text.contains("operation=\"first_output\",outcome=\"success\"} 1"));
    assert!(text.contains("operation=\"turn\"} 0"));
    assert!(!text.contains("codex:s"));
}

#[test]
fn cumulative_usage_rebaselines_without_double_counting_or_inventing_missing_values() {
    use agentix_metrics::{TokenDeltas, Usage};
    let metrics = Metrics::new(Service::Agentix, "test");
    let mut usage = TokenDeltas::default();
    let a = Usage {
        input: Some(100),
        output: Some(10),
        cached: None,
    };
    usage.record(&metrics, "session", a);
    usage.record(
        &metrics,
        "session",
        Usage {
            input: Some(150),
            output: Some(20),
            cached: None,
        },
    );
    usage.record(
        &metrics,
        "session",
        Usage {
            input: Some(150),
            output: Some(20),
            cached: None,
        },
    );
    usage.clear();
    usage.record(
        &metrics,
        "session",
        Usage {
            input: Some(200),
            output: Some(25),
            cached: None,
        },
    );
    usage.record(&metrics, "session", a);
    let text = metrics.encode().unwrap();
    assert!(text.contains("kind=\"input\"} 50"));
    assert!(text.contains("kind=\"output\"} 10"));
    assert!(!text.contains("kind=\"cached_input\""));
    assert!(!text.contains("session"));
}

#[test]
fn complete_finite_operation_space_stays_below_machine_series_budget() {
    let mut series = 0;
    for service in [Service::Agentix, Service::Taskix] {
        let metrics = Metrics::new(service, "test");
        for op in Operation::ALL {
            for result in [
                Outcome::Success,
                Outcome::Error,
                Outcome::Cancelled,
                Outcome::Timeout,
            ] {
                metrics.observe(*op, result, Duration::from_millis(1));
            }
        }
        let text = metrics.encode().unwrap();
        series += text
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .count();
        assert!(!text.contains("session="));
    }
    // Reserve 450 series for all finite snapshot, queue, event and token dimensions.
    assert!(
        series + 450 <= 2000,
        "operation series={series}, snapshots reserve=450"
    );
}

#[test]
fn connection_lifetimes_count_concurrent_peers_and_release_on_drop() {
    let metrics = Metrics::new(Service::Agentix, "test");
    let first = metrics.connected("pi");
    let second = metrics.connected("pi");
    assert!(
        metrics
            .encode()
            .unwrap()
            .contains("agentix_backend_connections{backend=\"pi\"} 2")
    );
    drop(first);
    assert!(
        metrics
            .encode()
            .unwrap()
            .contains("agentix_backend_connections{backend=\"pi\"} 1")
    );
    drop(second);
    assert!(
        metrics
            .encode()
            .unwrap()
            .contains("agentix_backend_connections{backend=\"pi\"} 0")
    );
}

#[test]
fn a_codex_disconnect_does_not_cancel_bridge_backend_turns() {
    let metrics = Metrics::new(Service::Agentix, "test");
    let mut turns = agentix_metrics::Turns::new(metrics.clone());
    turns.started("uuid-session", "codex-turn");
    turns.started("pi:session", "pi-turn");
    turns.disconnected("codex");
    let text = metrics.encode().unwrap();
    assert!(text.contains("operations_in_flight{component=\"pi\",operation=\"turn\"} 1"));
    assert!(text.contains("component=\"codex\",operation=\"turn\",outcome=\"cancelled\"} 1"));
}

#[tokio::test]
async fn exporter_samples_only_its_own_process_resources() {
    let metrics = Metrics::new(Service::Taskix, "test");
    let server = metrics.serve("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let text = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let text = metrics.encode().unwrap();
            if text.contains("taskix_process_resident_memory_bytes ") {
                break text;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("background process sample missing");
    let sample = |name: &str| -> f64 {
        text.lines()
            .find_map(|line| line.strip_prefix(&format!("taskix_process_{name} ")))
            .unwrap()
            .parse()
            .unwrap()
    };
    assert!(sample("resident_memory_bytes") > 0.0);
    assert!(sample("cpu_seconds_total") >= 0.0);
    assert!(sample("start_time_seconds") > 0.0);
    assert!(sample("uptime_seconds") >= 0.0);
    assert!(!text.contains("pid="));
    assert!(!text.contains("hostname"));
    drop(server);
}

#[test]
fn bridge_usage_counts_only_post_registration_deltas_for_each_bounded_source() {
    use agentix_metrics::{TokenSource, Usage};
    let metrics = Metrics::new(Service::Agentix, "test");
    for (source, name) in [
        (TokenSource::Pi, "pi_bridge"),
        (TokenSource::Omp, "omp_bridge"),
        (TokenSource::Claude, "claude_bridge"),
        (TokenSource::OpenCode, "opencode_bridge"),
    ] {
        let mut baseline = Usage {
            input: Some(100),
            output: None,
            cached: None,
        };
        let current = Usage {
            input: Some(117),
            output: Some(4),
            cached: None,
        };
        baseline.record_observed(&metrics, source, current);
        baseline.record_observed(&metrics, source, current);
        assert!(
            metrics
                .encode()
                .unwrap()
                .contains(&format!("source=\"{name}\",kind=\"input\"}} 17"))
        );
        assert!(
            metrics
                .encode()
                .unwrap()
                .contains(&format!("source=\"{name}\",kind=\"output\"}} 4"))
        );
    }
    assert!(!metrics.encode().unwrap().contains("cached_input"));
}

#[test]
fn bridge_cached_usage_is_optional_and_uses_its_own_baseline() {
    use agentix_metrics::{TokenSource, Usage};
    let metrics = Metrics::new(Service::Agentix, "test");
    let mut baseline = Usage::default();
    let next = Usage {
        input: Some(17),
        output: Some(4),
        cached: Some(7),
    };
    baseline.record_observed(&metrics, TokenSource::Claude, next);
    baseline.record_observed(&metrics, TokenSource::Claude, next);
    assert!(
        metrics
            .encode()
            .unwrap()
            .contains("source=\"claude_bridge\",kind=\"cached_input\"} 7")
    );
}
