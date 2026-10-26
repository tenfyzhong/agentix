# Application metrics

Agentix and Taskix expose opt-in OpenMetrics endpoints for an existing Grafana
Alloy installation to forward to Grafana Cloud Metrics. A local Prometheus server
is not needed in production. The applications never contact Grafana Cloud and do
not hold its credentials.

This integration exports application behavior and resources of the Agentix/Taskix
service process itself. It does not register host, other-process, child-agent,
runtime, exporter or collector metrics. The supplied Alloy
branch uses an exact metric-name allowlist **after scraping**, which also drops
Alloy-generated `up` and `scrape_*` samples. Keep the existing OTLP usage pipeline
unchanged. Other pre-existing Alloy pipelines are outside this branch's filter.

## Enable the endpoints

Add to the Agentix configuration (normally `~/.config/agentix/config.toml`):

```toml
[metrics]
enabled = true
listen = "127.0.0.1:9464"
```

Add to the Taskix configuration (normally `~/.config/taskix/config.toml`):

```toml
[metrics]
enabled = true
listen = "127.0.0.1:9465"
```

Metrics default to disabled. Only loopback IP addresses are accepted. Use the
existing native service restart procedure after changing these settings; reload
rejects metrics changes. Taskix can run `taskix serve` with metrics enabled and
Memory disabled; it does not initialize Memory storage or model providers in that
mode. Ordinary Taskix CLI invocations do not open metrics ports. The background
Taskix service reads the shared persisted database to observe CLI and plugin work.
When using a non-default config, pass the same `--config` path to the service.

Check locally:

```sh
curl --fail http://127.0.0.1:9464/metrics
curl --fail http://127.0.0.1:9465/metrics
```

Process CPU time, RSS, virtual memory, start time and uptime are included. There
are no PID, hostname, file path, Project, session, task, message, model name,
prompt, response or error-text labels. Labels are finite enums, except the
application build version and the explicitly configured application instance
alias. Use one stable alias per installation, not a generated identifier.

## Connect the existing Alloy

Append [alloy-applications.alloy](../monitoring/alloy-applications.alloy) to the
existing Alloy configuration. It reuses `prometheus.remote_write.default`; change
that reference if your existing component has another name. Set `instance` from
`local` to the same stable deployment alias on both targets. Do not use a hostname
or add host exporters. Remove a target if that application is intentionally absent.

The default scrape interval is 60 seconds and timeout is five seconds. Validate
the **complete combined configuration**, then restart/reload Alloy through its
existing native service management. Do not replace the existing remote-write
credentials or OTLP receiver/exporter definitions.

```sh
alloy validate /opt/homebrew/etc/alloy/config.alloy
```

The allowlist is generated alongside the dashboards; adding a new metric requires
an explicit update. It is intentionally narrower than `agentix_.*|taskix_.*`.
Do not put self-monitoring into this application's remote-write branch.

## Import dashboards and alerts

Import the single [agentix-applications.json](../monitoring/dashboards/agentix-applications.json)
using Grafana's **Dashboards → New → Import** and select your Grafana Cloud Metrics
Prometheus data source. This dashboard has eight expandable rows, a shared data
source and application instance selector, one-minute refresh, units and metric
semantics. Missing data stays missing rather than being substituted with zero.
The previous seven JSON files are retired; existing imported dashboards are not
automatically deleted from Grafana Cloud.

| Row | Coverage |
| --- | --- |
| Application overview | Builds, throughput, errors, queues and workflow state |
| Process resources | Per-service CPU, RSS, virtual memory, start time and uptime |
| Messages | Inbound traffic, sends, updates, latency, retries and rejections |
| Agents and dispatch | Live connections, backend RPC, turns, first output, in-flight work, queue capacity/age and lost events |
| Workflow | Task/Job/Issue states, age, transitions, expired leases and projection backlog |
| Memory | API/retrieval/work/provider latency and outcomes, queues, retries, projection, embedding and lifecycle |
| Observed token usage | Separately sourced Codex and Memory input/output/cached usage |
| Jev decisions | Decisions, adoption, mean/P95/P99 latency, mean confidence, fallbacks and human-reviewed accuracy |

[alerts.yaml](../monitoring/alerts.yaml) contains 14 Prometheus-compatible alerts.
Import these as data source-managed rules into Grafana Cloud Metrics using its
Ruler API or `mimirtool rules load`; they are not a Grafana-managed provisioning
file. Use your existing authorized Cloud access policy, namespace and notification
routing. Customize the two missing-data selectors to each expected `instance`
(and remove rules for applications you intentionally do not run).

Defaults: missing application build data for five minutes; operation error ratio
above 5% with at least 20 observations in ten minutes; queue occupancy above 80%;
pending work without progress; expired leases; failed Memory work; projection and
embedding backlogs. Interactive RPC/message p95 has a ten-second threshold;
Memory provider p95 has a separate two-minute threshold. Both latency alerts must
persist for ten minutes. Long Agent turns and human-waiting tasks have no generic
timeout alert. Tune defaults against actual traffic; a backlog alert means a
persistent backlog, not proof of a stalled writer. Missing application data does
not identify whether the application or its delivery path failed.

For an explanation of every chart, see the [complete dashboard panel guide](dashboard-panel-guide.md).

## Metric dictionary and semantics

`<service>` below is `agentix` or `taskix`. Families appear when their corresponding
operation/source has been observed. An empty successful database snapshot reports
zero for workflow states and durable Memory queues; an unavailable source is not
reported as zero.

| Family | Type / labels | Meaning |
| --- | --- | --- |
| `<service>_process_cpu_seconds_total` | counter | Cumulative process user/system CPU time; rate of 1 means one fully occupied core |
| `<service>_process_resident_memory_bytes` | gauge | Process resident memory (RSS) in bytes |
| `<service>_process_virtual_memory_bytes` | gauge | Process virtual address space in bytes, not physical RAM usage |
| `<service>_process_start_time_seconds` | gauge | Actual process start time as Unix seconds, not metrics enablement time |
| `<service>_process_uptime_seconds` | gauge | Elapsed process lifetime in seconds |
| `<service>_build_info` | info: `version` | Application build presence |
| `agentix_backend_connections` | gauge: `backend` | Live transport connections, including concurrent bridge peers |
| `<service>_operations_total` | counter: `component,operation,outcome` | Completed operations; success/error/cancelled/timeout categories |
| `<service>_operation_duration_seconds` | histogram: `component,operation,le` | Completed duration; boundaries .001,.005,.025,.1,.5,2,10,30,120,600,3600,+Inf seconds |
| `<service>_operations_in_flight` | gauge: `component,operation` | Currently observed operations |
| `<service>_events_total` | counter: `component,event` | Inbound messages, rate-limit retries, overload rejection, agent event loss, retrieval fallback/results |
| `agentix_queue_items` | gauge: `queue,state` | Engine/control pending and active work |
| `agentix_queue_capacity` | gauge: `queue` | Pending admission capacity |
| `agentix_queue_oldest_age_seconds` | gauge: `queue` | Oldest pending work |
| `<service>_tokens_total` | counter: `source,kind` | Real optional usage fields only; input/output/cached_input |
| `taskix_entities` | gauge: `kind,state` | Current Task/Job/Issue counts |
| `taskix_oldest_age_seconds` | gauge: `kind,state` | Oldest nonterminal time since entity update, not execution duration |
| `taskix_workflow_events_total` | counter: `event` | Persisted transitions observed after service startup baseline |
| `taskix_expired_leases` | gauge | Expired persisted execution leases |
| `taskix_pending_documents` | gauge | Pending workflow document projection |
| `taskix_memory_work_items` | gauge: `kind,state` | Durable extract/consolidate work state |
| `taskix_memory_oldest_pending_age_seconds` | gauge: `kind` | Oldest ready pending work, excluding future retry delays |
| `taskix_memory_retrying_items` | gauge | Pending work with previous attempts |
| `taskix_memory_failed_items` | gauge | Failed work |
| `taskix_memory_pending_projection` | gauge | Unpublished memory revisions |
| `taskix_memory_pending_embeddings` | gauge | Active, unexpired memories missing the configured embedding revision |
| `taskix_memory_embedding_failures` | gauge | Persisted embedding failure records |
| `taskix_memories` | gauge: `status` | Stored memory lifecycle state |
| `taskix_jev_decisions_total` | counter: `kind,outcome` | Persisted accepted/fallback decisions since baseline |
| `taskix_jev_duration_seconds` | histogram: `kind,le` | Persisted request durations observed after the baseline; sum/count retain the mean and buckets support P95/P99 |
| `taskix_jev_request_confidence_{sum,count}` | summary: `kind` | Available average answer confidence per request |
| `taskix_jev_fallbacks_total` | counter: `kind,reason` | Finite fallback reason categories; unknown values map to other |
| `taskix_jev_reviewed` | gauge: `kind,result` | Current reviewed records, including late reviews of historical requests |

Backend dimensions are codex/pi/omp/claude/opencode; channels are
telegram/feishu/slack. Internal operations use engine/control/memory/model/
embedding/provider. Unknown stored status/kind/reason values aggregate into
`other`; free-form strings never become dimensions. RPC outcomes cover the whole
logical request; retries are not separate successful requests. Dropping an
unfinished operation records cancellation. Errors currently include adapter and
provider timeouts; a separate timeout outcome is available to explicit callers.
Turns are deduplicated in bounded runtime-owned maps (256 active/completed keys),
and first output is observed once. Activity beyond the tracking bound is omitted,
not allowed to grow memory without a limit. Disconnect/reload can end incomplete
observations; these are not precise end-to-end user-response accounting.

Codex usage uses cumulative notification deltas, with a fresh baseline on the
first observation/reconnect/reset. Consequently it may undercount the initial
turn. Memory usage counts only fields present in real provider responses. Missing
fields are not converted into zero. Cached input is a subset of input. These are
**partial observed usage**, not costs or billing, and must never be added to the
existing OTLP usage for the same requests. Other hosts without existing usage
signals remain absent; this does not extend their protocols.

Jev adoption is not accuracy. Confidence is not a calibrated success probability.
Human-reviewed accuracy describes only reviewed requests. Jev schema v1 and v2 are
supported. Startup, detected source replacement and retention gaps establish a
new baseline rather than replaying historical traffic. Incremental reads process
at most 1,000 new records per cycle; bursts can lag beyond one cycle.

## Performance and failure isolation

Disabled recording avoids metric allocation and clock reads. Enabled recording
uses cached fixed-label counters/histograms and RAII gauges. Streaming fragments
do not update dispatch duration histograms. No raw samples or unbounded label maps
are retained. HTTP serving reads in-memory metrics only, with four connections,
five-second deadlines and bounded request buffers. The endpoint has no database
or network-provider calls.

Process resources are sampled every 15 seconds and limited to the current
process. On macOS, direct process-info and CPU-clock calls avoid PID enumeration;
other platforms use a current-PID filter with per-thread refresh disabled. Native
reads run on a blocking worker, not in request handlers; scraping reads the cached
snapshot. Dropping the exporter stops the sampler. Disabled metrics creates no process sampler or native
resource reads. CPU panels use cumulative CPU time rates; 100% means one core,
and multithreaded usage can exceed 100%. Agent child processes are excluded.
Process sampling failures omit data rather than synthesizing zero; cached samples
expire after two minutes. The dashboard shows process start time as a timestamp
and uptime as elapsed seconds.

One Taskix background worker reads the three databases sequentially every 60
seconds, using read-only connections and consistent read transactions. It never
creates missing databases or migrates schemas. Each source has a 25 ms busy wait,
a 500 ms SQLite progress deadline and a one-second outer timeout. Heavy snapshots
may therefore be omitted rather than completing an expensive scan. Last good
snapshots expire after two minutes. Snapshot errors stay in local logs; no
collector health/error metrics are uploaded. Reads still consume some database
I/O and CPU; the mechanism does not promise zero overhead.

The finite full operation space plus snapshot/queue/event/token allowance stays
under 2,000 application series per installation. Build changes create historical
version series; multiple installations multiply the budget. Existing OTLP series
are separate from this budget.

Reproducible checks and measurements are in
[monitoring/README.md](../monitoring/README.md). Target overhead is at most 3%
throughput loss and 5% p95 increase on representative workloads. Report measured
results and scope; local fixtures do not establish live provider, IM or Cloud
performance. Whole-host resource measurements, if taken for an offline benchmark, are
never exported by these endpoints.

## Rollout and rollback

First enable one application endpoint and validate its local output, then append
and validate the Alloy branch, import dashboards/rules, and verify actual Cloud
series and notification routing. Keep the instance alias consistent with alerts.
This repository's isolated tests do not perform that live Cloud acceptance or
restart existing user services. To roll back, disable metrics and restart each
application, remove the added Alloy scrape/relabel components, and remove or pause
the corresponding Cloud rules. Keep existing OTLP configuration intact.

References: [Alloy scrape](https://grafana.com/docs/alloy/latest/reference/components/prometheus/prometheus.scrape/),
[Cloud rule import](https://grafana.com/docs/grafana-cloud/observe-and-act/monitor-infrastructure/kubernetes-monitoring/configuration/config-other-methods/helm-operator-migration/import_rules/).


### Tail latency panels

Every P95 panel has a matching P99 panel, including message operations, agent
operations, first output, Memory operations, and Jev decisions. The dashboard has
78 charts in eight rows. Existing token sources are unchanged.

Jev duration buckets are 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10,
20, 30, 60, 120, 300, 600, 1800, 3600, and +Inf seconds. The collector folds
durations from its existing incremental query into fixed in-memory counters.
There are no additional database queries or schema changes. Invalid negative or
non-finite durations are excluded. Restart and database replacement rebaseline
without replaying old requests.

Deploy the updated Taskix binary and Alloy allowlist before using Jev quantiles,
then import the updated dashboard JSON. The existing duration sum/count series
remain available; their exposition family now has histogram type. New buckets
start with the new collector baseline, not historical data. Tail quantiles are
bucket-based estimates, become unstable with sparse traffic, and cannot resolve
values above the highest finite boundary. Read them alongside decision volume.
