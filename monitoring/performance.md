# Application metrics validation and performance

Historical baseline before the own-process sampler was added. These timings do
not measure the follow-up process-resource sampling overhead. Measured locally on 2026-10-26, macOS arm64, Rust 1.95.0. No production service,
model provider, IM account or Grafana Cloud endpoint was used. Timing runs were
separate from builds and the Rust regression suites. Values below are medians of
seven samples per mode, alternating disabled/enabled order. Differences near zero
are not evidence of a speedup.

| Fixture | Disabled throughput | Enabled throughput | Change | Disabled p95 | Enabled p95 | Change |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Release Unix Codex RPC, 100,000 requests/sample | 120,717/s | 119,131/s | -1.31% | 10.000 µs | 10.042 µs | +0.42% |
| Debug SQLite, 10,000 tasks, 20,000 forced writes/sample | 5,121/s | 5,136/s | +0.29% | 243.583 µs | 236.208 µs | -3.03% |
| Debug SQLite short burst, 10,000 tasks, 2,000 forced writes/sample | 4,985/s | 4,925/s | -1.21% | 226.042 µs | 265.334 µs | +17.38% |

The RPC fixture enables the real recorder/exporter and reads HTTP metrics at
10 Hz. Each enabled sample completed 8–10 scrapes; disabled samples completed
none. It measures real local RPC serialization, transport and response handling,
without model or IM latency. [Raw RPC samples](rpc-benchmark.json).

Both SQLite fixtures overlap one complete read-only application snapshot with
each write burst, with no snapshot in the baseline. Every update changes the
stored value. This is much more frequent than the production 60-second snapshot
cadence. The longer fixture writes each of 10,000 tasks twice per sample; the
short burst writes 2,000 tasks. These use SQLite's real filesystem/WAL behavior,
so filesystem synchronization and scheduling contribute to sample variance.
[Long-window samples](snapshot-benchmark.json),
[short-burst samples](snapshot-burst-benchmark.json).

The 3% throughput / 5% p95 target was met by the RPC and longer SQLite fixtures.
**The short-burst SQLite p95 target was not met.** Keep this result visible: the
long-window result does not erase short-window contention risk, and neither proves
zero overhead on a production database. Collection remains opt-in, read-only,
budgeted and infrequent. Large source databases can exceed the read budget and
lose their snapshot after its two-minute expiry instead of blocking requests.
No host/collector metrics are exported to hide or diagnose that failure remotely;
inspect local logs when application state disappears.

Reproduce the longer SQLite comparison with:

```sh
TASKIX_METRICS_BENCH_WRITES=20000 cargo test -p taskix --bin taskix \
  application_snapshot_contention_benchmark -- --ignored --nocapture
```

The default is 2,000 writes per sample. See [README](README.md) for the RPC and
pipeline commands. Microbenchmark deltas are reports, not flaky CI assertions.
No claim is made about production IM latency, model throughput or Cloud delivery.

## Functional verification

- Focused red-to-green tests cover disabled behavior, cancellation, connection
  ownership, per-backend turn deduplication, usage baselines, stale omission,
  endpoint ownership and bounded series counts.
- Taskix snapshots cover read-only opening, absent databases, VM deadlines,
  external writes, source replacement, retained/pruned history, Jev v1/v2,
  deduplication and late human reviews. A real subprocess verifies metrics-only
  serving without Memory storage/provider initialization.
- All affected application/adapters, Memory and Taskix regression suites were
  exercised. The initial full run hit an environment-only Python `tomllib` error;
  Taskix passed after selecting installed Python 3.14. One Codex owned-socket
  cleanup check failed during a parallel suite and passed on an isolated rerun;
  it also passed in the initial full run. These retries are not omitted from the
  validation record. Desktop/live-account tests remain opt-in and were skipped.
- An asynchronous stack growth regression found in channel integration was fixed;
  all ten channel/Codex end-to-end tests passed afterward.
- Clippy with warnings denied, rustfmt, TOML/YAML parsing and whitespace checks
  passed. Generated dashboard/allowlist tests and Prometheus rule syntax plus five
  alert-behavior cases passed.
- The real local Alloy 1.18.1 → Prometheus 3.15.1 remote-write fixture accepted the
  then-current Taskix endpoint output, rejected host/process/arbitrary metric names and
  generated `up`/`scrape_*` series, and parsed/evaluated all dashboard expressions.

The dashboards have not been visually accepted in the user's Grafana Cloud
instance. Installed application/Alloy services were not restarted or reconfigured,
and no Cloud rules or dashboards were uploaded. CI configuration was added but
has not run remotely; repository code is available in the implementation worktree.

## Counter hot-path optimization (2026-10-26)

The follow-up caches event/token counter handles lazily by finite enum index.
After first observation, recording needs neither label allocation nor a family
lookup/lock; the counter itself uses its existing atomic update. Unobserved
series remain absent. Concurrent first observations share the same counter.
Existing session token baselines now update in place, avoiding a temporary
session String on every cumulative usage notification. Cache cardinality and the
256-session baseline bound are unchanged. No database schema changes were made.

The reusable allocation tests first failed with 2,000 allocations for 1,000
warmed event/token pairs and 2,000 allocations for 1,000 established-session
usage updates. Both now pass with zero allocations. Disabled first-use event,
token and operation recording also passes with zero allocations. The
allocation-counter dependency is test-only; it does not replace the allocator in
production applications or in the separate timing benchmark executable.

| Release recorder-only fixture | Before median | After median |
| --- | ---: | ---: |
| Enabled: one event + one token increment | 105.216 ns/pair | 3.625 ns/pair |
| Disabled: same calls | 2.374 ns/pair | 2.377 ns/pair |

Enabled recording time fell approximately 96.6% in this microbenchmark. Disabled
time is effectively unchanged. These are **not** end-to-end application speedups.
The fixed fixture makes 1,000,000 pairs per sample, seven samples per mode.
Seven alternating before/after runs used separately built release executables,
with no builds or other test suites running concurrently. All 196 timing samples
are retained in [recorder-benchmark.json](recorder-benchmark.json). An initial
exploratory baseline run overlapped a debug test build; it is retained in the
same artifact separately and excluded from the paired-run medians above.

Run allocation checks and the current-version benchmark with:

```sh
cargo test -p agentix-metrics --test hot_path
cargo test -p agentix-metrics --release --test recorder_benchmark -- --ignored --nocapture
```

Performance boundaries at this stage (channel boxing and macOS sampling are
addressed by the subsequent follow-up below):

- Disabled metrics starts no process sampler or SQLite collector. The recorder's
  tested calls allocate nothing, but this does not establish zero overhead for
  every application call site versus the pre-metrics main branch. Feishu
  send/update and Telegram send still box futures to bound async stack size;
  removing those boxes without another stack-safe design risks the previously
  observed channel integration stack overflow.
- Queue gauges and connection updates still perform family lookups; these are
  low-frequency queue snapshots or transport lifecycle events, not per-output
  streaming operations.
- Own-process sampling remains every 15 seconds in a blocking worker, and SQL
  snapshots remain every 60 seconds with read-only connections and deadlines.
  HTTP scrapes read cached snapshots. No process-sampler timing or new SQLite
  contention result is claimed by this counter-only benchmark.
- Skipping SQL snapshots solely because the database has not changed would make
  lease expiry, work eligibility and memory expiry stale. Jev review aggregates
  must also account for late human reviews. Connection reuse would need fresh
  deadlines and source-replacement handling. These need separate designs and
  measurement; the earlier short-burst SQLite p95 failure remains unresolved.

## Further optimization and stopping boundary (2026-10-26)

Three additional changes were retained:

- Feishu send/update and Telegram send now hold an operation guard directly,
  removing the extra future boxes introduced by instrumentation. Cancellation
  still drops the guard, and results still record success/error. Poll-time
  allocation tests first failed (Feishu 4, Telegram 3 allocations for an invalid
  send/update pair), then passed with exactly the two existing error strings.
  The existing async-trait allocation remains outside this assertion; total
  application allocation is not zero. Both full channel suites and all ten
  real channel/Codex integration tests pass without increasing stack limits.
- macOS now queries only the current PID with libproc and uses the process CPU
  clock through cpu-time. Inspection found that sysinfo's macOS PID filter first
  enumerated all PIDs. The direct reader removes that work, and its warmed
  sampling test went from 20 allocations in ten reads to zero. Native byte counts
  and start time are preserved; CPU uses Duration, avoiding Mach tick conversion.
  Other platforms retain the sysinfo adapter. Its Windows GNU target compiles;
  this is a compilation check, not Windows runtime validation. macOS libproc's
  build uses bindgen/libclang and the macOS SDK.
- Workflow snapshots group raw states in SQLite and normalize only the aggregate
  rows in Rust, preserving ASCII case folding, unknown/null/numeric states,
  counts and oldest timestamps. The deterministic 1,000-row mixed-case fixture
  executes 19,075 VM instructions versus 20,041 before (-4.82%). No schema,
  indexes, scan frequency, deadlines or cross-poll cache were changed.

| Isolated fixture | Before median | After median | Change |
| --- | ---: | ---: | ---: |
| Release macOS own-process native read | 14.794 µs | 0.512 µs | -96.54% |
| Workflow aggregate, 10,000 rows with 2 KiB titles | 87.749 ms | 84.983 ms | -3.15% |

Each fixture alternates seven before/after pairs. Process batches contain 1,000
reads; SQL batches contain ten aggregates (90% completed / 10% in progress).
The process benchmark shares the production reader and links no allocation
counter. It excludes formatting, worker scheduling and HTTP. SQL uses the normal
debug test executable with native SQLite; it is not a release application latency
measurement. Raw samples: [process](process-benchmark.json),
[SQL aggregation](entity-benchmark.json).

A fresh, fixed short-burst contention run retained every sample: disabled
5,307 writes/s, enabled 5,147 writes/s (-3.01%); disabled p95 223.625 µs, enabled
228.250 µs (+2.07%). The 5% p95 target passes in this run; throughput is just beyond
the 3% target. This does not erase the earlier +17.38% p95 result or establish
that contention is solved. [Current short-burst samples](snapshot-optimized-benchmark.json).

Reproduce these fixtures with:

```sh
cargo test -p agentix-metrics --release --test process_benchmark -- --ignored --nocapture
cargo test -p taskix --all-features --bin taskix entity_aggregation_benchmark -- --ignored --nocapture
cargo test -p taskix --all-features --bin taskix application_snapshot_contention_benchmark -- --ignored --nocapture
```

The scoped optimization stops here because the remaining substantive cost is
exact SQLite aggregation over mutable external databases. Event cursors alone
cannot invalidate state caches: CLI/external writes, expiring leases/memories,
ready retry times and late reviews must all remain visible. Reusing connections
also requires source replacement and cancellation/deadline handling. Neither
indefinite caching nor shorter read budgets should be introduced merely to meet
a noisy benchmark by dropping or aging out valid metrics. Further material
reductions need a separately validated incremental snapshot design or changes to
source write paths/indexes. This is a measured engineering boundary, not proof
of a global optimum or zero production overhead.

All 19 metrics tests and 12 focused snapshot tests pass, including native CPU
units/start-time comparison, allocation regression checks and unknown-state
merging. The actual metrics-only Taskix subprocess and local Alloy forwarding
fixture verify the updated exposition. Clippy with warnings denied, formatting
and whitespace checks also pass. No production services or Cloud resources were
changed.
