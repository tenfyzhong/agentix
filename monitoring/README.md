# Application monitoring assets

See [setup, semantics and metric dictionary](../docs/application-metrics.md).
Import only `dashboards/agentix-applications.json`; its eight rows cover application
behavior and the two service processes. These assets do not monitor hosts or Alloy.

Regenerate and check assets:

```sh
node monitoring/generate.mjs
node --test monitoring/tests/*.test.mjs
promtool check rules monitoring/alerts.yaml
promtool test rules monitoring/tests/alerts.test.yaml
```

The pipeline test is opt-in and uses temporary local Alloy and Prometheus
processes only as a remote-write acceptance fixture. It does not modify installed
service configuration, use Cloud credentials or send data to the internet. It
checks the actual samples received after relabeling and evaluates every dashboard
PromQL expression with the real query engine:

```sh
ALLOY_BINARY=/path/to/alloy PROMETHEUS_BINARY=/path/to/prometheus \
  node --test monitoring/tests/pipeline.test.mjs
```

Reusable performance comparisons:

```sh
cargo test --release -p agentix-codex --test metrics_performance -- \
  --ignored --exact application_metrics_rpc_benchmark --nocapture
cargo test -p taskix --bin taskix application_snapshot_contention_benchmark -- \
  --ignored --nocapture
```

The RPC benchmark alternates seven disabled/enabled pairs, measuring 100,000 real
local Unix RPCs per sample after warmup. Enabled samples also scrape at 10 Hz
(600 times the production scrape frequency). It excludes real model inference,
IM and Cloud. The SQLite benchmark alternates seven pairs with 10,000 stored tasks
and 2,000 forced writes per sample, overlapping one real read-only snapshot per burst. Set `TASKIX_METRICS_BENCH_WRITES=20000` for the longer window.
Do not run timing comparisons concurrently with builds or other benchmarks.
Raw samples and measured scope are recorded in [performance results](performance.md).

The 14 alerts use Prometheus rule YAML (serialized as the JSON subset of YAML).
Import into the Cloud Metrics ruler, not Grafana's Grafana-managed provisioning
API. Customize expected instance selectors and notification routing before use.
